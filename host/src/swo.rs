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
//! since the anchor.
//!
//! An anchor whose timestamp is delayed (its control not 0), or dates other
//! packets too, is matched but dates nothing: such a timestamp is late by up
//! to hundreds of cycles for some of its packets. The stretch then runs from
//! the anchor before it to the one after. The stretch between two anchors is
//! dated only when:
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
//!
//! The exception packets the DWT sends when its exception trace is on
//! (ADR-102) are dated the same way: between two anchors, by the timestamp
//! after them. Those of a gap are dropped, never decoded.
//!
//! Without anchors, the DWT's periodic tick dates the stream instead
//! (ADR-104, `tick`): the local timestamps between two ticks add up to its
//! period, a control the stream carries on its own.

use crate::capture::Dwt;
use crate::events::{Catalogue, Pointee, Ring};
use crate::itm::{self, Function, Packet};
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
    /// The anchors matched with an event of the trace that date the samples.
    pub anchors: usize,
    /// The anchors matched whose timestamp is delayed or dates other packets
    /// too: they date nothing.
    pub set_aside: usize,
    /// The overflows the stream reports: packets the ITM could not send.
    pub overflows: usize,
    /// The samples dropped in a gap: their curve, and the cycles of the
    /// anchor before them, where the gap starts.
    pub dropped: Vec<(Curve, i64)>,
    /// The samples before the first anchor, after the last one, or without
    /// a timestamp after them at the end of the stream.
    pub undated: usize,
    /// The exception packets dated, in the order of the stream.
    pub exceptions: Vec<Exception>,
    /// The exception packets in a gap: dropped, not decoded.
    pub exceptions_dropped: usize,
    /// The exception packets outside the anchors.
    pub exceptions_undated: usize,
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
    let (stream, overflows) = read(bytes);

    // The anchors received, by timestamp: their places in the stream.
    let mut received: HashMap<u32, Vec<usize>> = HashMap::new();
    for (at, item) in stream.iter().enumerate() {
        if let Item::Dated {
            what: What::Value { comparator, value },
            itm: Some(_),
            ..
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
            let at = *received.get(&expected.timestamp)?.iter().find(|&&at| at >= after)?;
            after = at + 1;
            Some(at)
        })
        .collect();

    let itm_of = |at: usize| match stream[at] {
        Item::Dated { itm: Some(itm), .. } => itm as i64,
        _ => unreachable!("a matched anchor is dated"),
    };
    let mut dated = Dated {
        overflows,
        ..Dated::default()
    };
    let alone = |at: usize| matches!(stream[at], Item::Dated { alone: true, .. });
    let anchors: Vec<(usize, &Expected, usize)> = matched
        .iter()
        .enumerate()
        .filter_map(|(n, at)| Some((n, &expected[n], (*at)?)))
        .filter(|&(.., at)| alone(at))
        .collect();
    dated.anchors = anchors.len();
    dated.set_aside = matched.iter().flatten().count() - anchors.len();
    let sample = |item: &Item| match *item {
        Item::Dated {
            what: What::Value { comparator, .. },
            ..
        } if comparator == anchor => None,
        _ => sample(item, watches),
    };
    let first = anchors.first().map_or(stream.len(), |&(.., at)| at);
    let last = anchors.last().map_or(stream.len(), |&(.., at)| at);
    let outside = || stream[..first].iter().chain(stream.get(last..).unwrap_or_default());
    dated.undated = outside().filter(|item| sample(item).is_some()).count();
    dated.exceptions_undated = outside().filter(|item| exception(item).is_some()).count();
    for pair in anchors.windows(2) {
        let [(n, from, start), (m, to, end)] = [pair[0], pair[1]];
        let between = &stream[start + 1..end];
        let sound = ((itm_of(end) - itm_of(start)) - (to.cycles - from.cycles)).abs() <= TOLERANCE
            && !between.iter().any(|item| matches!(item, Item::Broken))
            && matched[n + 1..m].iter().all(Option::is_some);
        if !sound {
            dated.gaps.push((from.cycles, to.cycles));
        }
        for (watch, value, itm) in between.iter().filter_map(sample) {
            match itm {
                Some(itm) if sound => dated
                    .samples
                    .push((watch, value, from.cycles + itm as i64 - itm_of(start))),
                _ => dated.dropped.push((watch, from.cycles)),
            }
        }
        for (number, function, itm) in between.iter().filter_map(exception) {
            match itm {
                Some(itm) if sound => dated.exceptions.push(Exception {
                    number,
                    function,
                    cycles: from.cycles + itm as i64 - itm_of(start),
                }),
                _ => dated.exceptions_dropped += 1,
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

/// What dating reads of the stream `bytes`, and the overflows it reports.
fn read(bytes: &[u8]) -> (Vec<Item>, usize) {
    let (packets, _) = itm::packets(bytes);
    let mut stream = Vec::new();
    let mut waiting = Vec::new();
    let mut clock = 0u64;
    let mut overflows = 0;
    // The packets the next timestamp dates, those read past included.
    let mut since = 0usize;
    for packet in packets {
        let what = match packet {
            Packet::DataValue {
                comparator,
                write: true,
                value,
                ..
            } => Some(What::Value { comparator, value }),
            Packet::Exception { number, function } => Some(What::Exception { number, function }),
            Packet::Counter { wrapped } if wrapped & itm::CYC != 0 => Some(What::Tick),
            _ => None,
        };
        if let Some(what) = what {
            since += 1;
            waiting.push(stream.len());
            stream.push(Item::Dated {
                what,
                itm: None,
                alone: false,
            });
            continue;
        }
        match packet {
            Packet::DataValue { write: false, .. } | Packet::Source | Packet::Counter { .. } => since += 1,
            Packet::Timestamp { delta, control } => {
                clock += u64::from(delta) + 1;
                for &item in &waiting {
                    if let Item::Dated { itm, alone, .. } = &mut stream[item] {
                        *itm = Some(clock);
                        *alone = control == 0 && since == 1;
                    }
                }
                waiting.clear();
                since = 0;
            }
            Packet::Overflow | Packet::Malformed => {
                overflows += usize::from(packet == Packet::Overflow);
                stream.push(Item::Broken);
            }
            Packet::Sync | Packet::Other | Packet::DataValue { write: true, .. } | Packet::Exception { .. } => {}
        }
    }
    (stream, overflows)
}

/// The sample `item` is, when it is one of `watches`': the index of its
/// watch, its value and the ITM's clock that dates it.
fn sample(item: &Item, watches: &[Watch]) -> Option<(usize, u32, Option<u64>)> {
    match *item {
        Item::Dated {
            what: What::Value { comparator, value },
            itm,
            ..
        } => watches
            .iter()
            .position(|watch| watch.comparator == comparator)
            .map(|watch| (watch, value, itm)),
        _ => None,
    }
}

/// The exception packet `item` is: its number, its function and the ITM's
/// clock that dates it.
fn exception(item: &Item) -> Option<(u16, Function, Option<u64>)> {
    match *item {
        Item::Dated {
            what: What::Exception { number, function },
            itm,
            ..
        } => Some((number, function, itm)),
        _ => None,
    }
}

/// DWT_CTRL.CYCEVTENA: the DWT sends its periodic tick.
const CYCEVTENA: u32 = 1 << 22;
/// DWT_CTRL.CYCTAP: `POSTCNT` counts at bit 10 of `CYCCNT`, else at bit 6.
const CYCTAP: u32 = 1 << 9;

/// The periods the DWT's tick can have, in cycles: 64 or 1024, times
/// `POSTPRESET` plus one.
fn periods() -> impl Iterator<Item = i64> {
    [64, 1024]
        .into_iter()
        .flat_map(|tap| (1..=16).map(move |preset| tap * preset))
}

/// The period of the tick DWT_CTRL `ctrl` sets: none when it is off.
pub fn tick_period(ctrl: u32) -> Option<i64> {
    let tap = if ctrl & CYCTAP != 0 { 1024 } else { 64 };
    (ctrl & CYCEVTENA != 0).then(|| tap * (i64::from((ctrl >> 1) & 0xf) + 1))
}

/// The samples of `watches` in `bytes`, a SWO stream, and its exception
/// packets, dated by the DWT's periodic tick (ADR-104), checked against
/// DWT_CTRL `ctrl` when the capture holds it.
///
/// A tick is a counter packet with `CYC`; like an anchor, one whose
/// timestamp is delayed, or dates other packets too, dates nothing. The
/// period is the spacing, within `TOLERANCE`, that neighbouring ticks show
/// most often. The stretch between two ticks is dated when the ITM's cycles
/// between them are the period times the ticks they are apart, within
/// `TOLERANCE`, and no overflow or malformed packet lies between them.
///
/// The stretches dated one after the other are a segment, its cycles counted
/// from its first tick. A stretch not dated is a gap: the periods it lost
/// are not known, and the segment after it has an origin of its own.
pub fn tick(bytes: &[u8], watches: &[Watch], ctrl: Option<u32>) -> Ticked {
    let (stream, overflows) = read(bytes);
    let mut ticked = Ticked {
        overflows,
        ..Ticked::default()
    };
    let itm_of = |at: usize| match stream[at] {
        Item::Dated { itm: Some(itm), .. } => itm as i64,
        _ => unreachable!("a tick kept is dated"),
    };
    let received: Vec<(usize, bool)> = stream
        .iter()
        .enumerate()
        .filter_map(|(at, item)| match *item {
            Item::Dated {
                what: What::Tick,
                itm: Some(_),
                alone,
            } => Some((at, alone)),
            _ => None,
        })
        .collect();
    // Each tick kept: its rank among the ticks received, and its place.
    let kept: Vec<(usize, usize)> = received
        .iter()
        .enumerate()
        .filter(|&(_, &(_, alone))| alone)
        .map(|(n, &(at, _))| (n, at))
        .collect();
    ticked.ticks = kept.len();
    ticked.set_aside = received.len() - kept.len();
    let broken = |from: usize, to: usize| stream[from + 1..to].iter().any(|item| matches!(item, Item::Broken));

    let mut seen: HashMap<i64, usize> = HashMap::new();
    for pair in kept.windows(2) {
        let [(n, start), (m, end)] = [pair[0], pair[1]];
        if m == n + 1 && !broken(start, end) {
            let spacing = itm_of(end) - itm_of(start);
            if let Some(period) = periods().find(|period| (spacing - period).abs() <= TOLERANCE) {
                *seen.entry(period).or_default() += 1;
            }
        }
    }
    let Some(period) = seen
        .into_iter()
        .max_by_key(|&(period, count)| (count, period))
        .map(|(period, _)| period)
    else {
        ticked.refused = Some(if kept.len() < 2 {
            "the stream holds fewer than two ticks: the program did not call kymo::dwt::ticks".into()
        } else {
            "no two neighbouring ticks are a period of the DWT's tick apart".into()
        });
        return ticked;
    };
    if let Some(ctrl) = ctrl
        && tick_period(ctrl) != Some(period)
    {
        ticked.refused = Some(match tick_period(ctrl) {
            Some(set) => format!("the ticks come every {period} cycles, DWT_CTRL sets {set}"),
            None => format!("the ticks come every {period} cycles, DWT_CTRL sets none"),
        });
        return ticked;
    }
    ticked.period = Some(period);

    let first = kept.first().map_or(stream.len(), |&(_, at)| at);
    let last = kept.last().map_or(stream.len(), |&(_, at)| at);
    let outside = || stream[..first].iter().chain(stream.get(last..).unwrap_or_default());
    ticked.undated = outside().filter(|item| sample(item, watches).is_some()).count();
    ticked.exceptions_undated = outside().filter(|item| exception(item).is_some()).count();
    // The segment under way, and the rank of its first tick.
    let mut segment: Option<(Segment, usize)> = None;
    for pair in kept.windows(2) {
        let [(n, start), (m, end)] = [pair[0], pair[1]];
        let between = &stream[start + 1..end];
        let expected = (m - n) as i64 * period;
        let sound = ((itm_of(end) - itm_of(start)) - expected).abs() <= TOLERANCE && !broken(start, end);
        if !sound {
            ticked.segments.extend(segment.take().map(|(segment, _)| segment));
            ticked.gaps += 1;
            ticked.dropped += between.iter().filter(|item| sample(item, watches).is_some()).count();
            ticked.exceptions_dropped += between.iter().filter(|item| exception(item).is_some()).count();
            continue;
        }
        let (segment, origin) = segment.get_or_insert_with(|| (Segment::default(), n));
        let from = (n - *origin) as i64 * period;
        let at = |itm: u64| from + itm as i64 - itm_of(start);
        for (watch, value, itm) in between.iter().filter_map(|item| sample(item, watches)) {
            match itm {
                Some(itm) => segment.samples.push((watch, value, at(itm))),
                None => ticked.dropped += 1,
            }
        }
        for (number, function, itm) in between.iter().filter_map(exception) {
            match itm {
                Some(itm) => segment.exceptions.push(Exception {
                    number,
                    function,
                    cycles: at(itm),
                }),
                None => ticked.exceptions_dropped += 1,
            }
        }
        segment.cycles = (m - *origin) as i64 * period;
    }
    ticked.segments.extend(segment.map(|(segment, _)| segment));
    ticked
}

/// The samples and exception packets of a stream, dated by the DWT's tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ticked {
    /// What made the stream's ticks unusable, when something did.
    pub refused: Option<String>,
    /// The tick's period, in cycles.
    pub period: Option<i64>,
    /// The ticks kept, and those set aside: their timestamp delayed or
    /// shared.
    pub ticks: usize,
    pub set_aside: usize,
    pub overflows: usize,
    /// The segments, in the order of the stream, each with an origin of its
    /// own.
    pub segments: Vec<Segment>,
    /// The stretches not dated, and the samples and exception packets they
    /// dropped.
    pub gaps: usize,
    pub dropped: usize,
    pub exceptions_dropped: usize,
    /// The samples and exception packets before the first tick kept, after
    /// the last one, or without a timestamp after them.
    pub undated: usize,
    pub exceptions_undated: usize,
}

/// Stretches between ticks dated one after the other: their cycles, from
/// the first tick to the last, and what they hold, in cycles from the first
/// tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Segment {
    pub cycles: i64,
    /// The index of its watch, its value and its cycles.
    pub samples: Vec<(usize, u32, i64)>,
    pub exceptions: Vec<Exception>,
}

/// What the stream holds that dating reads.
#[derive(Clone, Copy, Debug)]
enum Item {
    /// A packet the timestamp after it dates, the ITM's clock at that
    /// timestamp, and whether it is synchronous and dates this packet alone.
    Dated { what: What, itm: Option<u64>, alone: bool },
    /// An overflow or a malformed packet: packets may be missing.
    Broken,
}

/// A packet dating reads: a data value written, an exception packet, or the
/// DWT's periodic tick.
#[derive(Clone, Copy, Debug)]
enum What {
    Value { comparator: u8, value: u32 },
    Exception { number: u16, function: Function },
    Tick,
}

/// An exception packet, dated: the exception's number, what the core did with
/// it, and its cycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exception {
    pub number: u16,
    pub function: Function,
    pub cycles: i64,
}

