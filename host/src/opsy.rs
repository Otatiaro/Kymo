//! OpSy's meaning of a trace: which task holds the core, which interrupt
//! handlers run, which mutexes are held and waited for, as spans between the
//! events of the OpSy adapter (target/opsy/include/kymo/opsy/hooks.hpp). The
//! adapter's events are known by their names, `kymo::opsy::<hook>`, and their
//! fields by name: this module changes with the adapter.
//!
//! Elements are taken in the trace's order and only added to, so the spans
//! grow with a recording. A span still open is not among the closed ones:
//! `open` gives it, ended at the last event.
//!
//! A loss hides what happened during it. The spans open when it comes end at
//! it, and the task on the core is unknown until the next task_started or
//! enter_idle. An interrupt exit, or a mutex release, with no entry or
//! taking seen since a loss starts its span at the loss. Which handlers are
//! open is unknown from a loss to the next task_started or enter_idle, which
//! OpSy emits with none open: a handler that runs meanwhile has no depth. A
//! recording that starts after the target's first event starts with such a
//! loss.
//!
//! An event that contradicts the adapter's order makes no span: an interrupt
//! exit that is not the last handler entered, a mutex taken while held,
//! released by another task than its owner or never taken, a task blocked
//! twice on the same mutex, a task made ready twice without becoming not
//! ready in between. Its element is listed among the contradictions.
//!
//! Besides spans, a task's activations: from its task_ready to the event
//! that makes it not ready again (task_sleep, task_wait, task_wait_timeout,
//! mutex_blocked, task_terminated). A task ready when the trace starts has no
//! activation before its first task_ready. A loss ends the activations open;
//! one that ends after a loss with no task_ready seen since, and none open
//! at it, starts at the loss.
//!
//! A task is ready, within an activation, while it does not hold the core:
//! from its task_ready to its first run, then each time it is preempted.
//!
//! Each task_ready has a cause, the context it is emitted in: the innermost
//! handler open but the kernel's, or else the task on the core; unknown from
//! a loss to the next task_started or enter_idle. A wake by a delay that ran
//! out is emitted in SysTick, whatever it interrupts: its cause is the delay.
//! A handler the adapter does not trace hides in the task it interrupts.
//!
//! The kernel's SysTick and service call handlers, when the kernel family is
//! recorded, run as handlers named `systick` and `service_call`. PendSV has
//! no exit event: it makes no span, and the task it puts on the core runs
//! from the task_started it emits.

use crate::events::{Catalogue, Kind, Pointee};
use crate::load::{Covered, Cpu};
pub use crate::model::Edge;
use crate::model::{Element, Trace};
use crate::stats::{Instance, Metric};
use crate::symbols::Symbols;
use crate::target::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const PREFIX: &str = "kymo::opsy::";
/// OpSy's wake reasons for a delay that ran out, given in SysTick.
const ELAPSED: [&str; 2] = ["timed_out", "sleep_elapsed"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: Edge,
    pub to: Edge,
}

/// A task on the core, or the idle task for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub task: Option<usize>,
    pub span: Span,
}

/// A handler running, `depth` handlers below it: `None` where a loss hides
/// which handlers were open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interrupt {
    pub handler: usize,
    pub depth: Option<usize>,
    pub span: Span,
}

/// A mutex held by a task, or waited for by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hold {
    pub mutex: usize,
    pub task: usize,
    pub waiting: bool,
    pub span: Span,
}

/// What made a task ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The innermost handler open, the kernel's left out.
    Handler(usize),
    /// The task on the core, or the idle task for none.
    Task(Option<usize>),
    /// A delay that ran out.
    Elapsed,
    /// Hidden by a loss.
    Unknown,
}

/// A task's activation: from its task_ready, by `reason` and `cause`, to the
/// event that makes it not ready. Started at a loss, it has neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub task: usize,
    pub reason: Option<String>,
    pub cause: Option<Cause>,
    pub span: Span,
}

/// A task_ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wake {
    pub task: usize,
    pub reason: String,
    pub cause: Cause,
    pub edge: Edge,
}

/// A task ready and not on the core, within an activation; `open` when it
/// ends with an activation still open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ready {
    pub task: usize,
    pub span: Span,
    pub open: bool,
    /// The activation it is part of.
    pub activation: Span,
    /// The task_ready that starts the activation, in `Opsy::wakes`; none for
    /// an activation that starts at a loss.
    pub wake: Option<usize>,
}

/// Spans, each list in the order its spans closed.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Spans {
    pub runs: Vec<Run>,
    pub interrupts: Vec<Interrupt>,
    pub holds: Vec<Hold>,
}

/// A task, a handler or a mutex, by the address the events give, named from
/// the image or else by its address.
pub struct Subject {
    pub address: u64,
    pub name: String,
    /// A task's priority, the last one an event gave: lower is higher.
    pub priority: Option<u8>,
    /// A task's priority over the trace, each from its edge on: `None` where
    /// it is unknown, from a loss to the next event that gives it.
    pub priorities: Vec<(Edge, Option<u8>)>,
}

