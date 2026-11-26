//! The kernel's meaning on the timeline: the spans `kymo::meaning` makes of the trace,
//! a row of spans each for a task, the idle task, a handler at each depth, a
//! mutex's holders and each of its waiters; and each task's time ready, with
//! what woke it. They grow with the trace.

use crate::band::Band;
use crate::levels::Levels;
use crate::names;
use crate::palette::{Palette, Shade};
use crate::spans::Spans;
use eframe::egui::Color32;
use kymo::events::Catalogue;
use kymo::load::{Covered, Cpu};
use kymo::meaning::{Context, Edge, Instant, Meaning as Kernel, Ready, Span, Subject};
use kymo::model::Element;
use kymo::stats::Instance;
use kymo::target::Result;
use std::collections::HashMap;

pub struct Meaning {
    pub kernel: Kernel<'static>,
    /// The closed runs, interrupts and holds already in a row.
    placed: [usize; 3],
    /// By task.
    pub runs: Vec<Spans>,
    pub idle: Spans,
    /// By handler, then by depth; a depth unknown with depth 0, as a handler
    /// that fills the row. A handler's spans never overlap.
    pub interrupts: Vec<Vec<Spans>>,
    /// By mutex.
    pub holds: Vec<Spans>,
    /// By mutex: each waiting task and its waits.
    pub waits: Vec<Vec<(usize, Spans)>>,
    /// Each task's time ready, as `kymo::meaning::ready` gives it, and by task,
    /// places in it.
    pub readies: Vec<Ready>,
    pub ready: Vec<Spans>,
    /// The names the rows show: tasks, handlers, mutexes.
    pub names: Vec<Vec<String>>,
    /// By task, its shade, from its name in full and the shades of the
    /// tasks before it; a task's never changes.
    pub shades: Vec<Shade>,
    pub load: Load,
    /// The CPU's line in bars: what holds the core.
    pub band: Band,
    /// The capture's first event and its last.
    start: Option<Edge>,
    end: Option<Edge>,
    /// The application's events by the context that emitted them, in the
    /// order the contexts first came: their cycles, and their elements.
    pub instants: Vec<(Context, Levels, Vec<usize>)>,
}

impl Meaning {
    /// None when the image has no kernel adapter.
    pub fn new(catalogue: &'static Catalogue, first: u32) -> Result<Option<Self>> {
        let kernel = Kernel::new(catalogue, first)?;
        Ok(kernel.traced().then(|| Self {
            kernel,
            placed: [0; 3],
            runs: Vec::new(),
            idle: Spans::default(),
            interrupts: Vec::new(),
            holds: Vec::new(),
            waits: Vec::new(),
            readies: Vec::new(),
            ready: Vec::new(),
            names: Vec::new(),
            shades: Vec::new(),
            load: Load::default(),
            band: Band::default(),
            start: None,
            end: None,
            instants: Vec::new(),
        }))
    }

