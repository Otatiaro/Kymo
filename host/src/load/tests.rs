use super::*;
use std::collections::BTreeMap;

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A row's spans, closed and open, as the oracle reads them: each cycle on
/// its own.
struct Row {
    closed: Vec<(i64, i64)>,
    open: Option<i64>,
}

impl Row {
    fn covers(&self, cycle: i64) -> bool {
        self.closed.iter().any(|&(from, to)| from <= cycle && cycle < to) || self.open.is_some_and(|open| open <= cycle)
    }

    fn covered(&self) -> Covered {
        let mut covered = Covered::default();
        for &(from, to) in &self.closed {
            covered.push(from, to);
        }
        covered.open(self.open);
        covered
    }
}

/// The bins the oracle finds, cycle by cycle: each cycle of the capture in
/// the bin `floor((cycle - origin) / width)`, the bins holding a cycle of
/// the range kept, with their first and last cycle in the capture and the
/// cycles the row covers among theirs.
fn oracle(row: &Row, width: i64, origin: i64, range: (i64, i64), capture: (i64, i64)) -> Vec<Bin> {
    let mut found: BTreeMap<i64, (Bin, bool)> = BTreeMap::new();
    for cycle in capture.0..capture.1 {
        let n = ((cycle - origin) as f64 / width as f64).floor() as i64;
        let (bin, kept) = found.entry(n).or_insert((
            Bin {
                from: cycle,
                to: cycle,
                covered: 0,
            },
            false,
        ));
        bin.to = cycle + 1;
        bin.covered += i64::from(row.covers(cycle));
        *kept |= range.0 <= cycle && cycle < range.1;
    }
    found
        .into_values()
        .filter(|(_, kept)| *kept)
        .map(|(bin, _)| bin)
        .collect()
}

/// Every bin, over random rows, captures, origins, views and zooms from a
/// tenth of a cycle to a thousand cycles per pixel, is the oracle's: its
/// bounds and the cycles covered in it.
#[test]
fn bins_count_the_cycles_covered_as_the_oracle_does() {
    let mut state = 0x9e37_79b9_7f4a_7c15;
    let (mut open, mut cut, mut many) = (0, 0, 0);
    for _ in 0..3000 {
        let base = (random(&mut state) % 2_000_000) as i64 - 1_000_000;
        let capture = (base, base + 1 + (random(&mut state) % 6000) as i64);
        // The origin is the first event in the trace's order, near the
        // earliest.
        let origin = capture.0 + (random(&mut state) % 50) as i64;
        let mut row = Row {
            closed: Vec::new(),
            open: None,
        };
        let mut at = capture.0 - (random(&mut state) % 20) as i64;
        while at < capture.1 {
            // Spans touching or apart, empty to long.
            at += match random(&mut state) % 3 {
                0 => 0,
                _ => (random(&mut state) % 300) as i64,
            };
            let to = at
                + match random(&mut state) % 4 {
                    0 => 0,
                    1 => (random(&mut state) % 2000) as i64,
                    _ => (random(&mut state) % 30) as i64,
                };
            row.closed.push((at, to));
            at = to;
        }
        if random(&mut state).is_multiple_of(2) {
            let from = at - (random(&mut state) % 500) as i64;
            row.closed.retain(|&(_, to)| to <= from);
            row.open = Some(from);
            open += 1;
        }
        let per_pixel = 10f64.powf((random(&mut state) % 400) as f64 / 100.0 - 1.0);
        let width = width(per_pixel);
        let low = capture.0 - 100 + (random(&mut state) % (capture.1 - capture.0 + 200) as u64) as i64;
        let range = (low, low + 1 + (random(&mut state) % 3000) as i64);
        let expected = oracle(&row, width, origin, range, capture);
        let bins = bins(&row.covered(), width, origin, range, capture);
        assert_eq!(
            bins, expected,
            "width {width} origin {origin} range {range:?} capture {capture:?}"
        );
        cut += usize::from(
            bins.first().is_some_and(|bin| bin.from == capture.0) || bins.last().is_some_and(|bin| bin.to == capture.1),
        );
        many += usize::from(bins.len() > 3);
    }
    assert!(
        open > 1000 && cut > 1000 && many > 1000,
        "{open} open, {cut} cut, {many} with more than three bins"
    );
}

/// A bin is the shortest 1, 2 or 5 × 10ⁿ cycles at least `PIXELS` wide.
#[test]
fn a_bin_is_the_shortest_round_width_a_few_pixels_wide() {
    let round: Vec<i64> = (0..10).flat_map(|n| [1, 2, 5].map(|m| m * 10i64.pow(n))).collect();
    for per_pixel in [1e-3, 0.1, 0.25, 0.26, 1.0, 1.3, 12.5, 12.6, 1e4, 3.7e6] {
        let width = width(per_pixel);
        let wanted = PIXELS * per_pixel;
        let at = round.iter().position(|&round| round == width).expect("a round width");
        assert!(width as f64 >= wanted, "{per_pixel}: {width}");
        assert!(at == 0 || (round[at - 1] as f64) < wanted, "{per_pixel}: {width}");
    }
}

/// Panning moves no bin: bins in two views of the same capture, counted
/// from an origin away from 0, are the same where the views meet.
#[test]
fn panning_moves_no_bin() {
    let row = Row {
        closed: vec![(1_000_003, 1_000_040), (1_000_041, 1_000_090)],
        open: Some(1_000_150),
    };
    let (origin, capture) = (1_000_001, (1_000_000, 1_000_200));
    let left = bins(&row.covered(), 20, origin, (1_000_000, 1_000_100), capture);
    let right = bins(&row.covered(), 20, origin, (1_000_033, 1_000_133), capture);
    let shared: Vec<&Bin> = left.iter().filter(|bin| right.contains(bin)).collect();
    assert_eq!(shared.len(), 4, "{left:?} {right:?}");
    assert_eq!(
        left[1],
        Bin {
            from: 1_000_001,
            to: 1_000_021,
            covered: 18
        }
    );
}

/// The bins at the capture's edges hold only its cycles, and a span still
/// open covers to its end.
#[test]
fn edge_bins_are_cut_to_the_capture_and_an_open_span_covers_to_its_end() {
    let row = Row {
        closed: Vec::new(),
        open: Some(95),
    };
    let found = bins(&row.covered(), 50, 0, (0, 1000), (10, 120));
    assert_eq!(
        found,
        [
            Bin {
                from: 10,
                to: 50,
                covered: 0
            },
            Bin {
                from: 50,
                to: 100,
                covered: 5
            },
            Bin {
                from: 100,
                to: 120,
                covered: 20
            },
        ]
    );
}

/// A view of less than a cycle per pixel has bins of a cycle: an idle row
/// covers none of them.
#[test]
fn bins_narrower_than_a_cycle_are_a_cycle() {
    let row = Row {
        closed: vec![(0, 5), (5, 10)],
        open: None,
    };
    assert_eq!(width(0.01), 1);
    let found = bins(&row.covered(), 1, 0, (10, 13), (0, 20));
    assert!(
        found.iter().all(|bin| bin.covered == 0 && bin.to - bin.from == 1),
        "{found:?}"
    );
    assert_eq!(found.len(), 3);
}
