use super::*;
use kymo::decode::Decoded;
use kymo::events::Event;

fn event() -> Event {
    Event {
        id: 1,
        name: "app::tick".to_owned(),
        size: 0,
        fields: Vec::new(),
    }
}

fn at(event: &Event, index: u32, cycles: i64) -> Element<'_> {
    Element::Event {
        event: Decoded {
            index,
            timestamp: 0,
            event,
            payload: Vec::new(),
        },
        cycles,
        seconds: None,
    }
}

/// A loss before the first event; events at 100, 110, then 105 recorded
/// later by a writer it had interrupted; a suspect line after 110; a loss
/// after 105; two events at 120.
fn elements(event: &Event) -> Vec<Element<'_>> {
    vec![
        Element::Lost { from: 0, to: 2 },
        at(event, 2, 100),
        at(event, 3, 110),
        Element::Suspect {
            from: 3,
            to: 9,
            writers: 1,
        },
        at(event, 4, 105),
        Element::Lost { from: 5, to: 7 },
        at(event, 7, 120),
        at(event, 8, 120),
    ]
}

fn listed(events: &Events, from: i64, to: i64) -> Vec<(i64, usize)> {
    events
        .within(from, to)
        .iter()
        .map(|entry| (entry.cycles, entry.element))
        .collect()
}

/// By time, equals in the trace's order; each loss or suspect line after
/// the event recorded before it, the one before the first event before it.
#[test]
fn the_list_is_by_time_with_losses_in_place() {
    let event = event();
    let mut events = Events::default();
    events.add(&elements(&event), 0);
    assert_eq!(
        listed(&events, i64::MIN, i64::MAX),
        [
            (100, 0),
            (100, 1),
            (105, 4),
            (105, 5),
            (110, 2),
            (110, 3),
            (120, 6),
            (120, 7)
        ]
    );
}

/// A range holds the entries at both its bounds, and none outside.
#[test]
fn a_range_includes_both_bounds() {
    let event = event();
    let mut events = Events::default();
    events.add(&elements(&event), 0);
    assert_eq!(listed(&events, 105, 110), [(105, 4), (105, 5), (110, 2), (110, 3)]);
    assert_eq!(listed(&events, 106, 109), []);
    assert_eq!(listed(&events, 121, 200), []);
    assert_eq!(listed(&events, 120, 100), []);
}

/// Elements taken in two batches list as taken in one.
#[test]
fn batches_list_as_one() {
    let event = event();
    let elements = elements(&event);
    let mut whole = Events::default();
    whole.add(&elements, 0);
    for split in 0..=elements.len() {
        let mut events = Events::default();
        events.add(&elements[..split], 0);
        events.add(&elements, split);
        assert_eq!(
            listed(&events, i64::MIN, i64::MAX),
            listed(&whole, i64::MIN, i64::MAX),
            "split at {split}"
        );
    }
}
