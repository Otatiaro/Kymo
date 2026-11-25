use super::*;
use crate::decode::Decoded;
use crate::events::{Event, Field as EventField, Kind, Pointee, Type};
use crate::interrupts::Interrupts;
use crate::meaning::*;
use crate::model::{Element, Trace};
use crate::symbols::Symbols;
use gimli::DwAte;
use std::collections::BTreeMap;

/// The length of a task's name, configMAX_TASK_NAME_LEN.
const NAME: u64 = 16;

/// The adapter's events, their fields 4-byte words but `name`.
const HOOKS: [(&str, &[&str]); 23] = [
    ("starting", &["core_clock", "idle"]),
    ("task_create", &["task", "priority", "name"]),
    ("task_delete", &["task"]),
    ("task_switched_in", &["task"]),
    ("task_priority_set", &["task", "priority"]),
    ("task_priority_inherit", &["task", "priority"]),
    ("task_priority_disinherit", &["task", "priority"]),
    ("moved_task_to_ready_state", &["task"]),
    ("task_delay", &["ticks"]),
    ("task_delay_until", &["time"]),
    ("task_suspend", &["task"]),
    ("task_resume", &["task"]),
    ("blocking_on_queue_receive", &["queue"]),
    ("blocking_on_stream_buffer_receive", &["stream"]),
    ("task_notify_wait_block", &["index"]),
    ("queue_send", &["queue"]),
    ("task_notify", &["task"]),
    ("mutex_taken", &["mutex"]),
    ("mutex_released", &["mutex"]),
    ("mutex_blocked", &["mutex"]),
    ("isr_enter", &["exception", "handler"]),
    ("isr_exit", &["exception"]),
    ("event_group_sync_block", &["group"]),
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
                    size: Some(if name == "name" { NAME } else { 4 }),
                    kind: match name {
                        "name" => Kind::Array(
                            Box::new(Type {
                                name: "char".to_owned(),
                                size: Some(1),
                                kind: Kind::Base(DwAte(gimli::DW_ATE_signed_char.0)),
                            }),
                            vec![NAME],
                        ),
                        "handler" => Kind::Pointer(Pointee::Function),
                        "task" | "idle" | "queue" | "mutex" | "stream" | "group" => {
                            Kind::Pointer(Pointee::Record(format!("{name}_t")))
                        }
                        _ => Kind::Base(DwAte(gimli::DW_ATE_unsigned.0)),
                    },
                },
            })
            .collect();
        let size = fields
            .iter()
            .map(|field| field.offset + field.ty.size.unwrap())
            .max()
            .unwrap_or(0);
        events.insert(
            id,
            Event {
                id,
                name: format!("{PREFIX}{hook}"),
                size,
                fields,
                text: None,
            },
        );
    }
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

/// A trace of `steps`: `"lost"`, or a hook and its field values, at index 0
/// on, 10 cycles apart. `task_create:<name>` creates a task of that name.
fn trace<'a>(catalogue: &'a Catalogue, steps: &[(&str, &[u32])]) -> Trace<'a> {
    let mut elements = Vec::new();
    let mut index = 0;
    for (n, &(step, values)) in steps.iter().enumerate() {
        if step == "lost" {
            elements.push(Element::Lost {
                from: index,
                to: index + 3,
            });
            index += 3;
            continue;
        }
        let (step, name) = step.split_once(':').unwrap_or((step, ""));
        let event = catalogue
            .events
            .values()
            .find(|event| event.name == format!("{PREFIX}{step}"))
            .expect("a hook of the catalogue");
        // A field left out is 0.
        let mut payload: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
        if step == "task_create" {
            payload.resize(8, 0);
            payload.extend(name.bytes());
        }
        payload.resize(event.size as usize, 0);
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
        first: 0,
        elements,
        end: Some(index),
        cut: None,
        frequency: None,
        measured: None,
    }
}

