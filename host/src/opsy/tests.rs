use super::*;
use crate::decode::Decoded;
use crate::events::{Event, Field as EventField, Type};
use crate::symbols::Symbols;
use gimli::DwAte;

/// The adapter's events, their fields 4-byte pointers but `priority`.
const HOOKS: [(&str, &[&str]); 10] = [
    ("task_added", &["task", "priority"]),
    ("task_priority_changed", &["task", "priority"]),
    ("task_terminated", &["task"]),
    ("task_started", &["task"]),
    ("enter_idle", &[]),
    ("isr_enter", &["handler"]),
    ("isr_exit", &["handler"]),
    ("mutex_taken", &["mutex", "owner"]),
    ("mutex_released", &["mutex", "owner"]),
    ("mutex_blocked", &["mutex", "waiter"]),
];

fn catalogue() -> Catalogue {
    let mut events = BTreeMap::new();
    for (n, (hook, fields)) in HOOKS.iter().enumerate() {
        let id = n as u16 + 1;
        let fields: Vec<EventField> = fields
            .iter()
            .enumerate()
            .map(|(n, &name)| EventField {
                name: name.to_owned(),
                offset: 4 * n as u64,
                ty: Type {
                    name: name.to_owned(),
                    size: Some(if name == "priority" { 1 } else { 4 }),
                    kind: match name {
                        "priority" => Kind::Base(DwAte(gimli::DW_ATE_unsigned.0)),
                        "handler" => Kind::Pointer(Pointee::Function),
                        _ => Kind::Pointer(Pointee::Record(format!("opsy::{name}"))),
                    },
                },
            })
            .collect();
        events.insert(
            id,
            Event {
                id,
                name: format!("{PREFIX}{hook}"),
                size: 4 * fields.len() as u64,
                fields,
            },
        );
    }
    // An event of the image that is no hook.
    events.insert(
        100,
        Event {
            id: 100,
            name: "app::tick".to_owned(),
            size: 0,
            fields: Vec::new(),
        },
    );
    Catalogue {
        events,
        rings: Vec::new(),
        symbols: Symbols::default(),
        warnings: Vec::new(),
    }
}

