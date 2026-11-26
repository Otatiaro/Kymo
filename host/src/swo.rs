//! The variables the DWT watches (ADR-098): each write to one is sent on SWO,
//! its value then a local timestamp, and is a sample of the variable's curve.
//!
//! The capture holds the DWT's configuration, read at the start, and the SWO
//! stream (capture.rs). A comparator that sends a data value on each write
//! watches a variable: the ELF names it, and says how its bytes read, from
//! the address the comparator matches; an address the ELF does not know
//! names the curve, read unsigned. One comparator is the anchor: it watches
//! the timestamp word of the ring's slot 0, so that each event that starts at
//! slot 0 sends its timestamp on SWO too, a point both streams share.
//!
//! The ITM's clock is the sum of its local timestamps, each counting the
//! cycles since the one before it, plus one; a data value takes the time of
//! the timestamp after it. A sample between two anchors is dated from the
//! one before it: its cycles are the anchor event's, plus the ITM's cycles
//! since the anchor. The stretch between two anchors is dated only when:
//!
//! - the ITM's cycles between them are those of their events, within
//!   `TOLERANCE`;
//! - no overflow and no malformed packet lies between them;
//! - every event of the trace between them that starts at slot 0 sent its
//!   anchor: one missing means the stream lost packets, or that the anchor's
//!   comparator was taken over.
//!
//! A stretch that is not dated is a gap: its samples are dropped, and the
//! curves' values are unknown over it. The samples before the first anchor
//! and after the last one cannot be dated: they are counted, not kept.

use crate::capture::Dwt;
use crate::events::{Catalogue, Pointee, Ring};
use crate::itm::{self, Packet};
use crate::model::{Edge, Element};
use crate::values::{Curve, Reading, Sampled};
use std::collections::HashMap;

/// The most the ITM's cycles between two anchors may differ from their
/// events' cycles: the local timestamps are emitted late by a few cycles,
/// and no packet is lost without a larger difference.
pub const TOLERANCE: i64 = 8;

/// DWT_DEVARCH.ARCHID of an ARMv8-M DWT; RAZ on ARMv7-M.
const ARMV8M: u32 = 0x1a02;
/// DWT_CTRL.NOTRCPKT: the DWT has no trace packets.
const NO_TRACE_PACKETS: u32 = 1 << 27;
/// DWT_FUNCTIONn's function: a data value on each write (MATCH on ARMv8-M,
/// FUNCTION on ARMv7-M).
const WRITE_VALUE: u32 = 0b1101;
/// DWT_FUNCTIONn.ACTION on ARMv8-M: a data value packet.
const DATA_VALUE_PACKET: u32 = 0b10;
/// The comparators a data value packet can name.
const NAMED: usize = 4;

/// A comparator that sends a data value on each write: its number, the
/// address it matches and how many bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    pub comparator: u8,
    pub address: u32,
    pub size: u8,
}

/// The comparators of `dwt` that send a data value on each write.
pub fn watches(dwt: &Dwt) -> Vec<Watch> {
    if dwt.ctrl & NO_TRACE_PACKETS != 0 {
        return Vec::new();
    }
    let armv8m = dwt.devarch & 0xffff == ARMV8M;
    let count = ((dwt.ctrl >> 28) as usize).min(NAMED);
    dwt.comparators
        .iter()
        .take(count)
        .enumerate()
        .filter_map(|(n, comparator)| {
            let function = comparator.function;
            let log2 = if armv8m {
                let action = (function >> 4) & 0b11;
                (function & 0xf == WRITE_VALUE && action == DATA_VALUE_PACKET).then_some((function >> 10) & 0b11)
            } else {
                (function & 0xf == WRITE_VALUE).then_some(comparator.mask)
            }?;
            (log2 <= 2).then(|| Watch {
                comparator: n as u8,
                address: comparator.comp,
                size: 1 << log2,
            })
        })
        .collect()
}

