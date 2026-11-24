use super::*;

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Cycles from `first` on, each set or not, in runs of random lengths; each
/// cycle set only where `within` is, when given.
fn cycles(state: &mut u64, length: usize, within: Option<&[bool]>) -> Vec<bool> {
    let mut set = Vec::with_capacity(length);
    while set.len() < length {
        let value = !random(state).is_multiple_of(3);
        let run = match random(state) % 4 {
            0 => 1 + random(state) % 3,
            1 => 1 + random(state) % 3000,
            _ => 1 + random(state) % 60,
        };
        set.extend(std::iter::repeat_n(value, run as usize));
    }
    set.truncate(length);
    if let Some(within) = within {
        for (cycle, known) in set.iter_mut().zip(within) {
            *cycle &= known;
        }
    }
    set
}

/// The runs of `set`, from `first`, as a row's spans; the last one open
/// when it reaches the end and `open` says so.
fn covered(set: &[bool], first: i64, open: bool) -> Covered {
    let mut covered = Covered::default();
    let mut n = 0;
    while n < set.len() {
        if !set[n] {
            n += 1;
            continue;
        }
        let from = n;
        while n < set.len() && set[n] {
            n += 1;
        }
        if n == set.len() && open {
            covered.open(Some(first + from as i64));
        } else {
            covered.push(first + from as i64, first + n as i64);
        }
    }
    covered
}

/// The cycles of `set`, from `first`, set from `from` to before `to`.
fn count(set: &[bool], first: i64, from: i64, to: i64) -> i64 {
    (from..to).filter(|&cycle| set[(cycle - first) as usize]).count() as i64
}

/// The cycles a bin from `from` to `to` shows for `statistic`, by brute
/// force: every sub-bin of `sub` cycles counted from `origin` within it,
/// cycle by cycle, those with time known sorted by share, `covered` over
/// `known`, in time order where equal; the min first, the lower median, the
/// max last. The bin itself for the mean, or when no sub-bin is known.
#[allow(clippy::too_many_arguments)]
fn shown(
    statistic: Statistic,
    covered: &[bool],
    known: &[bool],
    first: i64,
    origin: i64,
    sub: i64,
    (from, to): (i64, i64),
) -> (i64, i64) {
    let mut subs: Vec<(f64, (i64, i64))> = Vec::new();
    let mut at = origin + (from - origin).div_euclid(sub) * sub;
    while at < to {
        let (low, high) = (at.max(from), (at + sub).min(to));
        let known = count(known, first, low, high);
        if known > 0 {
            subs.push((count(covered, first, low, high) as f64 / known as f64, (low, high)));
        }
        at += sub;
    }
    subs.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("a share"));
    let n = subs.len();
    let picked = match statistic {
        Statistic::Mean => None,
        Statistic::Min => subs.first(),
        Statistic::Median => n.checked_sub(1).map(|last| &subs[last / 2]),
        Statistic::Max => subs.last(),
    };
    picked.map_or((from, to), |&(_, interval)| interval)
}

