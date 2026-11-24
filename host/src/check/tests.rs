use super::*;
use crate::opsy::{GROSS, HOLD, NET, PERIOD, PREEMPTED_BY_HANDLERS, PREEMPTED_BY_TASKS, RESPONSE, WAIT, WAKE_LATENCY};
use crate::stats::tests::{Json, fits_schema, violations};
use crate::stats::{Metric, distribution};

const SCHEMA: &str = include_str!("../../schema/check.schema.json");

/// A group of `values`, their instances at events 10, 20, … one second
/// apart, `excluded` more excluded by a loss.
fn group(kind: &'static str, subject: &str, metric: Metric, values: &[i64], excluded: usize) -> Group {
    let points: Vec<Point> = values
        .iter()
        .enumerate()
        .map(|(n, &value)| Point {
            value,
            element: n,
            index: 10 * (n as u32 + 1),
            cycles: 1_000 * (n as i64 + 1),
            seconds: Some(n as f64 + 1.0),
        })
        .collect();
    Group {
        kind,
        subject: subject.to_owned(),
        metric,
        detail: None,
        count: values.len(),
        excluded_loss: excluded,
        excluded_suspect: 0,
        cut: 0,
        distribution: distribution(&points, 10),
    }
}

fn report(groups: Vec<Group>, frequency: Option<f64>) -> Report {
    Report {
        frequency,
        from: 0,
        to: 1_000_000,
        origin: 0,
        known: 100,
        joined: None,
        losses: 2,
        suspects: 0,
        contradictions: 0,
        groups,
    }
}

fn one_to_ten() -> Vec<i64> {
    (1..=10).collect()
}

/// The verdicts of `rules`, a JSON array, on `report`.
fn judged(report: &Report, rules: &str, baseline: Option<&Baseline>) -> Result<Vec<Verdict>, String> {
    let (rules, _) = super::rules(&format!(r#"{{"rules":{rules}}}"#))?;
    judge(report, &rules, baseline)
}

fn outcomes(report: &Report, rules: &str) -> Vec<Outcome> {
    judged(report, rules, None)
        .unwrap()
        .into_iter()
        .map(|verdict| verdict.outcome)
        .collect()
}

#[test]
fn excluded_instances_widen_each_statistic_into_an_interval() {
    const INF: f64 = f64::INFINITY;
    let at = |excluded, stat| {
        interval(&group("task", "a", NET, &one_to_ten(), excluded), stat, 100)
            .unwrap()
            .unwrap()
    };
    // Nothing excluded: the value itself, as stats gives it.
    for (stat, value) in [
        (Stat::Min, 1.0),
        (Stat::P50, 5.0),
        (Stat::P90, 9.0),
        (Stat::P99, 10.0),
        (Stat::Max, 10.0),
        (Stat::Mean, 5.5),
        (Stat::Count, 10.0),
        (Stat::Share, 0.55),
    ] {
        assert_eq!(at(0, stat), (value, value), "{stat:?}");
    }
    // One excluded among 11: ranks 6, 10, 11.
    assert_eq!(at(1, Stat::Min), (-INF, 1.0));
    assert_eq!(at(1, Stat::P50), (5.0, 6.0));
    assert_eq!(at(1, Stat::P90), (9.0, 10.0));
    assert_eq!(at(1, Stat::P99), (10.0, INF));
    assert_eq!(at(1, Stat::Max), (10.0, INF));
    assert_eq!(at(1, Stat::Count), (10.0, 11.0));
    assert_eq!(at(1, Stat::Mean), (5.0, INF));
    assert_eq!(at(1, Stat::Share), (0.55, INF));
    // Three excluded among 13: ranks 7, 12, 13.
    assert_eq!(at(3, Stat::P50), (4.0, 7.0));
    assert_eq!(at(3, Stat::P90), (9.0, INF));
    assert_eq!(at(3, Stat::Max), (10.0, INF));
    assert_eq!(at(3, Stat::Count), (10.0, 13.0));
    assert_eq!(at(3, Stat::Mean), (55.0 / 13.0, INF));
    // More excluded than the rank: no lower bound.
    assert_eq!(at(10, Stat::P50), (-INF, 10.0));
    let period = group("task", "a", PERIOD, &one_to_ten(), 0);
    assert_eq!(interval(&period, Stat::Jitter, 100).unwrap(), Some((9.0, 9.0)));
    let period = group("task", "a", PERIOD, &one_to_ten(), 2);
    assert_eq!(interval(&period, Stat::Jitter, 100).unwrap(), Some((9.0, INF)));
    assert!(interval(&period, Stat::Share, 100).is_err());
    assert!(interval(&group("task", "a", NET, &[1], 0), Stat::Jitter, 100).is_err());
    assert_eq!(
        interval(&group("task", "a", NET, &[], 2), Stat::Max, 100).unwrap(),
        None
    );
}

#[test]
fn a_rule_passes_at_its_bound_and_fails_past_it() {
    let report = report(vec![group("task", "a", NET, &one_to_ten(), 0)], None);
    assert_eq!(
        outcomes(
            &report,
            r#"[{"subject":"a","metric":"net","stat":"max","max":"10cycles"},
                {"subject":"a","metric":"net","stat":"max","max":"9cycles"},
                {"subject":"a","metric":"net","stat":"min","min":"1cycles"},
                {"subject":"a","metric":"net","stat":"min","min":"2cycles"},
                {"subject":"a","metric":"net","stat":"p50","min":"5cycles","max":"5cycles"},
                {"subject":"a","metric":"net","stat":"count","max":10},
                {"subject":"a","metric":"net","stat":"count","max":9},
                {"subject":"a","metric":"net","stat":"share","max":"55%"},
                {"subject":"a","metric":"net","stat":"share","max":"54%"}]"#
        ),
        [
            Outcome::Pass,
            Outcome::Fail,
            Outcome::Pass,
            Outcome::Fail,
            Outcome::Pass,
            Outcome::Pass,
            Outcome::Fail,
            Outcome::Pass,
            Outcome::Fail
        ]
    );
}

#[test]
fn excluded_instances_leave_a_rule_unknown_unless_the_known_values_decide_it() {
    let report = report(vec![group("task", "a", NET, &one_to_ten(), 1)], None);
    assert_eq!(
        outcomes(
            &report,
            r#"[{"subject":"a","metric":"net","stat":"max","max":"100cycles"},
                {"subject":"a","metric":"net","stat":"max","max":"9cycles"},
                {"subject":"a","metric":"net","stat":"max","min":"10cycles"},
                {"subject":"a","metric":"net","stat":"p50","max":"6cycles"},
                {"subject":"a","metric":"net","stat":"p50","max":"5cycles"},
                {"subject":"a","metric":"net","stat":"p50","max":"4cycles"}]"#
        ),
        [
            Outcome::Unknown,
            Outcome::Fail,
            Outcome::Pass,
            Outcome::Pass,
            Outcome::Unknown,
            Outcome::Fail
        ]
    );
}

