//! The application's durations: each begin paired with its end
//! (target/include/kymo/kymo.hpp). A duration belongs to no context of the
//! kernel: a task that ends or a handler that exits closes nothing.
//!
//! - Without a key, an end closes the last begin of its type still open:
//!   durations of a type nest as a stack.
//! - With a key, an end closes the begin of its type opened under the same
//!   key, in any order.
//!
//! What contradicts this is reported, never repaired: an end that closes
//! nothing, a key opened again while open, a key closed while not open. A
//! begin still open at the end of the trace is open, as stats.rs counts
//! the cut. At a loss, the durations open are closed by it, their end
//! unknown: stats.rs excludes them. From then on, and from the start of a
//! trace joined after index 0, an end that closes nothing may close a
//! begin the trace did not read: it is not a contradiction.

use crate::events::{DURATION_KEY, Duration, Event};
use crate::model::{Edge, Element, Trace};
use crate::stats::{Instance, Metric};
use std::collections::BTreeMap;

/// A duration's metric: from its begin to its end.
pub const DURATION: Metric = Metric {
    name: "duration",
    share: false,
    jitter: false,
};

/// The kind of subject of a duration's instances; the subject is the
/// event the duration is of.
pub const KIND: &str = "duration";

/// The durations of a trace and the elements that contradict them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Paired {
    /// By subject, then begin.
    pub instances: Vec<Instance>,
    pub contradictions: Vec<usize>,
}

/// A duration, and its lane: the lowest free at its begin among the lanes
/// of its type's durations then open, so that a type's durations open at
/// once each have their own, and a type needs as many lanes as it ever has
/// durations open at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub instance: Instance,
    pub lane: usize,
}

/// A begin still open: where it starts, and its lane.
type Begun = (Edge, usize);

/// The key of a keyed begin or end, read from its payload.
fn key(event: &Event, payload: &[u8]) -> Option<u64> {
    let field = event.fields.iter().find(|field| field.name == DURATION_KEY)?;
    let size = usize::try_from(field.ty.size?).ok()?;
    let offset = usize::try_from(field.offset).ok()?;
    let bytes = payload.get(offset..offset + size)?;
    Some(bytes.iter().rev().fold(0, |value, byte| value << 8 | u64::from(*byte)))
}

/// The pairing of a trace's begins and ends, as its elements come in.
#[derive(Debug, Default)]
pub struct Pairing {
    stacks: BTreeMap<String, Vec<Begun>>,
    keyed: BTreeMap<(String, u64), Begun>,
    /// By type, whether each lane is taken.
    taken: BTreeMap<String, Vec<bool>>,
    /// Whether a begin may have gone unread: after a loss, or from the start
    /// of a trace joined late.
    unread: bool,
    last: Option<Edge>,
    /// The durations closed, in the order they close: in a lane, in the
    /// order they begin.
    pub closed: Vec<Placed>,
    pub contradictions: Vec<usize>,
}

impl Pairing {
    /// The pairing of a trace whose first index is `first`.
    pub fn new(first: u32) -> Self {
        Self {
            unread: first != 0,
            ..Self::default()
        }
    }

    /// The lowest lane of type `of` free, taken.
    fn take(&mut self, of: &str) -> usize {
        let lanes = self.taken.entry(of.to_owned()).or_default();
        let lane = lanes.iter().position(|taken| !taken).unwrap_or(lanes.len());
        if lane == lanes.len() {
            lanes.push(true);
        } else {
            lanes[lane] = true;
        }
        lane
    }

    fn free(&mut self, of: &str, lane: usize) {
        if let Some(taken) = self.taken.get_mut(of).and_then(|lanes| lanes.get_mut(lane)) {
            *taken = false;
        }
    }

    fn close(&mut self, of: String, (from, lane): Begun, to: Edge) {
        self.free(&of, lane);
        self.closed.push(Placed {
            instance: instance(of, from, to, false),
            lane,
        });
    }

    /// Takes in `elements` from `from` on.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        for (element, item) in elements.iter().enumerate().skip(from) {
            match item {
                Element::Event { event, cycles, .. } => {
                    let edge = Edge {
                        element,
                        cycles: *cycles,
                        lost: false,
                    };
                    self.last = Some(edge);
                    let Some(duration) = Duration::of(&event.event.name) else {
                        continue;
                    };
                    let of = duration.of;
                    let key = if duration.keyed {
                        key(event.event, &event.payload)
                    } else {
                        None
                    };
                    match (duration.begins, key) {
                        (true, None) => {
                            let lane = self.take(&of);
                            self.stacks.entry(of).or_default().push((edge, lane));
                        }
                        (true, Some(key)) => {
                            let lane = self.take(&of);
                            if let Some((_, dropped)) = self.keyed.insert((of.clone(), key), (edge, lane)) {
                                self.free(&of, dropped);
                                self.contradictions.push(element);
                            }
                        }
                        (false, None) => match self.stacks.get_mut(&of).and_then(Vec::pop) {
                            Some(begun) => self.close(of, begun, edge),
                            None if self.unread => {}
                            None => self.contradictions.push(element),
                        },
                        (false, Some(key)) => match self.keyed.remove(&(of.clone(), key)) {
                            Some(begun) => self.close(of, begun, edge),
                            None if self.unread => {}
                            None => self.contradictions.push(element),
                        },
                    }
                }
                Element::Lost { .. } => {
                    let at = Edge {
                        element,
                        cycles: self.last.map_or(0, |edge| edge.cycles),
                        lost: true,
                    };
                    for (of, begun) in self.begun() {
                        self.close(of, begun, at);
                    }
                    self.stacks.clear();
                    self.keyed.clear();
                    self.unread = true;
                }
                Element::Suspect { .. } => {}
            }
        }
    }

    /// The begins still open, by type, a type's unkeyed first.
    fn begun(&self) -> Vec<(String, Begun)> {
        let stacks = self
            .stacks
            .iter()
            .flat_map(|(of, begins)| begins.iter().map(move |&begun| (of.clone(), begun)));
        let keyed = self.keyed.iter().map(|((of, _), &begun)| (of.clone(), begun));
        stacks.chain(keyed).collect()
    }

    /// The durations still open, to the last event read.
    pub fn open(&self) -> Vec<Placed> {
        let Some(end) = self.last else {
            return Vec::new();
        };
        self.begun()
            .into_iter()
            .map(|(of, (from, lane))| Placed {
                instance: instance(of, from, end, true),
                lane,
            })
            .collect()
    }
}

/// A duration of `of` from `from` to `to`.
fn instance(of: String, from: Edge, to: Edge, open: bool) -> Instance {
    Instance {
        kind: KIND,
        subject: of,
        metric: DURATION,
        detail: None,
        value: to.cycles - from.cycles,
        from,
        to,
        open,
    }
}

/// Pairs the begins and ends of `trace`.
pub fn pair(trace: &Trace) -> Paired {
    let mut pairing = Pairing::new(trace.first);
    pairing.add(&trace.elements, 0);
    let open = pairing.open();
    let mut instances: Vec<Instance> = pairing
        .closed
        .into_iter()
        .chain(open)
        .map(|placed| placed.instance)
        .collect();
    instances.sort_by(|a, b| (&a.subject, a.from.element).cmp(&(&b.subject, b.from.element)));
    Paired {
        instances,
        contradictions: pairing.contradictions,
    }
}

#[cfg(test)]
mod tests;
