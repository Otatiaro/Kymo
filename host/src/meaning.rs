//! The meaning of a trace of a kernel's hooks: which task holds the core,
//! which interrupt handlers run, which mutexes are held and waited for, as
//! spans between the hooks. A dialect, one per kernel (meaning.rs), says which
//! of the catalogue's events are which hooks; the hooks below are named after
//! OpSy's events.
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
//! the kernel emits with none open: a handler that runs meanwhile has no depth. A
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
//! out, a reason the dialect names, is emitted in SysTick, whatever it
//! interrupts: its cause is the delay. A handler the adapter does not trace
//! hides in the task it interrupts.
//!
//! An interrupt handler is one subject per handler and exception it runs for,
//! named `<handler> (<interrupt>)` (interrupts.rs names the interrupt), so
//! that a handler several interrupts share is told apart. Its exit, which
//! gives the handler alone, closes the innermost one entered.
//!
//! The handlers the SWO traces (exceptions.rs) are subjects of their own,
//! one per exception, named by the interrupt; their spans are taken in once
//! the trace is (`Meaning::exceptions`). Where the adapter traces the same
//! handler, its spans that touch the SWO's coverage give way to the SWO's.
//!
//! The kernel's handlers the dialect names run as handlers of those names.
//! PendSV has no exit event: it makes no span, and the task it puts on the
//! core runs from the task_started it emits.
//!
//! A dialect may differ from OpSy's events (freertos.rs):
//!
//! - an event may leave its task out, for the task on the core: it is
//!   skipped while that one is unknown, or is the idle task;
//! - a wake may give no reason: its reason is then the wait the task leaves,
//!   named by the event that blocked it, `created` for a task's first wake,
//!   `unknown` after a loss; a wake emitted by the tick's handler is by a
//!   delay that ran out;
//! - a handler's exit may give its exception instead of its address;
//! - a priority change may put a task that is ready back in the ready list,
//!   with a task_ready that is no wake, right after the change or right
//!   before it (priority inheritance): a task_ready of a ready task is a
//!   contradiction only if the next hook is not its priority change;
//! - the scheduler's start may give the idle task, which is no task: the
//!   hooks before it, from the trace's start, are held back until it comes,
//!   or a loss; a recording that starts later counts the idle task as a
//!   task;
//! - a priority may be signed, read as it is (zephyr.rs);
//! - a task may be named after it is created;
//! - a timed wait of no time may be a yield, which blocks nothing;
//! - a block may be reported and not made (zephyr.rs): it is then the
//!   task's only if the core goes to another task before the task emits
//!   anything else but priority changes. It takes effect at its own event.
//!   Inside a handler nothing settles it; a wake of the task settles it
//!   (the task blocked, and is woken); a loss, or the end of the trace,
//!   leaves it unsettled, never made.

use crate::code::Code;
use crate::events::{Catalogue, Event, Kind, Pointee};
use crate::exceptions;
use crate::fault::{self, Fault};
use crate::interrupts::Interrupts;
use crate::lines::Lines;
use crate::load::{Covered, Cpu};
pub use crate::model::Edge;
use crate::model::{Element, Trace};
use crate::stats::{Instance, Metric};
use crate::swo::Watched;
use crate::symbols::Symbols;
use crate::target::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};

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

/// The adapter's cost on a run of a handler both channels trace: from the
/// SWO's entry to the adapter's, and from the adapter's exit to the SWO's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    /// The SWO's handler.
    pub handler: usize,
    pub entry: Span,
    pub exit: Span,
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
    /// The task_ready that starts the activation, in `Meaning::wakes`; none for
    /// an activation that starts at a loss.
    pub wake: Option<usize>,
}

/// Where a fault came: the context the core was in when it faulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    /// In thread mode: the task on the core, or the idle task for none.
    Task(Option<usize>),
    /// In the innermost handler entered for the exception the fault
    /// interrupted.
    Handler(usize),
    /// In the handler of an exception that has none entered: one the adapter
    /// does not trace.
    Exception(u16),
    /// In thread mode while the task on the core is unknown.
    Unknown,
}

/// A hardware fault, where it came and when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Faulted {
    pub fault: Fault,
    pub context: Context,
    pub edge: Edge,
}

/// An application event, and the context that emitted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instant {
    pub edge: Edge,
    pub context: Context,
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
    /// A task's priority, the last one known, compared by the meaning's
    /// `precedence`.
    pub priority: Option<i32>,
    /// A task's priority over the trace, each from its edge on: `None` where
    /// it is unknown, from a loss to the next event that gives it.
    pub priorities: Vec<(Edge, Option<i32>)>,
}

impl Subject {
    /// A task's priority at `cycles`: `None` before the first event that
    /// gives it, or since a loss.
    pub fn priority_at(&self, cycles: i64) -> Option<i32> {
        let after = self.priorities.partition_point(|(edge, _)| edge.cycles <= cycles);
        after.checked_sub(1).and_then(|last| self.priorities[last].1)
    }
}

#[derive(Default)]
pub struct Subjects {
    pub list: Vec<Subject>,
    by_address: HashMap<u64, usize>,
    /// Handlers, by address and exception.
    by_interrupt: HashMap<(u64, Option<u16>), usize>,
    by_name: HashMap<&'static str, usize>,
    /// Handlers the SWO traces, by exception.
    by_exception: HashMap<u16, usize>,
}