#[test]
fn the_exit_code_puts_a_failure_before_an_unknown() {
    let verdict = |outcome| Verdict {
        outcome,
        line: String::new(),
    };
    assert_eq!(code(&[]), 0);
    assert_eq!(code(&[verdict(Outcome::Pass)]), 0);
    assert_eq!(code(&[verdict(Outcome::Pass), verdict(Outcome::Unknown)]), 4);
    assert_eq!(code(&[verdict(Outcome::Unknown), verdict(Outcome::Fail)]), 3);
    assert_eq!(code(&[verdict(Outcome::Fail), verdict(Outcome::Unknown)]), 3);
}

#[test]
fn durations_take_their_unit() {
    // 1 MHz: a cycle is a microsecond.
    let report = report(vec![group("task", "a", NET, &[50_000], 0)], Some(1e6));
    let holds = |max: &str| {
        outcomes(
            &report,
            &format!(r#"[{{"subject":"a","metric":"net","stat":"max","max":"{max}"}}]"#),
        )[0]
    };
    assert_eq!(holds("50ms"), Outcome::Pass);
    assert_eq!(holds("49.999ms"), Outcome::Fail);
    assert_eq!(holds("50000us"), Outcome::Pass);
    assert_eq!(holds("49999 µs"), Outcome::Fail);
    assert_eq!(holds("0.05s"), Outcome::Pass);
    assert_eq!(holds("50000000ns"), Outcome::Pass);
    assert_eq!(holds("49999999ns"), Outcome::Fail);
    assert_eq!(holds("50000cycles"), Outcome::Pass);
    assert_eq!(holds("49999cycles"), Outcome::Fail);
    for wrong in ["50", "50 min", "us", "40%"] {
        assert!(
            judged(
                &report,
                &format!(r#"[{{"subject":"a","metric":"net","stat":"max","max":"{wrong}"}}]"#),
                None
            )
            .is_err(),
            "{wrong}"
        );
    }
    // Without a frequency, seconds cannot be judged.
    let report = super::tests::report(vec![group("task", "a", NET, &[50_000], 0)], None);
    assert!(
        judged(
            &report,
            r#"[{"subject":"a","metric":"net","stat":"max","max":"50ms"}]"#,
            None
        )
        .is_err()
    );
}

#[test]
fn a_failure_points_at_the_instance_past_the_bound() {
    let report = report(vec![group("task", "a", NET, &[3, 9, 4], 0)], Some(1.0));
    let lines: Vec<String> = judged(
        &report,
        r#"[{"subject":"a","metric":"net","stat":"max","max":"5cycles"},
            {"subject":"a","metric":"net","stat":"min","min":"4cycles"}]"#,
        None,
    )
    .unwrap()
    .into_iter()
    .map(|verdict| verdict.line)
    .collect();
    assert!(lines[0].starts_with("FAIL     a net max: "), "{}", lines[0]);
    assert!(lines[0].ends_with("(event 20)"), "{}", lines[0]);
    assert!(lines[1].ends_with("(event 10)"), "{}", lines[1]);
}

#[test]
fn a_group_absent_fails_and_names_the_groups_close_to_it() {
    let report = report(
        vec![group("task", "a", NET, &[1], 0), group("task", "b", GROSS, &[1], 0)],
        None,
    );
    let verdicts = judged(
        &report,
        r#"[{"subject":"a","metric":"gross","stat":"max","max":"1cycles"}]"#,
        None,
    )
    .unwrap();
    assert_eq!(verdicts[0].outcome, Outcome::Fail);
    assert!(verdicts[0].line.contains("task a net"), "{}", verdicts[0].line);
    assert!(verdicts[0].line.contains("task b gross"), "{}", verdicts[0].line);
    // A group without a complete instance is unknown, but for its count.
    let report = super::tests::report(vec![group("task", "c", NET, &[], 0)], None);
    assert_eq!(
        outcomes(
            &report,
            r#"[{"subject":"c","metric":"net","stat":"max","max":"1cycles"},
                {"subject":"c","metric":"net","stat":"count","min":1}]"#
        ),
        [Outcome::Unknown, Outcome::Fail]
    );
}