/// The bins drawn, a row's and the CPU's, hold every cycle of the view
/// within the capture, side by side, as wide as asked; each shows the
/// cycles the statistic picks, and its counts are those a count cycle by
/// cycle gives; panning keeps the bins it keeps in view; the pointer reads
/// the bin drawn under it, at either edge.
#[test]
fn the_bins_drawn_are_counted_cycle_by_cycle_and_read_under_the_pointer() {
    let mut state = 0x2545_f491_4f6c_dd1d;
    let (mut kept, mut edges, mut several, mut picked) = (0, 0, 0, [0; 4]);
    for _ in 0..300 {
        let length = 200 + (random(&mut state) % 20_000) as usize;
        let origin = 1_000_000_007;
        let first = origin + (random(&mut state) % 500) as i64;
        let capture = (first, first + length as i64);
        let open = random(&mut state).is_multiple_of(2);
        let known = cycles(&mut state, length, None);
        let set = cycles(&mut state, length, Some(&known));
        let handlers = cycles(&mut state, length, Some(&known));
        // A task other than idle runs: a few tasks, switching every fifty
        // cycles on average.
        let running = cycles(&mut state, length, Some(&known));
        let many = 1 + (random(&mut state) % 4) as usize;
        let mut owner = vec![0; length];
        for n in 1..length {
            owner[n] = if running[n] && running[n - 1] && !random(&mut state).is_multiple_of(50) {
                owner[n - 1]
            } else {
                random(&mut state) as usize % many
            };
        }
        let busy: Vec<bool> = running
            .iter()
            .zip(&handlers)
            .map(|(task, handler)| task | handler)
            .collect();
        let tasks: Vec<Vec<bool>> = (0..many)
            .map(|task| {
                (0..length)
                    .map(|n| running[n] && owner[n] == task && !handlers[n])
                    .collect()
            })
            .collect();
        let row_covered = covered(&set, first, open);
        let cpu_row = Cpu {
            known: covered(&known, first, open),
            handlers: covered(&handlers, first, open),
            busy: covered(&busy, first, open),
            tasks: tasks.iter().map(|task| covered(task, first, open)).collect(),
        };
        // The stack's order, not the tasks'.
        let order: Vec<usize> = (0..many).rev().collect();
        let span = 50.0 + (random(&mut state) % (length as u64 + 1000)) as f64;
        let view = View {
            origin,
            start: (first - origin) as f64 - 300.0 + (random(&mut state) % (length as u64 + 300)) as f64,
            span,
            left: 260.0,
            width: 100.0 + (random(&mut state) % 1500) as f64,
        };
        let statistic =
            [Statistic::Mean, Statistic::Median, Statistic::Min, Statistic::Max][random(&mut state) as usize % 4];
        let binning = Binning {
            pixels: [1, 4, 16, 37, 100][random(&mut state) as usize % 5],
            statistic,
        };
        let per_pixel = view.span / view.width;
        let width = load::width(f64::from(binning.pixels) * per_pixel);
        let sub = sub(width, per_pixel);
        let in_view = (
            (view.origin + view.start.floor() as i64).max(capture.0),
            (view.origin + (view.start + view.span).ceil() as i64).min(capture.1),
        );
        let subs = Subs::new(&view, binning, capture, &cpu_row.known);
        let drawn = row(&view, &subs, &row_covered);
        let drawn_cpu = cpu(&view, &subs, &cpu_row, &order);
        for bin in &drawn_cpu {
            let (low, high) = bin.counted;
            let each: Vec<i64> = order
                .iter()
                .map(|&task| count(&tasks[task], first, low, high))
                .collect();
            assert_eq!(bin.stack[1..], each, "{capture:?} {bin:?}");
            assert!(bin.covered() <= high - low, "{capture:?} {bin:?}");
            several += usize::from(each.iter().filter(|&&each| each > 0).count() > 1);
        }
        for (bins, layers) in [(&drawn, [&set, &set]), (&drawn_cpu, [&handlers, &busy])] {
            let context = format!("{capture:?} {} {} {} {binning:?}", view.start, view.span, view.width);
            if in_view.0 < in_view.1 {
                let (first_bin, last_bin) = (bins.first().expect(&context), bins.last().expect(&context));
                assert!(first_bin.from <= in_view.0 && last_bin.to >= in_view.1, "{context}");
            }
            for pair in bins.windows(2) {
                assert_eq!((pair[0].to, pair[0].right), (pair[1].from, pair[1].left), "{context}");
            }
            for (n, bin) in bins.iter().enumerate() {
                assert!(bin.from >= capture.0 && bin.to <= capture.1, "{context}");
                if bin.from > capture.0 && bin.to < capture.1 {
                    assert_eq!(bin.to - bin.from, width, "{context}");
                }
                let counted = shown(statistic, layers[1], &known, first, origin, sub, (bin.from, bin.to));
                assert_eq!(bin.counted, counted, "{context}");
                picked[statistic as usize] += usize::from(counted != (bin.from, bin.to));
                let (low, high) = counted;
                assert_eq!(bin.known, count(&known, first, low, high), "{context}");
                assert_eq!(bin.stack[0], count(layers[0], first, low, high), "{context}");
                assert_eq!(bin.covered(), count(layers[1], first, low, high), "{context}");
                assert_eq!(
                    (bin.left, bin.right),
                    (view.column(bin.from), view.column(bin.to)),
                    "{context}"
                );
                if bin.right > bin.left {
                    assert_eq!(at(bins, bin.left), Some(bin), "{context}");
                    assert_eq!(at(bins, bin.right - 0.25), Some(bin), "{context}");
                    // A bin cut by the capture may draw in no column.
                    let next = bins[n + 1..].iter().find(|next| next.right > next.left);
                    assert_eq!(at(bins, bin.right), next, "{context}");
                    edges += 1;
                }
            }
        }
        // Panned by up to half the view, either way.
        let panned = View {
            start: view.start + (random(&mut state) % 1001) as f64 / 1000.0 * view.span - view.span / 2.0,
            ..view
        };
        for bin in row(
            &panned,
            &Subs::new(&panned, binning, capture, &cpu_row.known),
            &row_covered,
        ) {
            // The edge bins hold only the part of the capture they cover.
            if let Some(same) = drawn.iter().find(|same| same.from == bin.from && same.to == bin.to) {
                assert_eq!(
                    (same.counted, same.known, &same.stack),
                    (bin.counted, bin.known, &bin.stack)
                );
                kept += 1;
            }
        }
    }
    assert!(
        kept > 10_000 && edges > 10_000 && several > 200 && picked[1..].iter().all(|&n| n > 1_000),
        "{kept} kept, {edges} edges, {several} CPU bins with several tasks, {picked:?} sub-bins picked"
    );
}