    /// Takes in `elements` from `from` on. The spans of a row close in the
    /// order they start, so each comes after those its row holds.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        let instants = self.kernel.instants.len();
        self.kernel.add(elements, from);
        by_context(&mut self.instants, &self.kernel.instants[instants..]);
        let closed = &self.kernel.closed;
        for (n, run) in closed.runs.iter().enumerate().skip(self.placed[0]) {
            let row = match run.task {
                Some(task) => grown(&mut self.runs, task),
                None => &mut self.idle,
            };
            row.push(run.span, n);
        }
        for (n, interrupt) in closed.interrupts.iter().enumerate().skip(self.placed[1]) {
            let depths = grown(&mut self.interrupts, interrupt.handler);
            grown(depths, interrupt.depth.unwrap_or(0)).push(interrupt.span, n);
        }
        for (n, hold) in closed.holds.iter().enumerate().skip(self.placed[2]) {
            if hold.waiting {
                let waiters = grown(&mut self.waits, hold.mutex);
                let at = match waiters.iter().position(|waiter| waiter.0 == hold.task) {
                    Some(at) => at,
                    None => {
                        waiters.push((hold.task, Spans::default()));
                        waiters.len() - 1
                    }
                };
                waiters[at].1.push(hold.span, n);
            } else {
                grown(&mut self.holds, hold.mutex).push(hold.span, n);
            }
        }
        self.placed = [closed.runs.len(), closed.interrupts.len(), closed.holds.len()];
        let kernel = &self.kernel;
        // Whole again: the time ready of an activation still open grows.
        self.readies = kymo::meaning::ready(kernel);
        self.ready.clear();
        for (n, ready) in self.readies.iter().enumerate() {
            grown(&mut self.ready, ready.task).push(ready.span, n);
        }
        self.names = names::shown(&[&kernel.tasks.list, &kernel.handlers.list, &kernel.mutexes.list]);
        for task in &kernel.tasks.list[self.shades.len()..] {
            let shade = Shade::of(&task.name, &self.shades);
            self.shades.push(shade);
        }
        self.load = Load::of(kernel);
        let event = |(element, at): (usize, &Element)| match at {
            Element::Event { cycles, .. } => Some(Edge {
                element,
                cycles: *cycles,
                lost: false,
            }),
            _ => None,
        };
        if self.start.is_none() {
            self.start = elements.iter().enumerate().skip(from).find_map(event);
        }
        let end = elements.iter().enumerate().skip(from).rev().find_map(event);
        self.end = end.or(self.end);
        // Whole again, as the load: the spans still open grow.
        let open = kernel.open();
        let closed = &kernel.closed;
        let runs: Vec<_> = closed
            .runs
            .iter()
            .map(|&run| (run, false))
            .chain(open.runs.iter().map(|&run| (run, true)))
            .collect();
        let interrupts: Vec<_> = closed
            .interrupts
            .iter()
            .map(|&interrupt| (interrupt, false))
            .chain(open.interrupts.iter().map(|&interrupt| (interrupt, true)))
            .collect();
        self.band = Band::of(&runs, &interrupts, self.start.zip(self.end));
    }

    /// The colour of `task`'s runs, the idle task's grey for None.
    pub fn task_color(&self, task: Option<usize>, palette: &Palette) -> Color32 {
        task.map_or(palette.idle, |task| palette.task(self.shades[task]))
    }

    /// The lists of subjects with the names shown.
    fn named(&self) -> Named<'_> {
        let kernel = &self.kernel;
        let names = |n: usize| self.names.get(n).map_or(&[][..], Vec::as_slice);
        [
            ("task", &kernel.tasks.list, names(0)),
            ("handler", &kernel.handlers.list, names(1)),
            ("mutex", &kernel.mutexes.list, names(2)),
        ]
    }

    /// The name shown of the subject an event field points at, by its
    /// address.
    pub fn shown_at(&self, address: u64) -> Option<&str> {
        shown_at(&self.named(), address)
    }

    /// The task at `address`, if one is.
    pub fn task_at(&self, address: u64) -> Option<usize> {
        let tasks = &self.kernel.tasks.list;
        tasks.iter().position(|task| task.address == address && address != 0)
    }

    /// `instances` with the names shown; see [`shorten`].
    pub fn shorten(&self, instances: Vec<Instance>) -> (Vec<Instance>, Full) {
        shorten(&self.named(), instances)
    }
}

/// The subjects of each kind, as `kymo::meaning::instances` names the kinds,
/// with the names shown, in the same order.
type Named<'a> = [(&'static str, &'a [Subject], &'a [String]); 3];

/// The names in full, by kind and name shown; a detail's under the kind
/// `detail`.
pub type Full = HashMap<(&'static str, String), String>;

/// The name shown of the subject at `address`; none at address 0, a
/// subject no event field gives.
fn shown_at<'a>(named: &Named<'a>, address: u64) -> Option<&'a str> {
    named.iter().find_map(|(_, list, names)| {
        let n = list
            .iter()
            .position(|subject| subject.address == address && address != 0)?;
        names.get(n).map(String::as_str)
    })
}

/// The name shown of the subject of `kind` named `name` in full.
fn shown<'a>(named: &Named<'a>, kind: &str, name: &str) -> Option<&'a str> {
    let (_, list, names) = named.iter().find(|(of, ..)| *of == kind)?;
    let n = list.iter().position(|subject| subject.name == name)?;
    names.get(n).map(String::as_str)
}

/// `instances` with the names shown, of their subjects and of the subjects
/// their details name, and the names in full.
fn shorten(named: &Named, instances: Vec<Instance>) -> (Vec<Instance>, Full) {
    let mut full = HashMap::new();
    let instances = instances
        .into_iter()
        .map(|mut instance| {
            if let Some(shown) = shown(named, instance.kind, &instance.subject) {
                let name = std::mem::replace(&mut instance.subject, shown.to_owned());
                full.insert((instance.kind, instance.subject.clone()), name);
            }
            // A cause names its subject after its kind, `handler:name`; a
            // mutex's detail is the task holding or waiting, by its name.
            let mutex = instance.kind == "mutex";
            if let Some(detail) = &mut instance.detail
                && let Some(shown) = if mutex {
                    shown(named, "task", detail).map(str::to_owned)
                } else {
                    detail
                        .split_once(':')
                        .and_then(|(kind, name)| Some(format!("{kind}:{}", shown(named, kind, name)?)))
                }
            {
                let detail = std::mem::replace(detail, shown.clone());
                full.insert(("detail", shown), detail);
            }
            instance
        })
        .collect();
    (instances, full)
}

