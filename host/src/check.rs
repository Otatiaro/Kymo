//! Rules on the statistics of a range, as `kymo check` judges them.
//!
//! A rule bounds a statistic of a group of `kymo stats`, or of the range
//! (losses, suspect writers, contradictions, faults), or holds it within a margin of
//! the same statistic in a reference `kymo stats --json`.
//!
//! Instances excluded from a group (a loss, an overrun write) have values
//! unknown: each statistic is then an interval that holds its true value,
//! found by putting the excluded values all above the known ones, then all
//! below. A rule holds when the whole interval keeps to it, fails when none
//! of it does, and is unknown otherwise.

use crate::stats::{self, Group, Point, Report};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub outcome: Outcome,
    pub line: String,
}

/// The exit code of verdicts: 3 when one fails, else 4 when one is unknown,
/// else 0.
pub fn code(verdicts: &[Verdict]) -> u8 {
    if verdicts.iter().any(|verdict| verdict.outcome == Outcome::Fail) {
        3
    } else if verdicts.iter().any(|verdict| verdict.outcome == Outcome::Unknown) {
        4
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stat {
    Min,
    P50,
    P90,
    P99,
    Max,
    Mean,
    Jitter,
    Count,
    Share,
}

const STATS: [(&str, Stat); 9] = [
    ("min", Stat::Min),
    ("p50", Stat::P50),
    ("p90", Stat::P90),
    ("p99", Stat::P99),
    ("max", Stat::Max),
    ("mean", Stat::Mean),
    ("jitter", Stat::Jitter),
    ("count", Stat::Count),
    ("share", Stat::Share),
];

impl Stat {
    fn name(self) -> &'static str {
        STATS
            .iter()
            .find(|(_, stat)| *stat == self)
            .map_or("", |(name, _)| name)
    }

    fn is_duration(self) -> bool {
        !matches!(self, Stat::Count | Stat::Share)
    }
}

/// A quantity as a rule writes it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Quantity {
    /// A number of seconds over a divisor: 1, 1e3, 1e6 or 1e9.
    Seconds(f64, f64),
    Cycles(f64),
    /// A percentage, as a fraction.
    Fraction(f64),
    Count(f64),
}