/// The name of the handler at `address`, which `field` gives: from the
/// image, or else the address.
fn handler_name(symbols: &Symbols, field: &Field, address: u64) -> String {
    field
        .pointee
        .as_ref()
        .and_then(|pointee| symbols.name(pointee, address))
        .unwrap_or_else(|| format!("0x{address:08x}"))
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

    /// The handler at the address `field` gives, run for `exception`: one
    /// subject per handler and interrupt, named `<handler> (<interrupt>)`, or
    /// by the handler alone when the exception is unknown or the interrupt
    /// bears the handler's name.
    fn interrupt(
        &mut self,
        symbols: &Symbols,
        interrupts: &Interrupts,
        field: &Field,
        exception: Option<u16>,
        payload: &[u8],
    ) -> usize {
        let address = field.read(payload);
        *self.by_interrupt.entry((address, exception)).or_insert_with(|| {
            let handler = handler_name(symbols, field, address);
            let name = match exception.map(|exception| interrupts.name(exception)) {
                Some(interrupt) if interrupt != handler => format!("{handler} ({interrupt})"),
                _ => handler,
            };
            self.list.push(Subject {
                address,
                name,
                priority: None,
                priorities: Vec::new(),
            });
            self.list.len() - 1
        })
    }

    /// A new subject at `address`, by `name`, or by the address when the name
    /// is empty: the address names it from then on.
    fn create(&mut self, address: u64, name: String) -> usize {
        let name = if name.is_empty() {
            format!("0x{address:08x}")
        } else {
            name
        };
        self.list.push(Subject {
            address,
            name,
            priority: None,
            priorities: Vec::new(),
        });
        self.by_address.insert(address, self.list.len() - 1);
        self.list.len() - 1
    }

    /// The handler the SWO traces for `exception`, by `name`: one subject per
    /// exception, at address 0.
    fn exception(&mut self, exception: u16, name: impl FnOnce() -> String) -> usize {
        *self.by_exception.entry(exception).or_insert_with(|| {
            self.list.push(Subject {
                address: 0,
                name: name(),
                priority: None,
                priorities: Vec::new(),
            });
            self.list.len() - 1
        })
    }

    /// Whether `subject` is a handler the SWO traces.
    pub fn is_hardware(&self, subject: usize) -> bool {
        self.by_exception.values().any(|&hardware| hardware == subject)
    }

    /// A handler entered for `exception`, the first one seen.
    fn for_exception(&self, exception: u16) -> Option<usize> {
        self.by_interrupt
            .iter()
            .filter(|((_, of), _)| *of == Some(exception))
            .map(|(_, &handler)| handler)
            .min()
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
pub(crate) struct Field {
    offset: usize,
    size: usize,
    pointee: Option<Pointee>,
    enumerators: Vec<(i64, String)>,
    /// Whether the value is a signed integer.
    signed: bool,
}

impl Field {
    /// `event`'s field `wanted`. One missing, or not a value of 1 to 8
    /// bytes, is an error: host and adapter disagree.
    pub(crate) fn of(event: &Event, wanted: &str) -> Result<Self> {
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
        Ok(Self {
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
            signed: matches!(field.ty.kind,
                Kind::Base(encoding) if encoding == gimli::DW_ATE_signed || encoding == gimli::DW_ATE_signed_char),
        })
    }

    fn read(&self, payload: &[u8]) -> u64 {
        payload[self.offset..self.offset + self.size]
            .iter()
            .rev()
            .fold(0, |value, byte| value << 8 | u64::from(*byte))
    }

    /// The value as a priority: a signed one sign-extended, an unsigned one
    /// past `i32::MAX` saturated.
    fn priority(&self, payload: &[u8]) -> i32 {
        let value = self.read(payload);
        if self.signed {
            let unused = 64 - 8 * self.size as u32;
            let value = ((value << unused) as i64) >> unused;
            i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
        } else {
            i32::try_from(value).unwrap_or(i32::MAX)
        }
    }

    /// The enumerator of the value, or else the value.
    fn label(&self, payload: &[u8]) -> String {
        let value = self.read(payload);
        self.enumerators
            .iter()
            .find(|(enumerator, _)| *enumerator as u64 == value)
            .map_or_else(|| value.to_string(), |(_, label)| label.clone())
    }

    /// A `char` array of any length, read up to its first NUL.
    pub(crate) fn text_of(event: &Event, wanted: &str) -> Result<Self> {
        let field = event
            .fields
            .iter()
            .find(|field| field.name == wanted)
            .ok_or_else(|| format!("{}: no field {wanted}, which the host reads", event.name))?;
        let size = field
            .ty
            .size
            .filter(|&size| size >= 1)
            .ok_or_else(|| format!("{}: {wanted} has no size", event.name))?;
        Ok(Self {
            offset: field.offset as usize,
            size: size as usize,
            pointee: None,
            enumerators: Vec::new(),
            signed: false,
        })
    }

    /// The bytes up to the first NUL, as UTF-8 with any invalid sequence
    /// replaced.
    fn text(&self, payload: &[u8]) -> String {
        let bytes = &payload[self.offset..self.offset + self.size];
        let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }
}

/// The hooks the spans are made of. A task given as `None` is the task on
/// the core: an event about it is skipped while that task is unknown or idle.
pub(crate) enum Hook {
    /// The scheduler starts with this idle task, which is no task of the
    /// meaning.
    Starting {
        idle: Field,
    },
    /// A task is created, with its priority, which is its base priority too,
    /// and its name when the event gives it: then it is a new subject even
    /// at the address of a task deleted before.
    TaskAdded {
        task: Field,
        priority: Field,
        name: Option<Field>,
    },
    /// A task's priority, its base one aside.
    TaskPriorityChanged {
        task: Field,
        priority: Field,
    },
    /// A task's base priority: its priority as well, unless it holds a
    /// higher one it inherited.
    TaskBasePriority {
        task: Field,
        priority: Field,
    },
    TaskTerminated {
        task: Field,
    },
    /// A task's name, given after its creation: it names the subject from
    /// then on, unless it is empty.
    TaskNamed {
        task: Field,
        name: Field,
    },
    TaskStarted {
        task: Field,
    },
    EnterIdle,
    /// A task made ready, by the reason the field gives, or else by the
    /// wait it leaves.
    TaskReady {
        task: Field,
        reason: Option<Field>,
    },
    /// A task blocks, in the wait named when it is one the wakes give. A
    /// `timeout` of 0, when the event gives one, is a yield: no block.
    TaskBlocked {
        task: Option<Field>,
        wait: Option<&'static str>,
        timeout: Option<Field>,
    },
    /// A task is suspended: it waits to be resumed, whatever it waited for.
    TaskSuspended {
        task: Field,
    },
    /// A handler entered for an exception, IPSR's.
    IsrEnter {
        handler: Field,
        exception: Field,
    },
    /// The innermost handler entered at that address returns.
    IsrExit {
        handler: Field,
    },
    /// The innermost handler entered for that exception returns.
    IsrExitException {
        exception: Field,
    },
    /// A kernel's handler, of that name, entered for that exception.
    KernelEnter {
        handler: &'static str,
        exception: u16,
    },
    KernelExit {
        handler: &'static str,
    },
    MutexTaken {
        mutex: Field,
        task: Option<Field>,
    },
    MutexReleased {
        mutex: Field,
        task: Option<Field>,
    },
    MutexBlocked {
        mutex: Field,
        task: Option<Field>,
    },
    /// A hardware fault, `kymo::fault`.
    Fault(fault::Layout),
}

/// How a kernel's priorities compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precedence {
    /// OpSy's: 0 is the highest.
    LowerIsHigher,
    /// FreeRTOS's: 0 is the idle task's, the lowest.
    HigherIsHigher,
}

impl Precedence {
    /// A key that sorts `priority` highest first.
    pub fn rank(self, priority: i32) -> i64 {
        match self {
            Self::LowerIsHigher => i64::from(priority),
            Self::HigherIsHigher => -i64::from(priority),
        }
    }

    /// How a priority value reads, for the user.
    pub fn reading(self) -> &'static str {
        match self {
            Self::LowerIsHigher => "lower is higher",
            Self::HigherIsHigher => "higher is higher",
        }
    }
}

