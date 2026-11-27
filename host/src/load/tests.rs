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

/// Random spans from before `capture` to its end, touching or apart, empty
/// to long, the last of them maybe still open.
fn row(state: &mut u64, capture: (i64, i64)) -> Row {
    let mut row = Row {
        closed: Vec::new(),
        open: None,
    };
    let mut at = capture.0 - (random(state) % 20) as i64;
    while at < capture.1 {
        at += match random(state) % 3 {
            0 => 0,
            _ => (random(state) % 300) as i64,
        };
        let to = at
            + match random(state) % 4 {
                0 => 0,
                1 => (random(state) % 2000) as i64,
                _ => (random(state) % 30) as i64,
            };
        row.closed.push((at, to));
        at = to;
    }
    if random(state).is_multiple_of(2) {
        let from = at - (random(state) % 500) as i64;
        row.closed.retain(|&(_, to)| to <= from);
        row.open = Some(from);
    }
    row
}

/// The bins the oracle finds, cycle by cycle: each cycle of the capture in
/// the bin `floor((cycle - origin) / width)`, the bins holding a cycle of
/// the range kept, with their first and last cycle in the capture, the
/// cycles known among theirs and those the row covers among the known.
fn oracle(row: &Row, known: &Row, width: i64, origin: i64, range: (i64, i64), capture: (i64, i64)) -> Vec<Bin> {
    let mut found: BTreeMap<i64, (Bin, bool)> = BTreeMap::new();
    for cycle in capture.0..capture.1 {
        let n = ((cycle - origin) as f64 / width as f64).floor() as i64;
        let (bin, kept) = found.entry(n).or_insert((
            Bin {
                from: cycle,
                to: cycle,
                covered: 0,
                known: 0,
            },
            false,
        ));
        bin.to = cycle + 1;
        bin.known += i64::from(known.covers(cycle));
        bin.covered += i64::from(known.covers(cycle) && row.covers(cycle));
        *kept |= range.0 <= cycle && cycle < range.1;
    }
    found
        .into_values()
        .filter(|(_, kept)| *kept)
        .map(|(bin, _)| bin)
        .collect()
}

/// Every bin, over random rows, known times, captures, origins, views and
/// zooms from a tenth of a cycle to a thousand cycles per pixel, is the
/// oracle's: its bounds, the cycles known in it and those covered among
/// them.
#[test]
fn bins_count_the_cycles_covered_and_known_as_the_oracle_does() {
    let mut state = 0x9e37_79b9_7f4a_7c15;
    let (mut open, mut cut, mut many, mut partly, mut beyond) = (0, 0, 0, 0, 0);
    for _ in 0..3000 {
        let base = (random(&mut state) % 2_000_000) as i64 - 1_000_000;
        let capture = (base, base + 1 + (random(&mut state) % 6000) as i64);
        // The origin is the first event in the trace's order, near the
        // earliest.
        let origin = capture.0 + (random(&mut state) % 50) as i64;
        let row = row(&mut state, capture);
        // Known everywhere, or between losses.
        let known = if random(&mut state).is_multiple_of(4) {
            Row {
                closed: Vec::new(),
                open: Some(capture.0 - 1),
            }
        } else {
            self::row(&mut state, capture)
        };
        open += usize::from(row.open.is_some());
        let wanted = 10f64.powf((random(&mut state) % 400) as f64 / 100.0 - 1.0);
        let width = width(wanted);
        let low = capture.0 - 100 + (random(&mut state) % (capture.1 - capture.0 + 200) as u64) as i64;
        let range = (low, low + 1 + (random(&mut state) % 3000) as i64);
        let expected = oracle(&row, &known, width, origin, range, capture);
        let (covered, known_covered) = (row.covered(), known.covered());
        let bins = bins(
            &covered.within(&known_covered),
            &known_covered,
            width,
            origin,
            range,
            capture,
        );
        assert_eq!(
            bins, expected,
            "width {width} origin {origin} range {range:?} capture {capture:?}"
        );
        cut += usize::from(
            bins.first().is_some_and(|bin| bin.from == capture.0) || bins.last().is_some_and(|bin| bin.to == capture.1),
        );
        many += usize::from(bins.len() > 3);
        partly += usize::from(bins.iter().any(|bin| 0 < bin.known && bin.known < bin.to - bin.from));
        // The row covers cycles not known, which the share leaves out.
        beyond += usize::from(bins.iter().any(|bin| {
            let whole = covered.until(bin.to) - covered.until(bin.from);
            whole > bin.covered
        }));
    }
    assert!(
        open > 1000 && cut > 1000 && many > 1000 && partly > 1000 && beyond > 1000,
        "{open} open, {cut} cut, {many} with more than three bins, {partly} partly known, {beyond} covering the unknown"
    );
}

