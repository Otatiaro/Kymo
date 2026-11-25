use super::*;
use crate::decode::Decoded;
use crate::events::{Catalogue, Field, Kind, Type};
use crate::interrupts::Interrupts;
use crate::symbols::Symbols;
use gimli::DwAte;

const NAMES: [&str; 7] = [
    "kymo::begin_of<a>",
    "kymo::end_of<a>",
    "kymo::begin_of<c>",
    "kymo::end_of<c>",
    "kymo::keyed_begin_of<b>",
    "kymo::keyed_end_of<b>",
    "app::tick",
];

fn catalogue() -> Catalogue {
    let events = NAMES
        .iter()
        .zip(0..)
        .map(|(&name, id)| {
            let fields = if name.starts_with("kymo::keyed_") {
                vec![Field {
                    name: DURATION_KEY.to_owned(),
                    offset: 0,
                    ty: Type {
                        name: "uint32_t".to_owned(),
                        size: Some(4),
                        kind: Kind::Base(DwAte(gimli::DW_ATE_unsigned.0)),
                    },
                }]
            } else {
                Vec::new()
            };
            let event = Event {
                id,
                name: name.to_owned(),
                size: 4 * fields.len() as u64,
                fields,
                text: None,
            };
            (id, event)
        })
        .collect();
    Catalogue {
        events,
        rings: Vec::new(),
        symbols: Symbols::default(),
        interrupts: Interrupts::default(),
        warnings: Vec::new(),
    }
}

/// A trace of `steps`, from index `first` on, 10 cycles apart: `"lost"`, or
/// an event by name, with its key if it has one.
fn trace<'a>(catalogue: &'a Catalogue, first: u32, steps: &[(&str, u32)]) -> Trace<'a> {
    let mut elements = Vec::new();
    let mut index = first;
    for (n, &(step, key)) in steps.iter().enumerate() {
        if step == "lost" {
            elements.push(Element::Lost {
                from: index,
                to: index + 3,
            });
            index += 3;
            continue;
        }
        let event = catalogue
            .events
            .values()
            .find(|event| event.name == step)
            .expect("an event of the catalogue");
        let mut payload = key.to_le_bytes().to_vec();
        payload.truncate(event.size as usize);
        elements.push(Element::Event {
            event: Decoded {
                index,
                timestamp: 0,
                event,
                payload,
            },
            cycles: 10 * n as i64,
            seconds: None,
        });
        index += 1;
    }
    Trace {
        first,
        elements,
        end: Some(index),
        cut: None,
        frequency: None,
        measured: None,
    }
}

const BEGIN_A: (&str, u32) = ("kymo::begin_of<a>", 0);
const END_A: (&str, u32) = ("kymo::end_of<a>", 0);
const BEGIN_C: (&str, u32) = ("kymo::begin_of<c>", 0);
const END_C: (&str, u32) = ("kymo::end_of<c>", 0);
const TICK: (&str, u32) = ("app::tick", 0);
const LOST: (&str, u32) = ("lost", 0);

fn begin_b(key: u32) -> (&'static str, u32) {
    ("kymo::keyed_begin_of<b>", key)
}

fn end_b(key: u32) -> (&'static str, u32) {
    ("kymo::keyed_end_of<b>", key)
}

/// Each instance as (subject, begin element, end element, open).
fn spans(paired: &Paired) -> Vec<(&str, usize, usize, bool)> {
    paired
        .instances
        .iter()
        .map(|instance| {
            assert_eq!((instance.kind, instance.metric), (KIND, DURATION));
            assert_eq!(instance.value, instance.to.cycles - instance.from.cycles);
            (
                instance.subject.as_str(),
                instance.from.element,
                instance.to.element,
                instance.open,
            )
        })
        .collect()
}

#[test]
fn durations_of_a_type_nest_as_a_stack() {
    let catalogue = catalogue();
    let paired = pair(&trace(&catalogue, 0, &[BEGIN_A, BEGIN_A, TICK, END_A, END_A]));
    assert_eq!(spans(&paired), [("a", 0, 4, false), ("a", 1, 3, false)]);
    assert!(paired.contradictions.is_empty());
}

#[test]
fn an_end_closes_its_own_type_only() {
    let catalogue = catalogue();
    let paired = pair(&trace(&catalogue, 0, &[BEGIN_A, BEGIN_C, END_A, END_C]));
    assert_eq!(spans(&paired), [("a", 0, 2, false), ("c", 1, 3, false)]);
    assert!(paired.contradictions.is_empty());
}