/// A kernel's dialect: the hooks among the catalogue's events, by type id,
/// and what differs from one kernel to another.
pub(crate) struct Dialect {
    pub hooks: HashMap<u16, Hook>,
    /// The wake reasons for a delay that ran out, given in SysTick.
    pub elapsed: &'static [&'static str],
    /// The exception whose handler makes ready the tasks whose delay ran
    /// out, when the wakes do not give it as a reason.
    pub tick: Option<u16>,
    /// Whether a priority change puts a task that is ready back in the ready
    /// list with a task_ready, which is then no wake.
    pub requeue: bool,
    /// Whether a block is the task's only once the core goes to another
    /// task: the kernel may report a block it does not make.
    pub confirm_blocks: bool,
    pub precedence: Precedence,
}

/// A block reported and not yet settled: the task, the wait it names, the
/// mutex it waits for, and its event.
#[derive(Clone, Copy)]
struct Block {
    task: usize,
    wait: Option<&'static str>,
    mutex: Option<usize>,
    edge: Edge,
}

pub struct Meaning<'a> {
    symbols: &'a Symbols,
    interrupts: &'a Interrupts,
    lines: &'a Lines,
    code: &'a Code,
    hooks: HashMap<u16, Hook>,
    /// The wake reasons for a delay that ran out.
    elapsed: &'static [&'static str],
    /// The exception of the handler that wakes the tasks whose delay ran out.
    tick: Option<u16>,
    requeue: bool,
    confirm_blocks: bool,
    /// The block reported and not yet settled, when blocks are confirmed.
    block: Option<Block>,
    /// How the tasks' priorities compare.
    pub precedence: Precedence,
    /// The idle task's address, once the scheduler's start gives it.
    idle: Option<u64>,
    /// The hooks taken in before the scheduler's start, which gives the idle
    /// task: while the dialect has it, from the trace's start to it or to a
    /// loss.
    buffering: bool,
    /// Each hook held back with its payload, or none for an application
    /// event.
    buffer: Vec<(Option<u16>, Vec<u8>, Edge)>,
    /// The task whose priority the last hook changed, not known to wait: a
    /// task_ready of it right after only moves it within the ready list.
    reinsert: Option<usize>,
    /// The task a task_ready moved while ready, and the element: a move
    /// within the ready list if the next hook changes its priority, as
    /// priority inheritance does after the move; else a contradiction.
    moved: Option<(usize, usize)>,
    /// By task: its base priority, while known.
    base: BTreeMap<usize, i32>,
    /// By task: the wait it is in, which its next wake leaves, `created` for
    /// the first one; unknown from a loss to its next block.
    waits: BTreeMap<usize, &'static str>,
    /// The first index recorded: past 0, what came before it is unknown, and
    /// read as a loss edge without being counted as a loss.
    first: u32,
    pub tasks: Subjects,
    pub handlers: Subjects,
    pub mutexes: Subjects,
    pub closed: Spans,
    /// The elements that contradict the adapter's order.
    pub contradictions: Vec<usize>,
    /// The hardware faults, in order.
    pub faults: Vec<Faulted>,
    /// The application's events, outside the `kymo::` namespace, in order.
    pub instants: Vec<Instant>,
    /// The task on the core and since when; none while unknown.
    running: Option<(Option<usize>, Edge)>,
    /// The handlers entered and not left, innermost last, each with its
    /// depth and the exception it runs for.
    entered: Vec<(usize, Edge, Option<usize>, Option<u16>)>,
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
    /// By exception: the first handler the adapter enters for it, and the
    /// handler's name.
    traced: BTreeMap<u16, (usize, String)>,
    /// The adapter's handlers the SWO traces too, and the stretches of the
    /// SWO's coverage: where their spans are the SWO's.
    doubly: BTreeSet<usize>,
    replaced: Vec<(i64, i64)>,
    /// The exception packets that contradict the nesting, placed in the trace.
    pub exception_contradictions: Vec<Edge>,
    /// The adapter's cost on each of its runs the SWO replaces, closed and
    /// held whole in a span of the SWO with no lost edge, in its order.
    pub costs: Vec<Cost>,
}

impl<'a> Meaning<'a> {
    /// Nothing taken in yet, for a trace of `catalogue`'s image that starts
    /// at index `first`, in the dialect of the kernel whose events the image
    /// has: FreeRTOS's, Zephyr's, or else OpSy's. An image with the events of
    /// more than one kernel is an error.
    pub fn new(catalogue: &'a Catalogue, first: u32) -> Result<Self> {
        let has = |prefix: &str| catalogue.events.values().any(|event| event.name.starts_with(prefix));
        let kernels = [
            ("OpSy's", crate::opsy::PREFIX),
            ("FreeRTOS's", crate::freertos::PREFIX),
            ("Zephyr's", crate::zephyr::PREFIX),
        ];
        let found: Vec<_> = kernels.iter().filter(|(_, prefix)| has(prefix)).collect();
        let Dialect {
            hooks,
            elapsed,
            tick,
            requeue,
            confirm_blocks,
            precedence,
        } = match found.as_slice() {
            [] | [(_, crate::opsy::PREFIX)] => crate::opsy::dialect(catalogue)?,
            [(_, crate::freertos::PREFIX)] => crate::freertos::dialect(catalogue)?,
            [(_, crate::zephyr::PREFIX)] => crate::zephyr::dialect(catalogue)?,
            several => {
                let names: Vec<_> = several.iter().map(|(name, _)| *name).collect();
                return Err(format!("the image has the adapter events of {}", names.join(" and ")).into());
            }
        };
        let mut hooks = hooks;
        if let Some((&id, event)) = catalogue.events.iter().find(|(_, event)| event.name == fault::EVENT) {
            hooks.insert(id, Hook::Fault(fault::Layout::of(event)?));
        }
        let buffering = first == 0 && hooks.values().any(|hook| matches!(hook, Hook::Starting { .. }));
        Ok(Self {
            symbols: &catalogue.symbols,
            interrupts: &catalogue.interrupts,
            lines: &catalogue.lines,
            code: &catalogue.code,
            hooks,
            elapsed,
            tick,
            requeue,
            confirm_blocks,
            block: None,
            precedence,
            idle: None,
            buffering,
            buffer: Vec::new(),
            reinsert: None,
            moved: None,
            base: BTreeMap::new(),
            waits: BTreeMap::new(),
            first,
            tasks: Subjects::default(),
            handlers: Subjects::default(),
            mutexes: Subjects::default(),
            closed: Spans::default(),
            contradictions: Vec::new(),
            faults: Vec::new(),
            instants: Vec::new(),
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
            traced: BTreeMap::new(),
            doubly: BTreeSet::new(),
            replaced: Vec::new(),
            exception_contradictions: Vec::new(),
            costs: Vec::new(),
        })
    }