/// `50us`, `1.5 ms`, `300cycles`, `40%`.
fn quantity(text: &str) -> Result<Quantity, String> {
    let text = text.trim();
    let split = text
        .find(|character: char| !(character.is_ascii_digit() || character == '.' || character == '-'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number
        .parse()
        .map_err(|_| format!("{text:?}: no number before the unit"))?;
    Ok(match unit.trim() {
        "s" => Quantity::Seconds(number, 1.0),
        "ms" => Quantity::Seconds(number, 1e3),
        "us" | "µs" => Quantity::Seconds(number, 1e6),
        "ns" => Quantity::Seconds(number, 1e9),
        "cycles" | "cycle" => Quantity::Cycles(number),
        "%" => Quantity::Fraction(number / 100.0),
        "" => return Err(format!("{text:?}: give a unit: s, ms, us, ns, cycles or %")),
        unit => return Err(format!("{text:?}: unknown unit {unit:?}")),
    })
}

/// A bound as JSON gives it: an integer count, or a quantity with its unit.
fn bound(value: &Value) -> Result<Quantity, String> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .map(|count| Quantity::Count(count as f64))
            .ok_or_else(|| format!("{number}: a count is a whole number")),
        Value::String(text) => quantity(text),
        other => Err(format!("{other}: a bound is a count or a quantity with its unit")),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Total {
    Losses,
    Suspects,
    Contradictions,
    Faults,
}

#[derive(Clone, Debug, PartialEq)]
enum What {
    Range(Total),
    Group {
        kind: Option<String>,
        subject: String,
        metric: String,
        detail: Option<String>,
        stat: Stat,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    what: What,
    min: Option<Quantity>,
    max: Option<Quantity>,
    regression: Option<Quantity>,
}

impl Rule {
    fn name(&self) -> String {
        match &self.what {
            What::Range(Total::Losses) => "losses".to_owned(),
            What::Range(Total::Suspects) => "suspects".to_owned(),
            What::Range(Total::Contradictions) => "contradictions".to_owned(),
            What::Range(Total::Faults) => "faults".to_owned(),
            What::Group {
                subject,
                metric,
                detail,
                stat,
                ..
            } => match detail {
                Some(detail) => format!("{subject} {metric} {detail} {}", stat.name()),
                None => format!("{subject} {metric} {}", stat.name()),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(rename = "$schema", default)]
    _schema: Option<String>,
    #[serde(default)]
    baseline: Option<String>,
    rules: Vec<Written>,
}

/// A rule as written, before its fields are checked together.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    capture: Option<String>,
    kind: Option<String>,
    subject: Option<String>,
    metric: Option<String>,
    detail: Option<String>,
    stat: Option<String>,
    min: Option<Value>,
    max: Option<Value>,
    regression: Option<String>,
}

fn rule(written: Written) -> Result<Rule, String> {
    let min = written.min.as_ref().map(bound).transpose()?;
    let max = written.max.as_ref().map(bound).transpose()?;
    let regression = written.regression.as_deref().map(quantity).transpose()?;
    if let Some(capture) = written.capture {
        let total = match capture.as_str() {
            "losses" => Total::Losses,
            "suspects" => Total::Suspects,
            "contradictions" => Total::Contradictions,
            "faults" => Total::Faults,
            other => return Err(format!("capture {other:?}: losses, suspects, contradictions or faults")),
        };
        if written.kind.is_some()
            || written.subject.is_some()
            || written.metric.is_some()
            || written.detail.is_some()
            || written.stat.is_some()
            || regression.is_some()
        {
            return Err(format!("capture {capture:?} takes only min and max"));
        }
        if min.is_none() && max.is_none() {
            return Err(format!("capture {capture:?}: give min or max"));
        }
        for quantity in [min, max].into_iter().flatten() {
            if !matches!(quantity, Quantity::Count(_)) {
                return Err(format!("capture {capture:?}: a bound is a count"));
            }
        }
        return Ok(Rule {
            what: What::Range(total),
            min,
            max,
            regression: None,
        });
    }
    let (Some(subject), Some(metric), Some(stat)) = (written.subject, written.metric, written.stat) else {
        return Err("a rule names a subject, a metric and a stat, or a capture total".to_owned());
    };
    let stat = STATS
        .iter()
        .find(|(name, _)| *name == stat)
        .map(|(_, stat)| *stat)
        .ok_or_else(|| format!("stat {stat:?}: one of min, p50, p90, p99, max, mean, jitter, count, share"))?;
    let name = format!("{subject} {metric} {}", stat.name());
    match (min.is_some() || max.is_some(), regression.is_some()) {
        (false, false) => return Err(format!("{name}: give min, max or regression")),
        (true, true) => return Err(format!("{name}: bounds or a regression, not both")),
        _ => {}
    }
    for quantity in [min, max].into_iter().flatten() {
        let fits = match quantity {
            Quantity::Seconds(..) | Quantity::Cycles(_) => stat.is_duration(),
            Quantity::Fraction(_) => stat == Stat::Share,
            Quantity::Count(_) => stat == Stat::Count,
        };
        if !fits {
            let what = match stat {
                Stat::Count => "a count",
                Stat::Share => "a percentage",
                _ => "a duration with its unit",
            };
            return Err(format!("{name}: a bound is {what}"));
        }
    }
    if let Some(Quantity::Seconds(..) | Quantity::Cycles(_)) = regression
        && !stat.is_duration()
    {
        return Err(format!("{name}: a regression of a {} is a percentage", stat.name()));
    }
    Ok(Rule {
        what: What::Group {
            kind: written.kind,
            subject,
            metric,
            detail: written.detail,
            stat,
        },
        min,
        max,
        regression,
    })
}

/// The rules of a rules file, and the reference it names, as written.
pub fn rules(text: &str) -> Result<(Vec<Rule>, Option<String>), String> {
    let file: File = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let rules = file
        .rules
        .into_iter()
        .enumerate()
        .map(|(n, written)| rule(written).map_err(|error| format!("rule {}: {error}", n + 1)))
        .collect::<Result<_, _>>()?;
    Ok((rules, file.baseline))
}

/// What `kymo check` reads of a reference `kymo stats --json`.
#[derive(Debug, Deserialize)]
pub struct Baseline {
    schema: String,
    version: u32,
    frequency: Option<f64>,
    groups: Vec<BaselineGroup>,
}

#[derive(Debug, Deserialize)]
struct BaselineGroup {
    kind: String,
    subject: String,
    metric: String,
    detail: Option<String>,
    count: u64,
    lower_bound: bool,
    jitter: Option<i64>,
    share: Option<f64>,
    distribution: Option<BaselineDistribution>,
}

#[derive(Debug, Deserialize)]
struct BaselineDistribution {
    min: BaselinePoint,
    p50: i64,
    p90: i64,
    p99: i64,
    max: BaselinePoint,
    mean: f64,
}

#[derive(Debug, Deserialize)]
struct BaselinePoint {
    value: i64,
}

pub fn baseline(text: &str) -> Result<Baseline, String> {
    let baseline: Baseline = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if baseline.schema != "kymo-stats" || baseline.version != 1 {
        return Err(format!(
            "a {} version {}, not a kymo stats --json version 1",
            baseline.schema, baseline.version
        ));
    }
    Ok(baseline)
}

impl BaselineGroup {
    /// The statistic, none without an instance counted.
    fn value(&self, stat: Stat) -> Option<f64> {
        if stat == Stat::Count {
            return Some(self.count as f64);
        }
        let distribution = self.distribution.as_ref()?;
        Some(match stat {
            Stat::Min => distribution.min.value as f64,
            Stat::P50 => distribution.p50 as f64,
            Stat::P90 => distribution.p90 as f64,
            Stat::P99 => distribution.p99 as f64,
            Stat::Max => distribution.max.value as f64,
            Stat::Mean => distribution.mean,
            Stat::Jitter => self.jitter? as f64,
            Stat::Share => self.share?,
            Stat::Count => unreachable!(),
        })
    }
}

/// The values between which the true value of rank `rank` lies, among the
/// group's known values and `excluded` more of any value.
fn ranked(values: &[i64], excluded: usize, rank: usize) -> (f64, f64) {
    let high = values.get(rank - 1).map_or(f64::INFINITY, |&value| value as f64);
    let low = if rank > excluded {
        values[rank - excluded - 1] as f64
    } else {
        f64::NEG_INFINITY
    };
    (low, high)
}

/// The interval that holds the statistic, none without an instance counted;
/// an error for a statistic the metric does not have.
fn interval(group: &Group, stat: Stat, known: i64) -> Result<Option<(f64, f64)>, String> {
    let excluded = group.excluded_loss + group.excluded_suspect;
    let count = group.count;
    if stat == Stat::Count {
        return Ok(Some((count as f64, (count + excluded) as f64)));
    }
    if stat == Stat::Jitter && !group.metric.jitter {
        return Err(format!("{} has no jitter: only a period has", group.metric.name));
    }
    if stat == Stat::Share && !group.metric.share {
        return Err(format!("{} has no share: only a net execution has", group.metric.name));
    }
    let Some(distribution) = &group.distribution else {
        return Ok(None);
    };
    let values = &distribution.values;
    let all = count + excluded;
    let unbounded = |low: f64| (low, if excluded > 0 { f64::INFINITY } else { low });
    Ok(Some(match stat {
        Stat::Min => ranked(values, excluded, 1),
        Stat::P50 => ranked(values, excluded, (50 * all).div_ceil(100).max(1)),
        Stat::P90 => ranked(values, excluded, (90 * all).div_ceil(100).max(1)),
        Stat::P99 => ranked(values, excluded, (99 * all).div_ceil(100).max(1)),
        Stat::Max => ranked(values, excluded, all),
        Stat::Mean => unbounded(distribution.total as f64 / all as f64),
        Stat::Jitter => unbounded((distribution.max.value - distribution.min.value) as f64),
        Stat::Share => unbounded(group.share(known).unwrap_or(0.0)),
        Stat::Count => unreachable!(),
    }))
}

fn decide(low: f64, high: f64, min: Option<f64>, max: Option<f64>) -> Outcome {
    let holds = min.is_none_or(|min| low >= min) && max.is_none_or(|max| high <= max);
    let fails = min.is_some_and(|min| high < min) || max.is_some_and(|max| low > max);
    if holds {
        Outcome::Pass
    } else if fails {
        Outcome::Fail
    } else {
        Outcome::Unknown
    }
}

/// A value of `stat`, `None` for a count of the range.
fn show(stat: Option<Stat>, value: f64, frequency: Option<f64>) -> String {
    match stat {
        Some(Stat::Share) => format!("{:.2} %", value * 100.0),
        Some(Stat::Count) | None => format!("{value}"),
        Some(_) => stats::duration(value.round() as i64, frequency),
    }
}

fn show_interval(stat: Option<Stat>, (low, high): (f64, f64), frequency: Option<f64>) -> String {
    let show = |value| show(stat, value, frequency);
    if low == high {
        show(low)
    } else if high == f64::INFINITY && low == f64::NEG_INFINITY {
        "anything".to_owned()
    } else if high == f64::INFINITY {
        format!("≥ {}", show(low))
    } else if low == f64::NEG_INFINITY {
        format!("≤ {}", show(high))
    } else {
        format!("{} to {}", show(low), show(high))
    }
}

/// How the value stands to its bounds.
fn relation(
    outcome: Outcome,
    stat: Option<Stat>,
    (low, high): (f64, f64),
    min: Option<f64>,
    max: Option<f64>,
    frequency: Option<f64>,
) -> String {
    let show = |value| show(stat, value, frequency);
    let mut parts = Vec::new();
    if let Some(min) = min {
        let sign = match outcome {
            Outcome::Fail if high < min => "<",
            Outcome::Pass => "≥",
            _ if low >= min => "≥",
            _ => "vs ≥",
        };
        parts.push(format!("{sign} {}", show(min)));
    }
    if let Some(max) = max {
        let sign = match outcome {
            Outcome::Fail if low > max => ">",
            Outcome::Pass => "≤",
            _ if high <= max => "≤",
            _ => "vs ≤",
        };
        parts.push(format!("{sign} {}", show(max)));
    }
    parts.join(" and ")
}

fn place(point: &Point) -> String {
    match point.seconds {
        Some(seconds) => format!(", at {seconds:.6} s (event {})", point.index),
        None => format!(", at event {}", point.index),
    }
}

fn label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Pass => "PASS",
        Outcome::Fail => "FAIL",
        Outcome::Unknown => "UNKNOWN",
    }
}

fn verdict(outcome: Outcome, name: &str, text: String) -> Verdict {
    Verdict {
        outcome,
        line: format!("{:<8} {name}: {text}", label(outcome)),
    }
}

/// The groups whose name is close to the rule's, for a rule that names none.
fn suggestions(report: &Report, subject: &str, metric: &str) -> String {
    let mut close: Vec<String> = report
        .groups
        .iter()
        .filter(|group| group.subject == subject || group.metric.name == metric)
        .map(|group| match &group.detail {
            Some(detail) => format!("{} {} {} {detail}", group.kind, group.subject, group.metric.name),
            None => format!("{} {} {}", group.kind, group.subject, group.metric.name),
        })
        .collect();
    close.dedup();
    if close.is_empty() {
        return String::new();
    }
    let more = close.len().saturating_sub(6);
    close.truncate(6);
    let mut text = format!("; groups close to it: {}", close.join(", "));
    if more > 0 {
        text += &format!(" and {more} more");
    }
    text
}

/// A quantity in the units of the statistic: cycles at the report's
/// frequency for a duration.
fn amount(quantity: Quantity, frequency: Option<f64>, name: &str) -> Result<f64, String> {
    match quantity {
        Quantity::Seconds(number, divisor) => frequency
            .map(|hz| number * hz / divisor)
            .ok_or_else(|| format!("{name}: a bound in seconds needs the capture's frequency: give it with --clock")),
        Quantity::Cycles(value) | Quantity::Fraction(value) | Quantity::Count(value) => Ok(value),
    }
}

fn judge_rule(report: &Report, rule: &Rule, baseline: Option<&Baseline>) -> Result<Verdict, String> {
    let name = rule.name();
    let frequency = report.frequency;
    let min = rule.min.map(|min| amount(min, frequency, &name)).transpose()?;
    let max = rule.max.map(|max| amount(max, frequency, &name)).transpose()?;
    let (kind, subject, metric, detail, stat) = match &rule.what {
        What::Range(total) => {
            let value = match total {
                Total::Losses => report.losses as f64,
                Total::Suspects => report.suspects as f64,
                Total::Contradictions => report.contradictions as f64,
                Total::Faults => report.faults as f64,
            };
            let outcome = decide(value, value, min, max);
            let text = format!(
                "{} {}",
                show(None, value, frequency),
                relation(outcome, None, (value, value), min, max, frequency)
            );
            return Ok(verdict(outcome, &name, text));
        }
        What::Group {
            kind,
            subject,
            metric,
            detail,
            stat,
        } => (kind, subject, metric, detail, *stat),
    };
    let found: Vec<&Group> = report
        .groups
        .iter()
        .filter(|group| {
            group.subject == *subject
                && group.metric.name == metric
                && group.detail == *detail
                && kind.as_ref().is_none_or(|kind| group.kind == kind)
        })
        .collect();
    let group = match found.as_slice() {
        [] => {
            let text = format!("no such group in the range{}", suggestions(report, subject, metric));
            return Ok(verdict(Outcome::Fail, &name, text));
        }
        [group] => *group,
        groups => {
            let kinds: Vec<&str> = groups.iter().map(|group| group.kind).collect();
            return Err(format!("{name}: give its kind, one of {}", kinds.join(", ")));
        }
    };
    let Some(interval) = interval(group, stat, report.known).map_err(|error| format!("{name}: {error}"))? else {
        return Ok(verdict(
            Outcome::Unknown,
            &name,
            "no complete instance in the range".to_owned(),
        ));
    };
    let excluded = group.excluded_loss + group.excluded_suspect;
    let mut note = String::new();
    if excluded > 0 {
        note = format!(
            " ({excluded} excluded: {} loss, {} suspect)",
            group.excluded_loss, group.excluded_suspect
        );
    }
    let (min, max, reference) = match rule.regression {
        None => (min, max, String::new()),
        Some(margin) => {
            let baseline = baseline.ok_or_else(|| format!("{name}: a regression needs a baseline"))?;
            let reference = baseline.groups.iter().find(|reference| {
                reference.kind == group.kind
                    && reference.subject == group.subject
                    && reference.metric == group.metric.name
                    && reference.detail == group.detail
            });
            let Some(reference) = reference else {
                return Ok(verdict(Outcome::Unknown, &name, "absent from the baseline".to_owned()));
            };
            if reference.lower_bound {
                return Ok(verdict(
                    Outcome::Unknown,
                    &name,
                    "the baseline excluded instances of it: make the baseline again".to_owned(),
                ));
            }
            let Some(mut value) = reference.value(stat) else {
                return Ok(verdict(
                    Outcome::Unknown,
                    &name,
                    "no instance of it in the baseline".to_owned(),
                ));
            };
            if stat.is_duration()
                && let (Some(now), Some(then)) = (frequency, baseline.frequency)
            {
                value *= now / then;
            }
            let (limit, margin) = match margin {
                Quantity::Fraction(fraction) => (value * (1.0 + fraction), format!("{:+} %", fraction * 100.0)),
                margin => {
                    let added = amount(margin, frequency, &name)?;
                    (value + added, format!("+ {}", show(Some(stat), added, frequency)))
                }
            };
            let text = format!(" (baseline {} {margin})", show(Some(stat), value, frequency));
            (None, Some(limit), text)
        }
    };
    let outcome = decide(interval.0, interval.1, min, max);
    let mut at = String::new();
    if outcome == Outcome::Fail && stat.is_duration() && stat != Stat::Mean && stat != Stat::Jitter {
        let distribution = group.distribution.as_ref().expect("an interval has a distribution");
        if max.is_some_and(|max| interval.0 > max) {
            at = place(&distribution.max);
        } else if min.is_some_and(|min| interval.1 < min) {
            at = place(&distribution.min);
        }
    }
    let text = format!(
        "{} {}{reference}{at}{note}",
        show_interval(Some(stat), interval, frequency),
        relation(outcome, Some(stat), interval, min, max, frequency)
    );
    Ok(verdict(outcome, &name, text))
}

/// Each rule's verdict on the report; an error for a rule that cannot be
/// judged as written.
pub fn judge(report: &Report, rules: &[Rule], baseline: Option<&Baseline>) -> Result<Vec<Verdict>, String> {
    rules.iter().map(|rule| judge_rule(report, rule, baseline)).collect()
}

/// The line that ends the verdicts.
pub fn summary(verdicts: &[Verdict]) -> String {
    let count = |outcome| verdicts.iter().filter(|verdict| verdict.outcome == outcome).count();
    format!(
        "{} rules: {} pass, {} fail, {} unknown",
        verdicts.len(),
        count(Outcome::Pass),
        count(Outcome::Fail),
        count(Outcome::Unknown)
    )
}

#[cfg(test)]
mod tests;