/// The variables watched in a capture, and their samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Watched {
    /// What made the capture's samples unusable, when something did: the
    /// anchor's comparator missing from the configuration.
    pub refused: Option<String>,
    /// The curves, one per comparator watching a variable, in the order of
    /// the comparators.
    pub curves: Vec<Curve>,
    /// In the order of the stream. A sample's element is the trace's last
    /// event at or before it.
    pub samples: Vec<Sampled>,
    /// The cycles from the first anchor to the last: none with fewer than
    /// two, when no sample is dated.
    pub span: Option<(i64, i64)>,
    /// The stretches between two anchors, in cycles, whose samples are
    /// dropped and the curves' values unknown.
    pub gaps: Vec<(i64, i64)>,
    /// The anchors matched with an event of the trace.
    pub anchors: usize,
    /// The overflows the stream reports: packets the ITM could not send.
    pub overflows: usize,
    /// The samples dropped in a gap: their curve, and the cycles of the
    /// anchor before them, where the gap starts.
    pub dropped: Vec<(Curve, i64)>,
    /// The samples before the first anchor, after the last one, or without
    /// a timestamp after them at the end of the stream.
    pub undated: usize,
}

/// An event of the trace that starts at slot 0: its element, its cycles and
/// the timestamp its anchor carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected {
    pub element: usize,
    pub cycles: i64,
    pub timestamp: u32,
}

/// The samples of `watches` in `bytes`, a SWO stream, dated against the
/// trace's events `expected` to start at slot 0 by the comparator `anchor`.
/// Each sample is its watch, its value and its cycles.
pub fn date(bytes: &[u8], anchor: u8, watches: &[Watch], expected: &[Expected]) -> Dated {
    let (packets, _) = itm::packets(bytes);
    let mut stream = Vec::new();
    let mut waiting = Vec::new();
    let mut clock = 0u64;
    let mut overflows = 0;
    for packet in packets {
        match packet {
            Packet::DataValue {
                comparator,
                write: true,
                value,
                ..
            } => {
                waiting.push(stream.len());
                stream.push(Item::Value { comparator, value, itm: None });
            }
            Packet::Timestamp { delta, .. } => {
                clock += u64::from(delta) + 1;
                for &item in &waiting {
                    if let Item::Value { itm, .. } = &mut stream[item] {
                        *itm = Some(clock);
                    }
                }
                waiting.clear();
            }
            Packet::Overflow | Packet::Malformed => {
                overflows += usize::from(packet == Packet::Overflow);
                stream.push(Item::Broken);
            }
            Packet::Sync | Packet::Other | Packet::DataValue { write: false, .. } => {}
        }
    }

    // The anchors received, by timestamp: their places in the stream.
    let mut received: HashMap<u32, Vec<usize>> = HashMap::new();
    for (at, item) in stream.iter().enumerate() {
        if let Item::Value {
            comparator,
            value,
            itm: Some(_),
        } = *item
            && comparator == anchor
        {
            received.entry(value).or_default().push(at);
        }
    }
    // Each expected anchor, matched with the first received after the last
    // match: none when it was not received.
    let mut after = 0;
    let matched: Vec<Option<usize>> = expected
        .iter()
        .map(|expected| {
            let at = *received
                .get(&expected.timestamp)?
                .iter()
                .find(|&&at| at >= after)?;
            after = at + 1;
            Some(at)
        })
        .collect();

    let itm_of = |at: usize| match stream[at] {
        Item::Value { itm: Some(itm), .. } => itm as i64,
        _ => unreachable!("a matched anchor is dated"),
    };
    let mut dated = Dated {
        overflows,
        ..Dated::default()
    };
    let anchors: Vec<(usize, &Expected, usize)> = matched
        .iter()
        .enumerate()
        .filter_map(|(n, at)| Some((n, &expected[n], (*at)?)))
        .collect();
    dated.anchors = anchors.len();
    let sample = |item: &Item| match *item {
        Item::Value { comparator, value, itm } if comparator != anchor => watches
            .iter()
            .position(|watch| watch.comparator == comparator)
            .map(|watch| (watch, value, itm)),
        _ => None,
    };
    let first = anchors.first().map_or(stream.len(), |&(.., at)| at);
    let last = anchors.last().map_or(stream.len(), |&(.., at)| at);
    dated.undated = stream[..first]
        .iter()
        .chain(stream.get(last..).unwrap_or_default())
        .filter(|item| sample(item).is_some())
        .count();
    for pair in anchors.windows(2) {
        let [(n, from, start), (m, to, end)] = [pair[0], pair[1]];
        let between = &stream[start + 1..end];
        let sound = ((itm_of(end) - itm_of(start)) - (to.cycles - from.cycles)).abs() <= TOLERANCE
            && !between.iter().any(|item| matches!(item, Item::Broken))
            && m == n + 1;
        if !sound {
            dated.gaps.push((from.cycles, to.cycles));
        }
        for (watch, value, itm) in between.iter().filter_map(sample) {
            match itm {
                Some(itm) if sound => dated.samples.push((watch, value, from.cycles + itm as i64 - itm_of(start))),
                _ => dated.dropped.push((watch, from.cycles)),
            }
        }
    }
    if let (Some(&(_, first, _)), Some(&(_, last, _))) = (anchors.first(), anchors.last())
        && anchors.len() > 1
    {
        dated.span = Some((first.cycles, last.cycles));
    }
    dated
}