    /// Where `context` is, by name and value as `kymo decode --meaning`
    /// writes it: `task=<name>`, `task=idle`, `handler=<name>`,
    /// `exception=<number>`, or `context=unknown`.
    pub fn place(&self, context: Context) -> (&'static str, String) {
        match context {
            Context::Task(Some(task)) => ("task", self.tasks.list[task].name.clone()),
            Context::Task(None) => ("task", "idle".to_owned()),
            Context::Handler(handler) => ("handler", self.handlers.list[handler].name.clone()),
            Context::Exception(exception) => ("exception", exception.to_string()),
            Context::Unknown => ("context", "unknown".to_owned()),
        }
    }

    /// Where `faulted` came, then what the fault is (`Fault::facts`).
    pub fn fault_facts(&self, faulted: &Faulted) -> Vec<(&'static str, String)> {
        let mut facts = vec![self.place(faulted.context)];
        facts.extend(faulted.fault.facts(self.symbols, self.lines, self.code));
        facts
    }

    /// `faulted` as `kymo decode --meaning` writes it after its edge: its
    /// facts, each `name=value`.
    pub fn describe_fault(&self, faulted: &Faulted) -> String {
        fault::written(&self.fault_facts(faulted))
    }

    /// The whole of `trace`, a trace of `catalogue`'s image taken in. An
    /// image without a kernel adapter's events is an error.
    pub fn of_trace(catalogue: &'a Catalogue, trace: &Trace) -> Result<Self> {
        let mut meaning = Self::new(catalogue, trace.first)?;
        if !meaning.traced() {
            return Err("the image has no kernel adapter events".into());
        }
        meaning.add(&trace.elements, 0);
        if let Some(watched) = &trace.watched {
            meaning.exceptions(watched, &trace.elements);
        }
        Ok(meaning)
    }

    /// Takes in the exceptions the SWO traced, `watched`'s, once the trace's
    /// `elements` are: each exception's handler is a subject of its own,
    /// named by its interrupt, the handler following in parentheses: certain
    /// when the adapter traces that exception, supposed, with a `?`, from
    /// the image's vector table otherwise. An edge falls after the last event
    /// at or before its cycles. A handler both channels trace keeps the
    /// adapter's spans that touch no stretch of the SWO's coverage alone.
    /// A trace without events takes in none.
    pub fn exceptions(&mut self, watched: &Watched, elements: &[Element]) {
        let events: Vec<(i64, usize)> = elements
            .iter()
            .enumerate()
            .filter_map(|(element, item)| match item {
                Element::Event { cycles, .. } => Some((*cycles, element)),
                _ => None,
            })
            .collect();
        if watched.exceptions.is_empty() || events.is_empty() {
            return;
        }
        let coverage = exceptions::coverage(watched);
        let nested = exceptions::nest(&watched.exceptions, &coverage);
        let edge = |at: exceptions::At| {
            let after = events.partition_point(|&(cycles, _)| cycles <= at.cycles);
            Edge {
                element: after.checked_sub(1).map_or(0, |before| events[before].1),
                cycles: at.cycles,
                lost: at.lost,
            }
        };
        let addresses: BTreeSet<u64> = self
            .traced
            .values()
            .filter(|&&(handler, _)| !self.handlers.is_named(handler))
            .map(|&(handler, _)| self.handlers.list[handler].address)
            .collect();
        let shared = (0..self.handlers.list.len()).filter(|&handler| {
            !self.handlers.is_named(handler) && addresses.contains(&self.handlers.list[handler].address)
        });
        self.doubly = self
            .traced
            .values()
            .map(|&(handler, _)| handler)
            .chain(shared)
            .collect();
        self.replaced = coverage;
        let (replaced, mut interrupts): (Vec<Interrupt>, Vec<Interrupt>) = std::mem::take(&mut self.closed.interrupts)
            .into_iter()
            .partition(|interrupt| self.replaces(interrupt.handler, interrupt.span));
        // By exception and entry: the SWO's spans with no lost edge.
        let mut whole: BTreeMap<(u16, i64), Interrupt> = BTreeMap::new();
        for line in nested.lines {
            let interrupt = Interrupt {
                handler: self.hardware(line.exception),
                depth: line.depth,
                span: Span {
                    from: edge(line.from),
                    to: edge(line.to),
                },
            };
            if !line.from.lost && !line.to.lost {
                whole.insert((line.exception, line.from.cycles), interrupt);
            }
            interrupts.push(interrupt);
        }
        interrupts.sort_by_key(|interrupt| interrupt.span.to.at());
        self.closed.interrupts = interrupts;
        let mut of_handler: BTreeMap<usize, u16> = BTreeMap::new();
        for (&exception, &(handler, _)) in &self.traced {
            of_handler.entry(handler).or_insert(exception);
        }
        let mut of_address: BTreeMap<u64, u16> = BTreeMap::new();
        for (&handler, &exception) in &of_handler {
            if !self.handlers.is_named(handler) {
                of_address
                    .entry(self.handlers.list[handler].address)
                    .or_insert(exception);
            }
        }
        self.costs = replaced
            .iter()
            .filter(|run| !run.span.from.lost && !run.span.to.lost)
            .filter_map(|run| {
                let exception = of_handler.get(&run.handler).or_else(|| {
                    (!self.handlers.is_named(run.handler))
                        .then(|| of_address.get(&self.handlers.list[run.handler].address))
                        .flatten()
                })?;
                let (&(of, _), swo) = whole.range(..=(*exception, run.span.from.cycles)).next_back()?;
                (of == *exception && swo.span.to.cycles >= run.span.to.cycles).then_some(Cost {
                    handler: swo.handler,
                    entry: Span {
                        from: swo.span.from,
                        to: run.span.from,
                    },
                    exit: Span {
                        from: run.span.to,
                        to: swo.span.to,
                    },
                })
            })
            .collect();
        self.exception_contradictions = nested
            .contradictions
            .iter()
            .map(|&cycles| edge(exceptions::At { cycles, lost: false }))
            .collect();
    }

    /// The handler the SWO traces for `exception`.
    fn hardware(&mut self, exception: u16) -> usize {
        let interrupt = self.interrupts.name(exception);
        let handler = match self.traced.get(&exception) {
            Some((_, handler)) => Some((handler.clone(), "")),
            None => self.symbols.vector(exception).map(|handler| (handler, "?")),
        };
        self.handlers.exception(exception, || match handler {
            Some((handler, mark)) if handler != interrupt => format!("{interrupt} ({handler}{mark})"),
            _ => interrupt,
        })
    }

    /// Whether the SWO's span replaces the adapter's `span` of `handler`.
    fn replaces(&self, handler: usize, span: Span) -> bool {
        self.doubly.contains(&handler)
            && self
                .replaced
                .iter()
                .any(|&(from, to)| span.from.cycles <= to && span.to.cycles >= from)
    }

    /// The adapter's handlers the SWO traces too, by name.
    pub fn doubly_traced(&self) -> Vec<String> {
        let names: BTreeSet<&str> = self
            .doubly
            .iter()
            .map(|&handler| self.handlers.list[handler].name.as_str())
            .collect();
        names.into_iter().map(str::to_owned).collect()
    }

