//! The lines of the timeline, and the rows of event types they show.

use super::tree::{self, Guide, Kind};
use super::{CPU, GAP, LANE, ROW, TASK, Timeline, VALUE};
use crate::levels::Levels;
use crate::meaning::Meaning;
use crate::names;
use kymo::meaning::Context;
use kymo::model::Element;

/// The tasks other than idle in the order of their lines: by namespace, then
/// highest priority first, as the kernel ranks them, those without one last,
/// then by name.
pub(super) fn tasks(meaning: &Meaning) -> Vec<usize> {
    let list = &meaning.kernel.tasks.list;
    let precedence = meaning.kernel.precedence;
    let names: Vec<&str> = list.iter().map(|task| task.name.as_str()).collect();
    by_namespace_then(&names, |task| {
        let task = &list[task];
        (
            task.priority.is_none(),
            task.priority.map(|priority| precedence.rank(priority)),
        )
    })
}

/// The indices of `names` by namespace, a namespace's own before its nested
/// ones' (names.rs); within one, by `key`, then by name.
pub(super) fn by_namespace_then<K: Ord>(names: &[&str], key: impl Fn(usize) -> K) -> Vec<usize> {
    let spaces: Vec<Vec<&str>> = names
        .iter()
        .map(|name| {
            let mut scopes = names::scopes(name);
            scopes.pop();
            scopes
        })
        .collect();
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by(|&a, &b| {
        spaces[a]
            .cmp(&spaces[b])
            .then_with(|| key(a).cmp(&key(b)))
            .then_with(|| names[a].cmp(names[b]))
    });
    order
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
    /// The durations of a type, its place in `Durations::types`, in as many
    /// lanes as it shows.
    Duration(usize, usize),
    /// A curve chosen, its place in `Values::curves`.
    Value(usize),
    /// The heading of a namespace, the `n`th of the column (tree.rs).
    Group(usize),
}

impl Line {
    /// Its height in pixels: the CPU's load the tallest, a task taller than
    /// the other lines, durations a lane higher for each lane past the first,
    /// a curve room for its steps.
    pub(super) fn height(self) -> f32 {
        match self {
            Line::Cpu => CPU,
            Line::Task(_) => TASK,
            Line::Duration(_, lanes) => ROW + LANE * lanes.saturating_sub(1) as f32,
            Line::Value(_) => VALUE,
            _ => ROW,
        }
    }

    /// Whether it keeps its height when the lines stretch: a section's
    /// title and a namespace's heading, which show no data.
    fn fixed(self) -> bool {
        matches!(self, Line::Section(_) | Line::Group(_))
    }

    /// Whether it is a subject's or an event type's, which the tree arranges.
    fn arranged(self) -> bool {
        matches!(
            self,
            Line::Task(Some(_))
                | Line::Handler(_)
                | Line::Mutex(_)
                | Line::Event(_)
                | Line::Duration(..)
                | Line::Value(_)
        )
    }
}

/// A line's entry in the column of names.
pub(super) struct Name {
    pub(super) text: String,
    /// Its name in full, for the hover.
    pub(super) full: String,
    /// How far it is indented, in pixels, before its guides.
    pub(super) indent: f32,
    pub(super) depth: usize,
    pub(super) guide: Guide,
    /// A heading's key in the headings folded, and whether it is.
    pub(super) heading: Option<(String, bool)>,
}

/// The lines of an image with a kernel adapter, of the `tasks`, `handlers`
/// and `mutexes` in order, those `shown` keeps: the CPU's load first and
/// idle last of its section, whatever `shown` says; a section without a
/// line kept left out.
pub(super) fn kernel_lines(
    tasks: Vec<usize>,
    handlers: Vec<usize>,
    mutexes: Vec<usize>,
    shown: impl Fn(Line) -> bool,
) -> Vec<Line> {
    let mut lines = vec![Line::Cpu, Line::Section("TASKS")];
    lines.extend(
        tasks
            .into_iter()
            .map(|task| Line::Task(Some(task)))
            .filter(|&line| shown(line)),
    );
    lines.push(Line::Task(None));
    for (name, section) in [
        (
            "INTERRUPTS",
            handlers.into_iter().map(Line::Handler).collect::<Vec<_>>(),
        ),
        ("MUTEXES", mutexes.into_iter().map(Line::Mutex).collect()),
    ] {
        let kept: Vec<Line> = section.into_iter().filter(|&line| shown(line)).collect();
        if !kept.is_empty() {
            lines.push(Line::Section(name));
            lines.extend(kept);
        }
    }
    lines
}

/// The lines a wake's arrow joins, from the line of what woke the task to
/// the task's: none when either is not shown.
pub(super) fn wake_rows(lines: &[Line], from: Line, to: Line) -> Option<(usize, usize)> {
    let row = |line: Line| lines.iter().position(|&other| other == line);
    Some((row(from)?, row(to)?))
}

