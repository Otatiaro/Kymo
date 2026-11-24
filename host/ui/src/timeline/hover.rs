//! What the pointer hovers, described.

use super::load::{self, Drawn, percent};
use super::rows::Line;
use super::status::counted;
use super::view::View;
use super::{HOVER, ROW, SWITCH, Timeline};
use crate::meaning::{self, Meaning};
use eframe::egui::{Pos2, Rect};
use kymo::decode;
use kymo::model::Element;
use kymo::opsy::{self, Cause, Edge, Span};
use std::fmt::Write;

impl Timeline {
    /// What the pointer is over: the nearest event of its row, or the span,
    /// and the marks at its column; over a task's span, the wake to draw.
    pub(super) fn hovered(
        &self,
        view: &View,
        area: Rect,
        pointer: Pos2,
        lines: &[Line],
        open: &opsy::Spans,
        drawn: &[(usize, Vec<Drawn>)],
    ) -> Option<(String, Option<Arrow>)> {
        if pointer.x < area.left() || pointer.y < area.top() {
            return None;
        }
        let low = view.cycles(pointer.x - HOVER);
        let high = view.cycles(pointer.x + HOVER);
        let mut text = String::new();
        let mut arrow = None;
        let n = ((pointer.y - area.top()) / ROW) as usize;
        let line = lines.get(n).copied();
        let bins = drawn.iter().find(|(line, _)| *line == n).map(|(_, bins)| bins);
        if let (Some(meaning), Some(line)) = (&self.meaning, line) {
            (text, arrow) = match bins {
                Some(bins) => (
                    load::at(bins, pointer.x)
                        .map(|bin| self.load_text(meaning, line, bin, open))
                        .unwrap_or_default(),
                    None,
                ),
                None => self.span_text(meaning, line, low, high, open),
            };
        }
        if let Some(Line::Event(row)) = line {
            let row = &self.rows[row];
            let cycles = row.levels.cycles();
            let from = cycles.partition_point(|&cycles| cycles < low);
            let to = cycles.partition_point(|&cycles| cycles <= high);
            let at = view.cycles(pointer.x);
            if let Some(nearest) = (from..to).min_by_key(|&n| (cycles[n] - at).abs())
                && let Element::Event { event, cycles, seconds } = &self.trace.elements[row.elements[nearest]]
            {
                text = decode::line(&self.catalogue.symbols, event, *cycles, *seconds);
                if to - from > 1 {
                    let _ = write!(text, "\n{} events of this type within {HOVER} pixels", to - from);
                }
            }
        }
        for mark in self.marks().filter(|mark| mark.from <= high && mark.to >= low).take(8) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&marked(&self.trace.elements[mark.element]).unwrap_or_default());
        }
        (!text.is_empty()).then_some((text, arrow))
    }
}

/// A loss or a suspect line, described; none for an event.
pub fn marked(element: &Element) -> Option<String> {
    match *element {
        Element::Lost { from, to } => {
            let slots = counted(u64::from(to.wrapping_sub(from)), "slot", "slots");
            Some(format!("gap between indexes {from} and {to}: {slots} lost"))
        }
        Element::Suspect { from, to, writers } => {
            let writes = counted(u64::from(writers), "write", "writes");
            Some(format!(
                "write overrun between indexes {from} and {to}: {writes} interrupted for a full ring lap; \
                 events recorded near it may be wrong"
            ))
        }
        Element::Event { .. } => None,
    }
}

