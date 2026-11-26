//! The application's values on the timeline (ADR-096): a line per curve
//! chosen, its samples as `kymo::values` reads them, the events' and the
//! variables' the DWT watched (ADR-098). A sample holds until the
//! next one, in steps; a loss cuts the step, and its value is unknown until
//! the next sample; a NaN is a gap. They grow with the trace.
//!
//! A line draws at most one bar per pixel column, from the lowest to the
//! highest value the column shows: the step that enters it and the samples
//! in it. A peak of a single sample stays visible at any zoom.

use crate::timeline::View;
use kymo::events::Catalogue;
use kymo::model::Element;
use kymo::swo::Watched;
use kymo::values::{Curve, Number, curves, samples_of};
use std::collections::BTreeSet;

/// How many pixels a line's view gives each sample, at least, for its
/// samples to be drawn as dots on their steps.
const DOTS: f64 = 8.0;

/// A sample of a curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub cycles: i64,
    pub element: usize,
    pub value: Number,
    /// When a loss comes before the curve's next sample: the cycles of the
    /// last event before the loss, where the step ends.
    pub cut: Option<i64>,
}

impl Sample {
    /// Whether its step reaches `cycles`, from its own on.
    pub fn holds_at(&self, cycles: i64) -> bool {
        self.cut.is_none_or(|cut| cut >= cycles)
    }
}

pub struct Values {
    /// The curves the image declares, sorted.
    pub curves: Vec<Curve>,
    /// By curve, sorted by cycles.
    pub samples: Vec<Vec<Sample>>,
    /// The curves shown, by their place in `curves`.
    pub chosen: BTreeSet<usize>,
    /// The cycles of the last event, in the trace's order.
    last: Option<i64>,
    /// The cycles of the latest event: where the last steps end.
    pub end: i64,
}

impl Values {
    /// None when the image declares no curve and the DWT watched no
    /// variable. A watched variable's samples are all in `watched`: a gap
    /// cuts the step before it, and the last step ends with the anchors.
    pub fn new(catalogue: &Catalogue, watched: Option<&Watched>) -> Option<Self> {
        let watched = watched.filter(|watched| watched.refused.is_none());
        let mut curves = curves(catalogue);
        curves.extend(watched.iter().flat_map(|watched| watched.curves.iter().cloned()));
        curves.sort();
        curves.dedup();
        let mut values = (!curves.is_empty()).then(|| Self {
            samples: vec![Vec::new(); curves.len()],
            curves,
            chosen: BTreeSet::new(),
            last: None,
            end: i64::MIN,
        })?;
        if let Some(watched) = watched {
            values.watch(watched);
        }
        Some(values)
    }

    /// Takes in the samples of the variables `watched`.
    fn watch(&mut self, watched: &Watched) {
        for sampled in &watched.samples {
            let curve = self.place(&sampled.curve).expect("a curve watched");
            self.samples[curve].push(Sample {
                cycles: sampled.at.cycles,
                element: sampled.at.element,
                value: sampled.value,
                cut: None,
            });
        }
        let last = watched.span.map(|(_, last)| last);
        for curve in &watched.curves {
            let samples = &mut self.samples[self.curves.binary_search(curve).expect("a curve watched")];
            samples.sort_by_key(|sample| sample.cycles);
            for n in 0..samples.len() {
                let next = samples.get(n + 1).map(|sample| sample.cycles);
                let at = samples[n].cycles;
                let gap = watched.gaps.iter().map(|&(start, _)| start).find(|&start| start >= at);
                samples[n].cut = match (gap, next) {
                    (Some(start), Some(next)) if start < next => Some(start),
                    (Some(start), None) => Some(start),
                    (_, Some(_)) => None,
                    (None, None) => last,
                };
            }
        }
    }

