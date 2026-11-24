//! Offline decoding: the events a copy of the trace ring holds, read with the
//! catalogue of the image that wrote it (events.rs).
//!
//! The copy is the bytes of a `kymo::ring`, as a debugger dumps them from a
//! stopped target. The ring's rule (target/include/kymo/ring.hpp) decides
//! what it holds: an event is known intact while the write index has not gone
//! a full ring past it, and complete when its first slot and every following
//! slot carry its type and the lap of their own index. Older events, and
//! events reserved but not yet published, are left out. An event that reaches
//! the end of the ring goes on at its first slot.

use crate::events::{Catalogue, Event, Kind, Pointee, Ring, Type};
use crate::symbols::Symbols;
use crate::target::Result;
use crate::time::Timeline;
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
pub fn slots_for(size: u64) -> u64 {
    if size <= HEAD_PAYLOAD {
        1
    } else {
        1 + (size - HEAD_PAYLOAD).div_ceil(TAIL_PAYLOAD)
    }
}

/// The lap the header of the slot at `index` carries.
pub fn lap(index: u32, ring: &Ring) -> u32 {
    ((index >> ring.slots.trailing_zeros()).wrapping_add(1)) & LAP_MASK
}

/// What the header of a slot says about the event at `index`.
pub enum Header {
    /// The first slot of an event at `index`.
    Head,
    /// A following slot of an event that started before `index`.
    Tail,
    /// Nothing written at `index` yet: an older lap, or a slot never written.
    Unwritten,
}

pub fn header(word: u32, index: u32, ring: &Ring) -> Header {
    if word >> LAP_SHIFT != lap(index, ring) {
        Header::Unwritten
    } else if word & HEAD_BIT != 0 {
        Header::Head
    } else {
        Header::Tail
    }
}

/// The event whose first slot is at `index`, read from `slots` (that slot and
/// those after it, 16 bytes each), if they hold it whole: the first slot
/// published in `index`'s lap, and as many following slots as the event
/// takes, each carrying its type and its own index's lap. A type the image
/// does not have is an error.
pub fn event<'a>(catalogue: &'a Catalogue, ring: &Ring, index: u32, slots: &[u8]) -> Result<Option<Decoded<'a>>> {
    let header = |slot: u64| word(slots, slot * SLOT_BYTES);
    let words = |slot: u64| &slots[(slot * SLOT_BYTES + 4) as usize..][..12];
    let head = header(0);
    if head & HEAD_BIT == 0 || head >> LAP_SHIFT != lap(index, ring) {
        return Ok(None);
    }
    let type_id = (head & TYPE_MASK) as u16;
    let event = catalogue
        .events
        .get(&type_id)
        .ok_or_else(|| format!("event {index}: type id {type_id} is not in the image"))?;
    let count = slots_for(event.size);
    if count * SLOT_BYTES > slots.len() as u64 {
        return Ok(None);
    }
    let tail = |slot: u64| u32::from(type_id) | lap(index.wrapping_add(slot as u32), ring) << LAP_SHIFT;
    if (1..count).any(|slot| header(slot) != tail(slot)) {
        return Ok(None);
    }
    let mut payload = words(0)[4..].to_vec();
    for slot in 1..count {
        payload.extend_from_slice(words(slot));
    }
    payload.truncate(event.size as usize);
    Ok(Some(Decoded {
        index,
        timestamp: word(words(0), 0),
        event,
        payload,
    }))
}

/// The slots most events take, the most any event of the image takes.
pub fn max_event_slots(catalogue: &Catalogue) -> u32 {
    catalogue
        .events
        .values()
        .map(|event| slots_for(event.size) as u32)
        .max()
        .unwrap_or(1)
}

/// The `count` slots from `index` on, out of the ring's `slots` (16 bytes
/// each, by position): past the last position, they go on at the first.
pub fn gather(slots: &[u8], ring: &Ring, index: u32, count: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity((count as u64 * SLOT_BYTES) as usize);
    for n in 0..count {
        let at = (u64::from(index.wrapping_add(n) & (ring.slots - 1)) * SLOT_BYTES) as usize;
        bytes.extend_from_slice(&slots[at..at + SLOT_BYTES as usize]);
    }
    bytes
}

/// The event whose first slot is at `index` among the ring's `slots`, if they
/// hold it whole, as `event` decides.
pub fn event_at<'a>(catalogue: &'a Catalogue, ring: &Ring, slots: &[u8], index: u32) -> Result<Option<Decoded<'a>>> {
    let head = word(slots, u64::from(index & (ring.slots - 1)) * SLOT_BYTES);
    if head & HEAD_BIT == 0 || head >> LAP_SHIFT != lap(index, ring) {
        return Ok(None);
    }
    let type_id = (head & TYPE_MASK) as u16;
    let count = catalogue
        .events
        .get(&type_id)
        .map_or(1, |event| slots_for(event.size).min(u64::from(ring.slots)) as u32);
    event(catalogue, ring, index, &gather(slots, ring, index, count))
}

fn word(bytes: &[u8], offset: u64) -> u32 {
    let offset = offset as usize;
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
}