impl Subject {
    /// A task's priority at `cycles`: `None` before the first event that
    /// gives it, or since a loss.
    pub fn priority_at(&self, cycles: i64) -> Option<u8> {
        let after = self.priorities.partition_point(|(edge, _)| edge.cycles <= cycles);
        after.checked_sub(1).and_then(|last| self.priorities[last].1)
    }
}

#[derive(Default)]
pub struct Subjects {
    pub list: Vec<Subject>,
    by_address: HashMap<u64, usize>,
    by_name: HashMap<&'static str, usize>,
}

impl Subjects {
    fn index(&mut self, symbols: &Symbols, field: &Field, payload: &[u8]) -> usize {
        let address = field.read(payload);
        *self.by_address.entry(address).or_insert_with(|| {
            let name = field
                .pointee
                .as_ref()
                .and_then(|pointee| symbols.name(pointee, address))
                .unwrap_or_else(|| format!("0x{address:08x}"));
            self.list.push(Subject {
                address,
                name,
                priority: None,
                priorities: Vec::new(),
            });
            self.list.len() - 1
        })
    }

    /// Whether `subject` is one no event field gives.
    fn is_named(&self, subject: usize) -> bool {
        self.by_name.values().any(|&named| named == subject)
    }

    /// A subject no event field gives, by name; its address is 0.
    fn named(&mut self, name: &'static str) -> usize {
        *self.by_name.entry(name).or_insert_with(|| {
            self.list.push(Subject {
                address: 0,
                name: name.to_owned(),
                priority: None,
                priorities: Vec::new(),
            });
            self.list.len() - 1
        })
    }
}

/// A field of an event: where its value is in the payload, what it points
/// at when it is a pointer, and its enumerators when it is an enumeration.
#[derive(Clone)]
struct Field {
    offset: usize,
    size: usize,
    pointee: Option<Pointee>,
    enumerators: Vec<(i64, String)>,
}

impl Field {
    fn read(&self, payload: &[u8]) -> u64 {
        payload[self.offset..self.offset + self.size]
            .iter()
            .rev()
            .fold(0, |value, byte| value << 8 | u64::from(*byte))
    }

    /// The enumerator of the value, or else the value.
    fn label(&self, payload: &[u8]) -> String {
        let value = self.read(payload);
        self.enumerators
            .iter()
            .find(|(enumerator, _)| *enumerator as u64 == value)
            .map_or_else(|| value.to_string(), |(_, label)| label.clone())
    }
}

/// The adapter's events the spans are made of.
enum Hook {
    TaskAdded {
        task: Field,
        priority: Field,
    },
    TaskPriorityChanged {
        task: Field,
        priority: Field,
    },
    TaskTerminated {
        task: Field,
    },
    TaskStarted {
        task: Field,
    },
    EnterIdle,
    TaskReady {
        task: Field,
        reason: Field,
    },
    /// task_sleep, task_wait, task_wait_timeout.
    TaskBlocked {
        task: Field,
    },
    IsrEnter {
        handler: Field,
    },
    IsrExit {
        handler: Field,
    },
    KernelEnter {
        handler: &'static str,
    },
    KernelExit {
        handler: &'static str,
    },
    MutexTaken {
        mutex: Field,
        task: Field,
    },
    MutexReleased {
        mutex: Field,
        task: Field,
    },
    MutexBlocked {
        mutex: Field,
        task: Field,
    },
}

/// The hooks among the catalogue's events, by type id. An adapter event
/// without a field the host reads is an error: host and adapter disagree.
fn hooks(catalogue: &Catalogue) -> Result<HashMap<u16, Hook>> {
    let mut hooks = HashMap::new();
    for (&id, event) in &catalogue.events {
        let Some(hook) = event.name.strip_prefix(PREFIX) else {
            continue;
        };
        let field = |wanted: &str| -> Result<Field> {
            let field = event
                .fields
                .iter()
                .find(|field| field.name == wanted)
                .ok_or_else(|| format!("{}: no field {wanted}, which the host reads", event.name))?;
            let size = field
                .ty
                .size
                .filter(|size| (1..=8).contains(size))
                .ok_or_else(|| format!("{}: {wanted} is not a value of 1 to 8 bytes", event.name))?;
            Ok(Field {
                offset: field.offset as usize,
                size: size as usize,
                pointee: match &field.ty.kind {
                    Kind::Pointer(pointee) => Some(pointee.clone()),
                    _ => None,
                },
                enumerators: match &field.ty.kind {
                    Kind::Enum(enumerators) => enumerators.clone(),
                    _ => Vec::new(),
                },
            })
        };
        let hook = match hook {
            "task_added" => Hook::TaskAdded {
                task: field("task")?,
                priority: field("priority")?,
            },
            "task_priority_changed" => Hook::TaskPriorityChanged {
                task: field("task")?,
                priority: field("priority")?,
            },
            "task_terminated" => Hook::TaskTerminated { task: field("task")? },
            "task_started" => Hook::TaskStarted { task: field("task")? },
            "enter_idle" => Hook::EnterIdle,
            "task_ready" => Hook::TaskReady {
                task: field("task")?,
                reason: field("reason")?,
            },
            "task_sleep" | "task_wait" | "task_wait_timeout" => Hook::TaskBlocked { task: field("task")? },
            "enter_systick" => Hook::KernelEnter { handler: "systick" },
            "exit_systick" => Hook::KernelExit { handler: "systick" },
            "enter_service_call" => Hook::KernelEnter {
                handler: "service_call",
            },
            "exit_service_call" => Hook::KernelExit {
                handler: "service_call",
            },
            "isr_enter" => Hook::IsrEnter {
                handler: field("handler")?,
            },
            "isr_exit" => Hook::IsrExit {
                handler: field("handler")?,
            },
            "mutex_taken" => Hook::MutexTaken {
                mutex: field("mutex")?,
                task: field("owner")?,
            },
            "mutex_released" => Hook::MutexReleased {
                mutex: field("mutex")?,
                task: field("owner")?,
            },
            "mutex_blocked" => Hook::MutexBlocked {
                mutex: field("mutex")?,
                task: field("waiter")?,
            },
            _ => continue,
        };
        hooks.insert(id, hook);
    }
    Ok(hooks)
}

