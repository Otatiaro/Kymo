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

/// A begin still open: the duration's type, and where it starts.
type Opened = (String, Edge);

/// The key of a keyed begin or end, read from its payload.
fn key(event: &Event, payload: &[u8]) -> Option<u64> {
    let field = event.fields.iter().find(|field| field.name == DURATION_KEY)?;
    let size = usize::try_from(field.ty.size?).ok()?;
    let offset = usize::try_from(field.offset).ok()?;
    let bytes = payload.get(offset..offset + size)?;
    Some(bytes.iter().rev().fold(0, |value, byte| value << 8 | u64::from(*byte)))
}

/// Pairs the begins and ends of `trace`.
pub fn pair(trace: &Trace) -> Paired {
    let mut paired = Paired::default();
    let mut stacks: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
    let mut keyed: BTreeMap<(String, u64), Edge> = BTreeMap::new();
    // Whether a begin may have gone unread: after a loss, or from the start
    // of a trace joined late.
    let mut unread = trace.first != 0;
    let mut last: Option<Edge> = None;
    let close = |paired: &mut Paired, (of, from): Opened, to: Edge, open: bool| {
        paired.instances.push(Instance {
            kind: KIND,
            subject: of,
            metric: DURATION,
            detail: None,
            value: to.cycles - from.cycles,
            from,
            to,
            open,
        });
    };
    for (element, item) in trace.elements.iter().enumerate() {
        match item {
            Element::Event { event, cycles, .. } => {
                let edge = Edge {
                    element,
                    cycles: *cycles,
                    lost: false,
                };
                last = Some(edge);
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
                    (true, None) => stacks.entry(of).or_default().push(edge),
                    (true, Some(key)) => {
                        if keyed.insert((of, key), edge).is_some() {
                            paired.contradictions.push(element);
                        }
                    }
                    (false, None) => match stacks.get_mut(&of).and_then(Vec::pop) {
                        Some(from) => close(&mut paired, (of, from), edge, false),
                        None if unread => {}
                        None => paired.contradictions.push(element),
                    },
                    (false, Some(key)) => match keyed.remove(&(of.clone(), key)) {
                        Some(from) => close(&mut paired, (of, from), edge, false),
                        None if unread => {}
                        None => paired.contradictions.push(element),
                    },
                }
            }
            Element::Lost { .. } => {
                let at = Edge {
                    element,
                    cycles: last.map_or(0, |edge| edge.cycles),
                    lost: true,
                };
                for (of, from) in std::mem::take(&mut stacks)
                    .into_iter()
                    .flat_map(|(of, froms)| froms.into_iter().map(move |from| (of.clone(), from)))
                    .chain(std::mem::take(&mut keyed).into_iter().map(|((of, _), from)| (of, from)))
                {
                    close(&mut paired, (of, from), at, false);
                }
                unread = true;
            }
            Element::Suspect { .. } => {}
        }
    }
    if let Some(end) = last {
        for (of, from) in stacks
            .into_iter()
            .flat_map(|(of, froms)| froms.into_iter().map(move |from| (of.clone(), from)))
            .chain(keyed.into_iter().map(|((of, _), from)| (of, from)))
        {
            close(&mut paired, (of, from), end, true);
        }
    }
    paired
        .instances
        .sort_by(|a, b| (&a.subject, a.from.element).cmp(&(&b.subject, b.from.element)));
    paired
}

#[cfg(test)]
mod tests;