/// The samples of a stream, dated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dated {
    /// The index of its watch, its value and its cycles.
    pub samples: Vec<(usize, u32, i64)>,
    pub span: Option<(i64, i64)>,
    pub gaps: Vec<(i64, i64)>,
    pub anchors: usize,
    /// The anchors matched whose timestamp is delayed or shared.
    pub set_aside: usize,
    pub overflows: usize,
    /// The index of its watch and the cycles of the anchor before.
    pub dropped: Vec<(usize, i64)>,
    pub undated: usize,
    pub exceptions: Vec<Exception>,
    pub exceptions_dropped: usize,
    pub exceptions_undated: usize,
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
    let variables: Vec<Watch> = all
        .iter()
        .filter(|watch| watch.comparator != anchor.comparator)
        .copied()
        .collect();
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
        set_aside: dated.set_aside,
        overflows: dated.overflows,
        dropped: dated
            .dropped
            .iter()
            .map(|&(watch, cycles)| (readings[watch].0.clone(), cycles))
            .collect(),
        undated: dated.undated,
        exceptions: dated.exceptions,
        exceptions_dropped: dated.exceptions_dropped,
        exceptions_undated: dated.exceptions_undated,
    }
}

/// What came of the SWO stream, for a summary: a warning when its samples
/// could not be dated, else their counts and what was lost of them.
pub fn summary(watched: &Watched) -> String {
    if let Some(refused) = &watched.refused {
        return format!("warning: the DWT's samples are not dated: {refused}");
    }
    let mut line = format!(
        "{} samples of {} watched variables, {} anchors",
        watched.samples.len(),
        watched.curves.len(),
        watched.anchors
    );
    if watched.set_aside > 0 {
        line += &format!(" ({} set aside: their timestamp delayed or shared)", watched.set_aside);
    }
    if !watched.gaps.is_empty() {
        line += &format!(
            ", {} gaps dropping {} samples ({} overflows)",
            watched.gaps.len(),
            watched.dropped.len(),
            watched.overflows
        );
    }
    if watched.undated > 0 {
        line += &format!(", {} undated outside the anchors", watched.undated);
    }
    if !watched.exceptions.is_empty() || watched.exceptions_dropped > 0 || watched.exceptions_undated > 0 {
        line += &format!("; {} exception packets", watched.exceptions.len());
        if watched.exceptions_dropped > 0 {
            line += &format!(", {} dropped in the gaps", watched.exceptions_dropped);
        }
        if watched.exceptions_undated > 0 {
            line += &format!(", {} undated outside the anchors", watched.exceptions_undated);
        }
    }
    line
}

#[cfg(test)]
mod tests;
