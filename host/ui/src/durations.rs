//! The application's durations on the timeline (ADR-088): a line per type the
//! catalogue declares, its durations in lanes, as `kymo::durations` pairs and
//! places them. They grow with the trace.

use crate::names;
use crate::spans::Spans;
use kymo::durations::{Pairing, Placed};
use kymo::events::{Catalogue, Duration};
use kymo::meaning::Span;
use kymo::model::Element;

/// The most lanes a line shows: the durations of the lanes past it are drawn
/// in its last.
pub const LANES: usize = 4;

pub struct Durations {
    pairing: Pairing,
    /// The durations closed already in a lane.
    placed: usize,
    /// The qualified names of the events the catalogue's begins are of,
    /// sorted.
    pub types: Vec<String>,
    /// Their names as shown (names.rs).
    pub names: Vec<String>,
    /// By type, then by lane: the durations closed, by their place in
    /// `closed`.
    pub lanes: Vec<Vec<Spans>>,
}

impl Durations {
    /// None when the catalogue has no begin.
    pub fn new(catalogue: &Catalogue, first: u32) -> Option<Self> {
        let mut types: Vec<String> = catalogue
            .events
            .values()
            .filter_map(|event| Duration::of(&event.name))
            .filter(|duration| duration.begins)
            .map(|duration| duration.of)
            .collect();
        types.sort();
        types.dedup();
        (!types.is_empty()).then(|| Self {
            pairing: Pairing::new(first),
            placed: 0,
            lanes: types.iter().map(|_| Vec::new()).collect(),
            names: names::types(&types.iter().map(String::as_str).collect::<Vec<_>>()),
            types,
        })
    }

    /// Takes in `elements` from `from` on.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        self.pairing.add(elements, from);
        for (n, placed) in self.pairing.closed.iter().enumerate().skip(self.placed) {
            let Some(row) = self.row(&placed.instance.subject) else {
                continue;
            };
            let lanes = &mut self.lanes[row];
            if lanes.len() <= placed.lane {
                lanes.resize_with(placed.lane + 1, Spans::default);
            }
            lanes[placed.lane].push(span(placed), n);
        }
        self.placed = self.pairing.closed.len();
    }

    /// The line of the durations of `of`.
    pub fn row(&self, of: &str) -> Option<usize> {
        self.types.binary_search_by(|other| other.as_str().cmp(of)).ok()
    }

    /// The durations closed.
    pub fn closed(&self) -> &[Placed] {
        &self.pairing.closed
    }

    /// The durations still open, to the last event read.
    pub fn open(&self) -> Vec<Placed> {
        self.pairing.open()
    }

    /// The lanes of line `row` shows, `open` among its durations: at least
    /// one, at most `LANES`.
    pub fn shown(&self, row: usize, open: &[Placed]) -> usize {
        let of = &self.types[row];
        let open = open.iter().filter(|placed| &placed.instance.subject == of);
        let used = open.map(|placed| placed.lane + 1).max().unwrap_or(0);
        self.lanes[row].len().max(used).clamp(1, LANES)
    }
}

/// The extent of a duration.
pub fn span(placed: &Placed) -> Span {
    Span {
        from: placed.instance.from,
        to: placed.instance.to,
    }
}

#[cfg(test)]
mod tests;
