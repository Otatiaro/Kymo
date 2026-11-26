use super::*;
use crate::decode::Decoded;
use crate::events::{Catalogue, Event};

const NAMES: [&str; 4] = ["app::request", "app::reply", "app::both", "app::tick"];

fn catalogue() -> Catalogue {
    let events = NAMES
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
        symbols: Symbols::default(),
        interrupts: Default::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }
}

/// A trace of `steps` from index 0: `"lost"` (an index lost), `"suspect"` (a
/// suspect line over the index before), or an event by its name after `app::`
/// at its cycles.
fn trace<'a>(catalogue: &'a Catalogue, steps: &[(&str, i64)]) -> Trace<'a> {
    let mut elements = Vec::new();
    let mut index = 0;
    for &(step, cycles) in steps {
        match step {
            "lost" => {
                elements.push(Element::Lost {
                    from: index,
                    to: index + 1,
                });
                index += 1;
            }
            "suspect" => elements.push(Element::Suspect {
                from: index - 1,
                to: index,
                writers: 1,
            }),
            name => {
                let name = format!("app::{name}");
                let event = catalogue
                    .events
                    .values()
                    .find(|event| event.name == name)
                    .expect("an event of the catalogue");
                elements.push(Element::Event {
                    event: Decoded {
                        index,
                        timestamp: 0,
                        event,
                        payload: Vec::new(),
                    },
                    cycles,
                    seconds: None,
                });
                index += 1;
            }
        }
    }
    Trace {
        first: 0,
        elements,
        end: Some(index),
        cut: None,
        frequency: None,
        measured: None,
        watched: None,
    }
}

fn words(text: &str) -> Words {
    Words::parse(text).unwrap()
}

/// The intervals from `a` to `b` of `steps` over `range`.
fn intervals(steps: &[(&str, i64)], a: &str, b: &str, range: Range) -> Intervals {
    let catalogue = catalogue();
    let trace = trace(&catalogue, steps);
    of(&trace, &catalogue.symbols, &words(a), &words(b), range, 10)
}

fn values(intervals: &Intervals) -> Vec<i64> {
    intervals
        .distribution
        .as_ref()
        .map(|distribution| distribution.worst.iter().map(|point| point.value).collect())
        .unwrap_or_default()
}

/// Each A gives the delay to its first B; a second B before the next A
/// is not one; an A followed by an A has no B; an A the trace ends after is
/// cut.
#[test]
fn an_interval_runs_from_an_a_to_its_first_b() {
    let steps = [
        ("request", 0),
        ("tick", 2),
        ("reply", 5),
        ("reply", 7),
        ("request", 10),
        ("reply", 30),
        ("request", 40),
        ("request", 50),
        ("reply", 51),
        ("request", 60),
    ];
    let found = intervals(&steps, "request", "reply", Range::default());
    assert_eq!(values(&found), [20, 5, 1]);
    assert_eq!((found.count, found.without, found.cut), (3, 1, 1));
    assert_eq!((found.excluded_loss, found.excluded_suspect), (0, 0));
    let distribution = found.distribution.as_ref().unwrap();
    assert_eq!((distribution.min.value, distribution.max.value), (1, 20));
    assert_eq!(distribution.max.index, 4);
    assert_eq!(found.ends[&distribution.max.element], 5);
}

/// An event both filters keep ends the interval open, then opens the next.
#[test]
fn an_event_both_filters_keep_ends_one_and_opens_the_next() {
    let steps = [("request", 0), ("both", 5), ("reply", 8)];
    let found = intervals(&steps, "request both", "reply both", Range::default());
    assert_eq!(values(&found), [5, 3]);
    assert_eq!((found.count, found.without, found.cut), (2, 0, 0));
}

/// A loss between an A and its B, or between an A and the next, excludes
/// the interval: its B may be lost. So does a suspect line over its indexes.
#[test]
fn a_loss_or_a_suspect_line_excludes_an_interval() {
    let steps = [
        ("request", 0),
        ("lost", 0),
        ("reply", 5),
        ("request", 10),
        ("lost", 0),
        ("request", 20),
        ("suspect", 0),
        ("reply", 25),
        ("request", 30),
        ("reply", 32),
    ];
    let found = intervals(&steps, "request", "reply", Range::default());
    assert_eq!((found.excluded_loss, found.excluded_suspect), (2, 1));
    assert_eq!((found.count, found.without, found.cut), (1, 0, 0));
    assert_eq!(values(&found), [2]);
}

/// Only the intervals within the range count; one a range edge cuts is cut;
/// one wholly outside is not counted.
#[test]
fn a_range_counts_its_intervals_and_cuts_those_on_its_edges() {
    let steps = [
        ("request", 0),
        ("reply", 5),
        ("request", 10),
        ("reply", 30),
        ("request", 40),
        ("reply", 45),
        ("request", 50),
        ("reply", 70),
        ("request", 80),
        ("reply", 81),
    ];
    let range = Range {
        from: Some(20),
        to: Some(60),
    };
    let found = intervals(&steps, "request", "reply", range);
    assert_eq!(values(&found), [5]);
    assert_eq!((found.count, found.cut), (1, 2));
}