#[test]
fn a_key_closes_its_own_begin_in_any_order() {
    let catalogue = catalogue();
    let paired = pair(&trace(&catalogue, 0, &[begin_b(0), begin_b(1), end_b(0), end_b(1)]));
    assert_eq!(spans(&paired), [("b", 0, 2, false), ("b", 1, 3, false)]);
    assert!(paired.contradictions.is_empty());
}

#[test]
fn what_contradicts_the_pairing_is_reported() {
    let catalogue = catalogue();
    let paired = pair(&trace(
        &catalogue,
        0,
        &[END_A, begin_b(3), begin_b(3), end_b(3), end_b(4), end_b(3)],
    ));
    // The end with no begin, the key opened again, the key never opened and
    // the key closed twice; the second begin is the one closed.
    assert_eq!(paired.contradictions, [0, 2, 4, 5]);
    assert_eq!(spans(&paired), [("b", 2, 3, false)]);
}

#[test]
fn a_begin_open_at_the_end_is_open() {
    let catalogue = catalogue();
    let paired = pair(&trace(&catalogue, 0, &[BEGIN_A, begin_b(1), TICK]));
    assert_eq!(spans(&paired), [("a", 0, 2, true), ("b", 1, 2, true)]);
    assert!(paired.contradictions.is_empty());
}

#[test]
fn a_loss_closes_what_is_open_and_excuses_the_ends_after_it() {
    let catalogue = catalogue();
    let paired = pair(&trace(
        &catalogue,
        0,
        &[BEGIN_A, begin_b(2), LOST, END_A, end_b(2), BEGIN_C, END_C, END_C],
    ));
    let lost: Vec<bool> = paired.instances.iter().map(|instance| instance.to.lost).collect();
    assert_eq!(
        spans(&paired),
        [("a", 0, 2, false), ("b", 1, 2, false), ("c", 5, 6, false)]
    );
    assert_eq!(lost, [true, true, false]);
    assert!(paired.contradictions.is_empty());
}

/// Each duration as (begin element, lane), in the order it closes, then
/// those still open.
fn lanes(trace: &Trace) -> Vec<(usize, usize)> {
    let mut pairing = Pairing::new(trace.first);
    pairing.add(&trace.elements, 0);
    let open = pairing.open();
    pairing
        .closed
        .iter()
        .chain(&open)
        .map(|placed| (placed.instance.from.element, placed.lane))
        .collect()
}

#[test]
fn a_duration_takes_the_lowest_lane_free_at_its_begin() {
    let catalogue = catalogue();
    // Nested, then one begun once both ended: lanes 1 and 0, then 0 again.
    let nested = trace(&catalogue, 0, &[BEGIN_A, BEGIN_A, END_A, END_A, BEGIN_A, END_A]);
    assert_eq!(lanes(&nested), [(1, 1), (0, 0), (4, 0)]);
    // A double buffer: the third batch takes the lane the first freed, and
    // a type's lanes are its own.
    let buffers = trace(
        &catalogue,
        0,
        &[
            begin_b(0),
            begin_b(1),
            BEGIN_C,
            end_b(0),
            begin_b(0),
            end_b(1),
            end_b(0),
            END_C,
        ],
    );
    assert_eq!(lanes(&buffers), [(0, 0), (1, 1), (4, 0), (2, 0)]);
    // A key opened again frees the lane of the begin it drops, once the new
    // begin has taken another; a loss frees them all.
    let again = trace(
        &catalogue,
        0,
        &[begin_b(3), begin_b(3), begin_b(4), BEGIN_A, LOST, BEGIN_A, TICK],
    );
    assert_eq!(lanes(&again), [(3, 0), (1, 1), (2, 0), (5, 0)]);
}

#[test]
fn pairing_in_pieces_pairs_as_at_once() {
    let catalogue = catalogue();
    let whole = trace(
        &catalogue,
        0,
        &[
            BEGIN_A,
            begin_b(0),
            BEGIN_A,
            END_A,
            begin_b(1),
            end_b(0),
            LOST,
            END_A,
            BEGIN_C,
            TICK,
        ],
    );
    let mut at_once = Pairing::new(0);
    at_once.add(&whole.elements, 0);
    for split in 0..=whole.elements.len() {
        let mut pieces = Pairing::new(0);
        pieces.add(&whole.elements[..split], 0);
        pieces.add(&whole.elements, split);
        assert_eq!(
            (&pieces.closed, &pieces.contradictions, pieces.open()),
            (&at_once.closed, &at_once.contradictions, at_once.open()),
            "split at {split}"
        );
    }
}

#[test]
fn a_trace_joined_late_excuses_an_end_with_no_begin() {
    let catalogue = catalogue();
    let paired = pair(&trace(&catalogue, 5, &[END_A, end_b(1)]));
    assert!(paired.instances.is_empty());
    assert!(paired.contradictions.is_empty());
}
