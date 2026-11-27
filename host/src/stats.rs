//! Statistics of a range of a trace: instances in, distributions out.
//!
//! An instance is a subject's metric between two edges of the trace, its
//! value in cycles: the meaning of a kernel's trace (meaning.rs) says what
//! makes one. Here they are
//! counted, over the range `[from, to)` in cycles:
//!
//! - an instance entirely in the range counts, unless it touches a loss (an
//!   edge at one, or one between its edges) or covers an overrun write (a
//!   suspect whose indexes meet its own): then it is excluded, and the
//!   distribution's high values are a lower bound;
//! - an instance cut by a range edge, or still open at the trace's end, is
//!   counted apart, as cut;
//! - an instance outside the range is not counted at all.
//!
//! A distribution gives the count, the min and its instance, the
//! percentiles 50, 90 and 99 by nearest rank (the value of rank ceil(p·n)
//! among the sorted values, no interpolation), the max, the mean, the total,
//! a linear histogram and the worst instances. The histogram's bins are
//! 1, 2 or 5 × 10ⁿ cycles wide, the narrowest that gives at most 32 bins
//! between the min and the max, their bounds multiples of the width.
//!
//! A share is over the time known in the range: the adapter says when the
//! state of what it measures is known (`Meaning::known`).
//!
//! A curve's samples (values.rs) count when their event is in the range. A
//! sample whose index an overrun write covers is excluded; a NaN is counted
//! apart. A loss excludes nothing: a sample is a single event. Their spread
//! gives the count, the min and the max with their samples, the percentiles
//! as above and the mean.

use crate::events::{self, Catalogue};
use crate::load::Covered;
use crate::model::{Edge, Element, Trace};
use crate::swo::Watched;
use crate::target::Result;
use crate::values::{self, Curve, Number, Values};
use crate::{durations, meaning};
use serde_json::{Value, json};
use std::collections::HashMap;

/// What an instance measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metric {
    pub name: &'static str,
    /// Whether the total, over the time known, means something: the share
    /// of the range the subject spent so.
    pub share: bool,
    /// Whether the spread of the values, max − min, is the jitter.
    pub jitter: bool,
}

/// A subject's metric between two edges of the trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    /// What the subject is: task, handler, mutex, duration.
    pub kind: &'static str,
    pub subject: String,
    pub metric: Metric,
    /// What divides the metric further, or none for its total.
    pub detail: Option<String>,
    pub value: i64,
    pub from: Edge,
    pub to: Edge,
    /// Still open at the end of the trace: `to` is the last event.
    pub open: bool,
    /// Dated by the SWO: only a lost edge of its own excludes it, the ring's
    /// losses and suspects hiding nothing of it.
    pub swo: bool,
}

/// The range, in cycles: none for no bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// A value and where its instance starts: an event's index and instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Point<V = i64> {
    pub value: V,
    pub element: usize,
    pub index: u32,
    pub cycles: i64,
    pub seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub width: i64,
    /// The first bin's lower bound; each bin is `[from, from + width)`.
    pub first: i64,
    pub counts: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Distribution {
    pub min: Point,
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
    /// The max and its instance, the earliest among equals.
    pub max: Point,
    pub mean: f64,
    pub total: i64,
    pub histogram: Histogram,
    /// The highest values first, the earliest first among equals.
    pub worst: Vec<Point>,
    /// Every value, lowest first.
    pub values: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: &'static str,
    pub subject: String,
    pub metric: Metric,
    pub detail: Option<String>,
    pub count: usize,
    pub excluded_loss: usize,
    pub excluded_suspect: usize,
    pub cut: usize,
    /// None without an instance counted.
    pub distribution: Option<Distribution>,
}

impl Group {
    /// Whether instances were excluded: the max and the high percentiles
    /// are then only a lower bound.
    pub fn lower_bound(&self) -> bool {
        self.excluded_loss + self.excluded_suspect > 0
    }

    pub fn jitter(&self) -> Option<i64> {
        self.distribution
            .as_ref()
            .filter(|_| self.metric.jitter)
            .map(|distribution| distribution.max.value - distribution.min.value)
    }