#[test]
fn a_kind_is_needed_only_when_the_name_is_ambiguous() {
    let report = report(
        vec![
            group("task", "x", GROSS, &[5], 0),
            group("handler", "x", GROSS, &[50], 0),
        ],
        None,
    );
    assert!(
        judged(
            &report,
            r#"[{"subject":"x","metric":"gross","stat":"max","max":"10cycles"}]"#,
            None
        )
        .is_err()
    );
    assert_eq!(
        outcomes(
            &report,
            r#"[{"kind":"task","subject":"x","metric":"gross","stat":"max","max":"10cycles"},
                {"kind":"handler","subject":"x","metric":"gross","stat":"max","max":"10cycles"}]"#
        ),
        [Outcome::Pass, Outcome::Fail]
    );
}

#[test]
fn a_rule_written_wrong_is_refused() {
    for wrong in [
        r#"{"rules":[{"subject":"a","metric":"net","stat":"max","max":"1us","extra":1}]}"#,
        r#"{"rules":[],"unknown":1}"#,
        r#"{"rules":[{"subject":"a","metric":"net","stat":"p95","max":"1us"}]}"#,
        r#"{"rules":[{"subject":"a","metric":"net","stat":"max"}]}"#,
        r#"{"rules":[{"subject":"a","metric":"net","stat":"max","max":"1us","regression":"5%"}]}"#,
        r#"{"rules":[{"subject":"a","metric":"net","max":"1us"}]}"#,
        r#"{"rules":[{"subject":"a","metric":"net","stat":"count","max":"1us"}]}"#,
        r#"{"rules":[{"subject":"a","metric":"net","stat":"count","regression":"2us"}]}"#,
        r#"{"rules":[{"capture":"losses"}]}"#,
        r#"{"rules":[{"capture":"losses","max":"1us"}]}"#,
        r#"{"rules":[{"capture":"drops","max":0}]}"#,
        r#"{"rules":[{"capture":"losses","subject":"a","max":0}]}"#,
        r#"{"rules":[{"subject":"a","metric":"net","stat":"max","max":-1}]}"#,
    ] {
        assert!(rules(wrong).is_err(), "{wrong} is read");
    }
}

#[test]
fn the_range_totals_are_bounded_too() {
    let report = report(Vec::new(), None);
    assert_eq!(
        outcomes(
            &report,
            r#"[{"capture":"losses","max":0},{"capture":"losses","max":2},{"capture":"suspects","max":0}]"#
        ),
        [Outcome::Fail, Outcome::Pass, Outcome::Pass]
    );
}