/// The lines of `steps` taken in at once, after checking that every split
/// in two pieces gives the same lines.
fn spans(steps: &[(&str, &[u32])]) -> Vec<String> {
    let catalogue = catalogue();
    let trace = trace(&catalogue, steps);
    let mut whole = Meaning::new(&catalogue, 0).unwrap();
    whole.add(&trace.elements, 0);
    let lines = lines(&trace, &whole);
    for split in 1..trace.elements.len() {
        let mut pieces = Meaning::new(&catalogue, 0).unwrap();
        pieces.add(&trace.elements[..split], 0);
        pieces.add(&trace.elements, split);
        assert_eq!(crate::meaning::lines(&trace, &pieces), lines, "split at {split}");
    }
    lines
}

/// The lines of `steps` that start with `kind`.
fn only(kind: &str, steps: &[(&str, &[u32])]) -> Vec<String> {
    spans(steps).into_iter().filter(|line| line.starts_with(kind)).collect()
}

const IDLE: u32 = 0x2000_0010;
const A: u32 = 0x2000_0100;
const B: u32 = 0x2000_0200;
const Q: u32 = 0x2000_0300;
const M: u32 = 0x2000_0400;
const H: u32 = 0x0800_0401;
const SYSTICK: u32 = 15;
const TIM2: u32 = 16 + 45;

/// The scheduler's start: A then B created at priorities 2 and 1, the idle
/// task, A on the core first. Indexes 0 to 6.
const START: [(&str, &[u32]); 7] = [
    ("task_create:PING", &[A, 2]),
    ("moved_task_to_ready_state", &[A]),
    ("task_create:PONG", &[B, 1]),
    ("moved_task_to_ready_state", &[B]),
    ("task_create:IDLE", &[IDLE, 0]),
    ("moved_task_to_ready_state", &[IDLE]),
    ("task_switched_in", &[A]),
];

fn started(rest: &[(&'static str, &'static [u32])]) -> Vec<(&'static str, &'static [u32])> {
    let mut steps = START.to_vec();
    steps.push(("starting", &[25_000_000, IDLE]));
    steps.extend_from_slice(rest);
    steps
}

#[test]
fn the_idle_task_is_no_task() {
    assert_eq!(
        spans(&started(&[
            ("task_delay", &[10]),
            ("task_switched_in", &[B]),
            ("task_delay", &[10]),
            ("task_switched_in", &[IDLE]),
        ])),
        [
            "run from=6 to=9 task=PING",
            "run from=9 to=11 task=PONG",
            "run from=11 to=open task=idle",
            "wake at=1 task=PING reason=created cause=unknown",
            "wake at=3 task=PONG reason=created cause=unknown",
            "ready from=1 to=6 task=PING",
            "ready from=3 to=9 task=PONG",
        ]
    );
}

#[test]
fn a_recording_without_the_start_counts_the_idle_task() {
    // Mutant check: without the start, the idle task is a task.
    let steps = [
        ("task_create:IDLE", &[IDLE, 0][..]),
        ("lost", &[]),
        ("task_switched_in", &[IDLE]),
    ];
    assert_eq!(spans(&steps), ["run from=4 to=open task=IDLE"]);
}