    /// The total over the time known, for a metric that has a share.
    pub fn share(&self, known: i64) -> Option<f64> {
        let total = self.distribution.as_ref().map_or(0, |distribution| distribution.total);
        (self.metric.share && known > 0).then(|| total as f64 / known as f64)
    }
}

/// The samples of a curve in the range.
#[derive(Clone, Debug, PartialEq)]
pub struct Spread {
    /// The min and its sample, the earliest among equals.
    pub min: Point<Number>,
    pub p50: Number,
    pub p90: Number,
    pub p99: Number,
    /// The max and its sample, the earliest among equals.
    pub max: Point<Number>,
    pub mean: f64,
    /// Every value, lowest first.
    pub values: Vec<Number>,
}

/// A curve's statistics: its samples in the range.
#[derive(Clone, Debug, PartialEq)]
pub struct Sampled {
    pub curve: Curve,
    pub count: usize,
    /// A watched variable's samples dropped in a gap of the SWO stream.
    pub excluded_loss: usize,
    /// The samples an overrun write may have made.
    pub excluded_suspect: usize,
    pub nan: usize,
    /// None without a sample counted.
    pub spread: Option<Spread>,
}

pub struct Report {
    /// The frequency of the seconds; none without one.
    pub frequency: Option<f64>,
    /// The range, within the trace's events: from the later of its start
    /// and the first event, to the earlier of its end and the last event.
    pub from: i64,
    pub to: i64,
    /// The cycles of the first event, where the seconds start.
    pub origin: i64,
    /// The cycles of the range in which the adapter knows the state.
    pub known: i64,
    /// The first index recorded, when the recording started after index 0:
    /// what came before it is unknown, without being a loss.
    pub joined: Option<u32>,
    /// The losses read in the range.
    pub losses: usize,
    pub suspects: u64,
    pub contradictions: usize,
    /// The hardware faults in the range.
    pub faults: usize,
    pub groups: Vec<Group>,
    /// The curves the image declares, in order.
    pub curves: Vec<Curve>,
    /// The curves sampled in the range, in order.
    pub values: Vec<Sampled>,
    /// What the trace's reader should know of how it was traced.
    pub warnings: Vec<String>,
}

