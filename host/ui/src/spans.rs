//! The spans of one row of the kernel's meaning: a task's runs, a handler's, a
//! mutex's holders or one of its waiters. They do not overlap, so they are
//! sorted by their start and by their end alike, as the trace's cycles are:
//! a view finds those it holds by a binary search, and a pixel column
//! however many spans fall in it by another.

use crate::timeline::View;
use kymo::meaning::Span;
use std::ops::Range;

#[derive(Default)]
pub struct Spans {
    from: Vec<i64>,
    to: Vec<i64>,
    /// Each span's place in the list of `kymo::meaning::Spans` it comes from.
    which: Vec<usize>,
}

/// Pixel columns `from..to` covered by `spans`, indices into the row.
#[derive(Debug, PartialEq)]
pub struct Band {
    pub from: f32,
    pub to: f32,
    pub spans: Range<usize>,
}

impl Spans {
    /// Appends `span`, which ends no earlier than those held.
    pub fn push(&mut self, span: Span, which: usize) {
        self.from.push(span.from.cycles);
        self.to.push(span.to.cycles.max(span.from.cycles));
        self.which.push(which);
    }

    /// The spans' starts, in cycles, sorted.
    pub fn starts(&self) -> &[i64] {
        &self.from
    }

    /// The pixel columns of `view` holding a span, merged into bands where
    /// they touch, as `Levels::columns` merges events: a span covers the
    /// columns of its start to that of its end.
    pub fn bands(&self, view: &View) -> Vec<Band> {
        let column = |cycles: i64| view.column(cycles);
        let right = view.left + view.width as f32;
        // The spans in view: ending in a column of the view or past it, and
        // starting in one or before it.
        let mut n = self.to.partition_point(|&to| column(to) < view.left);
        let stop = self.from.partition_point(|&from| column(from) < right);
        let mut bands = Vec::new();
        while n < stop {
            let from = column(self.from[n]);
            let mut to = column(self.to[n]) + 1.0;
            let mut end = n + 1;
            // The spans starting in a column the band covers join it: each
            // round takes them all, and goes on only if the last, which ends
            // latest, reaches further.
            loop {
                let next = end + gallop(&self.from[end..stop], |&start| column(start) <= to);
                if next == end {
                    break;
                }
                to = column(self.to[next - 1]) + 1.0;
                end = next;
            }
            bands.push(Band {
                from,
                to,
                spans: n..end,
            });
            n = end;
        }
        bands
    }

    /// The pixel columns of span `n` in `view`, `from..to`, and its place.
    pub fn columns(&self, n: usize, view: &View) -> (f32, f32, usize) {
        (view.column(self.from[n]), view.column(self.to[n]) + 1.0, self.which[n])
    }

    /// The place of the span whose pixel columns in `view`, as `bands` draws
    /// them, hold column `x`.
    pub fn at_column(&self, view: &View, x: f32) -> Option<usize> {
        let x = x.floor();
        let n = self.to.partition_point(|&to| view.column(to) < x);
        (n < self.from.len() && view.column(self.from[n]) <= x).then(|| self.which[n])
    }

    /// How many spans `view` shows, as `bands` finds them.
    pub fn count(&self, view: &View) -> usize {
        let right = view.left + view.width as f32;
        let n = self.to.partition_point(|&to| view.column(to) < view.left);
        self.from
            .partition_point(|&from| view.column(from) < right)
            .saturating_sub(n)
    }

    /// The places of the spans with cycles from `low` to before `high`.
    pub fn overlapping(&self, low: i64, high: i64) -> impl Iterator<Item = usize> + '_ {
        let n = self.to.partition_point(|&to| to <= low);
        let stop = self.from.partition_point(|&from| from < high);
        self.which[n..stop.max(n)].iter().copied()
    }

    /// The place of the first span from `low` to `high`, cycles included.
    pub fn at(&self, low: i64, high: i64) -> Option<usize> {
        let n = self.to.partition_point(|&to| to < low);
        (n < self.from.len() && self.from[n] <= high).then(|| self.which[n])
    }
}

