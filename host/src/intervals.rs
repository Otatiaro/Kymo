//! The delay from each event a filter A keeps to the first event a filter B
//! keeps before the next A: what kymo stats measures on the kernel's spans,
//! between any two kinds of events.
//!
//! An event both filters keep ends the interval open, as a B, then opens
//! the next, as an A. An A followed by the next A, with no B between, has
//! no B. The rules are those of stats.rs, an interval running from its A to
//! its B, or to the next A when it has none:
//! - one that a loss touches (a loss between its ends), or whose indexes a
//!   suspect line meets, is excluded and counted: its B may be hidden there;
//! - one that a range edge cuts, or still open at the trace's end, is cut;
//! - one wholly outside the range is not counted at all.

use crate::model::{Element, Trace};
use crate::stats::{self, Distribution, Point, Range};
use crate::symbols::Symbols;
use crate::words::Words;
use std::collections::HashMap;

/// The intervals from A to B over a range.
#[derive(Debug, PartialEq)]
pub struct Intervals {
    /// The intervals counted, each from an A to its B.
    pub count: usize,
    /// The A followed by the next A with no B between.
    pub without: usize,
    pub excluded_loss: usize,
    pub excluded_suspect: usize,
    pub cut: usize,
    /// None without an interval counted.
    pub distribution: Option<Distribution>,
    /// By the element of each A counted: the index of its B.
    pub ends: HashMap<usize, u32>,
}

/// An event's place: its element, index and instant.
#[derive(Clone, Copy)]
struct At {
    element: usize,
    index: u32,
    cycles: i64,
    seconds: Option<f64>,
}

/// An interval as the trace gives it, before the range judges it.
struct Raw {
    from: At,
    /// Its B, or the next A, or none when the trace ends first.
    to: Option<At>,
    reached: bool,
}

/// The intervals of `trace` from each event `a` keeps to the first `b` keeps
/// before the next A, over `range`, the `worst` longest kept.
pub fn of(trace: &Trace, symbols: &Symbols, a: &Words, b: &Words, range: Range, worst: usize) -> Intervals {
    let mut raws: Vec<Raw> = Vec::new();
    let mut losses: Vec<usize> = Vec::new();
    let mut suspects: Vec<(u32, u32)> = Vec::new();
    let mut open: Option<At> = None;
    for (element, item) in trace.elements.iter().enumerate() {
        let (event, cycles, seconds) = match item {
            Element::Event { event, cycles, seconds } => (event, *cycles, *seconds),
            Element::Lost { .. } => {
                losses.push(element);
                continue;
            }
            Element::Suspect { from, to, .. } => {
                suspects.push((*from, *to));
                continue;
            }
        };
        let at = At {
            element,
            index: event.index,
            cycles,
            seconds,
        };
        if b.keeps(symbols, event)
            && let Some(from) = open.take()
        {
            raws.push(Raw {
                from,
                to: Some(at),
                reached: true,
            });
        }
        if a.keeps(symbols, event) {
            if let Some(from) = open.take() {
                raws.push(Raw {
                    from,
                    to: Some(at),
                    reached: false,
                });
            }
            open = Some(at);
        }
    }
    if let Some(from) = open {
        raws.push(Raw {
            from,
            to: None,
            reached: false,
        });
    }

    let low = range.from.unwrap_or(i64::MIN);
    let high = range.to.unwrap_or(i64::MAX);
    let mut out = Intervals {
        count: 0,
        without: 0,
        excluded_loss: 0,
        excluded_suspect: 0,
        cut: 0,
        distribution: None,
        ends: HashMap::new(),
    };
    let mut points = Vec::new();
    for raw in &raws {
        let start = raw.from.cycles;
        let end = raw.to.map_or(i64::MAX, |to| to.cycles);
        let whole = raw.to.is_some() && start >= low && end <= high;
        if !whole && !(start < high && end >= low) {
            continue;
        }
        if !whole {
            out.cut += 1;
            continue;
        }
        let to = raw.to.expect("a whole interval has its end");
        let after = losses.partition_point(|&loss| loss <= raw.from.element);
        if losses.get(after).is_some_and(|&loss| loss < to.element) {
            out.excluded_loss += 1;
        } else if suspects
            .iter()
            .any(|&(from, until)| from <= to.index && until > raw.from.index)
        {
            out.excluded_suspect += 1;
        } else if !raw.reached {
            out.without += 1;
        } else {
            out.count += 1;
            out.ends.insert(raw.from.element, to.index);
            points.push(Point {
                value: end - start,
                element: raw.from.element,
                index: raw.from.index,
                cycles: start,
                seconds: raw.from.seconds,
            });
        }
    }
    out.distribution = stats::distribution(&points, worst);
    out
}

#[cfg(test)]
mod tests;
