use super::*;
use crate::decode::Decoded;
use crate::events::{Event, Field as EventField, Kind, Pointee, Type};
use crate::interrupts::Interrupts;
use crate::meaning::*;
use crate::model::{Element, Trace};
use crate::symbols::Symbols;
use gimli::DwAte;
use std::collections::BTreeMap;

/// The length of a thread's name, CONFIG_THREAD_MAX_NAME_LEN.
const NAME: u64 = 32;

/// The adapter's events, their fields 4-byte words but `name`.
const HOOKS: [(&str, &[&str]); 19] = [
    ("starting", &["core_clock", "idle"]),
    ("thread_create", &["thread", "priority", "name"]),
    ("thread_name_set", &["thread", "name"]),
    ("thread_sched_abort", &["thread"]),
    ("thread_switched_in", &["thread"]),
    ("thread_sched_priority_set", &["thread", "priority"]),
    ("thread_sched_ready", &["thread"]),
    ("thread_sleep_ticks_enter", &["ticks"]),
    ("sem_take_blocking", &["sem"]),
    ("msgq_get_blocking", &["msgq"]),
    ("msgq_put_blocking", &["msgq"]),
    ("event_wait_blocking", &["event"]),
    ("pipe_read_blocking", &["pipe"]),
    ("pipe_write_blocking", &["pipe"]),
    ("mutex_lock_blocking", &["mutex"]),
    ("mutex_taken", &["mutex"]),
    ("mutex_released", &["mutex"]),
    ("isr_enter", &["exception", "handler"]),
    ("isr_exit", &["exception"]),
];

fn word(name: &str) -> Type {
    Type {
        name: name.to_owned(),
        size: Some(4),
        kind: match name {
            "handler" => Kind::Pointer(Pointee::Function),
            "thread" | "idle" => Kind::Pointer(Pointee::Record("k_thread".to_owned())),
            "sem" | "msgq" | "event" | "pipe" | "mutex" => Kind::Pointer(Pointee::Record(format!("k_{name}"))),
            "priority" | "ticks" => Kind::Base(DwAte(gimli::DW_ATE_signed.0)),
            _ => Kind::Base(DwAte(gimli::DW_ATE_unsigned.0)),
        },
    }
}

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
                ty: if name == "name" {
                    Type {
                        name: name.to_owned(),
                        size: Some(NAME),
                        kind: Kind::Array(
                            Box::new(Type {
                                name: "char".to_owned(),
                                size: Some(1),
                                kind: Kind::Base(DwAte(gimli::DW_ATE_signed_char.0)),
                            }),
                            vec![NAME],
                        ),
                    }
                } else {
                    word(name)
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
/// on, 10 cycles apart. `<hook>:<name>` gives the event's `name` field.
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
        let wanted = format!("{PREFIX}{step}");
        let event = catalogue
            .events
            .values()
            .find(|event| event.name == wanted)
            .expect("a hook of the catalogue");
        // A field left out is 0.
        let mut payload: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
        payload.resize(event.size as usize, 0);
        if let Some(field) = event.fields.iter().find(|field| field.name == "name") {
            let at = field.offset as usize;
            payload[at..at + name.len()].copy_from_slice(name.as_bytes());
        }
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
        watched: None,
    }
}

/// The meaning of `steps` taken in at once.
fn meaning<'a>(catalogue: &'a Catalogue, trace: &Trace) -> Meaning<'a> {
    let mut meaning = Meaning::new(catalogue, 0).unwrap();
    meaning.add(&trace.elements, 0);
    meaning
}

/// The lines of `steps` taken in at once, after checking that every split
/// in two pieces gives the same lines.
fn spans(steps: &[(&str, &[u32])]) -> Vec<String> {
    let catalogue = catalogue();
    let trace = trace(&catalogue, steps);
    let lines = lines(&trace, &meaning(&catalogue, &trace));
    for split in 1..trace.elements.len() {
        let mut pieces = Meaning::new(&catalogue, 0).unwrap();
        pieces.add(&trace.elements[..split], 0);
        pieces.add(&trace.elements, split);
        assert_eq!(crate::meaning::lines(&trace, &pieces), lines, "split at {split}");
    }
    lines
}

/// The lines of `steps` that start with one of `kinds`.
fn only(kinds: &[&str], steps: &[(&str, &[u32])]) -> Vec<String> {
    spans(steps)
        .into_iter()
        .filter(|line| kinds.iter().any(|kind| line.starts_with(kind)))
        .collect()
}

const IDLE: u32 = 0x2000_0010;
const A: u32 = 0x2000_0100;
const B: u32 = 0x2000_0200;
const S: u32 = 0x2000_0300;
const M: u32 = 0x2000_0400;
const Q: u32 = 0x2000_0500;
const H: u32 = 0x0800_0401;
const SYSTICK: u32 = 15;
const TIM2: u32 = 16 + 45;
/// -1, a cooperative priority.
const COOPERATIVE: u32 = -1_i32 as u32;