/// The value of rank ceil(percent·n/100) among `sorted`, at least the first.
pub fn percentile<V: Copy>(sorted: &[V], percent: usize) -> V {
    let rank = (percent * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// The bins of values from `min` to `max`.
pub fn histogram(sorted: &[i64]) -> Histogram {
    let (min, max) = (sorted[0], sorted[sorted.len() - 1]);
    let mut width = 1;
    'widths: for decade in std::iter::successors(Some(1i64), |decade| decade.checked_mul(10)) {
        for step in [1, 2, 5] {
            width = step * decade;
            if max.div_euclid(width) - min.div_euclid(width) < 32 {
                break 'widths;
            }
        }
    }
    let first = min.div_euclid(width) * width;
    let mut counts = vec![0; (max.div_euclid(width) - min.div_euclid(width) + 1) as usize];
    for value in sorted {
        counts[(value.div_euclid(width) - min.div_euclid(width)) as usize] += 1;
    }
    Histogram { width, first, counts }
}

/// The distribution of `points`, the `worst` highest kept.
pub fn distribution(points: &[Point], worst: usize) -> Option<Distribution> {
    let mut by_value: Vec<&Point> = points.iter().collect();
    by_value.sort_by_key(|point| (point.value, point.element));
    let min = (*by_value.first()?).clone();
    let sorted: Vec<i64> = by_value.iter().map(|point| point.value).collect();
    let total: i64 = sorted.iter().sum();
    by_value.sort_by_key(|point| (std::cmp::Reverse(point.value), point.element));
    Some(Distribution {
        min,
        p50: percentile(&sorted, 50),
        p90: percentile(&sorted, 90),
        p99: percentile(&sorted, 99),
        max: (*by_value[0]).clone(),
        mean: total as f64 / sorted.len() as f64,
        total,
        histogram: histogram(&sorted),
        worst: by_value.into_iter().take(worst).cloned().collect(),
        values: sorted,
    })
}

/// The spread of a curve's samples `points`.
pub fn spread(points: &[Point<Number>]) -> Option<Spread> {
    let mut by_value: Vec<&Point<Number>> = points.iter().collect();
    by_value.sort_by(|a, b| a.value.total_cmp(&b.value).then(a.element.cmp(&b.element)));
    let min = (*by_value.first()?).clone();
    let sorted: Vec<Number> = by_value.iter().map(|point| point.value).collect();
    let top = sorted[sorted.len() - 1];
    let max = (*by_value[sorted.partition_point(|value| value.total_cmp(&top).is_lt())]).clone();
    let integers: Option<i128> = sorted
        .iter()
        .map(|value| match value {
            Number::Integer(value) => Some(*value),
            Number::Float(_) => None,
        })
        .sum();
    let total = integers.map_or_else(|| sorted.iter().map(|value| value.as_f64()).sum(), |total| total as f64);
    Some(Spread {
        min,
        p50: percentile(&sorted, 50),
        p90: percentile(&sorted, 90),
        p99: percentile(&sorted, 99),
        max,
        mean: total / sorted.len() as f64,
        values: sorted,
    })
}

/// The statistics of the curves `values` sampled over `range` of `trace`.
pub fn sampled(trace: &Trace, values: &Values, range: Range) -> Vec<Sampled> {
    let places = Places::new(trace);
    let low = range.from.unwrap_or(i64::MIN);
    let high = range.to.unwrap_or(i64::MAX);
    let mut curves: Vec<(Sampled, Vec<Point<Number>>)> = Vec::new();
    let mut by_curve: HashMap<&Curve, usize> = HashMap::new();
    let mut of = |curves: &mut Vec<(Sampled, Vec<Point<Number>>)>, curve| {
        *by_curve.entry(curve).or_insert_with(|| {
            let empty = Sampled {
                curve: curve.clone(),
                count: 0,
                excluded_loss: 0,
                excluded_suspect: 0,
                nan: 0,
                spread: None,
            };
            curves.push((empty, Vec::new()));
            curves.len() - 1
        })
    };
    let dropped = trace.watched.iter().flat_map(|watched| &watched.dropped);
    for (curve, cycles) in dropped {
        if *cycles >= low && *cycles < high {
            let curve = of(&mut curves, curve);
            curves[curve].0.excluded_loss += 1;
        }
    }
    for sample in &values.samples {
        if sample.at.cycles < low || sample.at.cycles >= high {
            continue;
        }
        let curve = of(&mut curves, &sample.curve);
        let (counts, points) = &mut curves[curve];
        let watched = sample.curve.field.is_none();
        if !watched && places.covers_suspect(sample.at, sample.at) {
            counts.excluded_suspect += 1;
        } else if sample.value.is_nan() {
            counts.nan += 1;
        } else {
            counts.count += 1;
            let mut point = places.point(sample.value, sample.at);
            if watched {
                point.seconds = places.seconds(sample.at.cycles);
            }
            points.push(point);
        }
    }
    curves.sort_by(|a, b| a.0.curve.cmp(&b.0.curve));
    curves
        .into_iter()
        .map(|(sampled, points)| Sampled {
            spread: spread(&points),
            ..sampled
        })
        .collect()
}

/// What the trace says around each element.
struct Places {
    /// By element: its cycles, or for a loss or a suspect, the event's
    /// before it (after it, at the start).
    cycles: Vec<i64>,
    /// By element: the index of its event, or of the event before it.
    index: Vec<u32>,
    seconds: Vec<Option<f64>>,
    /// The elements that are losses.
    losses: Vec<usize>,
    /// The suspects' indexes `from..to`.
    suspects: Vec<(u32, u32)>,
    /// The cycles of the first event, where the seconds start, and their
    /// frequency.
    origin: i64,
    frequency: Option<f64>,
}

impl Places {
    fn new(trace: &Trace) -> Self {
        let first = trace.elements.iter().find_map(|element| match element {
            Element::Event { event, cycles, .. } => Some((*cycles, event.index)),
            _ => None,
        });
        let (mut cycles_before, mut index_before) = first.unwrap_or((0, trace.first));
        let mut places = Self {
            cycles: Vec::new(),
            index: Vec::new(),
            seconds: Vec::new(),
            losses: Vec::new(),
            suspects: Vec::new(),
            origin: cycles_before,
            frequency: trace.frequency,
        };
        for (n, element) in trace.elements.iter().enumerate() {
            let mut seconds = None;
            match element {
                Element::Event {
                    event,
                    cycles,
                    seconds: at,
                } => {
                    (cycles_before, index_before) = (*cycles, event.index);
                    seconds = *at;
                }
                Element::Lost { .. } => places.losses.push(n),
                Element::Suspect { from, to, .. } => places.suspects.push((*from, *to)),
            }
            places.cycles.push(cycles_before);
            places.index.push(index_before);
            places.seconds.push(seconds);
        }
        // The samples of a trace without elements, of SWO alone, are at
        // element 0: the start.
        if trace.elements.is_empty() {
            places.cycles.push(cycles_before);
            places.index.push(index_before);
            places.seconds.push(None);
        }
        places
    }

    fn touches_loss(&self, from: Edge, to: Edge) -> bool {
        let after = self.losses.partition_point(|&loss| loss <= from.element);
        from.lost || to.lost || self.losses.get(after).is_some_and(|&loss| loss < to.element)
    }

    fn covers_suspect(&self, from: Edge, to: Edge) -> bool {
        let (low, high) = (self.index[from.element], self.index[to.element]);
        self.suspects.iter().any(|&(from, to)| from <= high && to > low)
    }

    /// The seconds from the first event to `cycles`.
    fn seconds(&self, cycles: i64) -> Option<f64> {
        self.frequency.map(|hz| (cycles - self.origin) as f64 / hz)
    }

    fn point<V>(&self, value: V, edge: Edge) -> Point<V> {
        Point {
            value,
            element: edge.element,
            index: self.index[edge.element],
            cycles: edge.cycles,
            seconds: self.seconds[edge.element],
        }
    }
}

/// What the adapter found in a trace: its instances, the elements against
/// its order and the time in which it knows the state.
pub struct Found<'a> {
    pub instances: &'a [Instance],
    pub contradictions: &'a [usize],
    /// The elements that are faults.
    pub faults: &'a [usize],
    pub known: &'a Covered,
}