/// A trace of `steps`: `"lost"`, or a hook and its field values, at index
/// `first` on, 10 cycles apart.
fn trace<'a>(catalogue: &'a Catalogue, first: u32, steps: &[(&str, &[u32])]) -> Trace<'a> {
    let mut elements = Vec::new();
    let mut index = first;
    for (n, &(step, values)) in steps.iter().enumerate() {
        if step == "lost" {
            elements.push(Element::Lost {
                from: index,
                to: index + 3,
            });
            index += 3;
            continue;
        }
        let name = if step == "tick" {
            "app::tick".to_owned()
        } else {
            format!("{PREFIX}{step}")
        };
        let event = catalogue
            .events
            .values()
            .find(|event| event.name == name)
            .expect("a hook of the catalogue");
        let payload = values.iter().flat_map(|value| value.to_le_bytes()).collect();
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

/// The lines of `steps` taken in at once, after checking that every split
/// in two pieces gives the same lines.
fn spans(first: u32, steps: &[(&str, &[u32])]) -> Vec<String> {
    let catalogue = catalogue();
    let trace = trace(&catalogue, first, steps);
    let mut whole = Opsy::new(&catalogue, first).unwrap();
    whole.add(&trace.elements, 0);
    let lines = lines(&trace, &whole);
    for split in 1..trace.elements.len() {
        let mut pieces = Opsy::new(&catalogue, first).unwrap();
        pieces.add(&trace.elements[..split], 0);
        pieces.add(&trace.elements, split);
        assert_eq!(super::lines(&trace, &pieces), lines, "split at {split}");
    }
    lines
}

const A: u32 = 0x2000_0100;
const B: u32 = 0x2000_0200;
const M: u32 = 0x2000_0300;
const H: u32 = 0x0800_0401;
const G: u32 = 0x0800_0501;

#[test]
fn a_task_runs_from_its_start_to_the_next() {
    assert_eq!(
        spans(
            0,
            &[
                ("task_started", &[A]),
                ("tick", &[]),
                ("task_started", &[B]),
                ("enter_idle", &[]),
                ("task_started", &[A]),
            ]
        ),
        [
            "run from=0 to=2 task=0x20000100",
            "run from=2 to=3 task=0x20000200",
            "run from=3 to=4 task=idle",
            "run from=4 to=open task=0x20000100",
        ]
    );
}

#[test]
fn nested_interrupts_close_innermost_first() {
    assert_eq!(
        spans(
            0,
            &[
                ("isr_enter", &[H]),
                ("isr_enter", &[G]),
                ("isr_exit", &[G]),
                ("isr_enter", &[G]),
            ]
        ),
        [
            "interrupt from=1 to=2 depth=1 handler=0x08000501",
            "interrupt from=3 to=open depth=1 handler=0x08000501",
            "interrupt from=0 to=open depth=0 handler=0x08000401",
        ]
    );
    assert_eq!(
        spans(
            0,
            &[
                ("isr_enter", &[H]),
                ("isr_enter", &[G]),
                ("isr_exit", &[G]),
                ("isr_exit", &[H]),
            ]
        ),
        [
            "interrupt from=1 to=2 depth=1 handler=0x08000501",
            "interrupt from=0 to=3 depth=0 handler=0x08000401",
        ]
    );
}

#[test]
fn an_exit_from_another_handler_than_the_innermost_contradicts() {
    assert_eq!(
        spans(0, &[("isr_enter", &[H]), ("isr_enter", &[G]), ("isr_exit", &[H])]),
        [
            "interrupt from=1 to=open depth=1 handler=0x08000501",
            "interrupt from=0 to=open depth=0 handler=0x08000401",
            "contradiction 2",
        ]
    );
    assert_eq!(spans(0, &[("isr_exit", &[H])]), ["contradiction 0"]);
    // After a loss too: H, entered before it, cannot exit before G.
    assert_eq!(
        spans(0, &[("lost", &[]), ("isr_enter", &[G]), ("isr_exit", &[H])]),
        ["interrupt from=3 to=open depth=0 handler=0x08000501", "contradiction 4"]
    );
}

#[test]
fn a_mutex_released_to_a_waiter_is_held_by_it_from_then() {
    assert_eq!(
        spans(
            0,
            &[
                ("mutex_taken", &[M, A]),
                ("mutex_blocked", &[M, B]),
                ("mutex_released", &[M, A]),
                ("mutex_taken", &[M, B]),
                ("mutex_released", &[M, B]),
            ]
        ),
        [
            "hold from=0 to=2 mutex=0x20000300 task=0x20000100",
            "wait from=1 to=3 mutex=0x20000300 task=0x20000200",
            "hold from=3 to=4 mutex=0x20000300 task=0x20000200",
        ]
    );
}

#[test]
fn a_waiter_terminated_waits_no_more() {
    assert_eq!(
        spans(
            0,
            &[
                ("mutex_taken", &[M, A]),
                ("mutex_blocked", &[M, B]),
                ("task_terminated", &[B]),
            ]
        ),
        [
            "wait from=1 to=2 mutex=0x20000300 task=0x20000200",
            "hold from=0 to=open mutex=0x20000300 task=0x20000100",
        ]
    );
}

#[test]
fn mutex_events_against_the_order_contradict() {
    // Taken while held, released by another task, released never taken,
    // blocked twice.
    assert_eq!(
        spans(
            0,
            &[
                ("mutex_taken", &[M, A]),
                ("mutex_taken", &[M, B]),
                ("mutex_released", &[M, B]),
                ("mutex_blocked", &[M, B]),
                ("mutex_blocked", &[M, B]),
                ("mutex_released", &[M, A]),
                ("mutex_released", &[M, A]),
            ]
        ),
        [
            "hold from=0 to=5 mutex=0x20000300 task=0x20000100",
            "wait from=3 to=open mutex=0x20000300 task=0x20000200",
            "contradiction 1",
            "contradiction 2",
            "contradiction 4",
            "contradiction 6",
        ]
    );
}

#[test]
fn a_loss_ends_what_is_open_and_hides_what_ends_after() {
    assert_eq!(
        spans(
            0,
            &[
                ("task_started", &[A]),
                ("mutex_taken", &[M, A]),
                ("mutex_blocked", &[M, B]),
                ("isr_enter", &[H]),
                ("lost", &[]),
                ("isr_exit", &[H]),
                ("mutex_released", &[M, A]),
                ("tick", &[]),
                ("task_started", &[B]),
            ]
        ),
        [
            "run from=0 to=lost task=0x20000100",
            "interrupt from=3 to=lost depth=0 handler=0x08000401",
            "interrupt from=lost to=7 depth=0 handler=0x08000401",
            "hold from=1 to=lost mutex=0x20000300 task=0x20000100",
            "wait from=2 to=lost mutex=0x20000300 task=0x20000200",
            "hold from=lost to=8 mutex=0x20000300 task=0x20000100",
            "run from=10 to=open task=0x20000200",
        ]
    );
}

#[test]
fn a_loss_is_at_the_cycles_of_the_event_before_it() {
    let catalogue = catalogue();
    let trace = trace(
        &catalogue,
        0,
        &[("tick", &[]), ("task_started", &[A]), ("lost", &[]), ("tick", &[])],
    );
    let mut opsy = Opsy::new(&catalogue, 0).unwrap();
    opsy.add(&trace.elements, 0);
    let to = opsy.closed.runs[0].span.to;
    assert_eq!(
        to,
        Edge {
            element: 2,
            cycles: 10,
            lost: true
        }
    );
}

#[test]
fn a_recording_that_starts_late_starts_with_a_loss() {
    assert_eq!(
        spans(
            40,
            &[("isr_exit", &[H]), ("mutex_released", &[M, A]), ("task_started", &[A])]
        ),
        [
            "interrupt from=lost to=40 depth=0 handler=0x08000401",
            "hold from=lost to=41 mutex=0x20000300 task=0x20000100",
            "run from=42 to=open task=0x20000100",
        ]
    );
    assert_eq!(spans(0, &[("isr_exit", &[H])]), ["contradiction 0"]);
}

#[test]
fn a_task_keeps_its_last_priority() {
    let catalogue = catalogue();
    let trace = trace(
        &catalogue,
        0,
        &[
            ("task_added", &[A, 3]),
            ("task_added", &[B, 5]),
            ("task_priority_changed", &[A, 1]),
        ],
    );
    let mut opsy = Opsy::new(&catalogue, 0).unwrap();
    assert!(opsy.traced());
    opsy.add(&trace.elements, 0);
    let priorities: Vec<_> = opsy
        .tasks
        .list
        .iter()
        .map(|task| (task.address, task.priority))
        .collect();
    assert_eq!(priorities, [(u64::from(A), Some(1)), (u64::from(B), Some(5))]);
}

#[test]
fn a_priority_is_unknown_until_given_and_from_a_loss() {
    let catalogue = catalogue();
    // At cycles 0, 10, 20 and 30; the loss at the cycles before it, 30;
    // then 50 and 60.
    let trace = trace(
        &catalogue,
        0,
        &[
            ("tick", &[]),
            ("task_added", &[A, 3]),
            ("task_priority_changed", &[A, 1]),
            ("tick", &[]),
            ("lost", &[]),
            ("tick", &[]),
            ("task_priority_changed", &[A, 3]),
        ],
    );
    let mut opsy = Opsy::new(&catalogue, 0).unwrap();
    opsy.add(&trace.elements, 0);
    let task = &opsy.tasks.list[0];
    let at = [0, 10, 20, 25, 30, 50, 60, 70].map(|cycles| task.priority_at(cycles));
    assert_eq!(at, [None, Some(3), Some(1), Some(1), None, None, Some(3), Some(3)]);
}

#[test]
fn a_hook_without_a_field_the_host_reads_is_refused() {
    let mut catalogue = catalogue();
    let exit = catalogue
        .events
        .values_mut()
        .find(|event| event.name.ends_with("isr_exit"))
        .unwrap();
    exit.fields[0].name = "routine".to_owned();
    let error = Opsy::new(&catalogue, 0).err().expect("refused").to_string();
    assert_eq!(error, "kymo::opsy::isr_exit: no field handler, which the host reads");
}

#[test]
fn an_image_without_the_adapter_is_not_traced() {
    let mut catalogue = catalogue();
    catalogue.events.retain(|_, event| !event.name.starts_with(PREFIX));
    assert!(!Opsy::new(&catalogue, 0).unwrap().traced());
}