impl Timeline {
    /// The span of an OpSy `line` from `low` to `high` cycles, described,
    /// empty when there is none; and for a task's, the wake to draw.
    fn span_text(
        &self,
        meaning: &Meaning,
        line: Line,
        low: i64,
        high: i64,
        open: &opsy::Spans,
    ) -> (String, Option<Arrow>) {
        let opsy = &meaning.opsy;
        let closed = &opsy.closed;
        let near = |span: &Span| span.from.cycles <= high && span.to.cycles >= low;
        let named = |task: Option<usize>| task.map_or("idle", |task| opsy.tasks.list[task].name.as_str());
        let mut text = String::new();
        let mut arrow = None;
        match line {
            Line::Task(task) => {
                let spans = task.map_or(Some(&meaning.idle), |task| meaning.runs.get(task));
                let found = spans
                    .and_then(|spans| spans.at(low, high))
                    .map(|which| (closed.runs[which], false))
                    .or_else(|| {
                        open.runs
                            .iter()
                            .find(|run| run.task == task && near(&run.span))
                            .map(|&run| (run, true))
                    });
                let ready = task.and_then(|task| meaning.ready.get(task));
                if found.is_none()
                    && let Some(which) = ready.and_then(|ready| ready.at(low, high))
                {
                    let piece = &meaning.readies[which];
                    let still = piece.open;
                    let _ = write!(
                        text,
                        "{} ready, not on the core\n{}",
                        named(task),
                        self.extent(piece.span, still)
                    );
                    let (woken, from) = self.woken(meaning, which);
                    let _ = write!(text, "\n{woken}");
                    arrow = from;
                }
                if let Some((run, still)) = found {
                    let _ = write!(text, "{}\n{}", named(run.task), self.extent(run.span, still));
                    if let Some(which) = meaning::before_run(&meaning.readies, ready, run.span.from) {
                        let (woken, from) = self.woken(meaning, which);
                        let _ = write!(text, "\n{woken}");
                        arrow = from;
                    }
                    // Left out where no event has given it yet, or since a loss.
                    let at = low + (high - low) / 2;
                    if let Some(priority) = run.task.and_then(|task| opsy.tasks.list[task].priority_at(at)) {
                        let _ = write!(text, "\npriority {priority} there, lower is higher");
                    }
                    if !still && !run.span.to.lost {
                        text.push_str("\nlast events, the switch last:");
                        for line in self.events_before(run.span) {
                            let _ = write!(text, "\n  {line}");
                        }
                    }
                }
            }
            Line::Handler(handler) => {
                let depths = meaning.interrupts.get(handler).map_or(&[][..], Vec::as_slice);
                // The innermost of the handler's spans there.
                let found = open
                    .interrupts
                    .iter()
                    .filter(|interrupt| interrupt.handler == handler && near(&interrupt.span))
                    .map(|&interrupt| (interrupt, true))
                    .chain(
                        depths
                            .iter()
                            .filter_map(|spans| spans.at(low, high))
                            .map(|which| (closed.interrupts[which], false)),
                    )
                    .max_by_key(|(interrupt, _)| interrupt.depth);
                if let Some((interrupt, still)) = found {
                    let below = interrupt.depth.map_or_else(
                        || "handlers below unknown since a loss".to_owned(),
                        |depth| format!("{depth} handlers below"),
                    );
                    let _ = write!(
                        text,
                        "{}, {below}\n{}",
                        opsy.handlers.list[handler].name,
                        self.extent(interrupt.span, still)
                    );
                }
            }
            Line::Mutex(mutex) => {
                let holds = meaning.holds.get(mutex).and_then(|spans| spans.at(low, high));
                let waits = meaning.waits.get(mutex).into_iter().flatten();
                let found = holds
                    .into_iter()
                    .chain(waits.filter_map(|(_, spans)| spans.at(low, high)))
                    .map(|which| (closed.holds[which], false))
                    .chain(
                        open.holds
                            .iter()
                            .filter(|hold| hold.mutex == mutex && near(&hold.span))
                            .map(|&hold| (hold, true)),
                    );
                for (hold, still) in found {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    let _ = write!(
                        text,
                        "{} {} by {}\n{}",
                        opsy.mutexes.list[mutex].name,
                        if hold.waiting { "waited for" } else { "held" },
                        named(Some(hold.task)),
                        self.extent(hold.span, still)
                    );
                }
            }
            Line::Section(_) | Line::Fold | Line::Cpu | Line::Event(_) => {}
        }
        (text, arrow)
    }

