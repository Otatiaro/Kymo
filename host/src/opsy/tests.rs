use super::*;
use crate::decode::Decoded;
use crate::events::{Event, Field as EventField, Type};
use crate::events::{Kind, Pointee};
use crate::interrupts::Interrupts;
use crate::load::Covered;
use crate::meaning::*;
use crate::model::{Element, Trace};
use crate::stats;
use crate::symbols::Symbols;
use gimli::DwAte;
use std::collections::BTreeMap;

/// The adapter's events, their fields 4-byte pointers but `priority` and
/// `reason`, a byte and an enumeration.
const HOOKS: [(&str, &[&str]); 15] = [
    ("task_ready", &["task", "reason"]),
    ("task_sleep", &["task"]),
    ("task_wait", &["task"]),
    ("enter_systick", &[]),
    ("exit_systick", &[]),
    ("task_added", &["task", "priority"]),
    ("task_priority_changed", &["task", "priority"]),
    ("task_terminated", &["task"]),
    ("task_started", &["task"]),
    ("enter_idle", &[]),
    ("isr_enter", &["handler", "exception"]),
    ("isr_exit", &["handler"]),
    ("mutex_taken", &["mutex", "owner"]),
    ("mutex_released", &["mutex", "owner"]),
    ("mutex_blocked", &["mutex", "waiter"]),
];

/// OpSy's wake reasons, by value.
const REASONS: [&str; 4] = ["notified", "timed_out", "sleep_elapsed", "mutex_granted"];

pub(crate) fn catalogue() -> Catalogue {
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
                        "priority" | "exception" => Kind::Base(DwAte(gimli::DW_ATE_unsigned.0)),
                        "reason" => Kind::Enum(
                            REASONS
                                .iter()
                                .zip(0..)
                                .map(|(name, value)| (value, (*name).to_owned()))
                                .collect(),
                        ),
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
                text: None,
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
            text: None,
        },
    );
    // Kymo's own event that is no hook: the start of a duration.
    events.insert(
        102,
        Event {
            id: 102,
            name: "kymo::begin_of<app::tick>".to_owned(),
            size: 0,
            fields: Vec::new(),
            text: None,
        },
    );
    // The fault event: its words, then its return addresses.
    let word = |name: &str, offset: u64, size: u64, kind: Kind| EventField {
        name: name.to_owned(),
        offset,
        ty: Type {
            name: name.to_owned(),
            size: Some(size),
            kind,
        },
    };
    let unsigned = Kind::Base(DwAte(gimli::DW_ATE_unsigned.0));
    let mut fields: Vec<EventField> = crate::fault::WORDS
        .iter()
        .zip(0..)
        .map(|(name, n)| word(name, 4 * n, 4, unsigned.clone()))
        .collect();
    let returns = word("uint32_t", 0, 4, unsigned.clone()).ty;
    fields.push(word("returns", 64, 32, Kind::Array(Box::new(returns), vec![8])));
    events.insert(
        101,
        Event {
            id: 101,
            name: crate::fault::EVENT.to_owned(),
            size: 96,
            fields,
            text: None,
        },
    );
    Catalogue {
        events,
        rings: Vec::new(),
        symbols: Symbols::default(),
        interrupts: Interrupts::from_enumerators(&[(45, "TIM2_IRQn".to_owned())]),
        lines: Default::default(),
        code: Default::default(),
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
        if step == "suspect" {
            // A write overrun while the index went past the event before.
            elements.push(Element::Suspect {
                from: index - 1,
                to: index,
                writers: 1,
            });
            continue;
        }
        let name = if step == "tick" {
            "app::tick".to_owned()
        } else if step == "begin" {
            "kymo::begin_of<app::tick>".to_owned()
        } else if step == "fault" {
            crate::fault::EVENT.to_owned()
        } else {
            format!("{PREFIX}{step}")
        };
        let event = catalogue
            .events
            .values()
            .find(|event| event.name == name)
            .expect("a hook of the catalogue");
        // A field left out is 0.
        let mut payload: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
        payload.resize(payload.len().max(event.size as usize), 0);
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
        watched: None,
    }
}

