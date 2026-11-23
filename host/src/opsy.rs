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
//! taking seen since a loss starts its span at the loss. A recording that
//! starts after the target's first event starts with such a loss.
//!
//! An event that contradicts the adapter's order makes no span: an interrupt
//! exit that is not the last handler entered, a mutex taken while held,
//! released by another task than its owner or never taken, a task blocked
//! twice on the same mutex. Its element is listed among the contradictions.

use crate::events::{Catalogue, Kind, Pointee};
use crate::model::{Element, Trace};
use crate::symbols::Symbols;
use crate::target::Result;
use std::collections::{BTreeMap, HashMap};

const PREFIX: &str = "kymo::opsy::";

/// Where a span starts or ends: an element of the trace, and its cycles. At
/// a loss, the edge is somewhere in what it hides; its cycles are those of
/// the event before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub element: usize,
    pub cycles: i64,
    pub lost: bool,
}

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

/// A handler running, `depth` handlers below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interrupt {
    pub handler: usize,
    pub depth: usize,
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
}

#[derive(Default)]
pub struct Subjects {
    pub list: Vec<Subject>,
    by_address: HashMap<u64, usize>,
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
            });
            self.list.len() - 1
        })
    }
}

/// A field of an event: where its value is in the payload, and what it
/// points at when it is a pointer.
#[derive(Clone)]
struct Field {
    offset: usize,
    size: usize,
    pointee: Option<Pointee>,
}

impl Field {
    fn read(&self, payload: &[u8]) -> u64 {
        payload[self.offset..self.offset + self.size]
            .iter()
            .rev()
            .fold(0, |value, byte| value << 8 | u64::from(*byte))
    }
}

/// The adapter's events the spans are made of.
enum Hook {
    TaskAdded { task: Field, priority: Field },
    TaskPriorityChanged { task: Field, priority: Field },
    TaskTerminated { task: Field },
    TaskStarted { task: Field },
    EnterIdle,
    IsrEnter { handler: Field },
    IsrExit { handler: Field },
    MutexTaken { mutex: Field, task: Field },
    MutexReleased { mutex: Field, task: Field },
    MutexBlocked { mutex: Field, task: Field },
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
    /// The handlers entered and not left, innermost last.
    entered: Vec<(usize, Edge)>,
    /// By mutex: its owner and since when.
    held: BTreeMap<usize, (usize, Edge)>,
    /// By mutex and task: since when the task waits for it.
    waiting: BTreeMap<(usize, usize), Edge>,
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
            held: BTreeMap::new(),
            waiting: BTreeMap::new(),
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
                self.tasks.list[task].priority = Some(priority.read(payload) as u8);
            }
            Hook::TaskTerminated { task } => {
                // A task killed while it waits for a mutex waits no more.
                let task = self.tasks.index(symbols, task, payload);
                let ended: Vec<_> = self.waiting.keys().filter(|key| key.1 == task).copied().collect();
                for (mutex, task) in ended {
                    let from = self.waiting.remove(&(mutex, task)).expect("a wait just found");
                    self.hold(mutex, task, true, from, edge);
                }
            }
            Hook::TaskStarted { task } => {
                let task = self.tasks.index(symbols, task, payload);
                self.run(Some(task), edge);
            }
            Hook::EnterIdle => self.run(None, edge),
            Hook::IsrEnter { handler } => {
                let handler = self.handlers.index(symbols, handler, payload);
                self.entered.push((handler, edge));
            }
            Hook::IsrExit { handler } => {
                let handler = self.handlers.index(symbols, handler, payload);
                let from = match (self.entered.last(), self.loss) {
                    (Some(&(open, from)), _) if open == handler => {
                        self.entered.pop();
                        from
                    }
                    (None, Some(loss)) => loss,
                    _ => return self.contradictions.push(edge.element),
                };
                self.closed.interrupts.push(Interrupt {
                    handler,
                    depth: self.entered.len(),
                    span: Span { from, to: edge },
                });
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
            }
        }
    }

    fn run(&mut self, task: Option<usize>, edge: Edge) {
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
        self.running = None;
        self.entered.clear();
        self.held.clear();
        self.waiting.clear();
        self.loss = Some(edge);
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
                .enumerate()
                .rev()
                .map(|(depth, &(handler, from))| Interrupt {
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
}

/// `kymo decode --opsy`'s output: the spans of a capture's trace, a line
/// each, those still open at its end last with `to=open`, then the
/// contradictions. An edge is the index of its event, or `lost`.
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
                interrupt.depth,
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
    for &element in &opsy.contradictions {
        if let Element::Event { event, .. } = &trace.elements[element] {
            lines.push(format!("contradiction {}", event.index));
        }
    }
    lines
}

#[cfg(test)]
mod tests;
