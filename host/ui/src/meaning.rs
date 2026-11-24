//! OpSy's meaning on the timeline: the spans `kymo::opsy` makes of the trace,
//! a row of spans each for a task, the idle task, a handler at each depth, a
//! mutex's holders and each of its waiters. They grow with the trace.

use crate::names;
use crate::spans::Spans;
use kymo::events::Catalogue;
use kymo::model::Element;
use kymo::opsy::Opsy;
use kymo::target::Result;

pub struct Meaning {
    pub opsy: Opsy<'static>,
    /// The closed runs, interrupts and holds already in a row.
    placed: [usize; 3],
    /// By task.
    pub runs: Vec<Spans>,
    pub idle: Spans,
    /// By handler, then by depth.
    pub interrupts: Vec<Vec<Spans>>,
    /// By mutex.
    pub holds: Vec<Spans>,
    /// By mutex: each waiting task and its waits.
    pub waits: Vec<Vec<(usize, Spans)>>,
    /// The names the rows show: tasks, handlers, mutexes.
    pub names: Vec<Vec<String>>,
}

impl Meaning {
    /// None when the image has no OpSy adapter.
    pub fn new(catalogue: &'static Catalogue, first: u32) -> Result<Option<Self>> {
        let opsy = Opsy::new(catalogue, first)?;
        Ok(opsy.traced().then(|| Self {
            opsy,
            placed: [0; 3],
            runs: Vec::new(),
            idle: Spans::default(),
            interrupts: Vec::new(),
            holds: Vec::new(),
            waits: Vec::new(),
            names: Vec::new(),
        }))
    }

    /// Takes in `elements` from `from` on. The spans of a row close in the
    /// order they start, so each comes after those its row holds.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        self.opsy.add(elements, from);
        let closed = &self.opsy.closed;
        for (n, run) in closed.runs.iter().enumerate().skip(self.placed[0]) {
            let row = match run.task {
                Some(task) => grown(&mut self.runs, task),
                None => &mut self.idle,
            };
            row.push(run.span, n);
        }
        for (n, interrupt) in closed.interrupts.iter().enumerate().skip(self.placed[1]) {
            let depths = grown(&mut self.interrupts, interrupt.handler);
            grown(depths, interrupt.depth).push(interrupt.span, n);
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
        let opsy = &self.opsy;
        self.names = names::shown(&[&opsy.tasks.list, &opsy.handlers.list, &opsy.mutexes.list]);
    }
}

/// `list[n]`, the list grown to hold it.
fn grown<T: Default>(list: &mut Vec<T>, n: usize) -> &mut T {
    if list.len() <= n {
        list.resize_with(n + 1, T::default);
    }
    &mut list[n]
}