/// A sub-bin is the longest 1, 2 or 5 × 10ⁿ cycles of a pixel at most that
/// divides its bin, a cycle at least.
#[test]
fn a_sub_bin_divides_its_bin_and_is_a_pixel_at_most() {
    let round: Vec<i64> = (0..=12).flat_map(|n| [1, 2, 5].map(|m| m * 10i64.pow(n))).collect();
    for &width in round.iter().filter(|&&width| width <= 1_000_000_000_000) {
        for per_pixel in [0.3, 1.0, 1.9, 2.0, 7.0, 12.5, 49.9, 1e3, 3.3e5, 1e12] {
            let sub = sub(width, per_pixel);
            let longest = round
                .iter()
                .copied()
                .filter(|&sub| width % sub == 0 && sub as f64 <= per_pixel)
                .max()
                .unwrap_or(1);
            assert_eq!(sub, longest, "{width} {per_pixel}");
            assert!(
                width % sub == 0 && (sub == 1 || sub as f64 <= per_pixel),
                "{width} {per_pixel}"
            );
        }
    }
    assert_eq!(sub(50, 30.0), 10);
    assert_eq!(sub(5_000_000, 2e6), 1_000_000);
    assert_eq!(sub(20, 10.0), 10);
}

/// The sub-bin a statistic picks: one with no time known is left out; the
/// median of an even count is the lower one.
#[test]
fn a_statistic_picks_a_known_sub_bin() {
    let picks = |subs: &[(i64, i64)]| {
        [Statistic::Mean, Statistic::Min, Statistic::Median, Statistic::Max]
            .map(|statistic| pick(subs.iter().copied(), statistic))
    };
    // Shares 50 %, unknown, 80 %, 20 %.
    let odd = [(10, 5), (0, 0), (10, 8), (10, 2)];
    assert_eq!(picks(&odd), [None, Some(3), Some(0), Some(2)]);
    // ... and 60 %: the median is 50 %, not 60 %.
    let even = [odd.as_slice(), &[(10, 6)]].concat();
    assert_eq!(picks(&even), [None, Some(3), Some(0), Some(2)]);
    // No time known: the bin.
    assert_eq!(picks(&[(0, 0), (0, 0)]), [None; 4]);
    // Equal shares: in time order.
    let equal = [(10, 5), (20, 10), (10, 5), (10, 5)];
    assert_eq!(picks(&equal), [None, Some(0), Some(1), Some(3)]);
}

fn bin<const N: usize>(known: i64, stack: [i64; N]) -> Drawn {
    Drawn {
        left: 0.0,
        right: 4.0,
        from: 0,
        to: 1_000_000,
        counted: (0, 1_000_000),
        known,
        stack: stack.to_vec(),
    }
}

