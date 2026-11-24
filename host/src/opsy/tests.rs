use super::*;
use crate::decode::Decoded;
use crate::events::{Event, Field as EventField, Type};
use crate::stats;
use crate::symbols::Symbols;
use gimli::DwAte;

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
                        "reason" => Kind::Enum(vec![(0, "signal".to_owned()), (1, "timeout".to_owned())]),
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
            "interrupt from=lost to=40 depth=unknown handler=0x08000401",
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

const SIGNAL: u32 = 0;
const TIMEOUT: u32 = 1;

/// The statistics of `steps` over `range`, after checking that every split
/// in two pieces gives the same instances, and that the JSON fits the schema.
fn report(steps: &[(&str, &[u32])], range: stats::Range) -> stats::Report {
    let catalogue = catalogue();
    let trace = trace(&catalogue, 0, steps);
    let mut whole = Opsy::new(&catalogue, 0).unwrap();
    whole.add(&trace.elements, 0);
    let instances = instances(&whole);
    for split in 1..trace.elements.len() {
        let mut pieces = Opsy::new(&catalogue, 0).unwrap();
        pieces.add(&trace.elements[..split], 0);
        pieces.add(&trace.elements, split);
        assert_eq!(super::instances(&pieces), instances, "split at {split}");
    }
    let report = stats::compute(&trace, &instances, &whole.contradictions, range, 100);
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
    ("task_ready", &[A, SIGNAL]),
    ("task_started", &[A]),
    ("isr_enter", &[H]),
    ("isr_enter", &[G]),
    ("isr_exit", &[G]),
    ("isr_exit", &[H]),
    ("task_ready", &[B, TIMEOUT]),
    ("task_started", &[B]),
    ("task_sleep", &[B]),
    ("task_started", &[A]),
    ("enter_systick", &[]),
    ("exit_systick", &[]),
    ("task_wait", &[A]),
    ("enter_idle", &[]),
];

#[test]
fn an_activation_splits_into_what_preempts_it() {
    let report = report(&PREEMPTED, stats::Range::default());
    let a = |metric| group(&report, &format!("task 0x20000100 {metric}"));
    // Ready at 10, runs 20 to 80 and 100 to 130, H from 30 to 60 with G
    // from 40 to 50 in it, the tick from 110 to 120.
    assert_eq!(a("wake_latency"), (1, 0, 0, 0, vec![10]));
    assert_eq!(a("wake_latency signal"), (1, 0, 0, 0, vec![10]));
    assert_eq!(a("gross"), (1, 0, 0, 0, vec![110]));
    assert_eq!(a("preempted_by_tasks"), (1, 0, 0, 0, vec![20]));
    assert_eq!(a("preempted_by_handlers"), (1, 0, 0, 0, vec![40]));
    assert_eq!(a("net"), (1, 0, 0, 0, vec![50]));
    assert_eq!(a("response"), (1, 0, 0, 0, vec![120]));
    let b = |metric| group(&report, &format!("task 0x20000200 {metric}"));
    assert_eq!(b("wake_latency timeout"), (1, 0, 0, 0, vec![10]));
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
        names(&report)[..8],
        [
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
fn an_instance_that_touches_a_loss_is_excluded() {
    let report = report(
        &[
            ("enter_idle", &[]),
            ("task_ready", &[A, SIGNAL]),
            ("task_started", &[A]),
            ("lost", &[]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("task_ready", &[A, SIGNAL]),
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
    // The loss hides 20 to 40.
    assert_eq!((report.losses, report.known), (1, 70));
    let text = stats::text(&report).join("\n");
    assert!(
        text.contains("task 0x20000100 response: 1, min 20 cycles at #8, p50 20 cycles, p90 ≥ 20 cycles"),
        "{text}"
    );
}

#[test]
fn an_instance_that_covers_an_overrun_write_is_excluded() {
    let report = report(
        &[
            ("enter_idle", &[]),
            ("task_ready", &[A, SIGNAL]),
            ("task_started", &[A]),
            ("suspect", &[]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("task_ready", &[A, SIGNAL]),
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
            ("task_ready", &[A, SIGNAL]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("task_ready", &[A, TIMEOUT]),
            ("task_started", &[A]),
            ("task_sleep", &[A]),
            ("enter_idle", &[]),
            ("tick", &[]),
            ("task_ready", &[A, SIGNAL]),
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
    assert_eq!(find("wake_latency", Some("signal")).count, 2);
    assert_eq!(find("wake_latency", Some("timeout")).count, 1);
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
            ("task_ready", &[A, SIGNAL]),
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
    let mut opsy = Opsy::new(&catalogue, 5).unwrap();
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
    let mut opsy = Opsy::new(&catalogue, 0).unwrap();
    opsy.add(&trace.elements, 0);
    let bin = |from, to, known, handlers, tasks| CpuBin {
        from,
        to,
        known,
        handlers,
        tasks,
    };
    // Known from 10 to the loss at 80, and from 120 on; handlers 20-50,
    // 70-80 and from 130; task A 60-80.
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
/// handler or a task other than idle does, the spans still open to the end.
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
        let mut opsy = Opsy::new(&catalogue, first).unwrap();
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
            idle_interrupted += usize::from(run == Some(None) && handler);
            hidden += usize::from(!known && handler);
        }
    }
    assert!(
        idle_interrupted > 1000 && hidden > 1000,
        "{idle_interrupted} cycles of idle interrupted, {hidden} of a handler while unknown"
    );
}