/// The write index of the copy of `ring`, and its events, oldest first.
pub fn decode<'a>(catalogue: &'a Catalogue, ring: &Ring, bytes: &[u8]) -> Result<(u32, Vec<Decoded<'a>>)> {
    if bytes.len() as u64 != ring.size {
        return Err(format!("the copy has {} bytes, {} {}", bytes.len(), ring.name, ring.size).into());
    }
    let slots = &bytes[ring.slots_offset as usize..(ring.slots_offset + u64::from(ring.slots) * SLOT_BYTES) as usize];
    let write_index = word(bytes, ring.write_index_offset);

    let mut decoded = Vec::new();
    for position in 0..ring.slots.min(write_index) {
        // The one index of the last full ring that maps to this slot.
        let index = write_index - 1 - (write_index - 1 - position) % ring.slots;
        let Some(event) = event_at(catalogue, ring, slots, index)? else {
            continue;
        };
        if u64::from(index) + slots_for(event.event.size) > u64::from(write_index) {
            continue;
        }
        decoded.push(event);
    }
    decoded.sort_by_key(|event| event.index);
    Ok((write_index, decoded))
}

/// `kymo decode`'s output for a copy of the ring: the write index, then one
/// line per event, oldest first, timed at `clock` hertz if given.
pub fn print(catalogue: &Catalogue, write_index: u32, events: &[Decoded], clock: Option<u64>) {
    println!("write_index {write_index}");
    let mut timeline = Timeline::new(clock);
    for decoded in events {
        let cycles = timeline.event(decoded.index, decoded.timestamp);
        let seconds = timeline.seconds(cycles, timeline.frequency());
        println!("{}", line(&catalogue.symbols, decoded, cycles, seconds));
    }
}

/// One event's line: its index, or the range of its slots when it takes more
/// than one, its cycles (its timestamp extended to 64 bits), the seconds since
/// the first event or `-` without a frequency, its type and each field's value.
pub fn line(symbols: &Symbols, decoded: &Decoded, cycles: i64, seconds: Option<f64>) -> String {
    let mut line = format!("event {}", decoded.index);
    let slots = slots_for(decoded.event.size) as u32;
    if slots > 1 {
        let _ = write!(line, "-{}", decoded.index.wrapping_add(slots - 1));
    }
    let _ = write!(line, " {cycles} ");
    match seconds {
        Some(seconds) => {
            let _ = write!(line, "{seconds:.9}");
        }
        None => line.push('-'),
    }
    let _ = write!(line, " {}", decoded.event.name);
    if !decoded.event.fields.is_empty() {
        line.push(' ');
        fields(&mut line, symbols, decoded);
    }
    line
}

/// What a pointer to its pointee holding an address is called, if anything.
pub type Names<'a> = &'a dyn Fn(&Pointee, u64) -> Option<String>;

/// Each field of the event as `name=value`, separated by spaces.
pub fn fields(out: &mut String, symbols: &Symbols, decoded: &Decoded) {
    fields_named(out, &|pointee, address| symbols.name(pointee, address), decoded);
}

/// Each field of the event as `name=value`, separated by spaces, pointers
/// called by `names`.
pub fn fields_named(out: &mut String, names: Names, decoded: &Decoded) {
    for (n, field) in decoded.event.fields.iter().enumerate() {
        if n > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{}=", field.name);
        value(out, names, &field.ty, &decoded.payload[field.offset as usize..]);
    }
}

/// The value of `ty` at the start of `bytes`: integers in decimal, booleans
/// as true or false, enumerations by enumerator, pointers by what `names`
/// calls them or else in hexadecimal, durations and time points as a count
/// and its unit, arrays in brackets, structures in braces.
fn value(out: &mut String, names: Names, ty: &Type, bytes: &[u8]) {
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
        Kind::Pointer(pointee) if size <= 8 => match names(pointee, unsigned()) {
            Some(name) => out.push_str(&name),
            None => {
                let _ = write!(out, "0x{:0width$x}", unsigned(), width = 2 * size);
            }
        },
        Kind::Duration(count, num, den) => {
            value(out, names, count, bytes);
            unit(out, *num, *den);
        }
        Kind::Array(element, lengths) => array(out, names, element, lengths, bytes),
        Kind::Record(fields) => {
            out.push('{');
            for (n, field) in fields.iter().enumerate() {
                if n > 0 {
                    out.push(' ');
                }
                let _ = write!(out, "{}=", field.name);
                value(out, names, &field.ty, &bytes[field.offset as usize..]);
            }
            out.push('}');
        }
        _ => hex(out, bytes),
    }
}

fn array(out: &mut String, names: Names, element: &Type, lengths: &[u64], bytes: &[u8]) {
    let Some((length, inner)) = lengths.split_first() else {
        return value(out, names, element, bytes);
    };
    let stride = element.size.unwrap_or_default() as usize * inner.iter().product::<u64>() as usize;
    out.push('[');
    for n in 0..*length as usize {
        if n > 0 {
            out.push(',');
        }
        array(out, names, element, inner, &bytes[n * stride..]);
    }
    out.push(']');
}

/// The unit of a count lasting `num` / `den` seconds: its SI symbol, or else
/// the ratio, `x(1/60)s`.
fn unit(out: &mut String, num: u64, den: u64) {
    let symbol = match (num, den) {
        (1, 1) => "s",
        (1, 1_000) => "ms",
        (1, 1_000_000) => "us",
        (1, 1_000_000_000) => "ns",
        (60, 1) => "min",
        (3_600, 1) => "h",
        (num, 1) => {
            let _ = write!(out, "x{num}s");
            return;
        }
        (num, den) => {
            let _ = write!(out, "x({num}/{den})s");
            return;
        }
    };
    out.push_str(symbol);
}

/// Bytes with no other reading, in memory order.
fn hex(out: &mut String, bytes: &[u8]) {
    out.push_str("0x");
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
}