#[test]
fn a_wake_is_by_the_wait_it_ends() {
    assert_eq!(
        only(
            "wake",
            &started(&[
                // 8: PING waits on Q; PONG runs and sends to it.
                ("blocking_on_queue_receive", &[Q]),
                ("task_switched_in", &[B]),
                ("queue_send", &[Q]),
                ("moved_task_to_ready_state", &[A]),
                ("task_switched_in", &[A]),
                // 13: PING delays; SysTick wakes it.
                ("task_delay", &[5]),
                ("task_switched_in", &[B]),
                ("isr_enter", &[SYSTICK, H]),
                ("moved_task_to_ready_state", &[A]),
                ("isr_exit", &[SYSTICK]),
                ("task_switched_in", &[A]),
                // 19: PING waits on a stream buffer, through its notification.
                ("blocking_on_stream_buffer_receive", &[Q]),
                ("task_notify_wait_block", &[0]),
                ("task_switched_in", &[B]),
                ("isr_enter", &[TIM2, H]),
                ("task_notify", &[A]),
                ("moved_task_to_ready_state", &[A]),
                ("isr_exit", &[TIM2]),
            ])
        ),
        [
            "wake at=1 task=PING reason=created cause=unknown",
            "wake at=3 task=PONG reason=created cause=unknown",
            "wake at=11 task=PING reason=queue_receive cause=task:PONG",
            "wake at=16 task=PING reason=delay cause=elapsed",
            "wake at=24 task=PING reason=stream_buffer_receive cause=handler:0x08000401 (TIM2)",
        ]
    );
}

#[test]
fn a_loss_forgets_the_waits() {
    assert_eq!(
        only(
            "wake",
            &started(&[
                ("task_delay", &[5]),
                ("lost", &[]),
                ("moved_task_to_ready_state", &[A]),
                ("task_switched_in", &[A]),
                ("task_delay", &[5]),
                ("task_switched_in", &[B]),
                ("moved_task_to_ready_state", &[A]),
            ])
        ),
        [
            "wake at=1 task=PING reason=created cause=unknown",
            "wake at=3 task=PONG reason=created cause=unknown",
            "wake at=12 task=PING reason=unknown cause=unknown",
            "wake at=16 task=PING reason=delay cause=task:PONG",
        ]
    );
}

#[test]
fn a_suspension_is_the_wait_whatever_the_task_waited_for() {
    assert_eq!(
        only(
            "wake",
            &started(&[
                ("task_delay", &[5]),
                ("task_switched_in", &[B]),
                ("task_suspend", &[A]),
                ("task_resume", &[A]),
                ("moved_task_to_ready_state", &[A]),
            ])
        ),
        [
            "wake at=1 task=PING reason=created cause=unknown",
            "wake at=3 task=PONG reason=created cause=unknown",
            "wake at=12 task=PING reason=suspend cause=task:PONG",
        ]
    );
}

#[test]
fn a_task_moved_within_the_ready_list_is_not_woken() {
    // PONG, ready, raised above PING; then PING, on the core, lowered.
    let steps = started(&[
        ("task_priority_set", &[B, 3]),
        ("moved_task_to_ready_state", &[B]),
        ("task_priority_set", &[A, 0]),
        ("moved_task_to_ready_state", &[A]),
    ]);
    let lines = spans(&steps);
    assert!(lines.iter().all(|line| !line.starts_with("contradiction")), "{lines:?}");
    assert_eq!(lines.iter().filter(|line| line.starts_with("wake")).count(), 2);
}

#[test]
fn a_task_made_ready_twice_is_a_contradiction() {
    // The move that follows another hook than a priority change, and is
    // followed by another hook than its task's priority change.
    let steps = started(&[
        ("task_priority_set", &[B, 3]),
        ("isr_enter", &[TIM2, H]),
        ("moved_task_to_ready_state", &[B]),
        ("moved_task_to_ready_state", &[A]),
        ("task_priority_inherit", &[B, 5]),
    ]);
    assert_eq!(only("contradiction", &steps), ["contradiction 10", "contradiction 11"]);
}

#[test]
fn a_ready_task_moved_before_it_inherits_is_not_woken() {
    // xTaskPriorityInherit moves the holder, ready, then emits the hook;
    // the disinherit emits the hook, then moves it.
    let steps = started(&[
        ("moved_task_to_ready_state", &[B]),
        ("task_priority_inherit", &[B, 5]),
        ("task_priority_disinherit", &[B, 4]),
        ("moved_task_to_ready_state", &[B]),
        ("task_switched_in", &[B]),
    ]);
    let lines = spans(&steps);
    assert!(lines.iter().all(|line| !line.starts_with("contradiction")), "{lines:?}");
    assert_eq!(lines.iter().filter(|line| line.starts_with("wake")).count(), 2);
}