/// A bar is the part of the cycles counted covered, a pixel at least when
/// any is; the time unknown is what lies above the time known.
#[test]
fn heights_show_the_part_covered_and_the_part_unknown() {
    let all = 1_000_000;
    let heights = |bin: Drawn| super::heights(&bin, 12.0);
    let expect = |stack: &[f32], known: f32| Heights {
        stack: stack.to_vec(),
        known,
    };
    assert_eq!(heights(bin(all, [0, 0])), expect(&[0.0, 0.0], 12.0));
    assert_eq!(heights(bin(all, [1, 0])), expect(&[1.0, 1.0], 12.0));
    assert_eq!(heights(bin(all, [all / 2, 0])), expect(&[6.0, 6.0], 12.0));
    assert_eq!(heights(bin(all, [all, 0])), expect(&[12.0, 12.0], 12.0));
    // Handlers, then a task for a cycle.
    assert_eq!(heights(bin(all, [all / 4, 1])), expect(&[3.0, 4.0], 12.0));
    // ... all known: the time unknown stays above the stack's floors.
    assert_eq!(heights(bin(all / 4 + 1, [all / 4, 1])), expect(&[3.0, 4.0], 4.0));
    assert_eq!(heights(bin(all / 2, [all / 4, 0])), expect(&[3.0, 3.0], 6.0));
    assert_eq!(heights(bin(0, [0, 0])), expect(&[0.0, 0.0], 0.0));
    // Handlers, then tasks: each layer with a cycle a pixel high, an empty
    // one none.
    assert_eq!(
        heights(bin(all, [all / 4, 0, 1, 1])),
        expect(&[3.0, 3.0, 4.0, 5.0], 12.0)
    );
    // ... past the time known, cut to it.
    assert_eq!(
        heights(bin(all / 4 + 2, [all / 4, 1, 1, 0])),
        expect(&[3.0, 4.0, 5.0, 5.0], 5.0)
    );
    // ... past the bar, cut to it.
    let floors: Vec<f32> = (1..=14).map(|n| (n as f32).min(12.0)).collect();
    assert_eq!(heights(bin(all, [1; 14])), expect(&floors, 12.0));
    // A cycle known, covered: a pixel known.
    assert_eq!(heights(bin(1, [1, 0])), expect(&[1.0, 1.0], 1.0));
    // A sub-bin a tenth of the bin, half covered: half the bar.
    let sub = Drawn {
        counted: (0, all / 10),
        ..bin(all / 10, [all / 20, 0])
    };
    assert_eq!(heights(sub), expect(&[6.0, 6.0], 12.0));
}

#[test]
fn auto_shows_the_load_beyond_a_span_per_four_pixels() {
    assert!(!Mode::Auto.load(250, 1000.0));
    assert!(Mode::Auto.load(251, 1000.0));
    assert!(!Mode::Bars.load(1_000_000, 1000.0));
    assert!(Mode::Load.load(0, 1000.0));
}

#[test]
fn a_percentage_is_never_0_or_100_when_it_is_not() {
    for (part, whole, shown) in [
        (0, 10, "0%"),
        (1, 1000, "<1%"),
        (68, 100, "68%"),
        (999, 1000, ">99%"),
        (10, 10, "100%"),
    ] {
        assert_eq!(percent(part, whole), shown);
    }
}

/// The hatching stays within its rectangle and covers the part of it its
/// lines' width over their spacing gives.
#[test]
fn hatching_fills_its_share_of_the_rectangle() {
    for rect in [
        Rect::from_min_max(pos2(260.5, 30.0), pos2(660.5, 150.0)),
        Rect::from_min_max(pos2(1003.0, 77.0), pos2(1507.0, 89.0)),
    ] {
        let mut mesh = egui::Mesh::default();
        hatch(&mut mesh, rect, Color32::WHITE);
        let inside = rect.expand(1e-3);
        assert!(mesh.vertices.iter().all(|vertex| inside.contains(vertex.pos)));
        let area: f32 = mesh
            .indices
            .chunks(3)
            .map(|triangle| {
                let [a, b, c] = [0, 1, 2].map(|k| mesh.vertices[triangle[k] as usize].pos);
                ((b - a).x * (c - a).y - (b - a).y * (c - a).x).abs() / 2.0
            })
            .sum();
        let share = area / rect.area();
        assert!((share - STROKE / HATCH).abs() < 0.01, "{share}");
    }
}