/// The statistics of `trace`, read with `catalogue`, over `range`: those of
/// its kernel adapter, its durations and its curves, the `worst` highest
/// instances kept for each group. An image with none is an error.
pub fn of_trace(catalogue: &Catalogue, trace: &Trace, range: Range, worst: usize) -> Result<Report> {
    let mut kernel = meaning::Meaning::new(catalogue, trace.first)?;
    let durations = durations::pair(trace);
    let has_durations = catalogue
        .events
        .values()
        .any(|event| events::Duration::of(&event.name).is_some());
    let values = values::sample(catalogue, trace);
    if !kernel.traced() && !has_durations && values.curves.is_empty() {
        return Err("the image has no kernel adapter events, durations or values".into());
    }
    kernel.add(&trace.elements, 0);
    if let Some(watched) = &trace.watched {
        kernel.exceptions(watched, &trace.elements);
    }
    let mut instances = meaning::instances(&kernel);
    instances.extend(durations.instances);
    let mut contradictions = kernel.contradictions.clone();
    contradictions.extend(durations.contradictions);
    contradictions.extend(kernel.exception_contradictions.iter().map(|edge| edge.element));
    let faults: Vec<usize> = kernel.faults.iter().map(|faulted| faulted.edge.element).collect();
    let known = match &trace.watched {
        Some(watched) if !has_events(trace) => segments(watched),
        _ => kernel.known(),
    };
    let found = Found {
        instances: &instances,
        contradictions: &contradictions,
        faults: &faults,
        known: &known,
    };
    let mut report = compute(trace, found, range, worst);
    report.values = sampled(trace, &values, range);
    report.curves = values.curves;
    report.warnings.extend(kernel.double_trace());
    Ok(report)
}

fn has_events(trace: &Trace) -> bool {
    trace
        .elements
        .iter()
        .any(|element| matches!(element, Element::Event { .. }))
}

