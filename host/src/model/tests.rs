use super::*;
use crate::events::Event;
use crate::symbols::Symbols;

fn event(id: u16, size: u64) -> Event {
    Event {
        id,
        name: format!("app::e{size}"),
        size,
        fields: Vec::new(),
    }
}

fn decoded(event: &Event, index: u32) -> Element<'_> {
    Element::Event {
        event: Decoded {
            index,
            timestamp: 0,
            event,
            payload: vec![0; event.size as usize],
        },
        cycles: 0,
        seconds: None,
    }
}

/// An event that takes more than one slot shows the range of its slots, so
/// that the index after it is not taken for a loss.
#[test]
fn an_event_shows_the_slots_it_takes() {
    let symbols = Symbols::default();
    let line = |event: &Event, index| {
        let Element::Event { event, cycles, seconds } = decoded(event, index) else {
            unreachable!()
        };
        decode::line(&symbols, &event, cycles, seconds)
    };
    assert_eq!(line(&event(1, 8), 7), "event 7 0 - app::e8");
    assert_eq!(line(&event(2, 9), 7), "event 7-8 0 - app::e9");
    assert_eq!(line(&event(3, 33), u32::MAX), "event 4294967295-2 0 - app::e33");
}

/// The slots the events take and those lost cover the recording.
#[test]
fn the_slots_taken_and_lost_cover_the_recording() {
    let (one, two) = (event(1, 4), event(2, 16));
    let trace = Trace {
        first: u32::MAX - 1,
        elements: vec![
            decoded(&two, u32::MAX - 1),
            Element::Lost { from: 0, to: 3 },
            Element::Suspect {
                from: 2,
                to: 3,
                writers: 1,
            },
            decoded(&one, 3),
        ],
        end: Some(4),
        cut: None,
        frequency: None,
        measured: None,
    };
    assert_eq!(trace.slots(), (3, 3));
    let (taken, lost) = trace.slots();
    assert_eq!(taken + lost, u64::from(trace.end.unwrap().wrapping_sub(trace.first)));
}