    /// What woke the activation that time ready `which` is part of, and when;
    /// how long the task waited for its first run; and the wake to draw, from
    /// the line of a handler or task that woke it.
    pub(super) fn woken(&self, meaning: &Meaning, which: usize) -> (String, Option<Arrow>) {
        let opsy = &meaning.opsy;
        let ready = &meaning.readies[which];
        let Some(wake) = ready.wake.map(|wake| &opsy.wakes[wake]) else {
            return ("ready since a loss: what woke it is unknown".to_owned(), None);
        };
        let named = |task: Option<usize>| task.map_or("idle", |task| opsy.tasks.list[task].name.as_str());
        let (cause, from) = match wake.cause {
            Cause::Handler(handler) => (opsy.handlers.list[handler].name.clone(), Some(Line::Handler(handler))),
            Cause::Task(task) => (named(task).to_owned(), Some(Line::Task(task))),
            Cause::Elapsed => ("its delay elapsing".to_owned(), None),
            Cause::Unknown => ("what a loss hides".to_owned(), None),
        };
        let time = self.time((wake.edge.cycles - self.origin()) as f64);
        let mut text = format!("woken by {cause} ({}) at {time}", wake.reason);
        if let Some(first) = meaning::waited(&meaning.readies, which) {
            let waited = self.duration(first.span.to.cycles - first.span.from.cycles);
            let at_least = if first.open { "at least " } else { "" };
            let _ = write!(text, ", waited {at_least}{waited}");
        }
        if matches!(wake.cause, Cause::Task(_)) && wake.reason == "notified" {
            text.push_str("\n(or a handler without Kymo's hooks, running in that task)");
        }
        let arrow = from.map(|from| Arrow {
            cycles: wake.edge.cycles,
            from,
        });
        (text, arrow)
    }

    /// The load of an OpSy `line` in `bin`, the bin drawn: its share of the
    /// time known, how much is known, and the longest span reaching into it.
    fn load_text(&self, meaning: &Meaning, line: Line, bin: &Drawn, open: &opsy::Spans) -> String {
        let opsy = &meaning.opsy;
        let named = |task: Option<usize>| task.map_or("idle", |task| opsy.tasks.list[task].name.as_str());
        let name = match line {
            Line::Cpu => "CPU".to_owned(),
            Line::Task(task) => named(task).to_owned(),
            Line::Handler(handler) => opsy.handlers.list[handler].name.clone(),
            Line::Mutex(mutex) => format!("{} held", opsy.mutexes.list[mutex].name),
            Line::Section(_) | Line::Fold | Line::Event(_) => return String::new(),
        };
        let origin = self.origin();
        let (from, to) = (
            self.time((bin.from - origin) as f64),
            self.time((bin.to - origin) as f64),
        );
        let length = bin.to - bin.from;
        let duration = self.duration(length);
        let mut text = if bin.known == 0 {
            format!("{name} unknown from {from} to {to}: {duration}")
        } else {
            format!(
                "{name} {} from {from} to {to}: {duration}, {} known",
                percent(bin.covered(), bin.known),
                percent(bin.known, length)
            )
        };
        if line == Line::Cpu && bin.known > 0 {
            let _ = write!(
                text,
                "\nhandlers {}, tasks other than idle {}",
                percent(bin.stack[0], bin.known),
                percent(bin.stack[1], bin.known)
            );
        }
        if let Some((who, span, still)) = self.longest(meaning, line, bin.from, bin.to, open) {
            let _ = write!(text, "\nlongest: {who}, {}", self.extent(span, still));
        }
        text
    }

