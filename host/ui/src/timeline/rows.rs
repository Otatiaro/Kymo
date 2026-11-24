//! The lines of the timeline, and the rows of event types they show.

use super::{CPU, ROW, TASK, Timeline};
use crate::levels::Levels;
use crate::meaning::Meaning;
use kymo::model::Element;

/// The tasks other than idle in the order of their lines: by priority value,
/// those without one last, then by name.
pub(super) fn tasks(meaning: &Meaning) -> Vec<usize> {
    let list = &meaning.kernel.tasks.list;
    let mut tasks: Vec<usize> = (0..list.len()).collect();
    tasks.sort_by_key(|&task| {
        let task = &list[task];
        (task.priority.is_none(), task.priority, &task.name)
    });
    tasks
}

/// The events of one type, by cycles, and their elements in the trace; its
/// name as shown (names.rs).
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
    /// The CPU's load: handlers, then each task other than idle.
    Cpu,
    /// A task, or the idle task.
    Task(Option<usize>),
    Handler(usize),
    Mutex(usize),
    /// The row of an event type.
    Event(usize),
}

impl Line {
    /// Its height in pixels: the CPU's load the tallest, a task taller than
    /// the other lines.
    pub(super) fn height(self) -> f32 {
        match self {
            Line::Cpu => CPU,
            Line::Task(_) => TASK,
            _ => ROW,
        }
    }
}

/// The top of each of `lines` below the top of the first, then the bottom of
/// the last, in `room` pixels: each line its height, the lines other than
/// sections stretched alike to fill the room they leave.
pub(super) fn tops(lines: &[Line], room: f32) -> Vec<f32> {
    let height = |line: &Line| line.height();
    let fixed: f32 = lines
        .iter()
        .filter(|line| matches!(line, Line::Section(_)))
        .map(height)
        .sum();
    let stretched: f32 = lines.iter().map(height).sum::<f32>() - fixed;
    let scale = if stretched > 0.0 {
        ((room - fixed) / stretched).max(1.0)
    } else {
        1.0
    };
    std::iter::once(0.0)
        .chain(lines.iter().scan(0.0, |top, line| {
            *top += match line {
                Line::Section(_) => line.height(),
                _ => line.height() * scale,
            };
            Some(*top)
        }))
        .collect()
}

/// The line `y` pixels below the top of the first, of those whose `tops`
/// are given.
pub(super) fn at(tops: &[f32], y: f32) -> Option<usize> {
    let n = tops.partition_point(|&top| top <= y);
    (y >= 0.0 && n > 0 && n < tops.len()).then(|| n - 1)
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

    /// The lines: the CPU's load, OpSy's sections, the tasks by priority,
    /// highest first; the rows of event types without OpSy, which the event
    /// list shows with it.
    pub(super) fn lines(&self) -> Vec<Line> {
        let Some(meaning) = &self.meaning else {
            return (0..self.rows.len()).map(Line::Event).collect();
        };
        let kernel = &meaning.kernel;
        let mut lines = vec![Line::Cpu, Line::Section("TASKS")];
        lines.extend(tasks(meaning).into_iter().map(|task| Line::Task(Some(task))));
        lines.push(Line::Task(None));
        if !kernel.handlers.list.is_empty() {
            lines.push(Line::Section("INTERRUPTS"));
            lines.extend((0..kernel.handlers.list.len()).map(Line::Handler));
        }
        if !kernel.mutexes.list.is_empty() {
            lines.push(Line::Section("MUTEXES"));
            lines.extend((0..kernel.mutexes.list.len()).map(Line::Mutex));
        }
        lines
    }

    pub(super) fn label(&self, line: Line) -> String {
        let shown = |kind: usize, n: usize| {
            let meaning = self.meaning.as_ref().expect("a subject's line has OpSy's meaning");
            meaning.names[kind][n].clone()
        };
        match line {
            Line::Section(name) => name.to_owned(),
            Line::Cpu => "CPU".to_owned(),
            Line::Task(None) => "idle".to_owned(),
            // No priority: it changes over the capture, with inheritance.
            Line::Task(Some(n)) => shown(0, n),
            Line::Handler(n) => shown(1, n),
            Line::Mutex(n) => shown(2, n),
            Line::Event(row) => self.rows[row].name.clone(),
        }
    }

    /// A line's label in full: a subject's name as in the image, else the
    /// label.
    pub(super) fn full_label(&self, line: Line) -> String {
        let kernel = self.meaning.as_ref().map(|meaning| &meaning.kernel);
        let name = match (line, kernel) {
            (Line::Task(Some(n)), Some(kernel)) => kernel.tasks.list.get(n),
            (Line::Handler(n), Some(kernel)) => kernel.handlers.list.get(n),
            (Line::Mutex(n), Some(kernel)) => kernel.mutexes.list.get(n),
            _ => None,
        };
        if let Line::Event(row) = line {
            return self.catalogue.events[&self.ids[row]].name.clone();
        }
        name.map_or_else(|| self.label(line), |subject| subject.name.clone())
    }

    /// How far a line's label is indented, in pixels: a section's lines
    /// under it.
    pub(super) fn indent(line: Line) -> f32 {
        match line {
            Line::Section(_) | Line::Cpu | Line::Event(_) => 0.0,
            Line::Task(_) | Line::Handler(_) | Line::Mutex(_) => 12.0,
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

#[cfg(test)]
mod tests;
