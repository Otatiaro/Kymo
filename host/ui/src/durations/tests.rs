use super::*;
use kymo::decode::Decoded;
use kymo::events::Event;
use std::collections::BTreeMap;

fn catalogue(names: &[&str]) -> Catalogue {
    let events: BTreeMap<_, _> = names
        .iter()
        .zip(0..)
        .map(|(&name, id)| {
            let event = Event {
                id,
                name: name.to_owned(),
                size: 0,
                fields: Vec::new(),
                text: None,
            };
            (id, event)
        })
        .collect();
    Catalogue {
        events,
        rings: Vec::new(),
        symbols: Default::default(),
        interrupts: Default::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }
}

/// The events named `steps`, 10 cycles apart.
fn elements<'a>(catalogue: &'a Catalogue, steps: &[&str]) -> Vec<Element<'a>> {
    steps
        .iter()
        .enumerate()
        .map(|(n, &step)| Element::Event {
            event: Decoded {
                index: n as u32,
                timestamp: 0,
                event: catalogue.events.values().find(|event| event.name == step).unwrap(),
                payload: Vec::new(),
            },
            cycles: 10 * n as i64,
            seconds: None,
        })
        .collect()
}

const BEGIN_A: &str = "kymo::begin_of<app::a>";
const END_A: &str = "kymo::end_of<app::a>";
const BEGIN_C: &str = "kymo::begin_of<app::io::c>";

#[test]
fn a_line_per_type_begun_sorted_none_without_a_begin() {
    let durations = Durations::new(&catalogue(&[BEGIN_C, END_A, BEGIN_A, "app::tick"]), 0).unwrap();
    assert_eq!(durations.types, ["app::a", "app::io::c"]);
    assert_eq!(durations.row("app::io::c"), Some(1));
    assert!(Durations::new(&catalogue(&[END_A, "app::tick"]), 0).is_none());
}

#[test]
fn durations_fill_their_lanes_as_they_close() {
    let catalogue = catalogue(&[BEGIN_A, END_A, BEGIN_C]);
    let steps = [BEGIN_A, BEGIN_A, BEGIN_A, END_A, BEGIN_C, END_A];
    let elements = elements(&catalogue, &steps);
    let mut durations = Durations::new(&catalogue, 0).unwrap();
    durations.add(&elements[..4], 0);
    // The third, in lane 2, closed; the other two still open.
    let counts = |durations: &Durations| {
        let all = |spans: &Spans| spans.overlapping(i64::MIN, i64::MAX).count();
        durations.lanes[0].iter().map(all).collect::<Vec<_>>()
    };
    assert_eq!(counts(&durations), [0, 0, 1]);
    assert_eq!(durations.shown(0, &durations.open()), 3);
    durations.add(&elements, 4);
    assert_eq!(counts(&durations), [0, 1, 1]);
    let open = durations.open();
    assert_eq!(durations.shown(0, &open), 3);
    // c's line: one lane, its duration open.
    assert_eq!(durations.shown(1, &open), 1);
}

#[test]
fn a_line_shows_at_most_lanes_lanes() {
    let catalogue = catalogue(&[BEGIN_A, END_A]);
    let steps = [BEGIN_A; LANES + 2];
    let mut durations = Durations::new(&catalogue, 0).unwrap();
    durations.add(&elements(&catalogue, &steps), 0);
    assert_eq!(durations.shown(0, &durations.open()), LANES);
}