/// What the stream holds that dating reads.
#[derive(Clone, Copy, Debug)]
enum Item {
    /// A data value written, and the ITM's clock at the timestamp after it.
    Value { comparator: u8, value: u32, itm: Option<u64> },
    /// An overflow or a malformed packet: packets may be missing.
    Broken,
}

/// The samples of a stream, dated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dated {
    /// The index of its watch, its value and its cycles.
    pub samples: Vec<(usize, u32, i64)>,
    pub span: Option<(i64, i64)>,
    pub gaps: Vec<(i64, i64)>,
    pub anchors: usize,
    pub overflows: usize,
    /// The index of its watch and the cycles of the anchor before.
    pub dropped: Vec<(usize, i64)>,
    pub undated: usize,
}

/// The variables of `dwt` watched in a trace's `elements`, read from `ring`,
/// and their samples in the SWO stream `bytes`.
pub fn watched(catalogue: &Catalogue, ring: &Ring, dwt: &Dwt, bytes: &[u8], elements: &[Element]) -> Watched {
    let anchor_address = ring.address + ring.slots_offset + 4;
    let all = watches(dwt);
    let Some(anchor) = all
        .iter()
        .find(|watch| u64::from(watch.address) == anchor_address && watch.size == 4)
    else {
        return Watched {
            refused: Some(format!(
                "no DWT comparator sends the anchor, the writes to {anchor_address:#010x}: \
                 the program did not call kymo::dwt::start, or its comparator was taken over"
            )),
            ..Watched::default()
        };
    };
    let variables: Vec<Watch> = all.iter().filter(|watch| watch.comparator != anchor.comparator).copied().collect();
    let readings: Vec<(Curve, Reading)> = variables
        .iter()
        .map(|watch| {
            let address = u64::from(watch.address);
            let subject = catalogue
                .symbols
                .name(&Pointee::Other, address)
                .unwrap_or_else(|| format!("{address:#010x}"));
            let reading = catalogue
                .symbols
                .number(address)
                .filter(|reading| reading.size() == usize::from(watch.size))
                .unwrap_or(Reading::Unsigned(usize::from(watch.size)));
            (Curve { subject, field: None }, reading)
        })
        .collect();

    let events: Vec<(usize, i64)> = elements
        .iter()
        .enumerate()
        .filter_map(|(n, element)| match element {
            Element::Event { cycles, .. } => Some((n, *cycles)),
            _ => None,
        })
        .collect();
    let expected: Vec<Expected> = elements
        .iter()
        .enumerate()
        .filter_map(|(n, element)| match element {
            Element::Event { event, cycles, .. } if event.index % ring.slots == 0 => Some(Expected {
                element: n,
                cycles: *cycles,
                timestamp: event.timestamp,
            }),
            _ => None,
        })
        .collect();
    let dated = date(bytes, anchor.comparator, &variables, &expected);
    let samples = dated
        .samples
        .iter()
        .filter_map(|&(watch, value, cycles)| {
            let (curve, reading) = &readings[watch];
            let value = reading.read(&value.to_le_bytes())?;
            let before = events.partition_point(|&(_, at)| at <= cycles);
            let element = before.checked_sub(1).map_or(0, |n| events[n].0);
            Some(Sampled {
                curve: curve.clone(),
                value,
                at: Edge {
                    element,
                    cycles,
                    lost: false,
                },
            })
        })
        .collect();
    Watched {
        refused: None,
        curves: readings.iter().map(|(curve, _)| curve.clone()).collect(),
        samples,
        span: dated.span,
        gaps: dated.gaps,
        anchors: dated.anchors,
        overflows: dated.overflows,
        dropped: dated
            .dropped
            .iter()
            .map(|&(watch, cycles)| (readings[watch].0.clone(), cycles))
            .collect(),
        undated: dated.undated,
    }
}

#[cfg(test)]
mod tests;
