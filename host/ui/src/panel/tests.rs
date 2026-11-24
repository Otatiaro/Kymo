use super::*;
use kymo::decode::Decoded;
use kymo::events::Event;
use kymo::model::{Edge, Element};
use kymo::stats::Metric;

const NET: Metric = Metric {
    name: "net",
    share: true,
    jitter: false,
};

fn event() -> Event {
    Event {
        id: 1,
        name: "app::tick".to_owned(),
        size: 0,
        fields: Vec::new(),
    }
}

/// Events 10 cycles apart, their indexes from 0, and a loss of three
/// indexes after the fourth.
fn trace(event: &Event) -> Trace<'_> {
    let mut elements = Vec::new();
    let mut index = 0;
    for n in 0..8 {
        if n == 4 {
            elements.push(Element::Lost {
                from: index,
                to: index + 3,
            });
            index += 3;
        }
        elements.push(Element::Event {
            event: Decoded {
                index,
                timestamp: 0,
                event,
                payload: Vec::new(),
            },
            cycles: 10 * i64::from(index),
            seconds: None,
        });
        index += 1;
    }
    Trace {
        first: 0,
        elements,
        end: Some(index),
        cut: None,
        frequency: None,
        measured: None,
    }
}

fn instance(trace: &Trace, subject: &str, value: i64, from: usize, to: usize) -> Instance {
    let edge = |element: usize| Edge {
        element,
        cycles: match &trace.elements[element] {
            Element::Event { cycles, .. } => *cycles,
            _ => unreachable!("instances start and end at events"),
        },
        lost: false,
    };
    Instance {
        kind: "task",
        subject: subject.to_owned(),
        metric: NET,
        detail: None,
        value,
        from: edge(from),
        to: edge(to),
        open: false,
    }
}

/// `a`: 10, 30 and 12, and 99 across the loss, excluded; `b`: 50. Element 4
/// is the loss.
fn panel(trace: &Trace) -> Panel {
    Panel::new(vec![
        instance(trace, "a", 10, 0, 1),
        instance(trace, "b", 50, 1, 2),
        instance(trace, "a", 30, 2, 3),
        instance(trace, "a", 99, 3, 5),
        instance(trace, "a", 12, 6, 7),
    ])
}

fn index(trace: &Trace, element: usize) -> u32 {
    match &trace.elements[element] {
        Element::Event { event, .. } => event.index,
        _ => unreachable!("instances start at events"),
    }
}

#[test]
fn a_min_or_a_max_leads_to_the_instance_kymo_stats_gives() {
    let event = event();
    let trace = trace(&event);
    let mut panel = panel(&trace);
    let report = panel.report(&trace, &[], Range::default());
    let json = stats::json(report);
    let groups = report.groups.clone();
    assert_eq!(groups.len(), 2);
    for group in &groups {
        let from = json
            .find(&format!(r#""subject":"{}""#, group.subject))
            .expect("the group in the JSON");
        for (column, member) in [(Column::Min, "min"), (Column::Max, "max")] {
            let point = point(group, column).expect("a group with instances");
            let instance = panel.instance(group, point).expect("the point's instance");
            assert_eq!(instance.subject, group.subject);
            let named = format!(
                r#""{member}":{{"value":{},"index":{}"#,
                instance.value,
                index(&trace, instance.from.element)
            );
            assert!(
                json[from..].starts_with(&format!(r#""subject":"{}""#, group.subject))
                    && json[from..]
                        .split(r#""subject":"#)
                        .nth(1)
                        .is_some_and(|own| own.contains(&named)),
                "{} {member}: kymo stats does not give {named}\n{json}",
                group.subject
            );
        }
    }
    let a = groups.iter().find(|group| group.subject == "a").unwrap();
    let max = panel.instance(a, point(a, Column::Max).unwrap()).unwrap();
    assert_eq!((max.value, max.from.element, max.to.element), (30, 2, 3));
}

#[test]
fn the_high_values_are_a_lower_bound_after_an_exclusion() {
    let event = event();
    let trace = trace(&event);
    let mut panel = panel(&trace);
    let report = panel.report(&trace, &[], Range::default());
    let cells = |subject: &str| {
        let group = report.groups.iter().find(|group| group.subject == subject).unwrap();
        Column::ALL[2..]
            .iter()
            .map(|&column| cell(group, column, None))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        cells("a"),
        ["3", "10 cycles", "12 cycles", "≥ 30 cycles", "≥ 30 cycles", "1"]
    );
    assert_eq!(
        cells("b"),
        ["1", "50 cycles", "50 cycles", "50 cycles", "50 cycles", "0"]
    );
}

#[test]
fn a_column_sorts_numbers_highest_first_then_the_other_way() {
    let event = event();
    let trace = trace(&event);
    let mut panel = panel(&trace);
    panel.report(&trace, &[], Range::default());
    let subjects = |panel: &Panel| {
        let report = &panel.report.as_ref().unwrap().1;
        panel
            .order(report)
            .into_iter()
            .map(|n| report.groups[n].subject.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(subjects(&panel), ["a", "b"]);
    panel.sort_by(Column::Max);
    assert_eq!(subjects(&panel), ["b", "a"]);
    panel.sort_by(Column::Max);
    assert_eq!(subjects(&panel), ["a", "b"]);
    panel.sort_by(Column::Count);
    assert_eq!(subjects(&panel), ["a", "b"]);
    panel.sort_by(Column::Subject);
    assert_eq!(subjects(&panel), ["a", "b"]);
    panel.sort_by(Column::Subject);
    assert_eq!(subjects(&panel), ["b", "a"]);
}

#[test]
fn a_range_counts_only_the_instances_it_holds() {
    let event = event();
    let trace = trace(&event);
    let mut panel = panel(&trace);
    // Elements 2 and 3, indexes 2 and 3: a's 30 only, b's cut.
    let report = panel.report(
        &trace,
        &[],
        Range {
            from: Some(20),
            to: Some(31),
        },
    );
    let counts: Vec<_> = report
        .groups
        .iter()
        .map(|group| (group.subject.as_str(), group.count, group.cut))
        .collect();
    assert_eq!(counts, [("b", 0, 1), ("a", 1, 1)]);
}