    /// The longest span of `line` with cycles from `low` to before `high`:
    /// its subject, itself, and whether it is still open. The CPU's are the
    /// runs of tasks other than idle and the handlers'.
    fn longest(
        &self,
        meaning: &Meaning,
        line: Line,
        low: i64,
        high: i64,
        open: &opsy::Spans,
    ) -> Option<(String, Span, bool)> {
        let opsy = &meaning.opsy;
        let closed = &opsy.closed;
        let near = move |span: &Span| span.from.cycles < high && span.to.cycles > low;
        let runs = |which: Option<usize>| {
            let spans = which.map_or(Some(&meaning.idle), |which| meaning.runs.get(which));
            let shut = spans.into_iter().flat_map(move |spans| spans.overlapping(low, high));
            let still = open.runs.iter().filter(move |run| run.task == which && near(&run.span));
            shut.map(|n| (closed.runs[n].span, false))
                .chain(still.map(|run| (run.span, true)))
                .map(move |(span, still)| (span, still, Who::Task(which)))
        };
        let handlers = |which: usize| {
            let depths = meaning.interrupts.get(which).into_iter().flatten();
            let shut = depths.flat_map(move |spans| spans.overlapping(low, high));
            let still = open
                .interrupts
                .iter()
                .filter(move |interrupt| interrupt.handler == which && near(&interrupt.span));
            shut.map(|n| (closed.interrupts[n].span, false))
                .chain(still.map(|interrupt| (interrupt.span, true)))
                .map(move |(span, still)| (span, still, Who::Handler(which)))
        };
        let candidates: Box<dyn Iterator<Item = (Span, bool, Who)>> = match line {
            Line::Cpu => Box::new(
                (0..opsy.tasks.list.len())
                    .map(Some)
                    .flat_map(runs)
                    .chain((0..opsy.handlers.list.len()).flat_map(handlers)),
            ),
            Line::Task(which) => Box::new(runs(which)),
            Line::Handler(which) => Box::new(handlers(which)),
            Line::Mutex(mutex) => {
                let spans = meaning.holds.get(mutex).into_iter();
                let shut = spans.flat_map(|spans| spans.overlapping(low, high));
                let still = open
                    .holds
                    .iter()
                    .filter(move |hold| hold.mutex == mutex && !hold.waiting && near(&hold.span));
                Box::new(
                    shut.map(|n| &closed.holds[n])
                        .map(|hold| (hold.span, false, Who::Holder(hold.task)))
                        .chain(still.map(|hold| (hold.span, true, Who::Holder(hold.task)))),
                )
            }
            Line::Section(_) | Line::Fold | Line::Event(_) => return None,
        };
        let (span, still, who) = candidates.max_by_key(|(span, ..)| span.to.cycles - span.from.cycles)?;
        let task = |task: Option<usize>| task.map_or("idle", |task| opsy.tasks.list[task].name.as_str());
        let who = match who {
            Who::Task(which) => task(which).to_owned(),
            Who::Handler(which) => opsy.handlers.list[which].name.clone(),
            Who::Holder(which) => format!("held by {}", task(Some(which))),
        };
        Some((who, span, still))
    }

    /// Where `span` starts and ends, and how long it lasts: at a loss, the
    /// cycles of the event before it, so that the duration is a bound.
    fn extent(&self, span: Span, still: bool) -> String {
        let at = |edge: Edge| {
            let time = self.time((edge.cycles - self.origin()) as f64);
            if edge.lost {
                format!("a loss after {time}")
            } else {
                time
            }
        };
        let duration = self.duration(span.to.cycles - span.from.cycles);
        match (span.from.lost, span.to.lost || still) {
            (false, false) => format!("from {} to {}: {duration}", at(span.from), at(span.to)),
            (true, false) => format!("from {} to {}: at most {duration}", at(span.from), at(span.to)),
            (false, true) if still => format!("from {}, open at the end: at least {duration}", at(span.from)),
            (false, true) => format!("from {} to {}: at least {duration}", at(span.from), at(span.to)),
            (true, true) => format!("from {} to {}", at(span.from), at(span.to)),
        }
    }

    /// The last events of `span`, the one that ends it last, as kymo decode
    /// prints them.
    fn events_before(&self, span: Span) -> Vec<String> {
        let mut lines: Vec<String> = self.trace.elements[span.from.element..=span.to.element]
            .iter()
            .rev()
            .filter_map(|element| match element {
                Element::Event { event, cycles, seconds } => {
                    Some(decode::line(&self.catalogue.symbols, event, *cycles, *seconds))
                }
                _ => None,
            })
            .take(SWITCH)
            .collect();
        lines.reverse();
        lines
    }
}

/// A wake to draw: from the line of what woke the task hovered, at its
/// cycles, to the task's line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Arrow {
    pub(super) cycles: i64,
    pub(super) from: Line,
}

/// Who the longest span is of: a task or idle, a handler, or the task
/// holding a mutex.
enum Who {
    Task(Option<usize>),
    Handler(usize),
    Holder(usize),
}