/// The lines of `steps` taken in at once, after checking that every split
/// in two pieces gives the same lines.
fn spans(first: u32, steps: &[(&str, &[u32])]) -> Vec<String> {
    let catalogue = catalogue();
    let trace = trace(&catalogue, first, steps);
    let mut whole = Meaning::new(&catalogue, first).unwrap();
    whole.add(&trace.elements, 0);
    let lines = lines(&trace, &whole);
    for split in 1..trace.elements.len() {
        let mut pieces = Meaning::new(&catalogue, first).unwrap();
        pieces.add(&trace.elements[..split], 0);
        pieces.add(&trace.elements, split);
        assert_eq!(crate::meaning::lines(&trace, &pieces), lines, "split at {split}");
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

/// A fault's words up to exc_return: a precise bus fault at 0x08000124,
/// coming with `exc_return` in exception `exception`.
pub(crate) fn fault(exc_return: u32, exception: u32) -> [u32; 13] {
    let cfsr = (1 << 9) | (1 << 15);
    [
        cfsr,
        0,
        0,
        0x4000_0000,
        0,
        0,
        0,
        0,
        0,
        0,
        0x0800_0124,
        0x0100_0000 | exception,
        exc_return,
    ]
}

const FAULT: &str = "bits=PRECISERR,BFARVALID bfar=0x40000000 pc=0x08000124 lr=0x00000000";

#[test]
fn a_fault_on_the_task_or_in_the_handler_it_came_in() {
    let thread = fault(0xffff_fffd, 0);
    let lines = spans(0, &[("task_started", &[A]), ("fault", &thread)]);
    assert_eq!(
        lines.last().unwrap(),
        &format!("fault at=1 task=0x20000100 {FAULT} stack=psp sp=0x00000000")
    );
    let lines = spans(0, &[("enter_idle", &[]), ("fault", &thread)]);
    assert_eq!(
        lines.last().unwrap(),
        &format!("fault at=1 task=idle {FAULT} stack=psp sp=0x00000000")
    );
    // Before any task is known on the core.
    let lines = spans(0, &[("fault", &thread)]);
    assert_eq!(
        lines.last().unwrap(),
        &format!("fault at=0 context=unknown {FAULT} stack=psp sp=0x00000000")
    );
    // In the innermost handler entered for the exception it interrupted.
    let handler = fault(0xffff_fff1, 45);
    let lines = spans(
        0,
        &[
            ("task_started", &[A]),
            ("isr_enter", &[H, 45]),
            ("isr_enter", &[G, 46]),
            ("isr_exit", &[G]),
            ("fault", &handler),
        ],
    );
    assert_eq!(
        lines.last().unwrap(),
        &format!("fault at=4 handler=0x08000401 (IRQ29) {FAULT} stack=msp sp=0x00000000")
    );
    // In an exception no handler entered is for.
    let untraced = fault(0xffff_fff1, 11);
    let lines = spans(
        0,
        &[("task_started", &[A]), ("isr_enter", &[H, 45]), ("fault", &untraced)],
    );
    assert_eq!(
        lines.last().unwrap(),
        &format!("fault at=2 exception=11 {FAULT} stack=msp sp=0x00000000")
    );
}

/// The context of each application event in `steps`, as `kymo decode
/// --meaning` names it, after checking that every split in two pieces gives
/// the same.
fn instants(first: u32, steps: &[(&str, &[u32])]) -> Vec<String> {
    let catalogue = catalogue();
    let trace = trace(&catalogue, first, steps);
    let described = |meaning: &Meaning| -> Vec<String> {
        meaning
            .instants
            .iter()
            .map(|instant| {
                let (name, value) = meaning.place(instant.context);
                format!("{} {name}={value}", instant.edge.element)
            })
            .collect()
    };
    let mut whole = Meaning::new(&catalogue, first).unwrap();
    whole.add(&trace.elements, 0);
    let instants = described(&whole);
    for split in 1..trace.elements.len() {
        let mut pieces = Meaning::new(&catalogue, first).unwrap();
        pieces.add(&trace.elements[..split], 0);
        pieces.add(&trace.elements, split);
        assert_eq!(described(&pieces), instants, "split at {split}");
    }
    instants
}

#[test]
fn an_application_event_is_in_the_context_that_emits_it() {
    assert_eq!(
        instants(
            0,
            &[
                ("tick", &[]),
                ("task_started", &[A]),
                ("tick", &[]),
                ("isr_enter", &[H, 45]),
                ("tick", &[]),
                ("enter_systick", &[]),
                ("tick", &[]),
                ("exit_systick", &[]),
                ("isr_exit", &[H]),
                ("enter_idle", &[]),
                ("tick", &[]),
                ("lost", &[]),
                ("tick", &[]),
                ("task_started", &[B]),
                ("tick", &[]),
                // Kymo's own, no application event.
                ("begin", &[]),
            ]
        ),
        [
            "0 context=unknown",
            "2 task=0x20000100",
            "4 handler=0x08000401 (IRQ29)",
            // The kernel's handler is left out, as for a wake's cause.
            "6 handler=0x08000401 (IRQ29)",
            "10 task=idle",
            "12 context=unknown",
            "14 task=0x20000200",
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
fn a_handler_shared_by_two_interrupts_is_told_apart_by_them() {
    // TIM2 is named by the catalogue's IRQn_Type, IRQ46 by its number; H
    // called from thread mode by itself.
    assert_eq!(
        spans(
            0,
            &[
                ("isr_enter", &[H, 16 + 45]),
                ("isr_exit", &[H]),
                ("isr_enter", &[H, 16 + 46]),
                ("isr_enter", &[H, 16 + 45]),
                ("isr_exit", &[H]),
                ("isr_exit", &[H]),
                ("isr_enter", &[H, 0]),
                ("isr_exit", &[H]),
            ]
        ),
        [
            "interrupt from=0 to=1 depth=0 handler=0x08000401 (TIM2)",
            "interrupt from=3 to=4 depth=1 handler=0x08000401 (TIM2)",
            "interrupt from=2 to=5 depth=0 handler=0x08000401 (IRQ46)",
            "interrupt from=6 to=7 depth=0 handler=0x08000401",
        ]
    );
    // An exit after a loss, its entry unseen: by the handler alone.
    assert_eq!(
        spans(0, &[("lost", &[]), ("isr_exit", &[H])]),
        ["interrupt from=lost to=3 depth=unknown handler=0x08000401"]
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
        [
            "interrupt from=3 to=open depth=unknown handler=0x08000501",
            "contradiction 4"
        ]
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
            "interrupt from=lost to=7 depth=unknown handler=0x08000401",
            "hold from=1 to=lost mutex=0x20000300 task=0x20000100",
            "wait from=2 to=lost mutex=0x20000300 task=0x20000200",
            "hold from=lost to=8 mutex=0x20000300 task=0x20000100",
            "run from=10 to=open task=0x20000200",
        ]
    );
}

#[test]
fn the_nesting_is_unknown_from_a_loss_to_a_switch_with_no_handler_open() {
    // H, entered before the loss, may still be open below G: G's depth is
    // unknown until a switch, which OpSy makes with no handler open. A
    // switch while a handler entered since the loss is open does not end it.
    assert_eq!(
        spans(
            0,
            &[
                ("isr_enter", &[H]),
                ("lost", &[]),
                ("isr_enter", &[G]),
                ("isr_exit", &[G]),
                ("isr_enter", &[G]),
                ("task_started", &[A]),
                ("isr_enter", &[H]),
                ("isr_exit", &[H]),
                ("isr_exit", &[G]),
                ("enter_idle", &[]),
                ("isr_enter", &[H]),
                ("isr_enter", &[G]),
            ]
        ),
        [
            "run from=7 to=11 task=0x20000100",
            "interrupt from=0 to=lost depth=0 handler=0x08000401",
            "interrupt from=4 to=5 depth=unknown handler=0x08000501",
            "interrupt from=8 to=9 depth=unknown handler=0x08000401",
            "interrupt from=6 to=10 depth=unknown handler=0x08000501",
            "run from=11 to=open task=idle",
            "interrupt from=13 to=open depth=1 handler=0x08000501",
            "interrupt from=12 to=open depth=0 handler=0x08000401",
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
    let mut opsy = Meaning::new(&catalogue, 0).unwrap();
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
            "interrupt from=lost to=40 depth=unknown handler=0x08000401",
            "hold from=lost to=41 mutex=0x20000300 task=0x20000100",
            "run from=42 to=open task=0x20000100",
        ]
    );
    assert_eq!(spans(0, &[("isr_exit", &[H])]), ["contradiction 0"]);
}

/// The wake and ready lines of `steps`, in the order printed.
fn wakes(steps: &[(&str, &[u32])]) -> Vec<String> {
    spans(0, steps)
        .into_iter()
        .filter(|line| line.starts_with("wake ") || line.starts_with("ready "))
        .collect()
}

const C: u32 = 0x2000_0400;

#[test]
fn a_wake_in_a_handler_is_caused_by_the_innermost() {
    assert_eq!(
        wakes(&[
            ("task_started", &[A]),
            ("isr_enter", &[H]),
            ("isr_enter", &[G]),
            ("task_ready", &[B, NOTIFIED]),
            ("isr_exit", &[G]),
            ("isr_exit", &[H]),
            ("task_started", &[B]),
            ("task_sleep", &[B]),
        ]),
        [
            "wake at=3 task=0x20000200 reason=notified cause=handler:0x08000501",
            "ready from=3 to=6 task=0x20000200",
        ]
    );
}

#[test]
fn a_wake_in_a_task_is_caused_by_it_whatever_kernel_handler_is_open() {
    assert_eq!(
        wakes(&[
            ("task_started", &[A]),
            ("enter_systick", &[]),
            ("task_ready", &[B, NOTIFIED]),
            ("exit_systick", &[]),
            ("task_ready", &[C, MUTEX_GRANTED]),
            ("enter_idle", &[]),
            ("isr_enter", &[H]),
            ("isr_exit", &[H]),
        ])[..2],
        [
            "wake at=2 task=0x20000200 reason=notified cause=task:0x20000100",
            "wake at=4 task=0x20000400 reason=mutex_granted cause=task:0x20000100",
        ]
    );
    assert_eq!(
        wakes(&[("enter_idle", &[]), ("task_ready", &[B, NOTIFIED])])[0],
        "wake at=1 task=0x20000200 reason=notified cause=task:idle"
    );
}

#[test]
fn a_delay_that_runs_out_is_the_cause_whatever_it_interrupts() {
    let lines = wakes(&[
        ("task_started", &[A]),
        ("isr_enter", &[H]),
        ("task_ready", &[B, TIMED_OUT]),
        ("isr_exit", &[H]),
        ("task_ready", &[C, SLEEP_ELAPSED]),
    ]);
    assert_eq!(
        lines[..2],
        [
            "wake at=2 task=0x20000200 reason=timed_out cause=elapsed",
            "wake at=4 task=0x20000400 reason=sleep_elapsed cause=elapsed",
        ]
    );
}

#[test]
fn a_notify_all_gives_each_task_woken_the_same_cause() {
    let lines = wakes(&[
        ("task_started", &[A]),
        ("isr_enter", &[H]),
        ("task_ready", &[B, NOTIFIED]),
        ("task_ready", &[C, NOTIFIED]),
    ]);
    assert_eq!(
        lines[..2],
        [
            "wake at=2 task=0x20000200 reason=notified cause=handler:0x08000401",
            "wake at=3 task=0x20000400 reason=notified cause=handler:0x08000401",
        ]
    );
}

#[test]
fn a_loss_hides_the_cause_until_the_next_task_switch() {
    let lines = wakes(&[
        ("task_started", &[A]),
        ("lost", &[]),
        ("task_ready", &[B, NOTIFIED]),
        ("isr_enter", &[H]),
        ("task_ready", &[C, NOTIFIED]),
        ("isr_exit", &[H]),
        ("task_started", &[B]),
        ("task_ready", &[A, NOTIFIED]),
    ]);
    assert_eq!(
        lines[..3],
        [
            "wake at=4 task=0x20000200 reason=notified cause=unknown",
            "wake at=6 task=0x20000400 reason=notified cause=handler:0x08000401",
            "wake at=9 task=0x20000100 reason=notified cause=task:0x20000200",
        ]
    );
}

#[test]
fn a_task_is_ready_until_it_runs_and_each_time_it_is_preempted() {
    assert_eq!(
        wakes(&[
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[B]),
            ("task_started", &[A]),
            ("task_started", &[B]),
            ("task_started", &[A]),
            ("task_started", &[B]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
        ]),
        [
            "wake at=1 task=0x20000100 reason=notified cause=task:idle",
            "ready from=1 to=3 task=0x20000100",
            "ready from=4 to=5 task=0x20000100",
            "ready from=6 to=7 task=0x20000100",
        ]
    );
}

#[test]
fn a_time_ready_ends_at_a_loss_or_open_at_the_end() {
    assert_eq!(
        wakes(&[
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("lost", &[]),
            ("task_ready", &[B, NOTIFIED]),
            ("task_started", &[B]),
            ("task_started", &[A]),
            ("task_started", &[B]),
            ("task_started", &[A]),
            ("tick", &[]),
        ]),
        [
            "wake at=1 task=0x20000100 reason=notified cause=task:idle",
            "wake at=5 task=0x20000200 reason=notified cause=unknown",
            "ready from=1 to=lost task=0x20000100",
            "ready from=5 to=6 task=0x20000200",
            "ready from=7 to=8 task=0x20000200",
            "ready from=9 to=open task=0x20000200",
        ]
    );
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
    let mut opsy = Meaning::new(&catalogue, 0).unwrap();
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
    let mut opsy = Meaning::new(&catalogue, 0).unwrap();
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
    let error = Meaning::new(&catalogue, 0).err().expect("refused").to_string();
    assert_eq!(error, "kymo::opsy::isr_exit: no field handler, which the host reads");
}

#[test]
fn an_image_without_the_adapter_is_not_traced() {
    let mut catalogue = catalogue();
    catalogue.events.retain(|_, event| !event.name.starts_with(PREFIX));
    assert!(!Meaning::new(&catalogue, 0).unwrap().traced());
}

const NOTIFIED: u32 = 0;
const TIMED_OUT: u32 = 1;
const SLEEP_ELAPSED: u32 = 2;
const MUTEX_GRANTED: u32 = 3;

/// The statistics of `steps` over `range`, after checking that every split
/// in two pieces gives the same instances, and that the JSON fits the schema.
fn report(steps: &[(&str, &[u32])], range: stats::Range) -> stats::Report {
    report_from(0, steps, range)
}

/// The report of a trace recorded from index `first`.
fn report_from(first: u32, steps: &[(&str, &[u32])], range: stats::Range) -> stats::Report {
    let catalogue = catalogue();
    let trace = trace(&catalogue, first, steps);
    let mut whole = Meaning::new(&catalogue, first).unwrap();
    whole.add(&trace.elements, 0);
    let instances = instances(&whole);
    for split in 1..trace.elements.len() {
        let mut pieces = Meaning::new(&catalogue, first).unwrap();
        pieces.add(&trace.elements[..split], 0);
        pieces.add(&trace.elements, split);
        assert_eq!(crate::meaning::instances(&pieces), instances, "split at {split}");
    }
    let faults: Vec<usize> = whole.faults.iter().map(|faulted| faulted.edge.element).collect();
    let found = stats::Found {
        instances: &instances,
        contradictions: &whole.contradictions,
        faults: &faults,
        known: &whole.known(),
    };
    let report = stats::compute(&trace, found, range, 100);
    crate::stats::tests::check_schema(&stats::json(&report));
    report
}

/// A group's counts: counted, excluded for a loss, for a suspect, cut; and
/// the values counted, in ascending order.
fn group(report: &stats::Report, name: &str) -> (usize, usize, usize, usize, Vec<i64>) {
    let group = report
        .groups
        .iter()
        .find(|group| {
            let mut full = format!("{} {} {}", group.kind, group.subject, group.metric.name);
            if let Some(detail) = &group.detail {
                full += &format!(" {detail}");
            }
            full == name
        })
        .unwrap_or_else(|| panic!("no group {name} in\n{}", stats::text(report).join("\n")));
    let mut values: Vec<i64> = group
        .distribution
        .iter()
        .flat_map(|distribution| distribution.worst.iter().map(|point| point.value))
        .collect();
    values.sort();
    (
        group.count,
        group.excluded_loss,
        group.excluded_suspect,
        group.cut,
        values,
    )
}

fn names(report: &stats::Report) -> Vec<String> {
    report
        .groups
        .iter()
        .map(|group| format!("{} {} {}", group.kind, group.subject, group.metric.name))
        .collect()
}

/// A task preempted by a task, by a handler that a second one interrupts,
/// and by the kernel's tick.
const PREEMPTED: [(&str, &[u32]); 15] = [
    ("enter_idle", &[]),
    ("task_ready", &[A, NOTIFIED]),
    ("task_started", &[A]),
    ("isr_enter", &[H]),
    ("isr_enter", &[G]),
    ("isr_exit", &[G]),
    ("isr_exit", &[H]),
    ("task_ready", &[B, TIMED_OUT]),
    ("task_started", &[B]),
    ("task_sleep", &[B]),
    ("task_started", &[A]),
    ("enter_systick", &[]),
    ("exit_systick", &[]),
    ("task_wait", &[A]),
    ("enter_idle", &[]),
];

#[test]
fn the_wake_latency_splits_by_reason_then_by_cause() {
    let report = report(&PREEMPTED, stats::Range::default());
    let details = |task: &str| -> Vec<Option<&str>> {
        report
            .groups
            .iter()
            .filter(|group| group.subject == task && group.metric.name == "wake_latency")
            .map(|group| group.detail.as_deref())
            .collect()
    };
    assert_eq!(details("0x20000100"), [None, Some("notified"), Some("task:idle")]);
    assert_eq!(details("0x20000200"), [None, Some("timed_out"), Some("elapsed")]);
}

/// Within a metric, the details in order of their text, and each one's
/// instances in order of their start, whatever order they end in.
#[test]
fn instances_come_by_detail_then_by_start() {
    let catalogue = catalogue();
    let trace = trace(
        &catalogue,
        0,
        &[
            ("task_ready", &[A, TIMED_OUT]),
            ("task_started", &[A]),
            ("mutex_taken", &[M, A]),
            ("mutex_blocked", &[M, B]),
            ("mutex_blocked", &[M, C]),
            ("mutex_released", &[M, A]),
            ("mutex_taken", &[M, C]),
            ("mutex_released", &[M, C]),
            ("mutex_taken", &[M, B]),
            ("task_wait", &[A]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_wait", &[A]),
        ],
    );
    let mut opsy = Meaning::new(&catalogue, 0).unwrap();
    opsy.add(&trace.elements, 0);
    let instances = instances(&opsy);
    let listed = |kind: &str, metric: &str| -> Vec<(Option<&str>, usize)> {
        instances
            .iter()
            .filter(|instance| instance.kind == kind && instance.metric.name == metric)
            .map(|instance| (instance.detail.as_deref(), instance.from.element))
            .collect()
    };
    assert_eq!(
        listed("task", "wake_latency"),
        [
            (None, 0),
            (None, 10),
            (Some("notified"), 10),
            (Some("timed_out"), 0),
            (Some("elapsed"), 0),
            (Some("task:0x20000100"), 10),
        ]
    );
    // C, blocked after B, is granted the mutex first.
    assert_eq!(
        listed("mutex", "wait"),
        [(None, 3), (None, 4), (Some("0x20000200"), 3), (Some("0x20000400"), 4)]
    );
}

#[test]
fn an_activation_splits_into_what_preempts_it() {
    let report = report(&PREEMPTED, stats::Range::default());
    let a = |metric| group(&report, &format!("task 0x20000100 {metric}"));
    // Ready at 10, runs 20 to 80 and 100 to 130, H from 30 to 60 with G
    // from 40 to 50 in it, the tick from 110 to 120.
    assert_eq!(a("wake_latency"), (1, 0, 0, 0, vec![10]));
    assert_eq!(a("wake_latency notified"), (1, 0, 0, 0, vec![10]));
    assert_eq!(a("wake_latency task:idle"), (1, 0, 0, 0, vec![10]));
    assert_eq!(a("gross"), (1, 0, 0, 0, vec![110]));
    assert_eq!(a("preempted_by_tasks"), (1, 0, 0, 0, vec![20]));
    assert_eq!(a("preempted_by_handlers"), (1, 0, 0, 0, vec![40]));
    assert_eq!(a("net"), (1, 0, 0, 0, vec![50]));
    assert_eq!(a("response"), (1, 0, 0, 0, vec![120]));
    let b = |metric| group(&report, &format!("task 0x20000200 {metric}"));
    assert_eq!(b("wake_latency timed_out"), (1, 0, 0, 0, vec![10]));
    assert_eq!(b("net"), (1, 0, 0, 0, vec![10]));
    assert_eq!(b("response"), (1, 0, 0, 0, vec![20]));
    let handler = |name| group(&report, &format!("handler {name}"));
    assert_eq!(handler("0x08000401 gross"), (1, 0, 0, 0, vec![30]));
    assert_eq!(handler("0x08000401 net"), (1, 0, 0, 0, vec![20]));
    assert_eq!(handler("0x08000501 net"), (1, 0, 0, 0, vec![10]));
    assert_eq!(handler("systick net"), (1, 0, 0, 0, vec![10]));
    assert_eq!((report.from, report.to, report.known, report.losses), (0, 140, 140, 0));
    let net = report
        .groups
        .iter()
        .find(|group| group.subject == "0x20000100" && group.metric.name == "net");
    assert_eq!(net.unwrap().share(report.known), Some(50.0 / 140.0));
    assert_eq!(
        names(&report)[..9],
        [
            "task 0x20000100 wake_latency",
            "task 0x20000100 wake_latency",
            "task 0x20000100 wake_latency",
            "task 0x20000100 preempted_by_tasks",
            "task 0x20000100 preempted_by_handlers",
            "task 0x20000100 net",
            "task 0x20000100 gross",
            "task 0x20000100 response",
            "task 0x20000200 wake_latency",
        ]
    );
}

#[test]
fn the_faults_of_the_range_are_counted() {
    let thread = fault(0xffff_fffd, 0);
    let steps: &[(&str, &[u32])] = &[("task_started", &[A]), ("fault", &thread), ("task_sleep", &[A])];
    let range = |from, to| stats::Range {
        from: Some(from),
        to: Some(to),
    };
    assert_eq!(report(steps, stats::Range::default()).faults, 1);
    assert_eq!(report(steps, range(10, 20)).faults, 1);
    assert_eq!(report(steps, range(0, 10)).faults, 0);
    assert_eq!(report(steps, range(20, 30)).faults, 0);
}
/// A recording that starts after index 0 joined the trace: the state before
/// it is unknown, so the instance it cuts is excluded, but it is not a loss.
#[test]
fn a_recording_that_joined_late_is_not_a_loss() {
    let steps: &[(&str, &[u32])] = &[
        ("task_started", &[A]),
        ("task_sleep", &[A]),
        ("enter_idle", &[]),
        ("task_ready", &[A, NOTIFIED]),
        ("task_started", &[A]),
        ("task_sleep", &[A]),
        ("enter_idle", &[]),
    ];
    let from_zero = report_from(0, steps, stats::Range::default());
    let joined = report_from(5, steps, stats::Range::default());
    assert_eq!((from_zero.joined, from_zero.losses), (None, 0));
    assert_eq!((joined.joined, joined.losses), (Some(5), 0));
    // The first run started before the recording: what woke it is unknown.
    let net = |report: &stats::Report| group(report, "task 0x20000100 net");
    assert_eq!(net(&from_zero), (1, 0, 0, 0, vec![10]));
    assert_eq!(net(&joined), (1, 1, 0, 0, vec![10]));
    let text = stats::text(&joined).join("\n");
    assert!(
        text.contains(", 0 losses, 0 suspect writers, 0 contradictions, 0 faults, joined at index 5\n"),
        "{text}"
    );
    let json = stats::json(&joined);
    assert!(json.contains(r#""joined":5,"losses":0,"#), "{json}");
    assert!(stats::json(&from_zero).contains(r#""joined":null,"#));
}

#[test]
fn an_instance_that_touches_a_loss_is_excluded() {
    let report = report(
        &[
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("lost", &[]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
        ],
        stats::Range::default(),
    );
    let a = |metric| group(&report, &format!("task 0x20000100 {metric}"));
    assert_eq!(a("response"), (1, 1, 0, 0, vec![20]));
    assert_eq!(a("wake_latency"), (1, 1, 0, 0, vec![10]));
    // The period from one task_ready to the next spans the loss.
    assert_eq!(a("period"), (0, 1, 0, 0, vec![]));
    assert!(
        report
            .groups
            .iter()
            .all(|group| group.lower_bound() == (group.excluded_loss > 0))
    );
    // The task on the core is unknown from the loss, after 20, to the
    // enter_idle at 50: the task_sleep at 40 does not say what runs next.
    assert_eq!((report.losses, report.known), (1, 60));
    let text = stats::text(&report).join("\n");
    assert!(
        text.contains("task 0x20000100 response: 1, min 20 cycles at #8, p50 20 cycles, p90 ≥ 20 cycles"),
        "{text}"
    );
}

/// A net execution's share is over the time known in the range: a range that
/// starts in the unknown after a loss counts only from the next switch.
#[test]
fn a_share_is_over_the_time_known_in_the_range() {
    let report = report(
        &[
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("lost", &[]),
            ("tick", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
        ],
        stats::Range {
            from: Some(50),
            to: Some(130),
        },
    );
    // Unknown from the loss, after 40, to the task_started at 80; A runs
    // 80 to 90 and 120 to 130. The loss sits at 40, before the range, which
    // does not count it but still lacks what it hid.
    assert_eq!((report.from, report.to, report.known, report.losses), (50, 130, 50, 0));
    let net = report
        .groups
        .iter()
        .find(|group| group.kind == "task" && group.metric.name == "net")
        .unwrap();
    assert_eq!(net.count, 2);
    assert_eq!(net.share(report.known), Some(20.0 / 50.0));
}

#[test]
fn an_instance_that_covers_an_overrun_write_is_excluded() {
    let report = report(
        &[
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("suspect", &[]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
        ],
        stats::Range::default(),
    );
    let a = |metric| group(&report, &format!("task 0x20000100 {metric}"));
    assert_eq!(a("response"), (1, 0, 1, 0, vec![20]));
    assert_eq!(a("period"), (0, 0, 1, 0, vec![]));
    assert_eq!((report.suspects, report.losses), (1, 0));
}

#[test]
fn the_period_jitter_and_worst_instances_point_at_their_start() {
    let report = report(
        &[
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("task_ready", &[A, TIMED_OUT]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("tick", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
        ],
        stats::Range::default(),
    );
    let find = |metric: &str, detail: Option<&str>| {
        report
            .groups
            .iter()
            .find(|group| group.metric.name == metric && group.detail.as_deref() == detail)
            .unwrap()
    };
    let period = find("period", None);
    let distribution = period.distribution.as_ref().unwrap();
    assert_eq!((distribution.min.value, distribution.min.index), (40, 1));
    assert_eq!(
        (distribution.max.value, distribution.max.index, distribution.max.cycles),
        (50, 5, 50)
    );
    assert_eq!(period.jitter(), Some(10));
    let response = find("response", None).distribution.as_ref().unwrap();
    let worst: Vec<(i64, u32)> = response.worst.iter().map(|point| (point.value, point.index)).collect();
    assert_eq!(worst, [(20, 1), (20, 5), (20, 10)]);
    assert_eq!(find("wake_latency", Some("notified")).count, 2);
    assert_eq!(find("wake_latency", Some("timed_out")).count, 1);
    assert_eq!(find("response", None).jitter(), None);
}

#[test]
fn a_range_counts_what_it_holds_and_cuts_what_crosses_it() {
    let report = report(
        &PREEMPTED,
        stats::Range {
            from: Some(0),
            to: Some(100),
        },
    );
    assert_eq!(group(&report, "task 0x20000100 response"), (0, 0, 0, 1, vec![]));
    assert_eq!(group(&report, "task 0x20000200 response"), (1, 0, 0, 0, vec![20]));
    assert_eq!(group(&report, "handler 0x08000401 gross"), (1, 0, 0, 0, vec![30]));
    assert!(!names(&report).iter().any(|name| name.contains("systick")));
    assert_eq!((report.from, report.to, report.known), (0, 100, 100));
    let empty = self::report(
        &PREEMPTED,
        stats::Range {
            from: Some(200),
            to: None,
        },
    );
    assert!(empty.groups.is_empty());
    assert_eq!(empty.known, 0);
}

#[test]
fn a_handler_net_is_less_its_direct_children_only() {
    // H from 0 to 50, G from 10 to 40 in it, the kernel's tick from 20 to 30
    // in G.
    let report = report(
        &[
            ("isr_enter", &[H]),
            ("isr_enter", &[G]),
            ("enter_systick", &[]),
            ("exit_systick", &[]),
            ("isr_exit", &[G]),
            ("isr_exit", &[H]),
        ],
        stats::Range::default(),
    );
    assert_eq!(group(&report, "handler 0x08000401 gross"), (1, 0, 0, 0, vec![50]));
    assert_eq!(group(&report, "handler 0x08000401 net"), (1, 0, 0, 0, vec![20]));
    assert_eq!(group(&report, "handler 0x08000501 net"), (1, 0, 0, 0, vec![20]));
    assert_eq!(group(&report, "handler systick net"), (1, 0, 0, 0, vec![10]));
}

#[test]
fn an_activation_still_open_at_the_end_is_cut() {
    let report = report(
        &[
            ("enter_idle", &[]),
            ("task_ready", &[A, NOTIFIED]),
            ("task_started", &[A]),
        ],
        stats::Range::default(),
    );
    assert_eq!(group(&report, "task 0x20000100 response"), (0, 0, 0, 1, vec![]));
    assert_eq!(group(&report, "task 0x20000100 wake_latency"), (0, 0, 0, 1, vec![]));
}

/// The time the task on the core is known: none before the first
/// task_started or enter_idle of a recording that starts late, none from a
/// loss to the next, the last run to the end of the capture. A handler run
/// while it is unknown counts in no bin's share.
#[test]
fn the_task_on_the_core_is_known_between_losses_and_the_next_switch() {
    use crate::load::{Bin, bins};
    let catalogue = catalogue();
    let trace = trace(
        &catalogue,
        5,
        &[
            ("tick", &[]),
            ("task_started", &[A]),
            ("tick", &[]),
            ("lost", &[]),
            ("isr_enter", &[H]),
            ("isr_exit", &[H]),
            ("task_started", &[B]),
            ("enter_idle", &[]),
            ("tick", &[]),
        ],
    );
    let mut opsy = Meaning::new(&catalogue, 5).unwrap();
    opsy.add(&trace.elements, 0);
    let known = opsy.known();
    // From the first switch to the loss, then from the next switch on.
    assert_eq!(
        [10, 20, 30, 60, 70, 80].map(|at| known.until(at)),
        [0, 10, 10, 10, 20, 30]
    );
    let mut handler = Covered::default();
    let span = opsy.closed.interrupts[0].span;
    handler.push(span.from.cycles, span.to.cycles);
    let mut task = Covered::default();
    let span = opsy.closed.runs.iter().find(|run| run.task == Some(0)).unwrap().span;
    task.push(span.from.cycles, span.to.cycles);
    let bin = |from, to, covered, known| Bin {
        from,
        to,
        covered,
        known,
    };
    assert_eq!(
        bins(&handler.within(&known), &known, 20, 0, (0, 80), (0, 80)),
        [
            bin(0, 20, 0, 10),
            bin(20, 40, 0, 0),
            bin(40, 60, 0, 0),
            bin(60, 80, 0, 20)
        ]
    );
    assert_eq!(
        bins(&task.within(&known), &known, 20, 0, (0, 80), (0, 80)),
        [
            bin(0, 20, 10, 10),
            bin(20, 40, 0, 0),
            bin(40, 60, 0, 0),
            bin(60, 80, 0, 20)
        ]
    );
}

/// The CPU row of a hand trace: idle interrupted by nested handlers is
/// busy, a task under a handler counts once, a handler run while the task
/// on the core is unknown counts nowhere, and a handler still open runs to
/// the end of the capture.
#[test]
fn the_cpu_stacks_handlers_at_any_depth_then_tasks_other_than_idle() {
    use crate::load::{CpuBin, cpu_bins};
    let catalogue = catalogue();
    let trace = trace(
        &catalogue,
        0,
        &[
            ("tick", &[]),
            ("enter_idle", &[]),
            ("isr_enter", &[H]),
            ("isr_enter", &[G]),
            ("isr_exit", &[G]),
            ("isr_exit", &[H]),
            ("task_started", &[A]),
            ("isr_enter", &[H]),
            ("isr_exit", &[H]),
            ("lost", &[]),
            ("isr_enter", &[G]),
            ("isr_exit", &[G]),
            ("enter_idle", &[]),
            ("isr_enter", &[H]),
            ("tick", &[]),
        ],
    );
    let mut opsy = Meaning::new(&catalogue, 0).unwrap();
    opsy.add(&trace.elements, 0);
    let tasks = opsy.tasks.list.len();
    let a = opsy
        .tasks
        .list
        .iter()
        .position(|task| task.address == u64::from(A))
        .unwrap();
    let bin = |from, to, known, handlers, of_a: i64| {
        let mut each = vec![0; tasks];
        each[a] = of_a;
        CpuBin {
            from,
            to,
            known,
            handlers,
            tasks: of_a,
            each,
        }
    };
    // Known from 10 to the loss at 80, and from 120 on; handlers 20-50,
    // 70-80 and from 130; task A 60-80, 60-70 without a handler.
    assert_eq!(
        cpu_bins(&opsy.cpu(), 50, 0, (0, 150), (0, 150)),
        [
            bin(0, 50, 40, 30, 0),
            bin(50, 100, 30, 10, 10),
            bin(100, 150, 30, 20, 0)
        ]
    );
}

/// A random trace of switches, nested handlers and losses.
fn random_steps(state: &mut u64) -> Vec<(&'static str, Vec<u32>)> {
    let mut next = || {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    };
    let (mut steps, mut open) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        let step = match next() % 12 {
            0 => ("lost", Vec::new()),
            1 | 2 if open.is_empty() => ("task_started", vec![if next().is_multiple_of(2) { A } else { B }]),
            3 if open.is_empty() => ("enter_idle", Vec::new()),
            4..=6 if open.len() < 3 => {
                let handler = [H, G][open.len() % 2];
                open.push(handler);
                ("isr_enter", vec![handler])
            }
            7..=9 if !open.is_empty() => ("isr_exit", vec![open.pop().unwrap()]),
            _ => ("tick", Vec::new()),
        };
        steps.push(step);
    }
    steps
}

/// Over random traces, each cycle of the CPU row is the oracle's: known
/// where a run covers it, handlers where a handler covers it, busy where a
/// handler or a task other than idle does, a task's own where it runs and no
/// handler does, the spans still open to the end.
#[test]
fn the_cpu_row_covers_each_cycle_as_the_oracle_does() {
    let catalogue = catalogue();
    let mut state = 0x1234_5678_9abc_def1u64;
    let (mut idle_interrupted, mut hidden) = (0, 0);
    for _ in 0..500 {
        let first = if state.is_multiple_of(3) { 5 } else { 0 };
        let steps = random_steps(&mut state);
        let steps: Vec<(&str, &[u32])> = steps.iter().map(|(step, values)| (*step, values.as_slice())).collect();
        let trace = trace(&catalogue, first, &steps);
        let mut opsy = Meaning::new(&catalogue, first).unwrap();
        opsy.add(&trace.elements, 0);
        let cpu = opsy.cpu();
        let open = opsy.open();
        let closed = &opsy.closed;
        let at = |covered: &Covered, cycle: i64| covered.until(cycle + 1) - covered.until(cycle) == 1;
        let inside = |span: &Span, cycle: i64| span.from.cycles <= cycle && cycle < span.to.cycles;
        let since = |span: &Span, cycle: i64| span.from.cycles <= cycle;
        for cycle in -10..10 * steps.len() as i64 + 30 {
            let run = closed
                .runs
                .iter()
                .find(|run| inside(&run.span, cycle))
                .map(|run| run.task)
                .or_else(|| open.runs.iter().find(|run| since(&run.span, cycle)).map(|run| run.task));
            let handler = closed.interrupts.iter().any(|interrupt| inside(&interrupt.span, cycle))
                || open.interrupts.iter().any(|interrupt| since(&interrupt.span, cycle));
            let known = run.is_some();
            assert_eq!(at(&cpu.known, cycle), known, "known at {cycle}: {steps:?}");
            assert_eq!(
                at(&cpu.handlers, cycle),
                known && handler,
                "handlers at {cycle}: {steps:?}"
            );
            assert_eq!(
                at(&cpu.busy, cycle),
                known && (handler || run.flatten().is_some()),
                "busy at {cycle}: {steps:?}"
            );
            for (task, covered) in cpu.tasks.iter().enumerate() {
                assert_eq!(
                    at(covered, cycle),
                    !handler && run == Some(Some(task)),
                    "task {task} at {cycle}: {steps:?}"
                );
            }
            idle_interrupted += usize::from(run == Some(None) && handler);
            hidden += usize::from(!known && handler);
        }
    }
    assert!(
        idle_interrupted > 1000 && hidden > 1000,
        "{idle_interrupted} cycles of idle interrupted, {hidden} of a handler while unknown"
    );
}

/// What the SWO watched: the exception packets `packets`, anchored from
/// `span.0` to `span.1`.
fn watched(span: (i64, i64), packets: &[(u16, crate::itm::Function, i64)]) -> crate::swo::Watched {
    crate::swo::Watched {
        span: Some(span),
        exceptions: packets
            .iter()
            .map(|&(number, function, cycles)| crate::swo::Exception {
                number,
                function,
                cycles,
            })
            .collect(),
        ..Default::default()
    }
}

/// The meaning of `steps` once the exceptions `watched` traced are taken in.
fn with_exceptions<'a>(
    catalogue: &'a Catalogue,
    steps: &[(&str, &[u32])],
    watched: crate::swo::Watched,
) -> (Trace<'a>, Meaning<'a>) {
    let mut trace = trace(catalogue, 0, steps);
    trace.watched = Some(watched);
    let meaning = Meaning::of_trace(catalogue, &trace).unwrap();
    (trace, meaning)
}

const ENTERED: crate::itm::Function = crate::itm::Function::Entered;
const EXITED: crate::itm::Function = crate::itm::Function::Exited;
const RETURNED: crate::itm::Function = crate::itm::Function::Returned;
const TICKS: [(&str, &[u32]); 6] = [("tick", &[]); 6];

#[test]
fn the_swo_traces_a_handler_named_by_its_interrupt() {
    // TIM2 (16 + 45) at 12 and 25, then an exit of an exception not open.
    let packets = [
        (0, RETURNED, 5),
        (61, ENTERED, 12),
        (61, EXITED, 25),
        (0, RETURNED, 25),
        (62, EXITED, 40),
    ];
    let catalogue = catalogue();
    let (trace, meaning) = with_exceptions(&catalogue, &TICKS, watched((0, 100), &packets));
    assert_eq!(
        lines(&trace, &meaning),
        ["interrupt from=@12 to=@25 depth=0 handler=TIM2", "contradiction @40",]
    );
    assert_eq!(meaning.exception_contradictions[0].element, 4);
    assert_eq!(meaning.double_trace(), None);
    // The vector table supposes TIM2's handler.
    let mut catalogue = catalogue;
    catalogue.symbols = Symbols::with_vectors(&[(0x0800_0600, 0x20, "TIM2_IRQHandler")], {
        let mut vectors = vec![0; 62];
        vectors[61] = 0x0800_0601;
        vectors
    });
    let (trace, meaning) = with_exceptions(&catalogue, &TICKS, watched((0, 100), &packets));
    assert_eq!(
        lines(&trace, &meaning)[0],
        "interrupt from=@12 to=@25 depth=0 handler=TIM2 (TIM2_IRQHandler?)"
    );
}

#[test]
fn the_swo_replaces_the_adapter_where_it_covers_the_handler() {
    // The adapter traces TIM2's handler H at 0, at 120, and from 140 to the
    // end; the SWO covers from 100 to 200.
    let mut steps: Vec<(&str, &[u32])> = vec![("isr_enter", &[H, 16 + 45]), ("isr_exit", &[H])];
    steps.extend([("tick", &[] as &[u32]); 10]);
    steps.extend([
        ("isr_enter", &[H, 16 + 45] as &[u32]),
        ("isr_exit", &[H]),
        ("isr_enter", &[H, 16 + 45]),
    ]);
    let packets = [
        (0, RETURNED, 105),
        (61, ENTERED, 121),
        (61, EXITED, 129),
        (0, RETURNED, 129),
        (61, ENTERED, 141),
    ];
    let catalogue = catalogue();
    let (trace, meaning) = with_exceptions(&catalogue, &steps, watched((100, 200), &packets));
    assert_eq!(
        lines(&trace, &meaning),
        [
            "interrupt from=0 to=1 depth=0 handler=0x08000401 (TIM2)",
            "interrupt from=@121 to=@129 depth=0 handler=TIM2 (0x08000401)",
            "interrupt from=@141 to=lost depth=0 handler=TIM2 (0x08000401)",
        ]
    );
    assert_eq!(
        meaning.double_trace().as_deref(),
        Some(
            "the SWO traces the exceptions of handlers the adapter traces too, at a cost in slots and cycles: \
             0x08000401 (TIM2)"
        )
    );
    let report = stats::of_trace(&catalogue, &trace, stats::Range::default(), 100).unwrap();
    assert_eq!(report.warnings, [meaning.double_trace().unwrap()]);
}

#[test]
fn the_adapter_handler_has_no_period_across_the_swo_coverage() {
    // The adapter traces H at 0, 120 and 210; the SWO covers from 100 to
    // 200, where it replaces the span at 120.
    let mut steps: Vec<(&str, &[u32])> = vec![("isr_enter", &[H, 16 + 45]), ("isr_exit", &[H])];
    steps.extend([("tick", &[] as &[u32]); 10]);
    steps.extend([("isr_enter", &[H, 16 + 45] as &[u32]), ("isr_exit", &[H])]);
    steps.extend([("tick", &[] as &[u32]); 7]);
    steps.extend([("isr_enter", &[H, 16 + 45] as &[u32]), ("isr_exit", &[H])]);
    let packets = [
        (0, RETURNED, 105),
        (61, ENTERED, 121),
        (61, EXITED, 129),
        (0, RETURNED, 129),
    ];
    let catalogue = catalogue();
    let (trace, _) = with_exceptions(&catalogue, &steps, watched((100, 200), &packets));
    let report = stats::of_trace(&catalogue, &trace, stats::Range::default(), 100).unwrap();
    let names = names(&report);
    assert_eq!(
        group(&report, "handler 0x08000401 (TIM2) gross"),
        (2, 0, 0, 0, vec![10, 10])
    );
    assert!(
        !names.contains(&"handler 0x08000401 (TIM2) period".to_owned()),
        "{names:?}"
    );
}

#[test]
fn the_adapter_cost_is_the_swo_span_around_its_own() {
    // The adapter traces H at 0, 120 and 210; the SWO sees TIM2 from 115 to
    // 134 around the adapter's 120 to 130, and nothing outside 100 to 200.
    let mut steps: Vec<(&str, &[u32])> = vec![("isr_enter", &[H, 16 + 45]), ("isr_exit", &[H])];
    steps.extend([("tick", &[] as &[u32]); 10]);
    steps.extend([("isr_enter", &[H, 16 + 45] as &[u32]), ("isr_exit", &[H])]);
    steps.extend([("tick", &[] as &[u32]); 7]);
    steps.extend([("isr_enter", &[H, 16 + 45] as &[u32]), ("isr_exit", &[H])]);
    let packets = [
        (0, RETURNED, 105),
        (61, ENTERED, 115),
        (61, EXITED, 134),
        (0, RETURNED, 134),
    ];
    let catalogue = catalogue();
    let (trace, _) = with_exceptions(&catalogue, &steps, watched((100, 200), &packets));
    let report = stats::of_trace(&catalogue, &trace, stats::Range::default(), 100).unwrap();
    assert_eq!(
        group(&report, "handler TIM2 (0x08000401) adapter_entry"),
        (1, 0, 0, 0, vec![5])
    );
    assert_eq!(
        group(&report, "handler TIM2 (0x08000401) adapter_exit"),
        (1, 0, 0, 0, vec![4])
    );
    // A span of the SWO the adapter's run does not fit in gives no cost.
    let packets = [
        (0, RETURNED, 105),
        (61, ENTERED, 115),
        (61, EXITED, 128),
        (0, RETURNED, 128),
    ];
    let (trace, _) = with_exceptions(&catalogue, &steps, watched((100, 200), &packets));
    let report = stats::of_trace(&catalogue, &trace, stats::Range::default(), 100).unwrap();
    let names = names(&report);
    assert!(!names.iter().any(|name| name.contains("adapter_")), "{names:?}");
}

#[test]
fn a_loss_of_the_ring_hides_nothing_the_swo_dates() {
    // TIM2 runs across a loss of the ring, and at the end of the coverage.
    let steps: [(&str, &[u32]); 4] = [("tick", &[]), ("lost", &[]), ("tick", &[]), ("tick", &[])];
    let packets = [
        (0, RETURNED, 1),
        (61, ENTERED, 5),
        (61, EXITED, 25),
        (0, RETURNED, 25),
        (61, ENTERED, 90),
    ];
    let catalogue = catalogue();
    let (trace, _) = with_exceptions(&catalogue, &steps, watched((0, 100), &packets));
    let report = stats::of_trace(&catalogue, &trace, stats::Range::default(), 100).unwrap();
    let names = names(&report);
    let name = names
        .iter()
        .find(|name| name.contains("TIM2"))
        .unwrap_or_else(|| panic!("no TIM2 in {names:?}"));
    assert_eq!(group(&report, name), (1, 1, 0, 0, vec![20]));
}