/// The kernel's start, at PRE_KERNEL_1: the idle thread, then A and B
/// created at priorities 3 and 2, made ready, A on the core first. Indexes 0
/// to 7.
const START: [(&str, &[u32]); 8] = [
    ("starting", &[25_000_000, IDLE]),
    ("thread_create:main", &[A, 3]),
    ("thread_create:idle", &[IDLE, 15]),
    ("thread_sched_ready", &[A]),
    ("thread_switched_in", &[A]),
    ("thread_create:pong", &[B, 2]),
    ("thread_sched_ready", &[B]),
    ("thread_switched_in", &[B]),
];

fn started(rest: &[(&'static str, &'static [u32])]) -> Vec<(&'static str, &'static [u32])> {
    let mut steps = START.to_vec();
    steps.extend_from_slice(rest);
    steps
}

/// The wakes of `steps` but the threads' creation.
fn wakes(steps: &[(&str, &[u32])]) -> Vec<String> {
    only(&["wake"], steps)
        .into_iter()
        .filter(|line| !line.contains("reason=created"))
        .collect()
}

#[test]
fn the_idle_thread_is_no_task() {
    // Mutant check: the idle thread, made ready at the kernel's init, would
    // be woken as a task.
    let mut steps = START[..3].to_vec();
    steps.push(("thread_sched_ready", &[IDLE]));
    steps.extend_from_slice(&START[3..]);
    steps.extend_from_slice(&[("thread_sleep_ticks_enter", &[1]), ("thread_switched_in", &[IDLE])]);
    let lines = spans(&steps);
    assert!(lines.contains(&"run from=10 to=open task=idle".to_owned()), "{lines:?}");
    assert!(lines.iter().all(|line| !line.contains("task=idle ")), "{lines:?}");
}

#[test]
fn a_cooperative_priority_is_negative() {
    let steps = started(&[
        ("thread_create:coop", &[S, COOPERATIVE]),
        ("thread_sched_priority_set", &[A, -2_i32 as u32]),
    ]);
    let catalogue = catalogue();
    let trace = trace(&catalogue, &steps);
    let meaning = meaning(&catalogue, &trace);
    assert_eq!(meaning.precedence, Precedence::LowerIsHigher);
    let priority = |name: &str| {
        let task = meaning.tasks.list.iter().find(|task| task.name == name).unwrap();
        task.priority_at(1000)
    };
    assert_eq!(priority("coop"), Some(-1));
    assert_eq!(priority("main"), Some(-2));
    assert!(Precedence::LowerIsHigher.rank(-1) < Precedence::LowerIsHigher.rank(3));
}

#[test]
fn a_thread_named_after_its_creation_takes_the_name() {
    let lines = only(
        &["run"],
        &started(&[
            ("thread_create", &[S, 4]),
            ("thread_name_set:receiver", &[S]),
            ("thread_name_set", &[S]),
            ("thread_sched_ready", &[S]),
            ("thread_sleep_ticks_enter", &[1]),
            ("thread_switched_in", &[S]),
        ]),
    );
    assert_eq!(lines.last().unwrap(), "run from=13 to=open task=receiver");
}

#[test]
fn a_sleep_of_no_tick_is_a_yield() {
    // pong yields to main and comes back: it was ready throughout.
    let steps = started(&[
        ("thread_sleep_ticks_enter", &[0]),
        ("thread_switched_in", &[A]),
        ("thread_sleep_ticks_enter", &[1]),
        ("thread_switched_in", &[B]),
    ]);
    assert_eq!(wakes(&steps), Vec::<String>::new());
    let lines = spans(&steps);
    assert!(lines.contains(&"ready from=9 to=11 task=pong".to_owned()), "{lines:?}");
}

#[test]
fn a_block_left_for_a_handed_message_is_no_block() {
    // main blocks putting to Q, full; pong then reports blocking on Q, and
    // the put of main's message that wakes main hands pong a message: pong
    // goes on without switching.
    let steps = started(&[
        ("thread_sleep_ticks_enter", &[1]),
        ("thread_switched_in", &[A]),
        ("msgq_put_blocking", &[Q]),
        ("isr_enter", &[SYSTICK, H]),
        ("thread_sched_ready", &[B]),
        ("isr_exit", &[SYSTICK]),
        ("thread_switched_in", &[B]),
        ("msgq_get_blocking", &[Q]),
        ("thread_sched_ready", &[A]),
        ("thread_sleep_ticks_enter", &[1]),
        ("thread_switched_in", &[A]),
        ("isr_enter", &[SYSTICK, H]),
        ("thread_sched_ready", &[B]),
        ("isr_exit", &[SYSTICK]),
    ]);
    let lines = spans(&steps);
    assert!(lines.iter().all(|line| !line.starts_with("contradiction")), "{lines:?}");
    assert_eq!(
        wakes(&steps),
        [
            "wake at=12 task=pong reason=sleep cause=elapsed",
            "wake at=16 task=main reason=msgq_put cause=task:pong",
            "wake at=20 task=pong reason=sleep cause=elapsed",
        ]
    );
}