/// `partition_point`, from the start of `items` by doubling steps: what it
/// costs grows with the place found, not with the length of `items`.
fn gallop<T>(items: &[T], mut before: impl FnMut(&T) -> bool) -> usize {
    let mut bound = 1;
    while bound <= items.len() && before(&items[bound - 1]) {
        bound *= 2;
    }
    let low = bound / 2;
    low + items[low..bound.min(items.len())].partition_point(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kymo::meaning::Edge;

    fn random(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn span(from: i64, to: i64) -> Span {
        let edge = |cycles| Edge {
            element: 0,
            cycles,
            lost: false,
        };
        Span {
            from: edge(from),
            to: edge(to),
        }
    }

    /// The bands drawn are the pixel columns of every span in view, each
    /// span placed on its own, at zooms from a few cycles to millions per
    /// pixel; a band names the spans in view it covers, with their columns.
    #[test]
    fn bands_cover_the_columns_of_the_spans_in_view() {
        let mut state = 0x9e37_79b9_7f4a_7c15;
        let mut singles = 0;
        let mut merged = 0;
        for _ in 0..2000 {
            let mut at = 1000 + (random(&mut state) % 1000) as i64;
            let mut spans = Spans::default();
            let mut placed = Vec::new();
            for which in 0..random(&mut state) % 300 {
                // Spans short or long, touching or far apart.
                at += match random(&mut state) % 4 {
                    0 => 0,
                    1 => (random(&mut state) % 100_000) as i64,
                    _ => (random(&mut state) % 50) as i64,
                };
                let from = at;
                at += if random(&mut state).is_multiple_of(5) {
                    (random(&mut state) % 200_000) as i64
                } else {
                    (random(&mut state) % 40) as i64
                };
                // A span's place is not its index in the row.
                spans.push(span(from, at), 3 * which as usize + 1);
                placed.push((from, at));
            }
            let span = 10f64.powi((random(&mut state) % 7) as i32) * (100 + random(&mut state) % 900) as f64;
            let origin = 1000;
            // A third of the views start or end on a span's edge, within a
            // cycle: the spans at the view's bounds.
            let mut start = (random(&mut state) % 400_000) as f64 - 1000.0;
            if !placed.is_empty() && random(&mut state).is_multiple_of(3) {
                let (from, to) = placed[random(&mut state) as usize % placed.len()];
                let shift = (random(&mut state) % 9) as f64 / 8.0 - 0.5;
                start = if random(&mut state).is_multiple_of(2) {
                    (to - origin) as f64 + shift
                } else {
                    (from - origin) as f64 + shift - span
                };
            }
            let view = View {
                origin,
                start,
                span,
                left: 260.0,
                width: 100.0 + (random(&mut state) % 1500) as f64,
            };
            let right = view.left + view.width as f32;
            let column = |cycles: i64| view.x((cycles - view.origin) as f64).floor();
            let place = |which: usize| (which - 1) / 3;
            let mut expected: Vec<f32> = placed
                .iter()
                .flat_map(|&(from, to)| (column(from) as i64..=column(to) as i64).map(|x| x as f32))
                .filter(|&x| x >= view.left && x < right)
                .collect();
            expected.sort_by(f32::total_cmp);
            expected.dedup();
            let bands = spans.bands(&view);
            let drawn: Vec<f32> = bands
                .iter()
                .flat_map(|band| (band.from as i64..band.to as i64).map(|x| x as f32))
                .filter(|&x| x >= view.left && x < right)
                .collect();
            assert_eq!(drawn, expected, "{placed:?}");
            // Bands that touched would be one.
            for pair in bands.windows(2) {
                assert!(pair[0].to < pair[1].from, "{placed:?}");
            }
            for band in &bands {
                // A band covers the columns of the spans in view that start
                // in it.
                let inside: Vec<usize> = placed
                    .iter()
                    .enumerate()
                    .filter(|(_, (from, to))| {
                        column(*to) >= view.left
                            && column(*from) < right
                            && column(*from) >= band.from
                            && column(*from) < band.to
                            && column(*to) < band.to
                    })
                    .map(|(n, _)| n)
                    .collect();
                let named: Vec<usize> = band.spans.clone().map(|n| place(spans.columns(n, &view).2)).collect();
                assert_eq!(inside, named, "{placed:?}");
                for n in band.spans.clone() {
                    let (from, to, which) = spans.columns(n, &view);
                    let (start, end) = placed[place(which)];
                    assert_eq!((from, to), (column(start), column(end) + 1.0));
                }
                if named.len() == 1 {
                    singles += 1;
                } else {
                    merged += 1;
                }
            }
        }
        assert!(singles > 1500 && merged > 1500, "{singles} single, {merged} merged");
    }

    #[test]
    fn gallop_finds_the_partition_point() {
        for len in 0..70 {
            let items: Vec<usize> = (0..len).collect();
            for split in 0..=len {
                assert_eq!(gallop(&items, |&item| item < split), split, "{len} {split}");
            }
        }
    }

    #[test]
    fn the_span_at_cycles_is_the_first_reaching_them() {
        let mut spans = Spans::default();
        for (which, (from, to)) in [(10, 20), (20, 30), (40, 50)].into_iter().enumerate() {
            spans.push(span(from, to), 10 + which);
        }
        assert_eq!(spans.at(0, 9), None);
        assert_eq!(spans.at(0, 10), Some(10));
        assert_eq!(spans.at(20, 20), Some(10));
        assert_eq!(spans.at(21, 25), Some(11));
        assert_eq!(spans.at(31, 39), None);
        assert_eq!(spans.at(35, 45), Some(12));
        assert_eq!(spans.at(51, 60), None);
    }
}