/// The cycles each row covers, cut to the time known, and the CPU row: what
/// the load view draws (ADR-061).
#[derive(Default)]
pub struct Load {
    pub cpu: Cpu,
    /// By task.
    pub tasks: Vec<Covered>,
    pub idle: Covered,
    /// By handler, at any depth.
    pub handlers: Vec<Covered>,
    /// By mutex, held.
    pub holds: Vec<Covered>,
}

/// A row's spans, closed then the earliest still open.
type Row = (Vec<(i64, i64)>, Option<i64>);

impl Load {
    fn of(kernel: &Kernel) -> Self {
        let cpu = kernel.cpu();
        let open = kernel.open();
        let span = |span: &Span| (span.from.cycles, span.to.cycles);
        let start = |row: &mut Row, from: i64| row.1 = Some(row.1.map_or(from, |open| open.min(from)));
        let mut tasks: Vec<Row> = vec![Row::default(); kernel.tasks.list.len()];
        let mut idle = Row::default();
        for run in &kernel.closed.runs {
            match run.task {
                Some(task) => grown(&mut tasks, task).0.push(span(&run.span)),
                None => idle.0.push(span(&run.span)),
            }
        }
        for run in &open.runs {
            match run.task {
                Some(task) => start(grown(&mut tasks, task), run.span.from.cycles),
                None => start(&mut idle, run.span.from.cycles),
            }
        }
        let mut handlers: Vec<Row> = vec![Row::default(); kernel.handlers.list.len()];
        for interrupt in &kernel.closed.interrupts {
            grown(&mut handlers, interrupt.handler).0.push(span(&interrupt.span));
        }
        for interrupt in &open.interrupts {
            start(grown(&mut handlers, interrupt.handler), interrupt.span.from.cycles);
        }
        let mut holds: Vec<Row> = vec![Row::default(); kernel.mutexes.list.len()];
        for hold in kernel.closed.holds.iter().filter(|hold| !hold.waiting) {
            grown(&mut holds, hold.mutex).0.push(span(&hold.span));
        }
        for hold in open.holds.iter().filter(|hold| !hold.waiting) {
            start(grown(&mut holds, hold.mutex), hold.span.from.cycles);
        }
        let known = &cpu.known;
        let cut = |row: Row| Covered::union(row.0, row.1).within(known);
        Self {
            tasks: tasks.into_iter().map(cut).collect(),
            idle: cut(idle),
            handlers: handlers.into_iter().map(cut).collect(),
            holds: holds.into_iter().map(cut).collect(),
            cpu,
        }
    }
}

/// The time ready, a place in `readies`, that a run starting at `at` ends:
/// in `ready`, the task's, the one reaching its cycles, if its activation
/// holds the run's start.
pub fn before_run(readies: &[Ready], ready: Option<&Spans>, at: Edge) -> Option<usize> {
    let which = ready?.at(at.cycles, at.cycles)?;
    let activation = readies[which].activation;
    (activation.from.element <= at.element && at.element <= activation.to.element).then_some(which)
}

/// The wait for the first run of the activation that time ready `which` is
/// part of: the activation's first time ready, when it starts with it.
pub fn waited(readies: &[Ready], which: usize) -> Option<&Ready> {
    let activation = readies[which].activation;
    let first = readies[..=which]
        .iter()
        .rposition(|other| other.activation != activation)
        .map_or(0, |before| before + 1);
    let first = &readies[first];
    (first.span.from == activation.from).then_some(first)
}

/// `list[n]`, the list grown to hold it.
fn grown<T: Default>(list: &mut Vec<T>, n: usize) -> &mut T {
    if list.len() <= n {
        list.resize_with(n + 1, T::default);
    }
    &mut list[n]
}

/// Application events by the context that emitted them, as
/// `Meaning::instants` holds them, `instants` appended: each to its context's
/// cycles and elements, a context new after the others.
fn by_context(contexts: &mut Vec<(Context, Levels, Vec<usize>)>, instants: &[Instant]) {
    // Each context's cycles, appended to its levels at once.
    let mut cycles: Vec<Vec<i64>> = vec![Vec::new(); contexts.len()];
    for instant in instants {
        let at = match contexts.iter().position(|(context, ..)| *context == instant.context) {
            Some(at) => at,
            None => {
                contexts.push((instant.context, Levels::default(), Vec::new()));
                cycles.push(Vec::new());
                contexts.len() - 1
            }
        };
        cycles[at].push(instant.edge.cycles);
        contexts[at].2.push(instant.edge.element);
    }
    for ((_, levels, _), cycles) in contexts.iter_mut().zip(&cycles) {
        levels.extend(cycles);
    }
}

#[cfg(test)]
mod tests;