#[test]
fn a_block_ended_before_the_switch_is_made() {
    // pong sleeps; SysTick, pending, runs before the switch and wakes it:
    // the core stays on pong.
    let steps = started(&[
        ("thread_sleep_ticks_enter", &[1]),
        ("isr_enter", &[SYSTICK, H]),
        ("thread_sched_ready", &[B]),
        ("isr_exit", &[SYSTICK]),
        ("thread_switched_in", &[B]),
    ]);
    let lines = spans(&steps);
    assert!(lines.iter().all(|line| !line.starts_with("contradiction")), "{lines:?}");
    assert_eq!(wakes(&steps), ["wake at=10 task=pong reason=sleep cause=elapsed"]);
}

#[test]
fn a_block_holds_through_an_inheritance_and_a_handler() {
    // pong blocks on M, held by main: main inherits pong's priority, a
    // handler runs, then the core goes to main.
    let mut steps = START[..5].to_vec();
    steps.push(("mutex_taken", &[M]));
    steps.extend_from_slice(&START[5..]);
    steps.extend_from_slice(&[
        ("mutex_lock_blocking", &[M]),
        ("thread_sched_priority_set", &[A, 2]),
        ("isr_enter", &[TIM2, H]),
        ("isr_exit", &[TIM2]),
        ("thread_switched_in", &[A]),
        ("mutex_released", &[M]),
        ("thread_sched_ready", &[B]),
    ]);
    let lines = spans(&steps);
    assert!(lines.iter().all(|line| !line.starts_with("contradiction")), "{lines:?}");
    assert!(
        lines.contains(&"wait from=9 to=15 mutex=0x20000400 task=pong".to_owned()),
        "{lines:?}"
    );
    assert_eq!(wakes(&steps), ["wake at=15 task=pong reason=mutex cause=task:main"]);
}

#[test]
fn a_block_holds_through_a_handler_that_readies_another() {
    // pong blocks on S; a handler readies main, then the core goes to main.
    let steps = started(&[
        ("thread_sleep_ticks_enter", &[1]),
        ("thread_switched_in", &[IDLE]),
        ("isr_enter", &[TIM2, H]),
        ("thread_sched_ready", &[B]),
        ("isr_exit", &[TIM2]),
        ("thread_switched_in", &[B]),
        ("sem_take_blocking", &[S]),
        ("isr_enter", &[TIM2, H]),
        ("thread_sched_ready", &[A]),
        ("isr_exit", &[TIM2]),
        ("thread_switched_in", &[A]),
        ("thread_sched_ready", &[B]),
    ]);
    assert_eq!(
        wakes(&steps),
        [
            "wake at=11 task=pong reason=sleep cause=handler:0x08000401 (TIM2)",
            "wake at=19 task=pong reason=sem_take cause=task:main",
        ]
    );
}

#[test]
fn a_wake_in_systick_is_elapsed() {
    let steps = started(&[
        ("thread_sleep_ticks_enter", &[5]),
        ("thread_switched_in", &[IDLE]),
        ("isr_enter", &[SYSTICK, H]),
        ("thread_sched_ready", &[B]),
        ("isr_exit", &[SYSTICK]),
    ]);
    assert_eq!(wakes(&steps), ["wake at=11 task=pong reason=sleep cause=elapsed"]);
}

#[test]
fn a_loss_forgets_a_reported_block() {
    let steps = started(&[
        ("sem_take_blocking", &[S]),
        ("lost", &[]),
        ("thread_switched_in", &[A]),
        ("thread_sched_ready", &[B]),
    ]);
    assert_eq!(wakes(&steps), ["wake at=13 task=pong reason=unknown cause=task:main"]);
}

#[test]
fn an_image_with_three_adapters_is_refused() {
    let mut catalogue = catalogue();
    for (id, name) in [
        (100, "kymo::opsy::task_started"),
        (101, "kymo::freertos::task_switched_in"),
    ] {
        let mut event = catalogue.events[&5].clone();
        event.id = id;
        event.name = name.to_owned();
        catalogue.events.insert(id, event);
    }
    let error = Meaning::new(&catalogue, 0).err().expect("refused").to_string();
    assert_eq!(
        error,
        "the image has the adapter events of OpSy's and FreeRTOS's and Zephyr's"
    );
}