pub struct Opsy<'a> {
    symbols: &'a Symbols,
    hooks: HashMap<u16, Hook>,
    /// The first index recorded: past 0, what came before it is lost.
    first: u32,
    pub tasks: Subjects,
    pub handlers: Subjects,
    pub mutexes: Subjects,
    pub closed: Spans,
    /// The elements that contradict the adapter's order.
    pub contradictions: Vec<usize>,
    /// The task on the core and since when; none while unknown.
    running: Option<(Option<usize>, Edge)>,
    /// The handlers entered and not left, innermost last, each with its
    /// depth.
    entered: Vec<(usize, Edge, Option<usize>)>,
    /// Whether every handler open is in `entered`: false from a loss to the
    /// next task_started or enter_idle that comes with `entered` empty.
    nesting_known: bool,
    /// By mutex: its owner and since when.
    held: BTreeMap<usize, (usize, Edge)>,
    /// By mutex and task: since when the task waits for it.
    waiting: BTreeMap<(usize, usize), Edge>,
    /// The activations that ended, in the order they did.
    pub activations: Vec<Activation>,
    /// Every task_ready, in order.
    pub wakes: Vec<Wake>,
    /// By task: its activation open, since when, why and by what.
    ready: BTreeMap<usize, (Edge, Option<String>, Option<Cause>)>,
    /// The tasks whose activations are known since the last loss: open at
    /// it, or made ready or not ready since.
    known: BTreeSet<usize>,
    /// The last loss.
    loss: Option<Edge>,
    /// The last event.
    last: Option<Edge>,
}

impl<'a> Opsy<'a> {
    /// Nothing taken in yet, for a trace of `catalogue`'s image that starts
    /// at index `first`.
    pub fn new(catalogue: &'a Catalogue, first: u32) -> Result<Self> {
        Ok(Self {
            symbols: &catalogue.symbols,
            hooks: hooks(catalogue)?,
            first,
            tasks: Subjects::default(),
            handlers: Subjects::default(),
            mutexes: Subjects::default(),
            closed: Spans::default(),
            contradictions: Vec::new(),
            running: None,
            entered: Vec::new(),
            nesting_known: true,
            held: BTreeMap::new(),
            waiting: BTreeMap::new(),
            activations: Vec::new(),
            wakes: Vec::new(),
            ready: BTreeMap::new(),
            known: BTreeSet::new(),
            loss: None,
            last: None,
        })
    }

    /// Whether the image has the adapter's events.
    pub fn traced(&self) -> bool {
        !self.hooks.is_empty()
    }