/// The line a fault that came in `context` is marked on: the task's or the
/// handler's, else the CPU's; none when neither is shown.
pub(super) fn fault_row(lines: &[Line], context: Context) -> Option<usize> {
    let row = |line: Line| lines.iter().position(|&other| other == line);
    let own = match context {
        Context::Task(task) => row(Line::Task(task)),
        Context::Handler(handler) => row(Line::Handler(handler)),
        Context::Exception(_) | Context::Unknown => None,
    };
    own.or_else(|| row(Line::Cpu))
}

/// The top of each of `lines` below the top of the first, then the bottom of
/// the last, in `room` pixels: each line its height, the lines other than
/// sections stretched alike to fill the room they leave.
pub(super) fn tops(lines: &[Line], room: f32) -> Vec<f32> {
    let height = |line: &Line| line.height();
    let fixed: f32 = lines.iter().filter(|line| line.fixed()).map(height).sum();
    let stretched: f32 = lines.iter().map(height).sum::<f32>() - fixed;
    let scale = if stretched > 0.0 {
        ((room - fixed) / stretched).max(1.0)
    } else {
        1.0
    };
    std::iter::once(0.0)
        .chain(lines.iter().scan(0.0, |top, line| {
            *top += if line.fixed() {
                line.height()
            } else {
                line.height() * scale
            };
            Some(*top)
        }))
        .collect()
}

/// The line `y` pixels below the top of the first, of those whose `tops`
/// are given.
/// The bar of lane `lane` of `lanes`, from the top, in a line's bars from
/// `high` to `low`: the lanes share them evenly, `GAP` apart.
pub(super) fn lane((high, low): (f32, f32), lane: usize, lanes: usize) -> (f32, f32) {
    let step = (low - high + GAP) / lanes.max(1) as f32;
    let from = high + step * lane as f32;
    (from, from + step - GAP)
}