#[test]
fn a_waiting_task_whose_priority_changes_is_woken_after() {
    // Mutant check: PING, delayed, raised; its wake is no move within the
    // ready list.
    assert_eq!(
        only(
            "wake",
            &started(&[
                ("task_delay", &[5]),
                ("task_switched_in", &[B]),
                ("task_priority_set", &[A, 3]),
                ("task_resume", &[A]),
                ("moved_task_to_ready_state", &[A]),
            ])
        ),
        [
            "wake at=1 task=PING reason=created cause=unknown",
            "wake at=3 task=PONG reason=created cause=unknown",
            "wake at=12 task=PING reason=delay cause=task:PONG",
        ]
    );
}

/// Each task's priority over the trace, as `(cycles, priority)` at each of
/// `cycles`.
fn priorities(steps: &[(&str, &[u32])], task: usize, cycles: &[i64]) -> Vec<Option<u8>> {
    let catalogue = catalogue();
    let trace = trace(&catalogue, steps);
    let mut meaning = Meaning::new(&catalogue, 0).unwrap();
    meaning.add(&trace.elements, 0);
    assert_eq!(meaning.precedence, Precedence::HigherIsHigher);
    cycles
        .iter()
        .map(|&at| meaning.tasks.list[task].priority_at(at))
        .collect()
}

#[test]
fn an_inherited_priority_holds_against_a_lower_base() {
    // At cycles 0, 10, ...: PING created at 2, inherits 4, its base set to 3
    // then 5, it disinherits to 5; PONG's base set after a loss.
    let steps = [
        ("task_create:PING", &[A, 2][..]),
        ("task_create:PONG", &[B, 1]),
        ("task_priority_inherit", &[A, 4]),
        ("task_priority_set", &[A, 3]),
        ("task_priority_set", &[A, 5]),
        ("task_priority_disinherit", &[A, 5]),
        ("lost", &[]),
        ("task_priority_set", &[B, 1]),
        ("task_priority_set", &[A, 6]),
    ];
    // The loss is at 50, the disinheritance's cycles.
    assert_eq!(
        priorities(&steps, 0, &[0, 20, 30, 40, 45, 80]),
        [Some(2), Some(4), Some(4), Some(5), Some(5), None]
    );
    assert_eq!(priorities(&steps, 1, &[10, 70]), [Some(1), None]);
}

#[test]
fn a_higher_base_after_a_loss_is_known() {
    // After a loss, a base set above the priority is the priority, and the
    // base from then: the next base set is the priority too, as in
    // vTaskPrioritySet. One set below it would leave it unknown: the task
    // may have inherited it or not.
    let steps = [
        ("task_create:PING", &[A, 2][..]),
        ("task_create:PONG", &[B, 1]),
        ("lost", &[]),
        ("task_priority_inherit", &[A, 4]),
        ("task_priority_set", &[A, 5]),
        ("task_priority_set", &[A, 3]),
        ("task_priority_inherit", &[B, 4]),
        ("task_priority_set", &[B, 3]),
    ];
    assert_eq!(priorities(&steps, 0, &[30, 40, 50]), [Some(4), Some(5), Some(3)]);
    assert_eq!(priorities(&steps, 1, &[60, 70]), [Some(4), None]);
}

#[test]
fn the_priorities_are_higher_the_higher() {
    assert!(Precedence::HigherIsHigher.rank(5) < Precedence::HigherIsHigher.rank(1));
    assert!(Precedence::LowerIsHigher.rank(1) < Precedence::LowerIsHigher.rank(5));
}

