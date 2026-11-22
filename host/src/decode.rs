//! Offline decoding: the events a copy of the trace ring holds, read with the
//! catalogue of the image that wrote it (events.rs).
//!
//! The copy is the bytes of `kymo::trace_ring`, as a debugger dumps them from
//! a stopped target. The ring's rule (target/include/kymo/ring.hpp) decides
//! what it holds: an event is known intact while the write index has not gone
//! a full ring past it, and complete when its first slot and every following
//! slot carry its header. Older events, and events reserved but not yet
//! published, are left out.

use crate::events::{Catalogue, Event, Kind, Ring, Type};
use crate::target::Result;
use gimli::DwAte;
use std::fmt::Write;

const SLOT_BYTES: u64 = 16;
const HEAD_PAYLOAD: u64 = 8;
const TAIL_PAYLOAD: u64 = 12;
const TYPE_MASK: u32 = 0xffff;
const HEAD_BIT: u32 = 1 << 16;
const LAP_SHIFT: u32 = 17;
const LAP_MASK: u32 = 0x7fff;

/// One event read back from the ring.
pub struct Decoded<'a> {
    pub index: u32,
    pub timestamp: u32,
    pub event: &'a Event,
    pub payload: Vec<u8>,
}

/// The slots an event of `size` bytes takes.
fn slots_for(size: u64) -> u64 {
    if size <= HEAD_PAYLOAD {
        1
    } else {
        1 + (size - HEAD_PAYLOAD).div_ceil(TAIL_PAYLOAD)
    }
}

fn lap(index: u32, ring: &Ring) -> u32 {
    ((index >> ring.slots.trailing_zeros()) + 1) & LAP_MASK
}

fn word(bytes: &[u8], offset: u64) -> u32 {
    let offset = offset as usize;
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
}

/// The write index of the copy, and its events, oldest first.
pub fn decode<'a>(catalogue: &'a Catalogue, bytes: &[u8]) -> Result<(u32, Vec<Decoded<'a>>)> {
    let ring = catalogue.ring.as_ref().ok_or("the image has no kymo::trace_ring")?;
    if bytes.len() as u64 != ring.size {
        return Err(format!("the copy has {} bytes, kymo::trace_ring {}", bytes.len(), ring.size).into());
    }
    let physical = u64::from(ring.slots + ring.max_event_slots - 1);
    let header = |slot: u64| word(bytes, ring.slots_offset + slot * SLOT_BYTES);
    let words = |slot: u64| &bytes[(ring.slots_offset + slot * SLOT_BYTES + 4) as usize..][..12];
    let write_index = word(bytes, ring.write_index_offset);

    let mut decoded = Vec::new();
    for position in 0..ring.slots {
        let head = header(u64::from(position));
        if head & HEAD_BIT == 0 || write_index <= position {
            continue;
        }
        // The one index of the last full ring that maps to this slot.
        let index = write_index - 1 - (write_index - 1 - position) % ring.slots;
        let type_id = (head & TYPE_MASK) as u16;
        if head >> LAP_SHIFT != lap(index, ring) {
            continue;
        }
        let event = catalogue
            .events
            .get(&type_id)
            .ok_or_else(|| format!("slot {position}: type id {type_id} is not in the image"))?;
        let count = slots_for(event.size);
        if u64::from(position) + count > physical || u64::from(index) + count > u64::from(write_index) {
            continue;
        }
        let tail = u32::from(type_id) | lap(index, ring) << LAP_SHIFT;
        let first = u64::from(position);
        if (1..count).any(|slot| header(first + slot) != tail) {
            continue;
        }
        let mut payload = words(first)[4..].to_vec();
        for slot in 1..count {
            payload.extend_from_slice(words(first + slot));
        }
        payload.truncate(event.size as usize);
        decoded.push(Decoded {
            index,
            timestamp: word(words(first), 0),
            event,
            payload,
        });
    }
    decoded.sort_by_key(|event| event.index);
    Ok((write_index, decoded))
}

/// `kymo decode`'s output: the write index, then one line per event, oldest
/// first: its index, its timestamp, its type and each field's value.
pub fn print(write_index: u32, events: &[Decoded]) {
    println!("write_index {write_index}");
    for decoded in events {
        let mut line = format!("event {} {} {}", decoded.index, decoded.timestamp, decoded.event.name);
        for field in &decoded.event.fields {
            let _ = write!(line, " {}=", field.name);
            value(&mut line, &field.ty, &decoded.payload[field.offset as usize..]);
        }
        println!("{line}");
    }
}

/// The value of `ty` at the start of `bytes`: integers in decimal, booleans
/// as true or false, enumerations by enumerator, pointers in hexadecimal,
/// arrays in brackets, structures in braces.
fn value(out: &mut String, ty: &Type, bytes: &[u8]) {
    let size = ty.size.unwrap_or_default() as usize;
    let bytes = &bytes[..size];
    let unsigned = || {
        bytes
            .iter()
            .rev()
            .fold(0u64, |value, byte| value << 8 | u64::from(*byte))
    };
    let signed = || {
        let shift = 64 - 8 * size as u32;
        ((unsigned() << shift) as i64) >> shift
    };
    match &ty.kind {
        Kind::Base(encoding) if size <= 8 => match *encoding {
            gimli::DW_ATE_boolean if unsigned() <= 1 => out.push_str(if unsigned() == 1 { "true" } else { "false" }),
            gimli::DW_ATE_signed | gimli::DW_ATE_signed_char => {
                let _ = write!(out, "{}", signed());
            }
            DwAte(_) if *encoding != gimli::DW_ATE_float => {
                let _ = write!(out, "{}", unsigned());
            }
            _ => hex(out, bytes),
        },
        Kind::Enum(enumerators) if size <= 8 => {
            match enumerators
                .iter()
                .find(|(value, _)| *value == signed() || *value as u64 == unsigned())
            {
                Some((_, name)) => out.push_str(name),
                None => {
                    let _ = write!(out, "{}", unsigned());
                }
            }
        }
        Kind::Pointer if size <= 8 => {
            let _ = write!(out, "0x{:0width$x}", unsigned(), width = 2 * size);
        }
        Kind::Array(element, lengths) => array(out, element, lengths, bytes),
        Kind::Record(fields) => {
            out.push('{');
            for (n, field) in fields.iter().enumerate() {
                if n > 0 {
                    out.push(' ');
                }
                let _ = write!(out, "{}=", field.name);
                value(out, &field.ty, &bytes[field.offset as usize..]);
            }
            out.push('}');
        }
        _ => hex(out, bytes),
    }
}

fn array(out: &mut String, element: &Type, lengths: &[u64], bytes: &[u8]) {
    let Some((length, inner)) = lengths.split_first() else {
        return value(out, element, bytes);
    };
    let stride = element.size.unwrap_or_default() as usize * inner.iter().product::<u64>() as usize;
    out.push('[');
    for n in 0..*length as usize {
        if n > 0 {
            out.push(',');
        }
        array(out, element, inner, &bytes[n * stride..]);
    }
    out.push(']');
}

/// Bytes with no other reading, in memory order.
fn hex(out: &mut String, bytes: &[u8]) {
    out.push_str("0x");
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
}
