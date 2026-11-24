//! The view: the cycles it shows, and their pixels.

use super::rows::Mark;
use super::{NARROWEST, TICK};

/// The view's mapping between cycles and pixels.
pub struct View {
    /// The cycles the view counts from.
    pub origin: i64,
    /// The left edge and the width, in cycles from `origin`.
    pub start: f64,
    pub span: f64,
    /// The left edge and the width, in pixels.
    pub left: f32,
    pub width: f64,
}

impl View {
    /// The pixel of `cycles` from the origin.
    pub fn x(&self, cycles: f64) -> f32 {
        self.left + ((cycles - self.start) / self.span * self.width) as f32
    }

    /// The pixel column of `cycles`, absolute.
    pub fn column(&self, cycles: i64) -> f32 {
        self.x((cycles - self.origin) as f64).floor()
    }

    /// The cycles, absolute, at pixel `x`.
    pub fn cycles(&self, x: f32) -> i64 {
        self.origin + (self.start + f64::from(x - self.left) / self.width * self.span).round() as i64
    }

    /// The marks in view, at least a pixel wide, merged where they touch.
    pub(super) fn bands(&self, marks: impl Iterator<Item = Mark>) -> Vec<(f32, f32)> {
        let (low, high) = (
            self.cycles(self.left),
            self.origin + (self.start + self.span).ceil() as i64,
        );
        let mut bands: Vec<(f32, f32)> = Vec::new();
        for mark in marks.filter(|mark| mark.to >= low && mark.from <= high) {
            let from = self.x((mark.from - self.origin) as f64).floor().max(self.left - 1.0);
            let to = self.x((mark.to - self.origin) as f64).ceil().max(from + 1.0);
            match bands.last_mut() {
                Some(band) if from <= band.1 => band.1 = band.1.max(to),
                _ => bands.push((from, to)),
            }
        }
        bands
    }
}

/// The widest view of a capture from `first` to `last` cycles.
pub(super) fn widest(first: f64, last: f64) -> f64 {
    ((last - first) * 1.05).max(NARROWEST)
}

/// The view of the whole capture from `first` to `last` cycles: its left
/// edge and its width, a margin after the last event.
pub(super) fn whole(first: f64, last: f64) -> (f64, f64) {
    (first, widest(first, last))
}

/// `start` within the bounds a view of `span` cycles may start at: no
/// earlier than the first event, no later than half a view before the last.
pub(super) fn clamped(start: f64, span: f64, first: f64, last: f64) -> f64 {
    start.clamp(first, (last - span / 2.0).max(first))
}

/// The view of `span` cycles centred on `at`, within the bounds of
/// `clamped`: its left edge and its width, the same.
pub(super) fn centred(at: f64, span: f64, first: f64, last: f64) -> (f64, f64) {
    (clamped(at - span / 2.0, span, first, last), span)
}

/// The ruler's ticks in a view from `start` over `span` cycles, `width`
/// pixels wide: each one's cycles and label, in seconds when the frequency is
/// known, else in cycles.
pub(super) fn ticks(start: f64, span: f64, width: f64, frequency: Option<f64>) -> Vec<(f64, String)> {
    let hz = frequency.unwrap_or(1.0);
    let wanted = TICK * span / width / hz;
    let magnitude = 10f64.powf(wanted.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|n| n * magnitude)
        .find(|step| *step >= wanted)
        .unwrap_or(10.0 * magnitude);
    let (unit, suffix) = match frequency {
        None => (1.0, " cycles"),
        Some(_) if step < 1e-6 => (1e-9, " ns"),
        Some(_) if step < 1e-3 => (1e-6, " µs"),
        Some(_) if step < 1.0 => (1e-3, " ms"),
        Some(_) => (1.0, " s"),
    };
    let decimals = (-(step / unit).log10().floor()).max(0.0) as usize;
    let mut ticks = Vec::new();
    let mut tick = (start / hz / step).ceil();
    loop {
        let at = tick * step * hz;
        if at > start + span {
            break;
        }
        // Adding zero turns the -0 of a tick at the origin into 0.
        ticks.push((at, format!("{:.decimals$}{suffix}", tick * step / unit + 0.0)));
        tick += 1.0;
    }
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole view shows every event and no time before the first.
    #[test]
    fn the_whole_view_starts_at_the_first_event_and_shows_the_last() {
        for (first, last) in [(0.0, 1e6), (-500.0, 2e9), (0.0, 10.0), (3.0, 3.0)] {
            let (start, span) = whole(first, last);
            assert_eq!(start, first, "{first} {last}");
            assert!(start + span >= last, "{first} {last}: ends at {}", start + span);
        }
    }

    /// Panning or zooming never shows time before the first event.
    #[test]
    fn a_view_never_starts_before_the_first_event() {
        for start in [-1e9, -1.0, 0.0] {
            assert_eq!(clamped(start, 1000.0, 0.0, 1e6), 0.0);
        }
        assert_eq!(clamped(5e5, 1000.0, 0.0, 1e6), 5e5);
    }

    /// Going to an event keeps the zoom and centres the view on it, or
    /// shows it at an end of the capture.
    #[test]
    fn going_to_an_event_centres_it_at_the_same_zoom() {
        assert_eq!(centred(5e5, 1000.0, 0.0, 1e6), (5e5 - 500.0, 1000.0));
        for at in [0.0, 100.0, 1e6] {
            let (start, span) = centred(at, 1000.0, 0.0, 1e6);
            assert_eq!(span, 1000.0);
            assert!(start <= at && at <= start + span, "{at}: {start} {span}");
        }
    }

    /// A tick at the origin reads 0, not -0, from a view starting just
    /// before it, in seconds and in cycles.
    #[test]
    fn no_tick_reads_minus_zero() {
        for frequency in [Some(250e6), None] {
            let ticks = ticks(-3.0, 1e6, 1000.0, frequency);
            assert!(!ticks.is_empty());
            for (_, label) in ticks {
                assert!(!label.starts_with("-0"), "{label}");
            }
        }
    }
}
