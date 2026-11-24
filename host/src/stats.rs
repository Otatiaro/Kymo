//! Statistics of a range of a trace: instances in, distributions out.
//!
//! An instance is a subject's metric between two edges of the trace, its
//! value in cycles: an adapter (opsy.rs) says what makes one. Here they are
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

use crate::model::{Edge, Element, Trace};
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
    /// What the subject is: task, handler, mutex.
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
}

/// The range, in cycles: none for no bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// A value and where its instance starts: an event's index and instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub value: i64,
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

pub struct Report {
    /// The frequency of the seconds; none without one.
    pub frequency: Option<f64>,
    /// The range, within the trace's events: from the later of its start
    /// and the first event, to the earlier of its end and the last event.
    pub from: i64,
    pub to: i64,
    /// The cycles of the first event, where the seconds start.
    pub origin: i64,
    /// The range less what its losses hid.
    pub known: i64,
    pub losses: usize,
    pub suspects: u64,
    pub contradictions: usize,
    pub groups: Vec<Group>,
}

/// The value of rank ceil(percent·n/100) among `sorted`, at least the first.
pub fn percentile(sorted: &[i64], percent: usize) -> i64 {
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
    })
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

    fn point(&self, value: i64, edge: Edge) -> Point {
        Point {
            value,
            element: edge.element,
            index: self.index[edge.element],
            cycles: edge.cycles,
            seconds: self.seconds[edge.element],
        }
    }
}

/// The statistics of `instances` over `range` of `trace`, `contradictions`
/// the elements the adapter found against its order, the `worst` highest
/// instances kept for each group.
pub fn compute(trace: &Trace, instances: &[Instance], contradictions: &[usize], range: Range, worst: usize) -> Report {
    let places = Places::new(trace);
    let events: Vec<i64> = trace
        .elements
        .iter()
        .filter_map(|element| match element {
            Element::Event { cycles, .. } => Some(*cycles),
            _ => None,
        })
        .collect();
    let origin = events.first().copied().unwrap_or(0);
    let last = events.last().copied().unwrap_or(0);
    let low = range.from.unwrap_or(i64::MIN);
    let high = range.to.unwrap_or(i64::MAX);
    let (from, to) = (low.max(origin), high.min(last).max(low.max(origin)));
    let inside = |cycles: i64| cycles >= low && cycles < high;
    // A loss hides the time between the events around it.
    let mut losses = usize::from(trace.first != 0 && low <= origin);
    let mut hidden = 0;
    for &loss in &places.losses {
        let before = places.cycles[loss];
        if inside(before) {
            losses += 1;
        }
        let after = trace.elements[loss..].iter().find_map(|element| match element {
            Element::Event { cycles, .. } => Some(*cycles),
            _ => None,
        });
        if let Some(after) = after {
            hidden += (after.min(to) - before.max(from)).max(0);
        }
    }
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
        known: (to - from - hidden).max(0),
        losses,
        suspects,
        contradictions,
        groups,
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
fn at(point: &Point) -> String {
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
        "range {} to {}: {} known, {} losses, {} suspect writers, {} contradictions",
        instant(report.from),
        instant(report.to),
        duration(report.known, frequency),
        report.losses,
        report.suspects,
        report.contradictions
    )];
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
    lines
}

fn string(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            character if (character as u32) < 0x20 => out += &format!("\\u{:04x}", character as u32),
            character => out.push(character),
        }
    }
    out + "\""
}

fn number(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map_or_else(|| "null".to_owned(), |value| format!("{value:?}"))
}

fn point(point: &Point) -> String {
    format!(
        "{{\"value\":{},\"index\":{},\"cycles\":{},\"seconds\":{}}}",
        point.value,
        point.index,
        point.cycles,
        number(point.seconds)
    )
}

/// The report as JSON, the shape host/schema/stats.schema.json gives.
pub fn json(report: &Report) -> String {
    let groups: Vec<String> = report
        .groups
        .iter()
        .map(|group| {
            let distribution = group.distribution.as_ref().map_or_else(
                || "null".to_owned(),
                |distribution| {
                    let counts: Vec<String> = distribution.histogram.counts.iter().map(u64::to_string).collect();
                    let worst: Vec<String> = distribution.worst.iter().map(point).collect();
                    format!(
                        "{{\"min\":{},\"p50\":{},\"p90\":{},\"p99\":{},\"max\":{},\"mean\":{},\"total\":{},\
                         \"histogram\":{{\"width\":{},\"first\":{},\"counts\":[{}]}},\"worst\":[{}]}}",
                        point(&distribution.min),
                        distribution.p50,
                        distribution.p90,
                        distribution.p99,
                        point(&distribution.max),
                        number(Some(distribution.mean)),
                        distribution.total,
                        distribution.histogram.width,
                        distribution.histogram.first,
                        counts.join(","),
                        worst.join(",")
                    )
                },
            );
            format!(
                "{{\"kind\":{},\"subject\":{},\"metric\":{},\"detail\":{},\"count\":{},\"excluded_loss\":{},\
                 \"excluded_suspect\":{},\"cut\":{},\"lower_bound\":{},\"jitter\":{},\"share\":{},\"distribution\":{}}}",
                string(group.kind),
                string(&group.subject),
                string(group.metric.name),
                group.detail.as_deref().map_or_else(|| "null".to_owned(), string),
                group.count,
                group.excluded_loss,
                group.excluded_suspect,
                group.cut,
                group.lower_bound(),
                group.jitter().map_or_else(|| "null".to_owned(), |jitter| jitter.to_string()),
                number(group.share(report.known)),
                distribution
            )
        })
        .collect();
    format!(
        "{{\"schema\":\"kymo-stats\",\"version\":1,\"frequency\":{},\"from\":{},\"to\":{},\"origin\":{},\
         \"known\":{},\"losses\":{},\"suspects\":{},\"contradictions\":{},\"groups\":[{}]}}",
        number(report.frequency),
        report.from,
        report.to,
        report.origin,
        report.known,
        report.losses,
        report.suspects,
        report.contradictions,
        groups.join(",")
    )
}

#[cfg(test)]
pub(crate) mod tests;