    /// Takes in `elements` from `from` on.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        let hooks = std::mem::take(&mut self.hooks);
        for (element, item) in elements.iter().enumerate().skip(from) {
            match item {
                Element::Event { event, cycles, .. } => {
                    let edge = Edge {
                        element,
                        cycles: *cycles,
                        lost: false,
                    };
                    if self.last.is_none() && self.loss.is_none() && self.first != 0 {
                        self.lost(Edge { lost: true, ..edge });
                    }
                    self.last = Some(edge);
                    if let Some(hook) = hooks.get(&event.event.id) {
                        self.hook(hook, &event.payload, edge);
                    }
                }
                Element::Lost { .. } => self.lost(Edge {
                    element,
                    cycles: self.last.map_or(0, |last| last.cycles),
                    lost: true,
                }),
                Element::Suspect { .. } => {}
            }
        }
        self.hooks = hooks;
    }

    fn hook(&mut self, hook: &Hook, payload: &[u8], edge: Edge) {
        let symbols = self.symbols;
        match hook {
            Hook::TaskAdded { task, priority } | Hook::TaskPriorityChanged { task, priority } => {
                let task = self.tasks.index(symbols, task, payload);
                let task = &mut self.tasks.list[task];
                task.priority = Some(priority.read(payload) as u8);
                task.priorities.push((edge, task.priority));
            }
            Hook::TaskTerminated { task } => {
                // A task killed while it waits for a mutex waits no more.
                let task = self.tasks.index(symbols, task, payload);
                let ended: Vec<_> = self.waiting.keys().filter(|key| key.1 == task).copied().collect();
                for (mutex, task) in ended {
                    let from = self.waiting.remove(&(mutex, task)).expect("a wait just found");
                    self.hold(mutex, task, true, from, edge);
                }
                self.not_ready(task, edge);
            }
            Hook::TaskStarted { task } => {
                let task = self.tasks.index(symbols, task, payload);
                self.run(Some(task), edge);
            }
            Hook::EnterIdle => self.run(None, edge),
            Hook::TaskReady { task, reason } => {
                let task = self.tasks.index(symbols, task, payload);
                if self.ready.contains_key(&task) {
                    return self.contradictions.push(edge.element);
                }
                let reason = reason.label(payload);
                let cause = if ELAPSED.contains(&reason.as_str()) {
                    Cause::Elapsed
                } else {
                    self.context()
                };
                self.ready.insert(task, (edge, Some(reason.clone()), Some(cause)));
                self.known.insert(task);
                self.wakes.push(Wake {
                    task,
                    reason,
                    cause,
                    edge,
                });
            }
            Hook::TaskBlocked { task } => {
                let task = self.tasks.index(symbols, task, payload);
                self.not_ready(task, edge);
            }
            Hook::IsrEnter { handler } => {
                let handler = self.handlers.index(symbols, handler, payload);
                self.enter(handler, edge);
            }
            Hook::IsrExit { handler } => {
                let handler = self.handlers.index(symbols, handler, payload);
                self.exit(handler, edge);
            }
            Hook::KernelEnter { handler } => {
                let handler = self.handlers.named(handler);
                self.enter(handler, edge);
            }
            Hook::KernelExit { handler } => {
                let handler = self.handlers.named(handler);
                self.exit(handler, edge);
            }
            Hook::MutexTaken { mutex, task } => {
                let mutex = self.mutexes.index(symbols, mutex, payload);
                let task = self.tasks.index(symbols, task, payload);
                if self.held.contains_key(&mutex) {
                    return self.contradictions.push(edge.element);
                }
                if let Some(from) = self.waiting.remove(&(mutex, task)) {
                    self.hold(mutex, task, true, from, edge);
                }
                self.held.insert(mutex, (task, edge));
            }
            Hook::MutexReleased { mutex, task } => {
                let mutex = self.mutexes.index(symbols, mutex, payload);
                let task = self.tasks.index(symbols, task, payload);
                let from = match (self.held.get(&mutex), self.loss) {
                    (Some(&(owner, from)), _) if owner == task => {
                        self.held.remove(&mutex);
                        from
                    }
                    (None, Some(loss)) => loss,
                    _ => return self.contradictions.push(edge.element),
                };
                self.hold(mutex, task, false, from, edge);
            }
            Hook::MutexBlocked { mutex, task } => {
                let mutex = self.mutexes.index(symbols, mutex, payload);
                let task = self.tasks.index(symbols, task, payload);
                if self.waiting.contains_key(&(mutex, task)) {
                    return self.contradictions.push(edge.element);
                }
                self.waiting.insert((mutex, task), edge);
                self.not_ready(task, edge);
            }
        }
    }

    /// The context an event is emitted in: the innermost handler open but
    /// the kernel's, or else the task on the core.
    fn context(&self) -> Cause {
        let handler = self
            .entered
            .iter()
            .rev()
            .find(|&&(handler, ..)| !self.handlers.is_named(handler));
        match (handler, self.running) {
            (Some(&(handler, ..)), _) => Cause::Handler(handler),
            (None, Some((task, _))) => Cause::Task(task),
            (None, None) => Cause::Unknown,
        }
    }

    /// Closes the innermost handler running, `handler`, at `edge`.
    fn exit(&mut self, handler: usize, edge: Edge) {
        let (from, depth) = match (self.entered.last(), self.loss) {
            (Some(&(open, from, depth)), _) if open == handler => {
                self.entered.pop();
                (from, depth)
            }
            (None, Some(loss)) => (loss, None),
            _ => return self.contradictions.push(edge.element),
        };
        self.closed.interrupts.push(Interrupt {
            handler,
            depth,
            span: Span { from, to: edge },
        });
    }

    /// Opens `handler` at `edge`, its depth known while the nesting is.
    fn enter(&mut self, handler: usize, edge: Edge) {
        let depth = self.nesting_known.then_some(self.entered.len());
        self.entered.push((handler, edge, depth));
    }

    /// Ends `task`'s activation at `edge`: the one open, or else, after a
    /// loss that hid its start, one from the loss.
    fn not_ready(&mut self, task: usize, edge: Edge) {
        let start = match (self.ready.remove(&task), self.loss) {
            (Some(start), _) => Some(start),
            (None, Some(loss)) if !self.known.contains(&task) => Some((loss, None, None)),
            _ => None,
        };
        self.known.insert(task);
        if let Some((from, reason, cause)) = start {
            self.activations.push(Activation {
                task,
                reason,
                cause,
                span: Span { from, to: edge },
            });
        }
    }

    fn run(&mut self, task: Option<usize>, edge: Edge) {
        // OpSy switches in PendSV, at the lowest priority: it runs with no
        // handler open.
        if self.entered.is_empty() {
            self.nesting_known = true;
        }
        if let Some((running, from)) = self.running.replace((task, edge)) {
            self.closed.runs.push(Run {
                task: running,
                span: Span { from, to: edge },
            });
        }
    }

    fn hold(&mut self, mutex: usize, task: usize, waiting: bool, from: Edge, to: Edge) {
        self.closed.holds.push(Hold {
            mutex,
            task,
            waiting,
            span: Span { from, to },
        });
    }

    /// Closes every open span at the loss `edge`.
    fn lost(&mut self, edge: Edge) {
        let open = self.ended_at(edge);
        self.closed.runs.extend(open.runs);
        self.closed.interrupts.extend(open.interrupts);
        self.closed.holds.extend(open.holds);
        self.activations.extend(self.activations_ended_at(edge));
        self.known = std::mem::take(&mut self.ready).into_keys().collect();
        self.running = None;
        self.entered.clear();
        self.nesting_known = false;
        self.held.clear();
        self.waiting.clear();
        self.loss = Some(edge);
        // A change may be among what the loss hides.
        for task in &mut self.tasks.list {
            if task.priorities.last().is_some_and(|(_, priority)| priority.is_some()) {
                task.priorities.push((edge, None));
            }
        }
    }

    /// The spans open, ended at `edge`: the innermost handler first.
    fn ended_at(&self, to: Edge) -> Spans {
        Spans {
            runs: self
                .running
                .iter()
                .map(|&(task, from)| Run {
                    task,
                    span: Span { from, to },
                })
                .collect(),
            interrupts: self
                .entered
                .iter()
                .rev()
                .map(|&(handler, from, depth)| Interrupt {
                    handler,
                    depth,
                    span: Span { from, to },
                })
                .collect(),
            holds: self
                .held
                .iter()
                .map(|(&mutex, &(task, from))| (mutex, task, false, from))
                .chain(
                    self.waiting
                        .iter()
                        .map(|(&(mutex, task), &from)| (mutex, task, true, from)),
                )
                .map(|(mutex, task, waiting, from)| Hold {
                    mutex,
                    task,
                    waiting,
                    span: Span { from, to },
                })
                .collect(),
        }
    }

    /// The spans still open, ended at the last event.
    pub fn open(&self) -> Spans {
        self.last.map(|last| self.ended_at(last)).unwrap_or_default()
    }

    fn activations_ended_at(&self, to: Edge) -> Vec<Activation> {
        self.ready
            .iter()
            .map(|(&task, (from, reason, cause))| Activation {
                task,
                reason: reason.clone(),
                cause: *cause,
                span: Span { from: *from, to },
            })
            .collect()
    }

    /// The time the task on the core is known: from each task_started or
    /// enter_idle to the next, or to a loss, the last one to the end of the
    /// capture. Which handlers are open is known from the same events.
    pub fn known(&self) -> Covered {
        let mut known = Covered::default();
        for run in &self.closed.runs {
            known.push(run.span.from.cycles, run.span.to.cycles);
        }
        known.open(self.running.map(|(_, from)| from.cycles));
        known
    }

    /// The CPU row: handlers at any depth and tasks other than idle, within
    /// the time known. A handler still open runs to the end of the capture.
    pub fn cpu(&self) -> Cpu {
        let span = |span: &Span| (span.from.cycles, span.to.cycles);
        let handlers = self.closed.interrupts.iter().map(|interrupt| span(&interrupt.span));
        let tasks = self
            .closed
            .runs
            .iter()
            .filter(|run| run.task.is_some())
            .map(|run| span(&run.span));
        let handler_open = self.entered.first().map(|&(_, from, _)| from.cycles);
        let task_open = self.running.and_then(|(task, from)| task.map(|_| from.cycles));
        let known = self.known();
        Cpu {
            handlers: Covered::union(handlers.clone(), handler_open).within(&known),
            busy: Covered::union(handlers.chain(tasks), handler_open.into_iter().chain(task_open).min()).within(&known),
            known,
        }
    }

    /// `cause` as `kymo decode --opsy` prints it: `handler:<name>`,
    /// `task:<name>` (`task:idle` for the idle task), `elapsed` or `unknown`.
    pub fn cause(&self, cause: Cause) -> String {
        match cause {
            Cause::Handler(handler) => format!("handler:{}", self.handlers.list[handler].name),
            Cause::Task(task) => format!("task:{}", task.map_or("idle", |task| &self.tasks.list[task].name)),
            Cause::Elapsed => "elapsed".to_owned(),
            Cause::Unknown => "unknown".to_owned(),
        }
    }

    /// The activations still open, ended at the last event.
    pub fn open_activations(&self) -> Vec<Activation> {
        self.last
            .map(|last| self.activations_ended_at(last))
            .unwrap_or_default()
    }
}