#[test]
fn a_regression_compares_with_the_baseline_that_stats_wrote() {
    let then = report(
        vec![
            group("task", "a", NET, &[100, 200], 0),
            group("task", "b", NET, &[100], 1),
        ],
        Some(1e6),
    );
    let baseline = baseline(&stats::json(&then)).unwrap();
    let judge = |values: &[i64], frequency, rules: &str| -> Vec<Outcome> {
        let now = report(
            vec![
                group("task", "a", NET, values, 0),
                group("task", "b", NET, &[100], 0),
                group("task", "c", NET, &[100], 0),
            ],
            frequency,
        );
        judged(&now, rules, Some(&baseline))
            .unwrap()
            .into_iter()
            .map(|verdict| verdict.outcome)
            .collect()
    };
    let relative = r#"[{"subject":"a","metric":"net","stat":"max","regression":"10%"}]"#;
    assert_eq!(judge(&[100, 220], Some(1e6), relative), [Outcome::Pass]);
    assert_eq!(judge(&[100, 221], Some(1e6), relative), [Outcome::Fail]);
    let absolute = r#"[{"subject":"a","metric":"net","stat":"p50","regression":"5us"}]"#;
    assert_eq!(judge(&[105, 105], Some(1e6), absolute), [Outcome::Pass]);
    assert_eq!(judge(&[106, 106], Some(1e6), absolute), [Outcome::Fail]);
    // At twice the frequency, the same seconds are twice the cycles.
    assert_eq!(judge(&[200, 440], Some(2e6), relative), [Outcome::Pass]);
    assert_eq!(judge(&[200, 441], Some(2e6), relative), [Outcome::Fail]);
    // A count holds within a percentage too.
    let count = r#"[{"subject":"a","metric":"net","stat":"count","regression":"50%"}]"#;
    assert_eq!(judge(&[1, 1, 1], Some(1e6), count), [Outcome::Pass]);
    assert_eq!(judge(&[1, 1, 1, 1], Some(1e6), count), [Outcome::Fail]);
    // Excluded in the baseline, or absent from it: unknown.
    let unknown = r#"[{"subject":"b","metric":"net","stat":"max","regression":"10%"},
                      {"subject":"c","metric":"net","stat":"max","regression":"10%"}]"#;
    assert_eq!(judge(&[1], Some(1e6), unknown), [Outcome::Unknown, Outcome::Unknown]);
    // Without a baseline, a regression cannot be judged.
    let now = report(vec![group("task", "a", NET, &[1], 0)], None);
    assert!(judged(&now, relative, None).is_err());
    // A file that is no kymo stats --json is refused.
    assert!(super::baseline(r#"{"schema":"kymo-check","version":1,"frequency":null,"groups":[]}"#).is_err());
}

#[test]
fn the_rules_fit_the_schema_and_its_checks_catch_what_they_should() {
    let example = r#"{
      "$schema": "https://raw.githubusercontent.com/Otatiaro/Kymo/master/host/schema/check.schema.json",
      "baseline": "reference/stats.json",
      "rules": [
        { "subject": "g_sampler", "metric": "wake_latency", "stat": "p99", "max": "50us" },
        { "subject": "g_sampler", "metric": "wake_latency", "detail": "notified", "stat": "max", "max": "200us" },
        { "kind": "handler", "subject": "TIM2_IRQHandler", "metric": "gross", "stat": "max", "max": "10us" },
        { "subject": "g_sampler", "metric": "net", "stat": "p99", "regression": "10%" },
        { "subject": "g_sampler", "metric": "period", "stat": "jitter", "regression": "2us" },
        { "subject": "g_sampler", "metric": "net", "stat": "count", "min": 1 },
        { "capture": "losses", "max": 0 }
      ]
    }"#;
    fits_schema(SCHEMA, example);
    rules(example).unwrap();
    let schema = Json::parse(SCHEMA);
    for (from, to) in [
        (r#""stat": "p99", "max""#, r#""stat": "p95", "max""#),
        (
            r#""metric": "wake_latency", "stat": "p99""#,
            r#""metric": "latency", "stat": "p99""#,
        ),
        (r#""kind": "handler""#, r#""kind": "isr""#),
        (r#""capture": "losses""#, r#""capture": "drops""#),
        (r#", "max": "50us""#, ""),
        (r#""subject": "g_sampler", "metric": "net", "stat": "p99", "#, ""),
        (r#""max": 0 }"#, r#""max": 0, "extra": 1 }"#),
        (r#""baseline""#, r#""reference""#),
    ] {
        assert!(example.contains(from), "{from}");
        let mut out = Vec::new();
        violations(
            &schema,
            &schema,
            &Json::parse(&example.replacen(from, to, 1)),
            "",
            &mut out,
        );
        assert!(!out.is_empty(), "{from} -> {to} passes");
    }
    // The schema's metrics are the metrics stats gives.
    let Json::Array(metrics) = schema.get("$defs").get("metric").get("enum") else {
        panic!("no metric enum");
    };
    let names: Vec<Json> = [
        WAKE_LATENCY,
        PREEMPTED_BY_TASKS,
        PREEMPTED_BY_HANDLERS,
        NET,
        GROSS,
        RESPONSE,
        PERIOD,
        HOLD,
        WAIT,
    ]
    .iter()
    .map(|metric| Json::String(metric.name.to_owned()))
    .collect();
    assert_eq!(metrics, &names);
}