    /// The warning that the adapter traces handlers the SWO traces too;
    /// none when it traces none of them.
    pub fn double_trace(&self) -> Option<String> {
        let names = self.doubly_traced();
        (!names.is_empty()).then(|| {
            format!(
                "the SWO traces the exceptions of handlers the adapter traces too, at a cost in slots and cycles: {}",
                names.join(", ")
            )
        })
    }

    /// Whether the image has a kernel adapter's events.
    pub fn traced(&self) -> bool {
        self.hooks.values().any(|hook| !matches!(hook, Hook::Fault(_)))
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
                    let id = event.event.id;
                    let Some(hook) = hooks.get(&id) else {
                        if crate::values::is_application(event.event) {
                            if self.buffering {
                                self.buffer.push((None, Vec::new(), edge));
                            } else {
                                self.instant(edge);
                            }
                        }
                        continue;
                    };
                    if let Hook::Fault(layout) = hook {
                        self.fault(layout.read(&event.payload), edge);
                    } else if !self.buffering {
                        self.hook(hook, &event.payload, edge);
                    } else if matches!(hook, Hook::Starting { .. }) {
                        self.hook(hook, &event.payload, edge);
                        self.flush(&hooks);
                    } else {
                        self.buffer.push((Some(id), event.payload.clone(), edge));
                    }
                }
                Element::Lost { .. } => {
                    self.flush(&hooks);
                    self.lost(Edge {
                        element,
                        cycles: self.last.map_or(0, |last| last.cycles),
                        lost: true,
                    });
                }
                Element::Suspect { .. } => {}
            }
        }
        self.hooks = hooks;
    }

    /// Takes in the hooks held back for the scheduler's start, and the next
    /// ones as they come.
    fn flush(&mut self, hooks: &HashMap<u16, Hook>) {
        self.buffering = false;
        for (id, payload, edge) in std::mem::take(&mut self.buffer) {
            match id {
                Some(id) => self.hook(&hooks[&id], &payload, edge),
                None => self.instant(edge),
            }
        }
    }

    /// An application event at `edge`, in the context an event is emitted
    /// in.
    fn instant(&mut self, edge: Edge) {
        let context = match self.context() {
            Cause::Handler(handler) => Context::Handler(handler),
            Cause::Task(task) => Context::Task(task),
            Cause::Elapsed | Cause::Unknown => Context::Unknown,
        };
        self.instants.push(Instant { edge, context });
    }

    /// The task `field` gives: none for the idle task.
    fn task(&mut self, field: &Field, payload: &[u8]) -> Option<usize> {
        (self.idle != Some(field.read(payload))).then(|| self.tasks.index(self.symbols, field, payload))
    }

    /// The task `field` gives, or else the task on the core: none for the
    /// idle task or while the task on the core is unknown.
    fn task_or_running(&mut self, field: Option<&Field>, payload: &[u8]) -> Option<usize> {
        match field {
            Some(field) => self.task(field, payload),
            None => self.running.and_then(|(task, _)| task),
        }
    }

    /// Gives `task` the priority `priority` from `edge` on, `None` for
    /// unknown.
    fn prioritize(&mut self, task: usize, priority: Option<i32>, edge: Edge) {
        let task = &mut self.tasks.list[task];
        if priority.is_some() {
            task.priority = priority;
        }
        task.priorities.push((edge, priority));
    }

    /// Expects the ready list's move of `task`, whose priority just changed,
    /// if the dialect has one and the task is not known to wait: a waiting
    /// task is in no ready list.
    fn requeue(&mut self, task: usize) {
        if self.requeue && !self.waits.contains_key(&task) {
            self.reinsert = Some(task);
        }
    }

    /// Ends the waits of `task` for mutexes at `edge`.
    fn stop_waiting(&mut self, task: usize, edge: Edge) {
        let ended: Vec<_> = self.waiting.keys().filter(|key| key.1 == task).copied().collect();
        for (mutex, task) in ended {
            let from = self.waiting.remove(&(mutex, task)).expect("a wait just found");
            self.hold(mutex, task, true, from, edge);
        }
    }

    fn hook(&mut self, hook: &Hook, payload: &[u8], edge: Edge) {
        let symbols = self.symbols;
        let reinsert = self.reinsert.take();
        // The priority change the last move of a ready task anticipated, if
        // this hook is it: no move is expected after it.
        let mut anticipated = false;
        if let Some((task, element)) = self.moved.take() {
            anticipated = match hook {
                Hook::TaskPriorityChanged { task: changed, .. } | Hook::TaskBasePriority { task: changed, .. } => {
                    self.task(changed, payload) == Some(task)
                }
                _ => false,
            };
            if !anticipated {
                self.contradictions.push(element);
            }
        }
        if let Some(block) = self.block.take() {
            self.settle(block, hook, payload);
        }
        match hook {
            Hook::Fault(layout) => self.fault(layout.read(payload), edge),
            Hook::Starting { idle } => {
                self.idle.get_or_insert(idle.read(payload));
            }
            Hook::TaskAdded { task, priority, name } => {
                let address = task.read(payload);
                if self.idle == Some(address) {
                    return;
                }
                let task = match name {
                    Some(name) => self.tasks.create(address, name.text(payload)),
                    None => self.tasks.index(symbols, task, payload),
                };
                let priority = priority.priority(payload);
                self.base.insert(task, priority);
                self.waits.insert(task, "created");
                self.prioritize(task, Some(priority), edge);
            }
            Hook::TaskPriorityChanged { task, priority } => {
                let Some(task) = self.task(task, payload) else {
                    return;
                };
                self.prioritize(task, Some(priority.priority(payload)), edge);
                if !anticipated {
                    self.requeue(task);
                }
            }
            Hook::TaskBasePriority { task, priority } => {
                let Some(task) = self.task(task, payload) else {
                    return;
                };
                let base = priority.priority(payload);
                let now = self.tasks.list[task].priorities.last().and_then(|&(_, now)| now);
                // The base priority is the task's own unless it inherited a
                // higher one, which a higher base still overrides.
                let after = match (self.base.insert(task, base), now) {
                    (_, Some(now)) if self.precedence.rank(base) < self.precedence.rank(now) => Some(base),
                    (Some(before), Some(now)) => Some(if before == now { base } else { now }),
                    _ => None,
                };
                self.prioritize(task, after, edge);
                if !anticipated {
                    self.requeue(task);
                }
            }
            Hook::TaskTerminated { task } => {
                let Some(task) = self.task(task, payload) else {
                    return;
                };
                // A task killed while it waits for a mutex waits no more.
                self.stop_waiting(task, edge);
                self.waits.remove(&task);
                self.base.remove(&task);
                self.not_ready(task, edge);
            }
            Hook::TaskNamed { task, name } => {
                let name = name.text(payload);
                if let Some(task) = self.task(task, payload).filter(|_| !name.is_empty()) {
                    self.tasks.list[task].name = name;
                }
            }
            Hook::TaskStarted { task } => {
                let task = self.task(task, payload);
                self.run(task, edge);
            }
            Hook::EnterIdle => self.run(None, edge),
            Hook::TaskReady { task, reason } => {
                let Some(task) = self.task(task, payload) else {
                    return;
                };
                if reinsert == Some(task) {
                    return;
                }
                if self.ready.contains_key(&task) {
                    if self.requeue {
                        self.moved = Some((task, edge.element));
                        return;
                    }
                    return self.contradictions.push(edge.element);
                }
                let reason = match reason {
                    Some(reason) => reason.label(payload),
                    None => {
                        // The kernel takes the mutex again after the wake,
                        // or times out.
                        self.stop_waiting(task, edge);
                        self.waits.remove(&task).unwrap_or("unknown").to_owned()
                    }
                };
                let tick = self
                    .entered
                    .last()
                    .is_some_and(|&(.., exception)| exception.is_some() && exception == self.tick);
                let cause = if tick || self.elapsed.contains(&reason.as_str()) {
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
            Hook::TaskBlocked { task, wait, timeout } => {
                if timeout.as_ref().is_some_and(|timeout| timeout.read(payload) == 0) {
                    return;
                }
                let Some(task) = self.task_or_running(task.as_ref(), payload) else {
                    return;
                };
                self.blocked(Block {
                    task,
                    wait: *wait,
                    mutex: None,
                    edge,
                });
            }
            Hook::TaskSuspended { task } => {
                let Some(task) = self.task(task, payload) else {
                    return;
                };
                self.stop_waiting(task, edge);
                self.waits.insert(task, "suspend");
                self.not_ready(task, edge);
            }
            Hook::IsrEnter { handler, exception } => {
                let exception = exception_of(exception, payload);
                let address = handler.read(payload);
                let subject = self
                    .handlers
                    .interrupt(symbols, self.interrupts, handler, exception, payload);
                if let Some(exception) = exception {
                    self.traced
                        .entry(exception)
                        .or_insert_with(|| (subject, handler_name(symbols, handler, address)));
                }
                self.enter(subject, exception, edge);
            }
            Hook::IsrExitException { exception } => {
                let exception = exception_of(exception, payload);
                let innermost = self
                    .entered
                    .last()
                    .filter(|&&(.., of)| of == exception)
                    .map(|&(handler, ..)| handler);
                match innermost.or_else(|| exception.and_then(|exception| self.handlers.for_exception(exception))) {
                    Some(handler) => self.exit(handler, edge),
                    None if self.loss.is_some() => {}
                    None => self.contradictions.push(edge.element),
                }
            }
            Hook::IsrExit { handler } => {
                let address = handler.read(payload);
                let open = self
                    .entered
                    .last()
                    .map(|&(open, ..)| open)
                    .filter(|&open| !self.handlers.is_named(open) && self.handlers.list[open].address == address);
                let handler = match open {
                    Some(open) => open,
                    None => self
                        .handlers
                        .interrupt(symbols, self.interrupts, handler, None, payload),
                };
                self.exit(handler, edge);
            }
            Hook::KernelEnter {
                handler: name,
                exception,
            } => {
                let handler = self.handlers.named(name);
                self.traced
                    .entry(*exception)
                    .or_insert_with(|| (handler, (*name).to_owned()));
                self.enter(handler, None, edge);
            }
            Hook::KernelExit { handler } => {
                let handler = self.handlers.named(handler);
                self.exit(handler, edge);
            }
            Hook::MutexTaken { mutex, task } => {
                let Some(task) = self.task_or_running(task.as_ref(), payload) else {
                    return;
                };
                let mutex = self.mutexes.index(symbols, mutex, payload);
                if self.held.contains_key(&mutex) {
                    return self.contradictions.push(edge.element);
                }
                if let Some(from) = self.waiting.remove(&(mutex, task)) {
                    self.hold(mutex, task, true, from, edge);
                }
                self.held.insert(mutex, (task, edge));
            }
            Hook::MutexReleased { mutex, task } => {
                let Some(task) = self.task_or_running(task.as_ref(), payload) else {
                    return;
                };
                let mutex = self.mutexes.index(symbols, mutex, payload);
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
                let Some(task) = self.task_or_running(task.as_ref(), payload) else {
                    return;
                };
                let mutex = self.mutexes.index(symbols, mutex, payload);
                if self.waiting.contains_key(&(mutex, task)) {
                    return self.contradictions.push(edge.element);
                }
                self.blocked(Block {
                    task,
                    wait: Some("mutex"),
                    mutex: Some(mutex),
                    edge,
                });
            }
        }
    }

    /// A block reported: made now, or held until the core goes to another
    /// task when the dialect confirms blocks.
    fn blocked(&mut self, block: Block) {
        if self.confirm_blocks {
            self.block = Some(block);
        } else {
            self.block(block);
        }
    }

    /// Makes `block`, at its event.
    fn block(
        &mut self,
        Block {
            task,
            wait,
            mutex,
            edge,
        }: Block,
    ) {
        if let Some(mutex) = mutex {
            self.waiting.insert((mutex, task), edge);
        }
        // A wait that blocks through another, a stream buffer's through a
        // notification's, is the first one.
        if let Some(wait) = wait {
            self.waits.entry(task).or_insert(wait);
        }
        self.not_ready(task, edge);
    }

    /// Settles the block held, by `hook`, the next hook: made by a switch to
    /// another task or a wake of the task; held on through a priority change
    /// or what a handler emits; dropped by anything else, which the task
    /// emits on the core it kept.
    fn settle(&mut self, block: Block, hook: &Hook, payload: &[u8]) {
        let in_handler = !self.entered.is_empty();
        let made = match hook {
            Hook::TaskStarted { task } => Some(self.task(task, payload) != Some(block.task)),
            Hook::TaskReady { task, .. } if self.task(task, payload) == Some(block.task) => Some(true),
            Hook::TaskPriorityChanged { .. }
            | Hook::TaskBasePriority { .. }
            | Hook::IsrEnter { .. }
            | Hook::IsrExit { .. }
            | Hook::IsrExitException { .. }
            | Hook::KernelEnter { .. }
            | Hook::KernelExit { .. } => None,
            _ if in_handler => None,
            _ => Some(false),
        };
        match made {
            Some(true) => self.block(block),
            Some(false) => {}
            None => self.block = Some(block),
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
            (Some(&(open, from, depth, _)), _) if open == handler => {
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

    /// Opens `handler`, run for `exception`, at `edge`, its depth known while
    /// the nesting is.
    fn enter(&mut self, handler: usize, exception: Option<u16>, edge: Edge) {
        let depth = self.nesting_known.then_some(self.entered.len());
        self.entered.push((handler, edge, depth, exception));
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

    /// A fault, in the context the core was in: in thread mode, the task on
    /// the core; in handler mode, the innermost handler entered for the
    /// exception it interrupted. It changes no span: what follows is the
    /// application's handler's.
    fn fault(&mut self, fault: Fault, edge: Edge) {
        let context = match fault.interrupted() {
            None => self.running.map_or(Context::Unknown, |(task, _)| Context::Task(task)),
            Some(exception) => self
                .entered
                .iter()
                .rev()
                .find(|entered| entered.3 == Some(exception))
                .map_or(Context::Exception(exception), |entered| Context::Handler(entered.0)),
        };
        self.faults.push(Faulted { fault, context, edge });
    }

    fn run(&mut self, task: Option<usize>, edge: Edge) {
        // The kernel switches in PendSV, at the lowest priority: it runs with
        // no handler open.
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
        self.reinsert = None;
        self.moved = None;
        self.block = None;
        self.base.clear();
        self.waits.clear();
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
                .map(|&(handler, from, depth, _)| Interrupt {
                    handler,
                    depth,
                    span: Span { from, to },
                })
                .filter(|interrupt| !self.replaces(interrupt.handler, interrupt.span))
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
    /// the time known, and each task where no handler runs. A handler still
    /// open runs to the end of the capture.
    pub fn cpu(&self) -> Cpu {
        let span = |span: &Span| (span.from.cycles, span.to.cycles);
        let handlers = self.closed.interrupts.iter().map(|interrupt| span(&interrupt.span));
        let tasks = self
            .closed
            .runs
            .iter()
            .filter(|run| run.task.is_some())
            .map(|run| span(&run.span));
        let handler_open = self
            .open()
            .interrupts
            .iter()
            .map(|interrupt| interrupt.span.from.cycles)
            .min();
        let task_open = self.running.and_then(|(task, from)| task.map(|_| from.cycles));
        let known = self.known();
        let handlers_known = Covered::union(handlers.clone(), handler_open).within(&known);
        let mut runs = vec![Vec::new(); self.tasks.list.len()];
        for run in &self.closed.runs {
            if let Some(task) = run.task {
                runs[task].push(span(&run.span));
            }
        }
        let each = runs
            .into_iter()
            .enumerate()
            .map(|(task, runs)| {
                let open = self
                    .running
                    .and_then(|(running, from)| (running == Some(task)).then_some(from.cycles));
                Covered::union(runs, open).within(&known).without(&handlers_known)
            })
            .collect();
        Cpu {
            busy: Covered::union(handlers.chain(tasks), handler_open.into_iter().chain(task_open).min()).within(&known),
            handlers: handlers_known,
            tasks: each,
            known,
        }
    }

    /// `cause` as `kymo decode --meaning` prints it: `handler:<name>`,
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

/// The exception `field` gives, IPSR's: none for 0, thread mode, where the
/// handler was called, not taken.
fn exception_of(field: &Field, payload: &[u8]) -> Option<u16> {
    u16::try_from(field.read(payload))
        .ok()
        .filter(|&exception| exception != 0)
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
/// A handler's, the adapter's cost where the SWO traces it too.
pub const ADAPTER_ENTRY: Metric = metric("adapter_entry", false, false);
pub const ADAPTER_EXIT: Metric = metric("adapter_exit", false, false);

const TASK_METRICS: [Metric; 7] = [
    WAKE_LATENCY,
    PREEMPTED_BY_TASKS,
    PREEMPTED_BY_HANDLERS,
    NET,
    GROSS,
    RESPONSE,
    PERIOD,
];
const HANDLER_METRICS: [Metric; 5] = [GROSS, NET, PERIOD, ADAPTER_ENTRY, ADAPTER_EXIT];
const MUTEX_METRICS: [Metric; 2] = [HOLD, WAIT];

/// The time `spans`, sorted and apart, take of `from..to`.
fn overlap(spans: &[Span], from: Edge, to: Edge) -> i64 {
    let first = spans.partition_point(|span| span.to.at() <= from.at());
    spans[first..]
        .iter()
        .take_while(|span| span.from.at() < to.at())
        .map(|span| {
            let low = if span.from.at() > from.at() { span.from } else { from };
            let high = if span.to.at() < to.at() { span.to } else { to };
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
            let from = interrupt.span.from.at();
            while held.last().is_some_and(|&last| interrupts[last].0.span.to.at() <= from) {
                held.pop();
            }
            let parent = held.last().copied();
            held.push(n);
            parent
        })
        .collect()
}

/// Each task's runs, closed or open, in order.
fn task_runs(meaning: &Meaning) -> HashMap<usize, Vec<Span>> {
    let open = meaning.open();
    let mut runs: Vec<Run> = meaning.closed.runs.iter().chain(&open.runs).copied().collect();
    runs.sort_by_key(|run| run.span.from.at());
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
    let first = runs.partition_point(|run| run.to.at() <= span.from.at());
    runs[first..]
        .iter()
        .take_while(|run| run.from.at() <= span.to.at())
        .map(|run| {
            let low = if run.from.at() > span.from.at() {
                run.from
            } else {
                span.from
            };
            let high = if run.to.at() < span.to.at() { run.to } else { span.to };
            (low, high)
        })
        .collect()
}

/// Each task's time ready, activation by activation, those that ended
/// first, then those still open: the activation less the task's runs.
pub fn ready(meaning: &Meaning) -> Vec<Ready> {
    let runs = task_runs(meaning);
    let still_open = meaning.open_activations();
    let activations = meaning
        .activations
        .iter()
        .map(|activation| (activation, false))
        .chain(still_open.iter().map(|activation| (activation, true)));
    let mut out = Vec::new();
    for (activation, open) in activations {
        let (task, span) = (activation.task, activation.span);
        let wake = activation.reason.as_ref().and_then(|_| {
            meaning
                .wakes
                .binary_search_by_key(&span.from.element, |wake| wake.edge.element)
                .ok()
        });
        let mut push = |from: Edge, to: Edge, open| {
            if to.at() > from.at() {
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

/// The instances for kymo::stats, by subject and metric:
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
///   one level within it; the period, from an entry to the next; and for a
///   handler of the SWO that the adapter traces too, the adapter's cost:
///   from the SWO's entry to the adapter's, from the adapter's exit to the
///   SWO's;
/// - a mutex's: each hold and each wait, by task and in total.
///
/// Each instance spans its activation, its handler's run, its period, its
/// hold or wait; those still open at the end of the trace are open.
pub fn instances(meaning: &Meaning) -> Vec<Instance> {
    let open = meaning.open();
    let mut interrupts: Vec<(Interrupt, bool)> = meaning
        .closed
        .interrupts
        .iter()
        .map(|&interrupt| (interrupt, false))
        .collect();
    interrupts.extend(open.interrupts.iter().map(|&interrupt| (interrupt, true)));
    interrupts.sort_by_key(|(interrupt, _)| (interrupt.span.from.at(), std::cmp::Reverse(interrupt.span.to.at())));
    let parents = parents(&interrupts);
    let outermost: Vec<Span> = interrupts
        .iter()
        .zip(&parents)
        .filter(|(_, parent)| parent.is_none())
        .map(|((interrupt, _), _)| interrupt.span)
        .collect();
    let task_runs = task_runs(meaning);

    let still_open = meaning.open_activations();
    // Sorted by kind, subject, metric, detail and start before being handed
    // out; the detail's rank is here, its text in the instance.
    type Order = (u8, usize, usize, Option<u8>);
    // At most 8 instances per activation, 3 per handler run, 2 per hold
    // and 1 per wake: reserved at once, the vector is never copied to grow.
    let holds = meaning.closed.holds.len() + open.holds.len();
    let most = 8 * (meaning.activations.len() + still_open.len())
        + 3 * interrupts.len()
        + 2 * meaning.costs.len()
        + 2 * holds
        + meaning.wakes.len();
    let mut out: Vec<(Order, Instance)> = Vec::with_capacity(most);
    let mut push = |kind: (u8, &'static str),
                    subject: usize,
                    metrics: &[Metric],
                    metric: Metric,
                    detail: Option<(u8, String)>,
                    value: i64,
                    span: Span,
                    open: bool| {
        // A period of the adapter's handler that spans the SWO's coverage
        // misses the spans the SWO's replace.
        if kind.0 == 1 && meaning.replaces(subject, span) {
            return;
        }
        let name = match kind.0 {
            0 => &meaning.tasks.list[subject].name,
            1 => &meaning.handlers.list[subject].name,
            _ => &meaning.mutexes.list[subject].name,
        };
        let rank = metrics.iter().position(|m| *m == metric).expect("a metric of the kind");
        out.push((
            (kind.0, subject, rank, detail.as_ref().map(|(rank, _)| *rank)),
            Instance {
                kind: kind.1,
                subject: name.clone(),
                metric,
                detail: detail.map(|(_, detail)| detail),
                value,
                from: span.from,
                to: span.to,
                open,
                swo: kind.0 == 1 && meaning.handlers.is_hardware(subject),
            },
        ));
    };
    const TASK: (u8, &str) = (0, "task");
    const HANDLER: (u8, &str) = (1, "handler");
    const MUTEX: (u8, &str) = (2, "mutex");
    // The wake latency's details: by reason, then by cause.
    const REASON: u8 = 0;
    const CAUSE: u8 = 1;

    let activations = meaning
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
            task_push(WAKE_LATENCY, Some((CAUSE, meaning.cause(cause))), latency);
        }
        task_push(PREEMPTED_BY_TASKS, None, gross - ran);
        task_push(PREEMPTED_BY_HANDLERS, None, handled);
        task_push(NET, None, ran - handled);
        task_push(GROSS, None, gross);
    }
    let mut wakes: Vec<&Wake> = meaning.wakes.iter().collect();
    wakes.sort_by_key(|wake| (wake.task, wake.edge.at()));
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
    entries.sort_by_key(|&(handler, edge)| (handler, edge.at()));
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
    for cost in &meaning.costs {
        for (metric, span) in [(ADAPTER_ENTRY, cost.entry), (ADAPTER_EXIT, cost.exit)] {
            let value = span.to.cycles - span.from.cycles;
            push(
                HANDLER,
                cost.handler,
                &HANDLER_METRICS,
                metric,
                None,
                value,
                span,
                false,
            );
        }
    }

    let holds = meaning
        .closed
        .holds
        .iter()
        .map(|hold| (hold, false))
        .chain(open.holds.iter().map(|hold| (hold, true)));
    for (hold, open) in holds {
        let metric = if hold.waiting { WAIT } else { HOLD };
        let value = hold.span.to.cycles - hold.span.from.cycles;
        let task = meaning.tasks.list[hold.task].name.clone();
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
    out.sort_by(|(a, x), (b, y)| {
        a.cmp(b)
            .then_with(|| x.detail.cmp(&y.detail))
            .then(x.from.at().cmp(&y.from.at()))
    });
    out.into_iter().map(|(_, instance)| instance).collect()
}

/// `kymo decode --meaning`'s output: the spans of a capture's trace, a line
/// each, those still open at its end last with `to=open`; each task_ready
/// and its cause; each time ready; each fault; then the contradictions. An
/// edge is the index of its event, or `lost`; an edge the SWO dates is
/// `@<cycles>`, or `lost`.
pub fn lines(trace: &Trace, meaning: &Meaning) -> Vec<String> {
    let mut lines = Vec::new();
    let edge = |edge: Edge| match &trace.elements[edge.element] {
        Element::Event { event, .. } if !edge.lost => event.index.to_string(),
        _ => "lost".to_owned(),
    };
    let swo_edge = |edge: Edge| {
        if edge.lost {
            "lost".to_owned()
        } else {
            format!("@{}", edge.cycles)
        }
    };
    let task = |task: Option<usize>| task.map_or("idle", |task| &meaning.tasks.list[task].name);
    let open = meaning.open();
    for (spans, end) in [(&meaning.closed, None), (&open, Some("open"))] {
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
            let hardware = meaning.handlers.is_hardware(interrupt.handler);
            let (from, to) = if hardware {
                (swo_edge(interrupt.span.from), swo_edge(interrupt.span.to))
            } else {
                (edge(interrupt.span.from), to(interrupt.span))
            };
            lines.push(format!(
                "interrupt from={from} to={to} depth={} handler={}",
                interrupt
                    .depth
                    .map_or_else(|| "unknown".to_owned(), |depth| depth.to_string()),
                meaning.handlers.list[interrupt.handler].name
            ));
        }
        for hold in &spans.holds {
            lines.push(format!(
                "{} from={} to={} mutex={} task={}",
                if hold.waiting { "wait" } else { "hold" },
                edge(hold.span.from),
                to(hold.span),
                meaning.mutexes.list[hold.mutex].name,
                task(Some(hold.task))
            ));
        }
    }
    for wake in &meaning.wakes {
        lines.push(format!(
            "wake at={} task={} reason={} cause={}",
            edge(wake.edge),
            task(Some(wake.task)),
            wake.reason,
            meaning.cause(wake.cause)
        ));
    }
    for ready in ready(meaning) {
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
    for faulted in &meaning.faults {
        lines.push(format!(
            "fault at={} {}",
            edge(faulted.edge),
            meaning.describe_fault(faulted)
        ));
    }
    for &element in &meaning.contradictions {
        if let Element::Event { event, .. } = &trace.elements[element] {
            lines.push(format!("contradiction {}", event.index));
        }
    }
    for &edge in &meaning.exception_contradictions {
        lines.push(format!("contradiction {}", swo_edge(edge)));
    }
    lines
}
