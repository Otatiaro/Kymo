//! The load view (ADR-061): in each bin of time, the share of the time known
//! a row covers, as the height of a bar; the time unknown hatched above it;
//! a line at the largest share of a sub-bin, which a burst reaches and the
//! bin's share smooths out. The CPU row stacks handlers, then tasks other
//! than idle. The pointer reads the bin the frame drew.

use super::view::View;
use crate::meaning::Meaning;
use eframe::egui::{self, Color32, Pos2, Rect, pos2};
use kymo::load::{self, Covered, Cpu};

/// How the rows of OpSy's meaning are drawn: their spans, their load, or
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
/// within the capture, and their counts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Drawn {
    pub left: f32,
    pub right: f32,
    pub from: i64,
    pub to: i64,
    pub known: i64,
    /// The cycles covered, from the bottom: a row's, then nothing; or the
    /// CPU's handlers, then its tasks other than idle.
    pub stack: [i64; 2],
    /// The largest share of a sub-bin, covered over known; none where no
    /// sub-bin is known.
    pub max: Option<f64>,
}

impl Drawn {
    pub(super) fn covered(&self) -> i64 {
        self.stack[0] + self.stack[1]
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

/// The cycles a sub-bin of a bin of `width` cycles lasts: the next 1-2-5
/// width down that divides it, 10ⁿ for 5 × 10ⁿ; `width` itself for a cycle.
pub(super) fn below(width: i64) -> i64 {
    let decade = 10i64.pow(width.ilog10());
    match width / decade {
        _ if width == 1 => 1,
        1 => width / 2,
        _ => decade,
    }
}

/// A bin's cycles and counts: `from`, `to`, `known`, `stack`.
type Counted = (i64, i64, i64, [i64; 2]);

/// The bins of `view`, from the sub-bins `count` counts over a range at a
/// width: each with its columns and the largest share of its sub-bins. A
/// bin is the sum of its sub-bins, which divide it and share its origin.
fn drawn(view: &View, count: impl Fn(i64, (i64, i64)) -> Vec<Counted>) -> Vec<Drawn> {
    let width = load::width(view.span / view.width);
    let bin = |cycles: i64| (cycles - view.origin).div_euclid(width);
    let mut drawn: Vec<Drawn> = Vec::new();
    for (from, to, known, stack) in count(below(width), range(view, width)) {
        let share = (known > 0).then(|| (stack[0] + stack[1]) as f64 / known as f64);
        match drawn.last_mut() {
            Some(last) if bin(last.from) == bin(from) => {
                last.to = to;
                last.known += known;
                last.stack = [last.stack[0] + stack[0], last.stack[1] + stack[1]];
                last.max = match (last.max, share) {
                    (Some(max), Some(share)) => Some(max.max(share)),
                    (max, share) => max.or(share),
                };
            }
            _ => drawn.push(Drawn {
                left: 0.0,
                right: 0.0,
                from,
                to,
                known,
                stack,
                max: share,
            }),
        }
    }
    for bin in &mut drawn {
        (bin.left, bin.right) = (view.column(bin.from), view.column(bin.to));
    }
    drawn
}

/// The bins of a row covering `covered` of the time `known`, in `view` of a
/// capture from `capture.0` to `capture.1` cycles.
pub(super) fn row(view: &View, capture: (i64, i64), covered: &Covered, known: &Covered) -> Vec<Drawn> {
    drawn(view, |width, range| {
        load::bins(covered, known, width, view.origin, range, capture)
            .into_iter()
            .map(|bin| (bin.from, bin.to, bin.known, [bin.covered, 0]))
            .collect()
    })
}

/// The bins of the CPU row, as `row` gives a row's.
pub(super) fn cpu(view: &View, capture: (i64, i64), cpu: &Cpu) -> Vec<Drawn> {
    drawn(view, |width, range| {
        load::cpu_bins(cpu, width, view.origin, range, capture)
            .into_iter()
            .map(|bin| (bin.from, bin.to, bin.known, [bin.handlers, bin.tasks]))
            .collect()
    })
}

/// The bin drawn at pixel `x`: from its left column to before its right.
pub(super) fn at(bins: &[Drawn], x: f32) -> Option<&Drawn> {
    let n = bins.partition_point(|bin| bin.right <= x);
    bins.get(n).filter(|bin| bin.left <= x)
}

/// A drawn bin's heights in pixels, from the bottom of a bar `height`
/// pixels high: the top of each layer of its stack, of its time known, and
/// the line of its max.
#[derive(Debug, PartialEq)]
pub(super) struct Heights {
    pub stack: [f32; 2],
    pub known: f32,
    pub max: Option<f32>,
}

pub(super) fn heights(bin: &Drawn, height: f32) -> Heights {
    let scale = f64::from(height) / (bin.to - bin.from) as f64;
    // Cycles counted, however few, are a pixel high at least.
    let pixels = |cycles: i64| {
        if cycles > 0 {
            ((cycles as f64 * scale).round() as f32).max(1.0)
        } else {
            0.0
        }
    };
    let first = pixels(bin.stack[0]);
    let second = pixels(bin.covered()).max(first + if bin.stack[1] > 0 { 1.0 } else { 0.0 });
    let known = pixels(bin.known).max(second).min(height);
    Heights {
        stack: [first.min(known), second.min(known)],
        known,
        max: bin.max.map(|max| (max * f64::from(known)).round() as f32),
    }
}

/// Paints `bins` in `mesh` within `area`, bars from `high` to `low`: the
/// layers of their stack in `colors`, their max line in the top layer's,
/// the time unknown hatched in `unknown`.
pub(super) fn paint(
    mesh: &mut egui::Mesh,
    bins: &[Drawn],
    area: Rect,
    (high, low): (f32, f32),
    colors: [Color32; 2],
    unknown: Color32,
) {
    let height = low - high;
    for bin in bins {
        let (left, right) = (bin.left.max(area.left()), bin.right.min(area.right()));
        if left >= right {
            continue;
        }
        let heights = heights(bin, height);
        let rect = |bottom: f32, top: f32| Rect::from_min_max(pos2(left, low - top), pos2(right, low - bottom));
        let [first, second] = heights.stack;
        if first > 0.0 {
            mesh.add_colored_rect(rect(0.0, first), colors[0]);
        }
        if second > first {
            mesh.add_colored_rect(rect(first, second), colors[1]);
        }
        if let Some(max) = heights.max
            && max > second
        {
            mesh.add_colored_rect(rect(max - 1.0, max), colors[1]);
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