    /// Takes in `elements` from `from` on. A loss cuts the step of each
    /// curve's last sample at the last event before it.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        for (element, item) in elements.iter().enumerate().skip(from) {
            match item {
                Element::Event { cycles, .. } => {
                    for sampled in samples_of(element, item) {
                        let curve = self.place(&sampled.curve).expect("a curve of the catalogue");
                        self.samples[curve].push(Sample {
                            cycles: sampled.at.cycles,
                            element,
                            value: sampled.value,
                            cut: None,
                        });
                    }
                    self.last = Some(*cycles);
                    self.end = self.end.max(*cycles);
                }
                Element::Lost { .. } => {
                    let Some(last) = self.last else {
                        continue;
                    };
                    // A watched variable's last step is cut already: at the
                    // gap after it, or at the last anchor.
                    for sample in self.samples.iter_mut().filter_map(|samples| samples.last_mut()) {
                        sample.cut.get_or_insert(last.max(sample.cycles));
                    }
                }
                Element::Suspect { .. } => {}
            }
        }
        // Channels interleave: a curve's samples may come out of order.
        for samples in &mut self.samples {
            if !samples.is_sorted_by_key(|sample| sample.cycles) {
                samples.sort_by_key(|sample| sample.cycles);
            }
        }
    }

    /// The place of `curve` in `curves`.
    pub fn place(&self, curve: &Curve) -> Option<usize> {
        self.curves.binary_search(curve).ok()
    }

    /// The place of the curve named `name`, as `event.field`.
    pub fn named(&self, name: &str) -> Option<usize> {
        self.curves.iter().position(|curve| curve.to_string() == name)
    }

    /// The place of the curve `name` designates: its name in full, or with
    /// scopes left out from the left, the anonymous namespaces never written.
    /// An error lists the curves it could designate: none, or several.
    pub fn designated(&self, name: &str) -> Result<usize, Vec<String>> {
        if let Some(n) = self.named(name) {
            return Ok(n);
        }
        let ends = |curve: &Curve| {
            let field = curve
                .field
                .as_ref()
                .map(|field| format!(".{field}"))
                .unwrap_or_default();
            let scopes = crate::names::scopes(&curve.subject);
            (0..scopes.len()).any(|from| format!("{}{field}", scopes[from..].join("::")) == name)
        };
        let found: Vec<usize> = (0..self.curves.len()).filter(|&n| ends(&self.curves[n])).collect();
        match found[..] {
            [n] => Ok(n),
            _ => Err(found.iter().map(|&n| self.curves[n].to_string()).collect()),
        }
    }

    /// The curves chosen, one a line, to save.
    pub fn saved(&self) -> String {
        let names: Vec<String> = self.chosen.iter().map(|&n| self.curves[n].to_string()).collect();
        names.join("\n")
    }

    /// Chooses the curves `saved` names, one a line, those the image has.
    pub fn restore(&mut self, saved: &str) {
        let chosen: Vec<usize> = saved.lines().filter_map(|name| self.named(name)).collect();
        self.chosen.extend(chosen);
    }

    /// The sample `curve` holds at `cycles`: the last at or before them.
    pub fn held(&self, curve: usize, cycles: i64) -> Option<&Sample> {
        let samples = &self.samples[curve];
        let n = samples.partition_point(|sample| sample.cycles <= cycles);
        n.checked_sub(1).map(|n| &samples[n])
    }
}

/// Pixel columns `from..to` drawn alike, from value `low` to `high`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub from: f32,
    pub to: f32,
    pub low: f64,
    pub high: f64,
}

/// A curve in a view: its bars, the lowest and highest value they reach,
/// and, zoomed in, a dot at each sample.
#[derive(Debug, Default, PartialEq)]
pub struct Drawn {
    pub bars: Vec<Bar>,
    pub low: f64,
    pub high: f64,
    pub dots: Vec<(f32, f64)>,
}

/// `samples`, sorted, in `view`, the capture ending at `end`. Each pixel
/// column shows the lowest and the highest of the step that enters it and of
/// the samples in it, those not NaN; a step that a loss has cut or a NaN
/// holds enters none, nor does one past `end`.
pub fn drawn(samples: &[Sample], end: i64, view: &View) -> Drawn {
    let column = |cycles: i64| view.column(cycles);
    let left = view.left.floor();
    let right = view.left + view.width as f32;
    let first = samples.partition_point(|sample| column(sample.cycles) < left);
    let stop = samples.partition_point(|sample| column(sample.cycles) < right);
    // The step a sample starts: its value and the last column it reaches.
    let step = |sample: &Sample| {
        (!sample.value.is_nan()).then(|| (sample.value.as_f64(), column(sample.cut.unwrap_or(end).min(end))))
    };
    let mut held = first.checked_sub(1).and_then(|n| step(&samples[n]));
    let mut next = first;
    let mut drawn = Drawn::default();
    let mut x = left;
    while x < right {
        let mut range = held.filter(|&(_, end)| end >= x).map(|(value, _)| (value, value));
        while next < stop && column(samples[next].cycles) <= x {
            let sample = &samples[next];
            if !sample.value.is_nan() {
                let value = sample.value.as_f64();
                range = Some(range.map_or((value, value), |(low, high)| (low.min(value), high.max(value))));
            }
            held = step(sample);
            next += 1;
        }
        if let Some((low, high)) = range {
            match drawn.bars.last_mut() {
                Some(bar) if bar.to == x && bar.low == low && bar.high == high => bar.to = x + 1.0,
                _ => drawn.bars.push(Bar {
                    from: x,
                    to: x + 1.0,
                    low,
                    high,
                }),
            }
        }
        x += 1.0;
    }
    drawn.low = drawn.bars.iter().map(|bar| bar.low).fold(f64::INFINITY, f64::min);
    drawn.high = drawn.bars.iter().map(|bar| bar.high).fold(f64::NEG_INFINITY, f64::max);
    if (stop - first) as f64 * DOTS <= view.width {
        drawn.dots = samples[first..stop]
            .iter()
            .filter(|sample| !sample.value.is_nan())
            .map(|sample| (view.x((sample.cycles - view.origin) as f64), sample.value.as_f64()))
            .collect();
    }
    drawn
}

/// A value on a line's edge: an integer as it is, else to six significant
/// digits.
pub fn edge(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{value:.0}");
    }
    let digits = (5 - value.abs().log10().floor() as i32).clamp(0, 12) as usize;
    let text = format!("{value:.digits$}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.to_owned()
}

#[cfg(test)]
mod tests;