const fn metric(name: &'static str, share: bool, jitter: bool) -> Metric {
    Metric { name, share, jitter }
}

/// A task's metrics, of its activations but the period.
pub const WAKE_LATENCY: Metric = metric("wake_latency", false, false);
pub const PREEMPTED_BY_TASKS: Metric = metric("preempted_by_tasks", false, false);
pub const PREEMPTED_BY_HANDLERS: Metric = metric("preempted_by_handlers", false, false);
pub const NET: Metric = metric("net", true, false);
pub const GROSS: Metric = metric("gross", false, false);
pub const RESPONSE: Metric = metric("response", false, false);
pub const PERIOD: Metric = metric("period", false, true);
pub const HOLD: Metric = metric("hold", false, false);
pub const WAIT: Metric = metric("wait", false, false);

const TASK_METRICS: [Metric; 7] = [
    WAKE_LATENCY,
    PREEMPTED_BY_TASKS,
    PREEMPTED_BY_HANDLERS,
    NET,
    GROSS,
    RESPONSE,
    PERIOD,
];
const HANDLER_METRICS: [Metric; 3] = [GROSS, NET, PERIOD];
const MUTEX_METRICS: [Metric; 2] = [HOLD, WAIT];

/// The time `spans`, sorted and apart, take of `from..to`.
fn overlap(spans: &[Span], from: Edge, to: Edge) -> i64 {
    let first = spans.partition_point(|span| span.to.element <= from.element);
    spans[first..]
        .iter()
        .take_while(|span| span.from.element < to.element)
        .map(|span| {
            let low = if span.from.element > from.element {
                span.from
            } else {
                from
            };
            let high = if span.to.element < to.element { span.to } else { to };
            high.cycles - low.cycles
        })
        .sum()
}