/// The time a trace of SWO alone is known: its span less its gaps.
fn segments(watched: &Watched) -> Covered {
    let mut known = Covered::default();
    if let Some((first, last)) = watched.span {
        let mut from = first;
        for &(start, end) in &watched.gaps {
            known.push(from, start);
            from = end;
        }
        known.push(from, last);
    }
    known
}

/// The statistics of what the adapter `found` over `range` of `trace`, the
/// `worst` highest instances kept for each group.
pub fn compute(trace: &Trace, found: Found, range: Range, worst: usize) -> Report {
    let Found {
        instances,
        contradictions,
        faults,
        known,
    } = found;
    let places = Places::new(trace);
    let events: Vec<i64> = trace
        .elements
        .iter()
        .filter_map(|element| match element {
            Element::Event { cycles, .. } => Some(*cycles),
            _ => None,
        })
        .collect();
    // Without events, of SWO alone, the trace spans the ticks.
    let span = trace.watched.as_ref().and_then(|watched| watched.span);
    let gaps: &[(i64, i64)] = trace.watched.as_ref().map_or(&[], |watched| &watched.gaps);
    let (origin, last) = match (events.first(), events.last(), span) {
        (Some(&first), Some(&last), _) => (first, last),
        (_, _, Some(span)) => span,
        _ => (0, 0),
    };
    let low = range.from.unwrap_or(i64::MIN);
    let high = range.to.unwrap_or(i64::MAX);
    let (from, to) = (low.max(origin), high.min(last).max(low.max(origin)));
    let inside = |cycles: i64| cycles >= low && cycles < high;
    // Without events, the losses are the SWO's gaps.
    let losses = if events.is_empty() {
        gaps.iter().filter(|&&(gap, _)| inside(gap)).count()
    } else {
        places
            .losses
            .iter()
            .filter(|&&loss| inside(places.cycles[loss]))
            .count()
    };
    let suspects = trace
        .elements
        .iter()
        .enumerate()
        .map(|(n, element)| match element {
            Element::Suspect { writers, .. } if inside(places.cycles[n]) => u64::from(*writers),
            _ => 0,
        })
        .sum();
    let contradictions = contradictions
        .iter()
        .filter(|&&element| inside(places.cycles[element]))
        .count();
    let faults = faults.iter().filter(|&&element| inside(places.cycles[element])).count();

    let mut groups: Vec<Group> = Vec::new();
    let mut points: Vec<Vec<Point>> = Vec::new();
    let mut by_key: HashMap<(&str, &str, &str, Option<&str>), usize> = HashMap::new();
    for instance in instances {
        let (start, end) = (instance.from.cycles, instance.to.cycles);
        let whole = !instance.open && start >= low && end <= high;
        if !whole && !(start < high && end >= low) {
            continue;
        }
        let key = (
            instance.kind,
            instance.subject.as_str(),
            instance.metric.name,
            instance.detail.as_deref(),
        );
        let group = *by_key.entry(key).or_insert_with(|| {
            groups.push(Group {
                kind: instance.kind,
                subject: instance.subject.clone(),
                metric: instance.metric,
                detail: instance.detail.clone(),
                count: 0,
                excluded_loss: 0,
                excluded_suspect: 0,
                cut: 0,
                distribution: None,
            });
            points.push(Vec::new());
            groups.len() - 1
        });
        let counts = &mut groups[group];
        if !whole {
            counts.cut += 1;
        } else if instance.swo {
            // A gap of the SWO dropped the packets in it: a span across one,
            // a period, may miss an entry.
            let across = gaps
                .iter()
                .any(|&(gap, _)| gap >= instance.from.cycles && gap < instance.to.cycles);
            if instance.from.lost || instance.to.lost || across {
                counts.excluded_loss += 1;
            } else {
                counts.count += 1;
                let mut point = places.point(instance.value, instance.from);
                point.seconds = places.seconds(instance.from.cycles);
                points[group].push(point);
            }
        } else if places.touches_loss(instance.from, instance.to) {
            counts.excluded_loss += 1;
        } else if places.covers_suspect(instance.from, instance.to) {
            counts.excluded_suspect += 1;
        } else {
            counts.count += 1;
            points[group].push(places.point(instance.value, instance.from));
        }
    }
    for (group, points) in groups.iter_mut().zip(&points) {
        group.distribution = distribution(points, worst);
    }
    Report {
        frequency: trace.frequency,
        from,
        to,
        origin,
        known: known.until(to) - known.until(from),
        joined: (trace.first != 0).then_some(trace.first),
        losses,
        suspects,
        contradictions,
        faults,
        groups,
        curves: Vec::new(),
        values: Vec::new(),
        warnings: Vec::new(),
    }
}

