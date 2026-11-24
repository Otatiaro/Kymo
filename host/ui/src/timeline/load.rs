//! The load view (ADR-061, ADR-084): in each bin of time, the share of the
//! time known a row covers, as the height of a bar; the time unknown hatched
//! above it. A bin shows one statistic for every row: the mean is the whole
//! bin's share; the median, the min and the max are the share of the sub-bin
//! that has it, a pixel wide at most, drawn whole. The CPU row stacks
//! handlers, then each task other than idle, each with the time it takes in
//! the whole bin, whatever the statistic. The pointer reads the bin the
//! frame drew.

use super::view::View;
use crate::meaning::Meaning;
use eframe::egui::{self, Color32, Pos2, Rect, pos2};
use kymo::load::{self, Covered, Cpu};

/// How the rows of the kernel's meaning are drawn: their spans, their load, or
/// whichever suits the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Bars,
    Load,
}

/// The fewest pixels per span at which bars stay readable.
const SPAN: f64 = 4.0;

impl Mode {
    /// Whether the rows show their load in a view `width` pixels wide whose
    /// busiest row shows `busiest` spans: in Auto, when they are more than
    /// one per `SPAN` pixels. Auto decides for all rows at once.
    pub(super) fn load(self, busiest: usize, width: f64) -> bool {
        match self {
            Mode::Auto => busiest as f64 > width / SPAN,
            Mode::Bars => false,
            Mode::Load => true,
        }
    }
}

/// The statistic of its sub-bins a bin shows, on every row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Statistic {
    Mean,
    Median,
    Min,
    Max,
}

impl Statistic {
    pub(super) fn name(self) -> &'static str {
        match self {
            Statistic::Mean => "mean",
            Statistic::Median => "median",
            Statistic::Min => "min",
            Statistic::Max => "max",
        }
    }
}

/// The width of a bin by default, in pixels.
pub(super) const PIXELS: u32 = 16;

/// How the load view bins time: the width a bin is at least, in pixels, and
/// the statistic it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Binning {
    pub pixels: u32,
    pub statistic: Statistic,
}

impl Default for Binning {
    fn default() -> Self {
        Binning {
            pixels: PIXELS,
            statistic: Statistic::Mean,
        }
    }
}

/// The most spans a row of `meaning` shows in `view`.
pub(super) fn busiest(meaning: &Meaning, view: &View) -> usize {
    meaning
        .runs
        .iter()
        .chain([&meaning.idle])
        .chain(meaning.interrupts.iter().flatten())
        .chain(&meaning.holds)
        .chain(meaning.waits.iter().flatten().map(|(_, waits)| waits))
        .map(|spans| spans.count(view))
        .max()
        .unwrap_or_default()
}

/// A bin as drawn: its pixel columns `left..right`, its cycles `from..to`
/// within the capture, the cycles `counted`, and their counts. The cycles
/// counted are the bin's for the mean, and the sub-bin's that has the
/// statistic for the others; the bin's again when none of its sub-bins is
/// known.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Drawn {
    pub left: f32,
    pub right: f32,
    pub from: i64,
    pub to: i64,
    pub counted: (i64, i64),
    pub known: i64,
    /// The cycles covered, from the bottom: a row's; or the CPU's handlers,
    /// then each task other than idle, in the order `cpu` is given.
    pub stack: Vec<i64>,
}

impl Drawn {
    pub(super) fn covered(&self) -> i64 {
        self.stack.iter().sum()
    }
}

/// The cycles the bins of `width` cycles of `view` hold: those of its left
/// column to those of its right one, whole, out to the edges of their bins.
fn range(view: &View, width: i64) -> (i64, i64) {
    let edge = |cycles: i64| view.origin + (cycles - view.origin).div_euclid(width) * width;
    (
        edge(view.origin + view.start.floor() as i64),
        edge(view.origin + (view.start + view.span).ceil() as i64) + width,
    )
}

/// The cycles a sub-bin of a bin of `width` cycles lasts at `per_pixel`
/// cycles per pixel: the longest 1, 2 or 5 × 10ⁿ cycles, a pixel at most,
/// that divides the bin; a cycle at least.
pub(super) fn sub(width: i64, per_pixel: f64) -> i64 {
    let mut longest = 1;
    let mut decade: i64 = 1;
    while decade <= width {
        for sub in [decade, 2 * decade, 5 * decade] {
            if width % sub == 0 && sub as f64 <= per_pixel {
                longest = longest.max(sub);
            }
        }
        let Some(next) = decade.checked_mul(10) else { break };
        decade = next;
    }
    longest
}