#[test]
fn a_mutex_is_the_running_task_s() {
    assert_eq!(
        spans(&started(&[
            // 8: PING takes M, PONG blocks on it, PING gives it.
            ("mutex_taken", &[M]),
            ("task_delay", &[1]),
            ("task_switched_in", &[B]),
            ("mutex_blocked", &[M]),
            ("task_priority_inherit", &[A, 2]),
            ("task_switched_in", &[IDLE]),
            ("moved_task_to_ready_state", &[A]),
            ("task_switched_in", &[A]),
            ("mutex_released", &[M]),
            ("moved_task_to_ready_state", &[B]),
            ("task_priority_disinherit", &[A, 2]),
            ("moved_task_to_ready_state", &[A]),
            ("task_switched_in", &[B]),
            ("mutex_taken", &[M]),
        ]))
        .into_iter()
        .filter(|line| line.starts_with("hold") || line.starts_with("wait") || line.starts_with("wake at=1"))
        .collect::<Vec<_>>(),
        [
            "hold from=8 to=16 mutex=0x20000400 task=PING",
            "wait from=11 to=17 mutex=0x20000400 task=PONG",
            "hold from=21 to=open mutex=0x20000400 task=PONG",
            "wake at=1 task=PING reason=created cause=unknown",
            "wake at=14 task=PING reason=delay cause=task:idle",
            "wake at=17 task=PONG reason=mutex cause=task:PING",
        ]
    );
}

#[test]
fn a_mutex_after_a_loss_waits_for_the_running_task() {
    // Mutant check: the task on the core is unknown from the loss to the
    // switch, so the take at 11 is no one's; the release at 13 is PING's.
    let lines = spans(&started(&[
        ("lost", &[]),
        ("mutex_taken", &[M]),
        ("task_switched_in", &[A]),
        ("mutex_released", &[M]),
    ]));
    let holds: Vec<_> = lines.iter().filter(|line| line.starts_with("hold")).collect();
    assert_eq!(holds, ["hold from=lost to=13 mutex=0x20000400 task=PING"]);
}

#[test]
fn a_handler_exits_by_its_exception() {
    assert_eq!(
        only(
            "interrupt",
            &started(&[
                ("isr_enter", &[TIM2, H]),
                ("isr_enter", &[SYSTICK, H]),
                ("isr_exit", &[SYSTICK]),
                ("isr_exit", &[TIM2]),
                ("lost", &[]),
                ("isr_exit", &[TIM2]),
                ("isr_exit", &[17]),
            ])
        ),
        [
            "interrupt from=9 to=10 depth=1 handler=0x08000401 (SysTick)",
            "interrupt from=8 to=11 depth=0 handler=0x08000401 (TIM2)",
            "interrupt from=lost to=15 depth=unknown handler=0x08000401 (TIM2)",
        ]
    );
}

#[test]
fn an_exit_of_no_handler_entered_is_a_contradiction() {
    assert_eq!(
        only("contradiction", &started(&[("isr_exit", &[TIM2])])),
        ["contradiction 8"]
    );
}

#[test]
fn a_task_created_where_one_was_deleted_is_another() {
    let lines = only(
        "wake",
        &started(&[
            ("task_delete", &[B]),
            ("task_create:EPHEMERAL", &[B, 1]),
            ("moved_task_to_ready_state", &[B]),
        ]),
    );
    assert_eq!(
        lines,
        [
            "wake at=1 task=PING reason=created cause=unknown",
            "wake at=3 task=PONG reason=created cause=unknown",
            "wake at=10 task=EPHEMERAL reason=created cause=task:PING",
        ]
    );
}

#[test]
fn an_image_with_both_adapters_is_refused() {
    let mut catalogue = catalogue();
    let mut opsy = catalogue.events[&1].clone();
    opsy.id = 100;
    opsy.name = "kymo::opsy::task_started".to_owned();
    catalogue.events.insert(100, opsy);
    let error = Meaning::new(&catalogue, 0).err().expect("refused").to_string();
    assert_eq!(error, "the image has both OpSy's and FreeRTOS's adapter events");
}
