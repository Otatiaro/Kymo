//! OpSy's meaning on the timeline: the spans `kymo::meaning` makes of the trace,
//! a row of spans each for a task, the idle task, a handler at each depth, a
//! mutex's holders and each of its waiters; and each task's time ready, with
//! what woke it. They grow with the trace.

use crate::names;
use crate::spans::Spans;
use kymo::events::Catalogue;
use kymo::load::{Covered, Cpu};
use kymo::meaning::{Edge, Meaning as Kernel, Ready, Span};
use kymo::model::Element;
use kymo::target::Result;

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
    pub load: Load,
}

impl Meaning {
    /// None when the image has no OpSy adapter.
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
            load: Load::default(),
        }))
    }

    /// Takes in `elements` from `from` on. The spans of a row close in the
    /// order they start, so each comes after those its row holds.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        self.kernel.add(elements, from);
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
        self.load = Load::of(kernel);
    }
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

#[cfg(test)]
mod tests;