/// `cycles` as a duration: in seconds at the frequency, three significant
/// digits, or else in cycles.
pub fn duration(cycles: i64, frequency: Option<f64>) -> String {
    let Some(hz) = frequency else {
        return format!("{cycles} cycles");
    };
    let seconds = cycles as f64 / hz;
    let (value, unit) = match seconds.abs() {
        s if s >= 1.0 || s == 0.0 => (seconds, "s"),
        s if s >= 1e-3 => (seconds * 1e3, "ms"),
        s if s >= 1e-6 => (seconds * 1e6, "µs"),
        _ => (seconds * 1e9, "ns"),
    };
    let digits = if value.abs() >= 100.0 {
        0
    } else if value.abs() >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.digits$} {unit}")
}

/// Where an instance starts: its event's index, and its instant.
fn at<V>(point: &Point<V>) -> String {
    match point.seconds {
        Some(seconds) => format!("#{} ({seconds:.6} s)", point.index),
        None => format!("#{}", point.index),
    }
}

/// The report, a line for the range and one for each group.
pub fn text(report: &Report) -> Vec<String> {
    let frequency = report.frequency;
    let instant = |cycles: i64| match frequency {
        Some(hz) => format!("{:.6} s", (cycles - report.origin) as f64 / hz),
        None => format!("{} cycles", cycles - report.origin),
    };
    let mut lines = vec![format!(
        "range {} to {}: {} known, {} losses, {} suspect writers, {} contradictions, {} faults",
        instant(report.from),
        instant(report.to),
        duration(report.known, frequency),
        report.losses,
        report.suspects,
        report.contradictions,
        report.faults
    )];
    if let Some(index) = report.joined {
        lines[0] += &format!(", joined at index {index}");
    }
    for group in &report.groups {
        let mut line = format!("{} {} {}", group.kind, group.subject, group.metric.name);
        if let Some(detail) = &group.detail {
            line += &format!(" {detail}");
        }
        line += &format!(": {}", group.count);
        if let Some(distribution) = &group.distribution {
            let bound = if group.lower_bound() { "≥ " } else { "" };
            let value = |cycles| duration(cycles, frequency);
            line += &format!(
                ", min {} at {}, p50 {}, p90 {bound}{}, p99 {bound}{}, max {bound}{} at {}, mean {}",
                value(distribution.min.value),
                at(&distribution.min),
                value(distribution.p50),
                value(distribution.p90),
                value(distribution.p99),
                value(distribution.max.value),
                at(&distribution.max),
                value(distribution.mean.round() as i64),
            );
        }
        if let Some(jitter) = group.jitter() {
            let bound = if group.lower_bound() { "≥ " } else { "" };
            line += &format!(", jitter {bound}{}", duration(jitter, frequency));
        }
        if let Some(share) = group.share(report.known) {
            line += &format!(", {:.2} % of the time known", share * 100.0);
        }
        if group.lower_bound() {
            line += &format!(
                " (excluded: {} loss, {} suspect)",
                group.excluded_loss, group.excluded_suspect
            );
        }
        if group.cut > 0 {
            line += &format!(" ({} cut)", group.cut);
        }
        lines.push(line);
    }
    for sampled in &report.values {
        let curve = &sampled.curve;
        let mut line = format!("{kind} {} {kind}", curve.subject, kind = values::KIND);
        if let Some(field) = &curve.field {
            line += &format!(" {field}");
        }
        line += &format!(": {}", sampled.count);
        if let Some(spread) = &sampled.spread {
            line += &format!(
                ", min {} at {}, p50 {}, p90 {}, p99 {}, max {} at {}, mean {}",
                spread.min.value,
                at(&spread.min),
                spread.p50,
                spread.p90,
                spread.p99,
                spread.max.value,
                at(&spread.max),
                spread.mean,
            );
        }
        if sampled.nan > 0 {
            line += &format!(", {} NaN", sampled.nan);
        }
        if sampled.excluded_loss + sampled.excluded_suspect > 0 {
            line += &format!(
                " (excluded: {} loss, {} suspect)",
                sampled.excluded_loss, sampled.excluded_suspect
            );
        }
        lines.push(line);
    }
    lines
}