/// Each interrupt's parent, the innermost one whose span holds its entry,
/// `interrupts` sorted by entry and, at one entry, outermost first: spans
/// nest, a loss included, where depths after a loss are unknown.
fn parents(interrupts: &[(Interrupt, bool)]) -> Vec<Option<usize>> {
    let mut held: Vec<usize> = Vec::new();
    interrupts
        .iter()
        .enumerate()
        .map(|(n, (interrupt, _))| {
            let from = interrupt.span.from.element;
            while held
                .last()
                .is_some_and(|&last| interrupts[last].0.span.to.element <= from)
            {
                held.pop();
            }
            let parent = held.last().copied();
            held.push(n);
            parent
        })
        .collect()
}

/// Each task's runs, closed or open, in order.
fn task_runs(opsy: &Opsy) -> HashMap<usize, Vec<Span>> {
    let open = opsy.open();
    let mut runs: Vec<Run> = opsy.closed.runs.iter().chain(&open.runs).copied().collect();
    runs.sort_by_key(|run| run.span.from.element);
    let mut by_task: HashMap<usize, Vec<Span>> = HashMap::new();
    for run in runs {
        if let Some(task) = run.task {
            by_task.entry(task).or_default().push(run.span);
        }
    }
    by_task
}

/// The parts of `runs`, sorted and apart, within `span`, as edges.
fn within(runs: &[Span], span: Span) -> Vec<(Edge, Edge)> {
    let first = runs.partition_point(|run| run.to.element <= span.from.element);
    runs[first..]
        .iter()
        .take_while(|run| run.from.element <= span.to.element)
        .map(|run| {
            let low = if run.from.element > span.from.element {
                run.from
            } else {
                span.from
            };
            let high = if run.to.element < span.to.element {
                run.to
            } else {
                span.to
            };
            (low, high)
        })
        .collect()
}

/// Each task's time ready, activation by activation, those that ended
/// first, then those still open: the activation less the task's runs.
pub fn ready(opsy: &Opsy) -> Vec<Ready> {
    let runs = task_runs(opsy);
    let still_open = opsy.open_activations();
    let activations = opsy
        .activations
        .iter()
        .map(|activation| (activation, false))
        .chain(still_open.iter().map(|activation| (activation, true)));
    let mut out = Vec::new();
    for (activation, open) in activations {
        let (task, span) = (activation.task, activation.span);
        let wake = activation.reason.as_ref().and_then(|_| {
            opsy.wakes
                .binary_search_by_key(&span.from.element, |wake| wake.edge.element)
                .ok()
        });
        let mut push = |from: Edge, to: Edge, open| {
            if to.element > from.element {
                out.push(Ready {
                    task,
                    span: Span { from, to },
                    open,
                    activation: span,
                    wake,
                });
            }
        };
        let mut from = span.from;
        for (low, high) in runs.get(&task).map_or_else(Vec::new, |runs| within(runs, span)) {
            push(from, low, false);
            from = high;
        }
        push(from, span.to, open);
    }
    out
}