/// A row cut to the time known keeps the cycles both cover, an open span
/// open only where both are.
#[test]
fn a_row_within_the_time_known_keeps_what_both_cover() {
    let both = |row: Row, known: Row| {
        let within = row.covered().within(&known.covered());
        (
            within
                .from
                .iter()
                .copied()
                .zip(within.to.iter().copied())
                .collect::<Vec<_>>(),
            within.open,
        )
    };
    let row = || Row {
        closed: vec![(0, 10), (15, 30)],
        open: Some(40),
    };
    assert_eq!(
        both(
            row(),
            Row {
                closed: vec![(5, 20), (25, 45)],
                open: Some(50)
            }
        ),
        (vec![(5, 10), (15, 20), (25, 30), (40, 45)], Some(50))
    );
    assert_eq!(
        both(
            row(),
            Row {
                closed: vec![(0, 100)],
                open: None
            }
        ),
        (vec![(0, 10), (15, 30), (40, 100)], None)
    );
    assert_eq!(
        both(
            row(),
            Row {
                closed: Vec::new(),
                open: Some(20)
            }
        ),
        (vec![(20, 30)], Some(40))
    );
}

/// A bin is the shortest 1, 2 or 5 × 10ⁿ cycles at least as long as wanted.
#[test]
fn a_bin_is_the_shortest_round_width_as_long_as_wanted() {
    let round: Vec<i64> = (0..10).flat_map(|n| [1, 2, 5].map(|m| m * 10i64.pow(n))).collect();
    for wanted in [4e-3, 0.4, 1.0, 1.04, 4.0, 5.2, 50.0, 50.4, 4e4, 1.48e7] {
        let width = width(wanted);
        let at = round.iter().position(|&round| round == width).expect("a round width");
        assert!(width as f64 >= wanted, "{wanted}: {width}");
        assert!(at == 0 || (round[at - 1] as f64) < wanted, "{wanted}: {width}");
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
    let known = known_from(capture.0);
    let left = bins(&row.covered(), &known, 20, origin, (1_000_000, 1_000_100), capture);
    let right = bins(&row.covered(), &known, 20, origin, (1_000_033, 1_000_133), capture);
    let shared: Vec<&Bin> = left.iter().filter(|bin| right.contains(bin)).collect();
    assert_eq!(shared.len(), 4, "{left:?} {right:?}");
    assert_eq!(
        left[1],
        Bin {
            from: 1_000_001,
            to: 1_000_021,
            covered: 18,
            known: 20
        }
    );
}

/// The time known from `from` to the end of the capture.
fn known_from(from: i64) -> Covered {
    let mut known = Covered::default();
    known.open(Some(from));
    known
}

/// The bins at the capture's edges hold only its cycles, and a span still
/// open covers to its end.
#[test]
fn edge_bins_are_cut_to_the_capture_and_an_open_span_covers_to_its_end() {
    let row = Row {
        closed: Vec::new(),
        open: Some(95),
    };
    let found = bins(&row.covered(), &known_from(10), 50, 0, (0, 1000), (10, 120));
    assert_eq!(
        found,
        [
            Bin {
                from: 10,
                to: 50,
                covered: 0,
                known: 40
            },
            Bin {
                from: 50,
                to: 100,
                covered: 5,
                known: 50
            },
            Bin {
                from: 100,
                to: 120,
                covered: 20,
                known: 20
            },
        ]
    );
}

/// A view of less than a cycle per pixel has bins of a cycle: an idle row
/// covers none of them, known all.
#[test]
fn bins_narrower_than_a_cycle_are_a_cycle() {
    let row = Row {
        closed: vec![(0, 5), (5, 10)],
        open: None,
    };
    assert_eq!(width(0.01), 1);
    let found = bins(&row.covered(), &known_from(0), 1, 0, (10, 13), (0, 20));
    assert!(
        found
            .iter()
            .all(|bin| bin.covered == 0 && bin.known == 1 && bin.to - bin.from == 1),
        "{found:?}"
    );
    assert_eq!(found.len(), 3);
}

/// Rows of overlapping spans, some open, as the oracle reads them: a cycle
/// is covered when any of them covers it.
fn rows(state: &mut u64, capture: (i64, i64)) -> Vec<Row> {
    (0..1 + random(state) % 3).map(|_| row(state, capture)).collect()
}

fn union(rows: &[Row]) -> Covered {
    Covered::union(
        rows.iter().flat_map(|row| row.closed.iter().copied()),
        rows.iter().filter_map(|row| row.open).min(),
    )
}

/// The union of overlapping spans in any order covers each cycle any of
/// them covers, from before the capture to after it.
#[test]
fn a_union_covers_the_cycles_any_span_covers() {
    let mut state = 0x2545_f491_4f6c_dd1d;
    let (mut overlapping, mut swallowed) = (0, 0);
    for _ in 0..1000 {
        let capture = (0, 1 + (random(&mut state) % 3000) as i64);
        let rows = rows(&mut state, capture);
        let union = union(&rows);
        let open = rows.iter().filter_map(|row| row.open).min();
        for cycle in capture.0 - 30..capture.1 + 30 {
            let count = rows.iter().filter(|row| row.covers(cycle)).count();
            assert_eq!(
                union.until(cycle + 1) - union.until(cycle),
                i64::from(count > 0),
                "cycle {cycle}"
            );
            overlapping += usize::from(count > 1);
        }
        // A closed span still running when the open one starts.
        swallowed += usize::from(open.is_some_and(|open| {
            rows.iter()
                .flat_map(|row| &row.closed)
                .any(|&(from, to)| from < open && open < to)
        }));
    }
    assert!(
        overlapping > 1000 && swallowed > 100,
        "{overlapping} cycles overlapping, {swallowed} closed spans running into the open one"
    );
}

/// Short runs, touching or apart, dealt at random to one to four tasks,
/// which never run at once, one of them maybe still running at the end.
fn tasks(state: &mut u64, capture: (i64, i64)) -> Vec<Row> {
    let count = 1 + (random(state) % 4) as usize;
    let mut tasks: Vec<Row> = (0..count)
        .map(|_| Row {
            closed: Vec::new(),
            open: None,
        })
        .collect();
    let mut at = capture.0 - (random(state) % 20) as i64;
    while at < capture.1 {
        at += (random(state) % 30) as i64;
        let to = at + (random(state) % 60) as i64;
        tasks[random(state) as usize % count].closed.push((at, to));
        at = to;
    }
    if random(state).is_multiple_of(2) {
        let from = at - (random(state) % 500) as i64;
        for task in &mut tasks {
            task.closed.retain(|&(_, to)| to <= from);
        }
        tasks[random(state) as usize % count].open = Some(from);
    }
    tasks
}

/// Every CPU bin is the oracle's: handlers, then tasks other than idle
/// where no handler runs, all of them and each, within the time known,
/// never above it; the tasks' parts add up to the tasks'.
#[test]
fn cpu_bins_stack_handlers_then_tasks_within_the_time_known() {
    let mut state = 0x0bad_5eed_1234_5678;
    let (mut both, mut stacked, mut several) = (0, 0, 0);
    for _ in 0..1000 {
        let capture = (0, 1 + (random(&mut state) % 3000) as i64);
        let (known, handlers, tasks) = (
            row(&mut state, capture),
            rows(&mut state, capture),
            self::tasks(&mut state, capture),
        );
        let known_covered = known.covered();
        let handlers_covered = union(&handlers).within(&known_covered);
        let busy = Covered::union(
            handlers.iter().chain(&tasks).flat_map(|row| row.closed.iter().copied()),
            handlers.iter().chain(&tasks).filter_map(|row| row.open).min(),
        );
        let cpu = Cpu {
            busy: busy.within(&known_covered),
            tasks: tasks
                .iter()
                .map(|task| task.covered().within(&known_covered).without(&handlers_covered))
                .collect(),
            handlers: handlers_covered,
            handled: known_covered.clone(),
            known: known_covered,
        };
        let width = 1 + (random(&mut state) % 200) as i64;
        let found = cpu_bins(&cpu, width, 0, capture, capture);
        let mut expected = Vec::new();
        for from in (capture.0..capture.1).step_by(width as usize) {
            let to = (from + width).min(capture.1);
            let mut bin = CpuBin {
                from,
                to,
                known: 0,
                handlers: 0,
                tasks: 0,
                each: vec![0; tasks.len()],
            };
            for cycle in from..to {
                let any = |rows: &[Row]| rows.iter().any(|row| row.covers(cycle));
                if known.covers(cycle) {
                    bin.known += 1;
                    if any(&handlers) {
                        bin.handlers += 1;
                    } else if any(&tasks) {
                        bin.tasks += 1;
                        for (each, task) in bin.each.iter_mut().zip(&tasks) {
                            *each += i64::from(task.covers(cycle));
                        }
                    }
                }
            }
            both += usize::from(bin.handlers > 0 && bin.tasks > 0);
            several += usize::from(bin.each.iter().filter(|&&each| each > 0).count() > 1);
            expected.push(bin);
        }
        assert_eq!(found, expected, "width {width} capture {capture:?}");
        for bin in &found {
            assert!(bin.handlers + bin.tasks <= bin.known, "{bin:?}");
            assert_eq!(bin.each.iter().sum::<i64>(), bin.tasks, "{bin:?}");
        }
        stacked += usize::from(handlers.iter().any(|h| {
            tasks
                .iter()
                .any(|t| (capture.0..capture.1).any(|c| h.covers(c) && t.covers(c) && known.covers(c)))
        }));
    }
    assert!(
        both > 1000 && stacked > 500 && several > 500,
        "{both} bins with handlers and tasks, {stacked} cases with a handler over a task, \
         {several} bins with several tasks"
    );
}