fn point(point: &Point) -> Value {
    json!({
        "value": point.value,
        "index": point.index,
        "cycles": point.cycles,
        "seconds": point.seconds,
    })
}

/// A sample's value as JSON: an integer exactly, a floating-point number
/// null when it is not finite.
fn number(value: Number) -> Value {
    match value {
        Number::Integer(value) => i64::try_from(value)
            .map(Value::from)
            .or_else(|_| u64::try_from(value).map(Value::from))
            .unwrap_or(Value::Null),
        Number::Float(value) => json!(value),
    }
}

fn sample(point: &Point<Number>) -> Value {
    json!({
        "value": number(point.value),
        "index": point.index,
        "cycles": point.cycles,
        "seconds": point.seconds,
    })
}

/// A curve's statistics as a group of the JSON.
fn curve(sampled: &Sampled) -> Value {
    let distribution = sampled.spread.as_ref().map(|spread| {
        json!({
            "min": sample(&spread.min),
            "p50": number(spread.p50),
            "p90": number(spread.p90),
            "p99": number(spread.p99),
            "max": sample(&spread.max),
            "mean": spread.mean,
        })
    });
    json!({
        "kind": values::KIND,
        "subject": sampled.curve.subject,
        "metric": values::KIND,
        "detail": sampled.curve.field,
        "count": sampled.count,
        "excluded_loss": sampled.excluded_loss,
        "excluded_suspect": sampled.excluded_suspect,
        "cut": 0,
        "lower_bound": sampled.excluded_loss + sampled.excluded_suspect > 0,
        "jitter": null,
        "share": null,
        "nan": sampled.nan,
        "distribution": distribution,
    })
}

/// The report as JSON, the shape host/schema/stats.schema.json gives. A
/// number that is not finite is null.
pub fn json(report: &Report) -> String {
    let groups: Vec<Value> = report
        .groups
        .iter()
        .map(|group| {
            let distribution = group.distribution.as_ref().map(|distribution| {
                json!({
                    "min": point(&distribution.min),
                    "p50": distribution.p50,
                    "p90": distribution.p90,
                    "p99": distribution.p99,
                    "max": point(&distribution.max),
                    "mean": distribution.mean,
                    "total": distribution.total,
                    "histogram": {
                        "width": distribution.histogram.width,
                        "first": distribution.histogram.first,
                        "counts": distribution.histogram.counts,
                    },
                    "worst": distribution.worst.iter().map(point).collect::<Vec<_>>(),
                })
            });
            json!({
                "kind": group.kind,
                "subject": group.subject,
                "metric": group.metric.name,
                "detail": group.detail,
                "count": group.count,
                "excluded_loss": group.excluded_loss,
                "excluded_suspect": group.excluded_suspect,
                "cut": group.cut,
                "lower_bound": group.lower_bound(),
                "jitter": group.jitter(),
                "share": group.share(report.known),
                "distribution": distribution,
            })
        })
        .chain(report.values.iter().map(curve))
        .collect();
    json!({
        "schema": "kymo-stats",
        "version": 1,
        "frequency": report.frequency,
        "from": report.from,
        "to": report.to,
        "origin": report.origin,
        "known": report.known,
        "joined": report.joined,
        "losses": report.losses,
        "suspects": report.suspects,
        "contradictions": report.contradictions,
        "faults": report.faults,
        "groups": groups,
    })
    .to_string()
}

#[cfg(test)]
pub(crate) mod tests;