/// OpSy's instances for kymo::stats, by subject and metric:
///
/// - a task's, of each activation from its task_ready to its end: the wake
///   latency, to its first run in the activation, in total, by reason and
///   by cause, after the reasons;
///   the gross execution, from that run to the end; the net execution, its
///   runs less the handlers that run within them, outermost ones whole, the
///   kernel's among them; the time preempted, the gross less its runs by
///   tasks, its handlers by handlers; the response time, from task_ready to
///   the end. An activation in which the task never runs has only a
///   response time. And the period, from a task_ready to the task's next;
/// - a handler's: gross, from entry to exit; net, less the handlers nested
///   one level within it; the period, from an entry to the next;
/// - a mutex's: each hold and each wait, by task and in total.
///
/// Each instance spans its activation, its handler's run, its period, its
/// hold or wait; those still open at the end of the trace are open.
pub fn instances(opsy: &Opsy) -> Vec<Instance> {
    let open = opsy.open();
    let mut interrupts: Vec<(Interrupt, bool)> = opsy
        .closed
        .interrupts
        .iter()
        .map(|&interrupt| (interrupt, false))
        .collect();
    interrupts.extend(open.interrupts.iter().map(|&interrupt| (interrupt, true)));
    interrupts.sort_by_key(|(interrupt, _)| {
        (
            interrupt.span.from.element,
            std::cmp::Reverse(interrupt.span.to.element),
        )
    });
    let parents = parents(&interrupts);
    let outermost: Vec<Span> = interrupts
        .iter()
        .zip(&parents)
        .filter(|(_, parent)| parent.is_none())
        .map(|((interrupt, _), _)| interrupt.span)
        .collect();
    let task_runs = task_runs(opsy);

    // Sorted by kind, subject, metric, detail and start before being handed out.
    type Order = (u8, usize, usize, Option<(u8, String)>, usize);
    let mut out: Vec<(Order, Instance)> = Vec::new();
    let mut push = |kind: (u8, &'static str),
                    subject: usize,
                    metrics: &[Metric],
                    metric: Metric,
                    detail: Option<(u8, String)>,
                    value: i64,
                    span: Span,
                    open: bool| {
        let name = match kind.0 {
            0 => &opsy.tasks.list[subject].name,
            1 => &opsy.handlers.list[subject].name,
            _ => &opsy.mutexes.list[subject].name,
        };
        let rank = metrics.iter().position(|m| *m == metric).expect("a metric of the kind");
        out.push((
            (kind.0, subject, rank, detail.clone(), span.from.element),
            Instance {
                kind: kind.1,
                subject: name.clone(),
                metric,
                detail: detail.map(|(_, detail)| detail),
                value,
                from: span.from,
                to: span.to,
                open,
            },
        ));
    };
    const TASK: (u8, &str) = (0, "task");
    const HANDLER: (u8, &str) = (1, "handler");
    const MUTEX: (u8, &str) = (2, "mutex");
    // The wake latency's details: by reason, then by cause.
    const REASON: u8 = 0;
    const CAUSE: u8 = 1;

    let still_open = opsy.open_activations();
    let activations = opsy
        .activations
        .iter()
        .map(|activation| (activation, false))
        .chain(still_open.iter().map(|activation| (activation, true)));
    for (activation, open) in activations {
        let (task, span) = (activation.task, activation.span);
        let mut task_push = |metric, detail, value| push(TASK, task, &TASK_METRICS, metric, detail, value, span, open);
        task_push(RESPONSE, None, span.to.cycles - span.from.cycles);
        let windows = task_runs.get(&task).map_or_else(Vec::new, |runs| within(runs, span));
        let Some(&(first, _)) = windows.first() else {
            continue;
        };
        let ran: i64 = windows.iter().map(|(low, high)| high.cycles - low.cycles).sum();
        let handled: i64 = windows.iter().map(|&(low, high)| overlap(&outermost, low, high)).sum();
        let gross = span.to.cycles - first.cycles;
        let latency = first.cycles - span.from.cycles;
        task_push(WAKE_LATENCY, None, latency);
        if let Some(reason) = &activation.reason {
            task_push(WAKE_LATENCY, Some((REASON, reason.clone())), latency);
        }
        if let Some(cause) = activation.cause {
            task_push(WAKE_LATENCY, Some((CAUSE, opsy.cause(cause))), latency);
        }
        task_push(PREEMPTED_BY_TASKS, None, gross - ran);
        task_push(PREEMPTED_BY_HANDLERS, None, handled);
        task_push(NET, None, ran - handled);
        task_push(GROSS, None, gross);
    }
    let mut wakes: Vec<&Wake> = opsy.wakes.iter().collect();
    wakes.sort_by_key(|wake| (wake.task, wake.edge.element));
    for pair in wakes.windows(2).filter(|pair| pair[0].task == pair[1].task) {
        let span = Span {
            from: pair[0].edge,
            to: pair[1].edge,
        };
        push(
            TASK,
            pair[0].task,
            &TASK_METRICS,
            PERIOD,
            None,
            span.to.cycles - span.from.cycles,
            span,
            false,
        );
    }

    let mut net: Vec<i64> = interrupts
        .iter()
        .map(|(interrupt, _)| interrupt.span.to.cycles - interrupt.span.from.cycles)
        .collect();
    for (&parent, (interrupt, _)) in parents.iter().zip(&interrupts) {
        if let Some(parent) = parent {
            net[parent] -= interrupt.span.to.cycles - interrupt.span.from.cycles;
        }
    }
    for (n, &(interrupt, open)) in interrupts.iter().enumerate() {
        let span = interrupt.span;
        push(
            HANDLER,
            interrupt.handler,
            &HANDLER_METRICS,
            GROSS,
            None,
            span.to.cycles - span.from.cycles,
            span,
            open,
        );
        push(
            HANDLER,
            interrupt.handler,
            &HANDLER_METRICS,
            NET,
            None,
            net[n],
            span,
            open,
        );
    }
    let mut entries: Vec<(usize, Edge)> = interrupts
        .iter()
        .filter(|(interrupt, _)| !interrupt.span.from.lost)
        .map(|(interrupt, _)| (interrupt.handler, interrupt.span.from))
        .collect();
    entries.sort_by_key(|&(handler, edge)| (handler, edge.element));
    for pair in entries.windows(2).filter(|pair| pair[0].0 == pair[1].0) {
        let span = Span {
            from: pair[0].1,
            to: pair[1].1,
        };
        push(
            HANDLER,
            pair[0].0,
            &HANDLER_METRICS,
            PERIOD,
            None,
            span.to.cycles - span.from.cycles,
            span,
            false,
        );
    }

    let holds = opsy
        .closed
        .holds
        .iter()
        .map(|hold| (hold, false))
        .chain(open.holds.iter().map(|hold| (hold, true)));
    for (hold, open) in holds {
        let metric = if hold.waiting { WAIT } else { HOLD };
        let value = hold.span.to.cycles - hold.span.from.cycles;
        let task = opsy.tasks.list[hold.task].name.clone();
        push(MUTEX, hold.mutex, &MUTEX_METRICS, metric, None, value, hold.span, open);
        push(
            MUTEX,
            hold.mutex,
            &MUTEX_METRICS,
            metric,
            Some((0, task)),
            value,
            hold.span,
            open,
        );
    }
    out.sort_by(|(a, _), (b, _)| a.cmp(b));
    out.into_iter().map(|(_, instance)| instance).collect()
}

/// `kymo decode --opsy`'s output: the spans of a capture's trace, a line
/// each, those still open at its end last with `to=open`; each task_ready
/// and its cause; each time ready; then the contradictions. An edge is the
/// index of its event, or `lost`.
pub fn lines(trace: &Trace, opsy: &Opsy) -> Vec<String> {
    let mut lines = Vec::new();
    let edge = |edge: Edge| match &trace.elements[edge.element] {
        Element::Event { event, .. } if !edge.lost => event.index.to_string(),
        _ => "lost".to_owned(),
    };
    let task = |task: Option<usize>| task.map_or("idle", |task| &opsy.tasks.list[task].name);
    let open = opsy.open();
    for (spans, end) in [(&opsy.closed, None), (&open, Some("open"))] {
        let to = |span: Span| end.map_or_else(|| edge(span.to), str::to_owned);
        for run in &spans.runs {
            lines.push(format!(
                "run from={} to={} task={}",
                edge(run.span.from),
                to(run.span),
                task(run.task)
            ));
        }
        for interrupt in &spans.interrupts {
            lines.push(format!(
                "interrupt from={} to={} depth={} handler={}",
                edge(interrupt.span.from),
                to(interrupt.span),
                interrupt
                    .depth
                    .map_or_else(|| "unknown".to_owned(), |depth| depth.to_string()),
                opsy.handlers.list[interrupt.handler].name
            ));
        }
        for hold in &spans.holds {
            lines.push(format!(
                "{} from={} to={} mutex={} task={}",
                if hold.waiting { "wait" } else { "hold" },
                edge(hold.span.from),
                to(hold.span),
                opsy.mutexes.list[hold.mutex].name,
                task(Some(hold.task))
            ));
        }
    }
    for wake in &opsy.wakes {
        lines.push(format!(
            "wake at={} task={} reason={} cause={}",
            edge(wake.edge),
            task(Some(wake.task)),
            wake.reason,
            opsy.cause(wake.cause)
        ));
    }
    for ready in ready(opsy) {
        lines.push(format!(
            "ready from={} to={} task={}",
            edge(ready.span.from),
            if ready.open {
                "open".to_owned()
            } else {
                edge(ready.span.to)
            },
            task(Some(ready.task))
        ));
    }
    for &element in &opsy.contradictions {
        if let Element::Event { event, .. } = &trace.elements[element] {
            lines.push(format!("contradiction {}", event.index));
        }
    }
    lines
}

#[cfg(test)]
mod tests;