/// Which of the sub-bins, given by the cycles each knows and covers, has
/// `statistic`: their shares, covered over known, ranked from the smallest,
/// those of equal share in time order, and the sub-bins with no time known
/// left out; the min is the first, the median the lower middle one, the max
/// the last. None for the mean, or when none is known.
fn pick(subs: impl Iterator<Item = (i64, i64)>, statistic: Statistic) -> Option<usize> {
    if statistic == Statistic::Mean {
        return None;
    }
    let mut ranked: Vec<(f64, usize)> = subs
        .enumerate()
        .filter(|&(_, (known, _))| known > 0)
        .map(|(n, (known, covered))| (covered as f64 / known as f64, n))
        .collect();
    let last = ranked.len().checked_sub(1)?;
    let at = match statistic {
        Statistic::Mean | Statistic::Min => 0,
        Statistic::Median => last / 2,
        Statistic::Max => last,
    };
    let (_, &mut (_, n), _) = ranked.select_nth_unstable_by(at, |a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    Some(n)
}

/// The sub-bins of a view, the same on every row, and the time known in
/// each: counted once a frame. The mean counts the bins themselves; the
/// other statistics, sub-bins that divide them and share their origin.
pub(super) struct Subs {
    origin: i64,
    /// The bins' width, in cycles.
    width: i64,
    statistic: Statistic,
    /// The first sub-bin's start, then each one's end.
    edges: Vec<i64>,
    known: Vec<i64>,
}

impl Subs {
    /// The sub-bins of `view` binned as `binning` asks, in a capture from
    /// `capture.0` to `capture.1` cycles of which `known` is known.
    pub(super) fn new(view: &View, binning: Binning, capture: (i64, i64), known: &Covered) -> Subs {
        let per_pixel = view.span / view.width;
        let width = load::width(f64::from(binning.pixels) * per_pixel);
        let sub = match binning.statistic {
            Statistic::Mean => width,
            _ => sub(width, per_pixel),
        };
        let edges = load::edges(sub, view.origin, range(view, width), capture);
        Subs {
            origin: view.origin,
            width,
            statistic: binning.statistic,
            known: known.counts(&edges),
            edges,
        }
    }
}

/// The bins of `view` from its sub-bins `subs` and the cycles each layer of
/// the stack covers in them, from the bottom: each bin with its columns and
/// the counts of the cycles it shows for `statistic`.
fn drawn(view: &View, subs: &Subs, layers: &[Vec<i64>], statistic: Statistic) -> Vec<Drawn> {
    let stack = |n: usize| layers.iter().map(move |layer| layer[n]);
    let bin = |cycles: i64| (cycles - subs.origin).div_euclid(subs.width);
    let mut first = 0;
    subs.edges
        .windows(2)
        .collect::<Vec<_>>()
        .chunk_by(|a, b| bin(a[0]) == bin(b[0]))
        .map(|edges| {
            let held = first..first + edges.len();
            first = held.end;
            let (from, to) = (edges[0][0], edges[edges.len() - 1][1]);
            let shares = held.clone().map(|n| (subs.known[n], stack(n).sum()));
            let (counted, known, stack) = match pick(shares, statistic) {
                Some(n) => {
                    let n = held.start + n;
                    ((subs.edges[n], subs.edges[n + 1]), subs.known[n], stack(n).collect())
                }
                None => (
                    (from, to),
                    subs.known[held.clone()].iter().sum(),
                    layers.iter().map(|layer| layer[held.clone()].iter().sum()).collect(),
                ),
            };
            Drawn {
                left: view.column(from),
                right: view.column(to),
                from,
                to,
                counted,
                known,
                stack,
            }
        })
        .collect()
}

/// The bins of a row covering `covered`, within the time known, in `view`
/// cut in `subs`.
pub(super) fn row(view: &View, subs: &Subs, covered: &Covered) -> Vec<Drawn> {
    drawn(view, subs, &[covered.counts(&subs.edges)], subs.statistic)
}

/// The statistic `line` shows when the bins show `statistic`: the CPU's,
/// stacked, is always the mean.
pub(super) fn statistic(cpu: bool, statistic: Statistic) -> Statistic {
    if cpu { Statistic::Mean } else { statistic }
}

/// The bins of the CPU row: handlers, then the tasks in `order`, every task
/// other than idle once, each with the cycles it covers in the whole bin.
pub(super) fn cpu(view: &View, subs: &Subs, cpu: &Cpu, order: &[usize]) -> Vec<Drawn> {
    let layers: Vec<Vec<i64>> = std::iter::once(&cpu.handlers)
        .chain(order.iter().map(|&task| &cpu.tasks[task]))
        .map(|covered| covered.counts(&subs.edges))
        .collect();
    drawn(view, subs, &layers, statistic(true, subs.statistic))
}

/// The bin drawn at pixel `x`: from its left column to before its right.
pub(super) fn at(bins: &[Drawn], x: f32) -> Option<&Drawn> {
    let n = bins.partition_point(|bin| bin.right <= x);
    bins.get(n).filter(|bin| bin.left <= x)
}

/// A drawn bin's heights in pixels, from the bottom of a bar `height`
/// pixels high: the top of each layer of its stack and of its time known,
/// over the cycles it counts.
#[derive(Debug, PartialEq)]
pub(super) struct Heights {
    pub stack: Vec<f32>,
    pub known: f32,
}

pub(super) fn heights(bin: &Drawn, height: f32) -> Heights {
    let scale = f64::from(height) / (bin.counted.1 - bin.counted.0) as f64;
    // Cycles counted, however few, are a pixel high at least, and so is
    // each layer that counts any.
    let pixels = |cycles: i64| {
        if cycles > 0 {
            ((cycles as f64 * scale).round() as f32).max(1.0)
        } else {
            0.0
        }
    };
    let (mut below, mut top) = (0, 0.0);
    let mut stack: Vec<f32> = bin
        .stack
        .iter()
        .map(|&layer| {
            below += layer;
            top = pixels(below).max(top + if layer > 0 { 1.0 } else { 0.0 });
            top
        })
        .collect();
    let known = pixels(bin.known).max(top).min(height);
    for top in &mut stack {
        *top = top.min(known);
    }
    Heights { stack, known }
}

/// The pixels left empty between two bins of the CPU's line, so that each
/// reads as one column.
pub(super) const GAP: f32 = 1.0;

/// Paints `bins` in `mesh` within `area`, bars from `high` to `low` with
/// `gap` pixels left empty at the right of each bin twice as wide: the
/// layers of their stack in `colors`, one each, the time unknown hatched in
/// `unknown`.
pub(super) fn paint(
    mesh: &mut egui::Mesh,
    bins: &[Drawn],
    area: Rect,
    (high, low): (f32, f32),
    gap: f32,
    colors: &[Color32],
    unknown: Color32,
) {
    let height = low - high;
    for bin in bins {
        let end = if bin.right - bin.left >= 2.0 * gap {
            bin.right - gap
        } else {
            bin.right
        };
        let (left, right) = (bin.left.max(area.left()), end.min(area.right()));
        if left >= right {
            continue;
        }
        let heights = heights(bin, height);
        let rect = |bottom: f32, top: f32| Rect::from_min_max(pos2(left, low - top), pos2(right, low - bottom));
        let mut below = 0.0;
        for (&top, &layer) in heights.stack.iter().zip(colors) {
            if top > below {
                mesh.add_colored_rect(rect(below, top), layer);
                below = top;
            }
        }
        if heights.known < height {
            hatch(mesh, rect(heights.known, height), unknown);
        }
    }
}

/// The spacing of the hatching's lines and their width, across the screen:
/// lines rising to the right, at the same places on every rectangle, so
/// that neighbouring rectangles join.
const HATCH: f32 = 4.0;
const STROKE: f32 = 1.5;

/// Hatches `rect` in `color`.
pub(super) fn hatch(mesh: &mut egui::Mesh, rect: Rect, color: Color32) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let first = ((rect.left() + rect.top()) / HATCH).floor() as i32;
    let last = ((rect.right() + rect.bottom()) / HATCH).ceil() as i32;
    for n in first..=last {
        // The band from x + y = line to x + y = line + STROKE.
        let line = n as f32 * HATCH;
        let band = clip(&clip(&corners, |p| p.x + p.y - line), |p| line + STROKE - p.x - p.y);
        if band.len() < 3 {
            continue;
        }
        let base = mesh.vertices.len() as u32;
        for &point in &band {
            mesh.colored_vertex(point, color);
        }
        for k in 1..band.len() as u32 - 1 {
            mesh.add_triangle(base, base + k, base + k + 1);
        }
    }
}

/// The convex polygon `points` cut to where `inside` is positive or zero.
fn clip(points: &[Pos2], inside: impl Fn(Pos2) -> f32) -> Vec<Pos2> {
    let mut kept = Vec::with_capacity(points.len() + 1);
    for (n, &a) in points.iter().enumerate() {
        let b = points[(n + 1) % points.len()];
        let (at_a, at_b) = (inside(a), inside(b));
        if at_a >= 0.0 {
            kept.push(a);
        }
        if (at_a >= 0.0) != (at_b >= 0.0) {
            kept.push(a + (b - a) * (at_a / (at_a - at_b)));
        }
    }
    kept
}

/// `part` of `whole` in percent, rounded, but neither 0 % nor 100 % when it
/// is not.
pub(super) fn percent(part: i64, whole: i64) -> String {
    let rounded = (100.0 * part as f64 / whole as f64).round();
    if rounded == 0.0 && part > 0 {
        "<1%".to_owned()
    } else if rounded == 100.0 && part < whole {
        ">99%".to_owned()
    } else {
        format!("{rounded}%")
    }
}

#[cfg(test)]
mod tests;