/// The lane of `lanes` at `y`, in a line's bars from `high` to `low`.
pub(super) fn lane_at((high, low): (f32, f32), y: f32, lanes: usize) -> usize {
    let step = (low - high + GAP) / lanes.max(1) as f32;
    (((y - high) / step).max(0.0) as usize).min(lanes.saturating_sub(1))
}

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
        if let Some(durations) = &mut self.durations {
            durations.add(&self.trace.elements, from);
        }
        if let Some(values) = &mut self.values {
            values.add(&self.trace.elements, from);
        }
    }

    /// The lines the namespace filter shows: the CPU's load, the kernel's
    /// sections, the tasks by priority, highest first, the handlers and the
    /// mutexes by namespace (names.rs); the rows of event types without a
    /// kernel adapter, which the event list shows with one; then the
    /// durations, by namespace; then the curves chosen, by namespace.
    pub(super) fn lines(&self) -> Vec<Line> {
        let shown = |line: Line| self.line_shown(line);
        let mut lines = match &self.meaning {
            None => {
                let types: Vec<&str> = self
                    .ids
                    .iter()
                    .map(|id| self.catalogue.events[id].name.as_str())
                    .collect();
                by_namespace_then(&types, |_| ())
                    .into_iter()
                    .map(Line::Event)
                    .filter(|&line| shown(line))
                    .collect()
            }
            Some(meaning) => {
                let kernel = &meaning.kernel;
                kernel_lines(
                    tasks(meaning),
                    names::by_namespace(&kernel.handlers.list),
                    names::by_namespace(&kernel.mutexes.list),
                    shown,
                )
            }
        };
        if let Some(durations) = &self.durations {
            let open = durations.open();
            let types: Vec<&str> = durations.types.iter().map(String::as_str).collect();
            let kept: Vec<Line> = by_namespace_then(&types, |_| ())
                .into_iter()
                .map(|row| Line::Duration(row, durations.shown(row, &open)))
                .filter(|&line| shown(line))
                .collect();
            if !kept.is_empty() {
                lines.push(Line::Section("DURATIONS"));
                lines.extend(kept);
            }
        }
        if let Some(values) = &self.values {
            let chosen: Vec<usize> = values.chosen.iter().copied().collect();
            let names: Vec<String> = chosen.iter().map(|&n| values.curves[n].to_string()).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let kept: Vec<Line> = by_namespace_then(&names, |_| ())
                .into_iter()
                .map(|n| Line::Value(chosen[n]))
                .filter(|&line| shown(line))
                .collect();
            if !kept.is_empty() {
                lines.push(Line::Section("VALUES"));
                lines.extend(kept);
            }
        }
        lines
    }

    /// Whether the namespace filter shows `line`: the CPU's and idle's
    /// always, the others by their subject's or their type's namespace.
    fn line_shown(&self, line: Line) -> bool {
        let spaces = &self.spaces;
        let space = match line {
            Line::Task(Some(n)) => spaces.subjects[0].get(n),
            Line::Handler(n) => spaces.subjects[1].get(n),
            Line::Mutex(n) => spaces.subjects[2].get(n),
            Line::Event(row) => spaces.types.get(row),
            Line::Duration(row, _) => spaces.durations.get(row),
            Line::Value(n) => spaces.values.get(n),
            Line::Section(_) | Line::Cpu | Line::Task(None) | Line::Group(_) => None,
        };
        space.is_none_or(|space| self.filter.shown(space))
    }

    /// The lines the filter shows, the tree's headings among them, those
    /// under a heading folded left out; and their names in the column.
    pub(super) fn layout(&self) -> (Vec<Line>, Vec<Name>) {
        let lines = self.lines();
        let mut laid = Vec::with_capacity(lines.len());
        let mut shown = Vec::with_capacity(lines.len());
        let mut section = "";
        let mut headings = 0;
        let mut n = 0;
        while n < lines.len() {
            let line = lines[n];
            if !line.arranged() {
                if let Line::Section(name) = line {
                    section = name;
                }
                laid.push(line);
                shown.push(Name {
                    text: self.label(line),
                    full: self.full_label(line),
                    indent: Self::indent(line),
                    depth: 0,
                    guide: Guide::default(),
                    heading: None,
                });
                n += 1;
                continue;
            }
            let end = n + lines[n..].iter().take_while(|line| line.arranged()).count();
            let run = &lines[n..end];
            let fulls: Vec<String> = run.iter().map(|&line| self.full_label(line)).collect();
            let scopes: Vec<Vec<&str>> = fulls.iter().map(|full| names::scopes(full)).collect();
            let key = |path: &str| format!("{section}/{path}");
            let nodes = tree::tree(&scopes, |path| self.folded.contains(&key(path)));
            let depths: Vec<usize> = nodes.iter().map(|node| node.depth).collect();
            for (node, guide) in nodes.into_iter().zip(tree::guides(&depths)) {
                let (line, text, full, heading) = match node.kind {
                    Kind::Heading { path, folded, count } => {
                        headings += 1;
                        let text = match folded {
                            true => format!("{} ({count})", node.text),
                            false => node.text,
                        };
                        let heading = Some((key(&path), folded));
                        (Line::Group(headings - 1), text, path, heading)
                    }
                    Kind::Line(m) => {
                        // Names alike take their address (names.rs).
                        let text = match self.label(run[m]).rsplit_once(" @0x") {
                            Some((_, address)) => format!("{} @0x{address}", node.text),
                            None => node.text,
                        };
                        (run[m], text, fulls[m].clone(), None)
                    }
                };
                laid.push(line);
                shown.push(Name {
                    text,
                    full,
                    indent: Self::indent(run[0]),
                    depth: node.depth,
                    guide,
                    heading,
                });
            }
            n = end;
        }
        (laid, shown)
    }

    pub(super) fn label(&self, line: Line) -> String {
        let shown = |kind: usize, n: usize| {
            let meaning = self
                .meaning
                .as_ref()
                .expect("a subject's line has the kernel's meaning");
            meaning.names[kind][n].clone()
        };
        match line {
            Line::Section(name) => name.to_owned(),
            Line::Group(_) => String::new(),
            Line::Cpu => "CPU".to_owned(),
            Line::Task(None) => "idle".to_owned(),
            // No priority: it changes over the capture, with inheritance.
            Line::Task(Some(n)) => shown(0, n),
            Line::Handler(n) => shown(1, n),
            Line::Mutex(n) => shown(2, n),
            Line::Event(row) => self.rows[row].name.clone(),
            Line::Duration(row, _) => self.durations().names[row].clone(),
            Line::Value(n) => self.values().curves[n].to_string(),
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
        match line {
            Line::Event(row) => return self.catalogue.events[&self.ids[row]].name.clone(),
            Line::Duration(row, _) => return self.durations().types[row].clone(),
            _ => {}
        }
        name.map_or_else(|| self.label(line), |subject| subject.name.clone())
    }

    /// How far a line's label is indented, in pixels: a section's lines
    /// under it.
    pub(super) fn indent(line: Line) -> f32 {
        match line {
            Line::Section(_) | Line::Cpu | Line::Event(_) => 0.0,
            Line::Task(_)
            | Line::Handler(_)
            | Line::Mutex(_)
            | Line::Duration(..)
            | Line::Value(_)
            | Line::Group(_) => 12.0,
        }
    }

    /// The durations, which a line of durations comes from.
    pub(super) fn durations(&self) -> &crate::durations::Durations {
        self.durations.as_ref().expect("a line of durations has the durations")
    }

    /// The values, which a curve's line comes from.
    pub(super) fn values(&self) -> &crate::values::Values {
        self.values.as_ref().expect("a curve's line has the values")
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
