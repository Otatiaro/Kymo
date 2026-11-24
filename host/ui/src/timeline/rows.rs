//! The lines of the timeline, and the rows of event types they show.

use super::Timeline;
use crate::levels::Levels;
use kymo::model::Element;

/// The events of one type, by cycles, and their elements in the trace.
pub(super) struct Row {
    pub(super) name: String,
    pub(super) levels: Levels,
    pub(super) elements: Vec<usize>,
}

/// A loss or suspect line, between the cycles of the events recorded
/// before and after it.
#[derive(Clone, Copy)]
pub(super) struct Mark {
    pub(super) from: i64,
    pub(super) to: i64,
    pub(super) element: usize,
}

/// A line of the timeline, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Line {
    Section(&'static str),
    /// The CPU's load: handlers, then tasks other than idle.
    Cpu,
    /// The line that folds and unfolds the rows of event types.
    Fold,
    /// A task, or the idle task.
    Task(Option<usize>),
    Handler(usize),
    Mutex(usize),
    /// The row of an event type.
    Event(usize),
}

impl Timeline {
    /// Takes in the trace's elements from `from` on. Each row's new events,
    /// sorted, come after those it holds.
    pub(super) fn add(&mut self, from: usize) {
        let mut added: Vec<Vec<(i64, usize)>> = vec![Vec::new(); self.rows.len()];
        for (n, element) in self.trace.elements.iter().enumerate().skip(from) {
            match element {
                Element::Event { event, cycles, .. } => {
                    let cycles = *cycles;
                    let row = self
                        .ids
                        .binary_search(&event.event.id)
                        .expect("a type of the catalogue");
                    added[row].push((cycles, n));
                    let before = self.before.unwrap_or(cycles);
                    self.marks.extend(self.pending.drain(..).map(|element| Mark {
                        from: before.min(cycles),
                        to: before.max(cycles),
                        element,
                    }));
                    self.before = Some(cycles);
                    self.origin.get_or_insert(cycles);
                    let (lowest, highest) = self.bounds.get_or_insert((cycles, cycles));
                    *lowest = (*lowest).min(cycles);
                    *highest = (*highest).max(cycles);
                    self.events += 1;
                }
                Element::Lost { from, to } => {
                    self.losses += 1;
                    self.lost += u64::from(to.wrapping_sub(*from));
                    self.pending.push(n);
                }
                Element::Suspect { writers, .. } => {
                    self.suspects += u64::from(*writers);
                    self.pending.push(n);
                }
            }
        }
        self.marks.sort_by_key(|mark| mark.from);
        for (row, mut events) in self.rows.iter_mut().zip(added) {
            events.sort_by_key(|event| event.0);
            let cycles: Vec<i64> = events.iter().map(|event| event.0).collect();
            row.levels.extend(&cycles);
            row.elements.extend(events.iter().map(|event| event.1));
        }
        self.list.add(&self.trace.elements, from);
        if let Some(meaning) = &mut self.meaning {
            meaning.add(&self.trace.elements, from);
        }
    }

    /// The lines: the CPU's load, OpSy's sections, the tasks by priority, highest first,
    /// then the rows of event types unless folded; those rows alone without
    /// OpSy.
    pub(super) fn lines(&self) -> Vec<Line> {
        let events = (0..self.rows.len()).map(Line::Event);
        let Some(meaning) = &self.meaning else {
            return events.collect();
        };
        let opsy = &meaning.opsy;
        let mut tasks: Vec<usize> = (0..opsy.tasks.list.len()).collect();
        tasks.sort_by_key(|&task| {
            let task = &opsy.tasks.list[task];
            (task.priority.is_none(), task.priority, &task.name)
        });
        let mut lines = vec![Line::Cpu, Line::Section("TASKS")];
        lines.extend(tasks.into_iter().map(|task| Line::Task(Some(task))));
        lines.push(Line::Task(None));
        if !opsy.handlers.list.is_empty() {
            lines.push(Line::Section("INTERRUPTS"));
            lines.extend((0..opsy.handlers.list.len()).map(Line::Handler));
        }
        if !opsy.mutexes.list.is_empty() {
            lines.push(Line::Section("MUTEXES"));
            lines.extend((0..opsy.mutexes.list.len()).map(Line::Mutex));
        }
        lines.push(Line::Fold);
        if !self.folded {
            lines.extend(events);
        }
        lines
    }

    pub(super) fn label(&self, line: Line) -> String {
        let shown = |kind: usize, n: usize| {
            let meaning = self.meaning.as_ref().expect("a subject's line has OpSy's meaning");
            &meaning.names[kind][n]
        };
        match line {
            Line::Section(name) => name.to_owned(),
            Line::Cpu => "CPU".to_owned(),
            // After the triangle `timeline` draws: the font has no glyph for it.
            Line::Fold => format!("    EVENTS ({} types)", self.rows.len()),
            Line::Task(None) => "  idle".to_owned(),
            // No priority: it changes over the capture, with inheritance.
            Line::Task(Some(n)) => format!("  {}", shown(0, n)),
            Line::Handler(n) => format!("  {}", shown(1, n)),
            Line::Mutex(n) => format!("  {}", shown(2, n)),
            Line::Event(row) => self.rows[row].name.clone(),
        }
    }

    /// The marks, those after the last event at its cycles.
    pub(super) fn marks(&self) -> impl Iterator<Item = Mark> + '_ {
        let after = self.before.unwrap_or_default();
        self.marks
            .iter()
            .copied()
            .chain(self.pending.iter().map(move |&element| Mark {
                from: after,
                to: after,
                element,
            }))
    }
}
