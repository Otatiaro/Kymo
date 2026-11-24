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

/// The largest share, `covered` over `known`, of the sub-bins of `sub`
/// cycles counted from `origin` within the bin from `from` to `to`.
fn largest(covered: &[bool], known: &[bool], first: i64, origin: i64, sub: i64, (from, to): (i64, i64)) -> Option<f64> {
    let mut largest: Option<f64> = None;
    let mut at = origin + (from - origin).div_euclid(sub) * sub;
    while at < to {
        let (low, high) = (at.max(from), (at + sub).min(to));
        let known = count(known, first, low, high);
        if known > 0 {
            let share = count(covered, first, low, high) as f64 / known as f64;
            largest = Some(largest.map_or(share, |largest| largest.max(share)));
        }
        at += sub;
    }
    largest
}

/// The bins drawn, a row's and the CPU's, hold every cycle of the view
/// within the capture, side by side; each one's counts and max are those a
/// count cycle by cycle gives; panning keeps the bins it keeps in view; the
/// pointer reads the bin drawn under it, at either edge.
#[test]
fn the_bins_drawn_are_counted_cycle_by_cycle_and_read_under_the_pointer() {
    let mut state = 0x2545_f491_4f6c_dd1d;
    let (mut kept, mut edges) = (0, 0);
    for _ in 0..300 {
        let length = 200 + (random(&mut state) % 20_000) as usize;
        let origin = 1_000_000_007;
        let first = origin + (random(&mut state) % 500) as i64;
        let capture = (first, first + length as i64);
        let open = random(&mut state).is_multiple_of(2);
        let known = cycles(&mut state, length, None);
        let set = cycles(&mut state, length, Some(&known));
        let handlers = cycles(&mut state, length, Some(&known));
        let busy: Vec<bool> = cycles(&mut state, length, Some(&known))
            .iter()
            .zip(&handlers)
            .map(|(task, handler)| task | handler)
            .collect();
        let (known_covered, row_covered) = (covered(&known, first, open), covered(&set, first, open));
        let cpu_row = Cpu {
            known: covered(&known, first, open),
            handlers: covered(&handlers, first, open),
            busy: covered(&busy, first, open),
        };
        let span = 50.0 + (random(&mut state) % (length as u64 + 1000)) as f64;
        let view = View {
            origin,
            start: (first - origin) as f64 - 300.0 + (random(&mut state) % (length as u64 + 300)) as f64,
            span,
            left: 260.0,
            width: 100.0 + (random(&mut state) % 1500) as f64,
        };
        let width = load::width(view.span / view.width);
        let sub = below(width);
        let shown = (
            (view.origin + view.start.floor() as i64).max(capture.0),
            (view.origin + (view.start + view.span).ceil() as i64).min(capture.1),
        );
        let drawn = row(&view, capture, &row_covered, &known_covered);
        let drawn_cpu = cpu(&view, capture, &cpu_row);
        for (bins, layers) in [(&drawn, [&set, &set]), (&drawn_cpu, [&handlers, &busy])] {
            let context = format!("{capture:?} {} {} {}", view.start, view.span, view.width);
            if shown.0 < shown.1 {
                let (first_bin, last_bin) = (bins.first().expect(&context), bins.last().expect(&context));
                assert!(first_bin.from <= shown.0 && last_bin.to >= shown.1, "{context}");
            }
            for pair in bins.windows(2) {
                assert_eq!((pair[0].to, pair[0].right), (pair[1].from, pair[1].left), "{context}");
            }
            for (n, bin) in bins.iter().enumerate() {
                assert!(bin.from >= capture.0 && bin.to <= capture.1, "{context}");
                assert_eq!(bin.known, count(&known, first, bin.from, bin.to), "{context}");
                let bottom = count(layers[0], first, bin.from, bin.to);
                assert_eq!(bin.stack[0], bottom, "{context}");
                assert_eq!(bin.covered(), count(layers[1], first, bin.from, bin.to), "{context}");
                assert_eq!(
                    bin.max,
                    largest(layers[1], &known, first, origin, sub, (bin.from, bin.to)),
                    "{context}"
                );
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
        for bin in row(&panned, capture, &row_covered, &known_covered) {
            // The edge bins hold only the part of the capture they cover.
            if let Some(same) = drawn.iter().find(|same| same.from == bin.from && same.to == bin.to) {
                assert_eq!((same.known, same.stack, same.max), (bin.known, bin.stack, bin.max));
                kept += 1;
            }
        }
    }
    assert!(kept > 10_000 && edges > 10_000, "{kept} kept, {edges} edges");
}

#[test]
fn a_sub_bin_divides_its_bin() {
    for (width, sub) in [
        (1, 1),
        (2, 1),
        (5, 1),
        (10, 5),
        (20, 10),
        (50, 10),
        (100, 50),
        (5_000_000, 1_000_000),
    ] {
        assert_eq!(below(width), sub, "{width}");
    }
}

fn bin(known: i64, stack: [i64; 2], max: Option<f64>) -> Drawn {
    Drawn {
        left: 0.0,
        right: 4.0,
        from: 0,
        to: 1_000_000,
        known,
        stack,
        max,
    }
}

/// A bar is the part of the bin covered, a pixel at least when any is; the
/// time unknown is what lies above the time known; the max line is the
/// largest share of the time known.
#[test]
fn heights_show_the_part_covered_and_the_part_unknown() {
    let all = 1_000_000;
    let heights = |bin: Drawn| super::heights(&bin, 12.0);
    let expect = |stack: [f32; 2], known: f32, max: Option<f32>| Heights { stack, known, max };
    assert_eq!(
        heights(bin(all, [0, 0], Some(0.0))),
        expect([0.0, 0.0], 12.0, Some(0.0))
    );
    assert_eq!(heights(bin(all, [1, 0], None)), expect([1.0, 1.0], 12.0, None));
    assert_eq!(
        heights(bin(all, [all / 2, 0], Some(1.0))),
        expect([6.0, 6.0], 12.0, Some(12.0))
    );
    assert_eq!(
        heights(bin(all, [all, 0], Some(1.0))),
        expect([12.0, 12.0], 12.0, Some(12.0))
    );
    // Handlers, then a task for a cycle.
    assert_eq!(heights(bin(all, [all / 4, 1], None)), expect([3.0, 4.0], 12.0, None));
    // ... all known: the time unknown stays above the stack's floors.
    assert_eq!(
        heights(bin(all / 4 + 1, [all / 4, 1], None)),
        expect([3.0, 4.0], 4.0, None)
    );
    // Half the bin unknown: the half known is the scale of the max.
    assert_eq!(
        heights(bin(all / 2, [all / 4, 0], Some(1.0))),
        expect([3.0, 3.0], 6.0, Some(6.0))
    );
    assert_eq!(heights(bin(0, [0, 0], None)), expect([0.0, 0.0], 0.0, None));
    // A cycle known, covered: a pixel known.
    assert_eq!(heights(bin(1, [1, 0], Some(1.0))), expect([1.0, 1.0], 1.0, Some(1.0)));
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
