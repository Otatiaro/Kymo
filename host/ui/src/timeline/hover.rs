//! What the pointer hovers, described: a card per thing under it.

use super::load::{self, Drawn, percent};
use super::rows::Line;
use super::status::counted;
use super::view::View;
use super::{HOVER, SWITCH, Timeline};
use crate::meaning::{self, Meaning};
use crate::palette::{Palette, Shade};
use eframe::egui::{self, Pos2, Rect};
use kymo::meaning::{Cause, Edge, Span};
use kymo::model::Element;

/// The widest a hover gets, in pixels.
const WIDTH: f32 = 480.0;

/// What a hover shows: a card per thing under the pointer.
#[derive(Debug)]
pub(super) struct Hover {
    cards: Vec<Card>,
}

/// One thing hovered: its name and kind, the colour it is drawn in, its facts
/// by name, notes, and event lines under a heading.
#[derive(Debug, Default)]
struct Card {
    title: String,
    kind: &'static str,
    swatch: Option<Swatch>,
    facts: Vec<(&'static str, String)>,
    notes: Vec<String>,
    events: (&'static str, Vec<String>),
}

impl Card {
    fn new(title: String, kind: &'static str, swatch: Swatch) -> Self {
        Self {
            title,
            kind,
            swatch: Some(swatch),
            ..Self::default()
        }
    }
}

/// The colour a card's thing is drawn in, from the theme's palette.
#[derive(Clone, Copy, Debug)]
enum Swatch {
    Event,
    Task(Shade),
    Idle,
    Ready,
    Interrupt,
    Hold,
    Wait,
    Loss,
    Suspect,
}

impl Swatch {
    /// The swatch of `task`'s runs, the idle task's for None.
    fn task(meaning: &Meaning, task: Option<usize>) -> Self {
        task.map_or(Self::Idle, |task| Self::Task(meaning.shades[task]))
    }

    fn color(self, palette: &Palette) -> egui::Color32 {
        match self {
            Self::Event => palette.event,
            Self::Task(shade) => palette.task(shade),
            Self::Idle => palette.idle,
            Self::Ready => palette.ready,
            Self::Interrupt => palette.interrupt,
            Self::Hold => palette.hold,
            Self::Wait => palette.wait,
            Self::Loss => palette.loss,
            Self::Suspect => palette.suspect,
        }
    }
}

impl Hover {
    /// The cards, a separator between them, at most `WIDTH` wide.
    pub(super) fn show(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui.visuals());
        ui.set_max_width(WIDTH);
        for (n, card) in self.cards.iter().enumerate() {
            if n > 0 {
                ui.separator();
            }
            card.show(ui, &palette, n);
        }
    }
}

impl Card {
    /// The chip and the title, then the facts in a grid, the notes and the
    /// events.
    fn show(&self, ui: &mut egui::Ui, palette: &Palette, n: usize) {
        ui.horizontal(|ui| {
            if let Some(swatch) = self.swatch {
                let (chip, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(chip, 2.0, swatch.color(palette));
            }
            ui.label(egui::RichText::new(self.title.as_str()).strong());
            if !self.kind.is_empty() {
                ui.label(egui::RichText::new(self.kind).weak());
            }
        });
        if !self.facts.is_empty() {
            egui::Grid::new(("hover", n))
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    for (name, value) in &self.facts {
                        ui.label(egui::RichText::new(*name).weak());
                        ui.label(value.as_str());
                        ui.end_row();
                    }
                });
        }
        for note in &self.notes {
            ui.label(note.as_str());
        }
        let (heading, events) = &self.events;
        if !heading.is_empty() {
            ui.label(egui::RichText::new(*heading).weak());
        }
        for event in events {
            ui.label(egui::RichText::new(event.as_str()).monospace());
        }
    }
}

impl Timeline {
    /// What the pointer is over: the nearest event of its row, or the span,
    /// and the marks at its column; over a task's span, the wake to draw.
    pub(super) fn hovered(
        &self,
        view: &View,
        area: Rect,
        pointer: Pos2,
        lines: &[Line],
        open: &kymo::meaning::Spans,
        drawn: &[(usize, Vec<Drawn>)],
    ) -> Option<(Hover, Option<Arrow>)> {
        if pointer.x < area.left() || pointer.y < area.top() {
            return None;
        }
        let low = view.cycles(pointer.x - HOVER);
        let high = view.cycles(pointer.x + HOVER);
        let mut cards = Vec::new();
        let mut arrow = None;
        let n = super::rows::at(&super::rows::tops(lines, area.height()), pointer.y - area.top());
        let line = n.map(|n| lines[n]);
        let bins = drawn.iter().find(|(line, _)| Some(*line) == n).map(|(_, bins)| bins);
        if let (Some(meaning), Some(line)) = (&self.meaning, line) {
            match bins {
                Some(bins) => {
                    cards.extend(load::at(bins, pointer.x).and_then(|bin| self.load_card(meaning, line, bin, open)))
                }
                None => (cards, arrow) = self.span_cards(meaning, line, low, high, open),
            }
        }
        if let Some(Line::Event(index)) = line {
            let row = &self.rows[index];
            let cycles = row.levels.cycles();
            let from = cycles.partition_point(|&cycles| cycles < low);
            let to = cycles.partition_point(|&cycles| cycles <= high);
            let at = view.cycles(pointer.x);
            if let Some(nearest) = (from..to).min_by_key(|&n| (cycles[n] - at).abs())
                && let Element::Event { event, cycles, .. } = &self.trace.elements[row.elements[nearest]]
            {
                let mut card = self.card(Line::Event(index), "event", Swatch::Event);
                card.facts.push(("time", self.time((*cycles - self.origin()) as f64)));
                card.facts.push(("index", event.index.to_string()));
                let fields = self.fields(event);
                if !fields.is_empty() {
                    card.facts.push(("fields", fields));
                }
                if to - from > 1 {
                    card.notes
                        .push(format!("{} events of this type within {HOVER} pixels", to - from));
                }
                cards.push(card);
            }
        }
        cards.extend(
            self.marks()
                .filter(|mark| mark.from <= high && mark.to >= low)
                .take(8)
                .filter_map(|mark| mark_card(&self.trace.elements[mark.element])),
        );
        (!cards.is_empty()).then_some((Hover { cards }, arrow))
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

/// A loss or a suspect line, as a card; none for an event.
fn mark_card(element: &Element) -> Option<Card> {
    match *element {
        Element::Lost { from, to } => {
            let mut card = Card::new("Gap".to_owned(), "", Swatch::Loss);
            card.facts = vec![
                ("indexes", format!("{from} to {to}")),
                ("lost", counted(u64::from(to.wrapping_sub(from)), "slot", "slots")),
            ];
            Some(card)
        }
        Element::Suspect { from, to, writers } => {
            let mut card = Card::new("Write overrun".to_owned(), "", Swatch::Suspect);
            card.facts = vec![
                ("indexes", format!("{from} to {to}")),
                ("interrupted", counted(u64::from(writers), "write", "writes")),
            ];
            card.notes
                .push("Interrupted for a full ring lap: events recorded near it may be wrong.".to_owned());
            Some(card)
        }
        Element::Event { .. } => None,
    }
}

impl Timeline {
    /// A card titled with `line`'s label, its name in full among its facts
    /// when the label shortens it.
    fn card(&self, line: Line, kind: &'static str, swatch: Swatch) -> Card {
        let mut card = Card::new(self.label(line), kind, swatch);
        let full = self.full_label(line);
        if full != card.title {
            card.facts.push(("name", full));
        }
        card
    }

    /// The spans of a kernel `line` from `low` to `high` cycles, a card each;
    /// and for a task's, the wake to draw.
    fn span_cards(
        &self,
        meaning: &Meaning,
        line: Line,
        low: i64,
        high: i64,
        open: &kymo::meaning::Spans,
    ) -> (Vec<Card>, Option<Arrow>) {
        let kernel = &meaning.kernel;
        let closed = &kernel.closed;
        let near = |span: &Span| span.from.cycles <= high && span.to.cycles >= low;
        let mut cards = Vec::new();
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
                    let mut card = self.card(line, "ready, not on the core", Swatch::Ready);
                    card.facts.extend(self.extent(piece.span, piece.open).facts());
                    arrow = self.woken(meaning, which, &mut card);
                    cards.push(card);
                }
                if let Some((run, still)) = found {
                    let mut card = self.card(line, "run", Swatch::task(meaning, run.task));
                    card.facts.extend(self.extent(run.span, still).facts());
                    if let Some(which) = meaning::before_run(&meaning.readies, ready, run.span.from) {
                        arrow = self.woken(meaning, which, &mut card);
                    }
                    // Left out where no event has given it yet, or since a loss.
                    let at = low + (high - low) / 2;
                    if let Some(priority) = run.task.and_then(|task| kernel.tasks.list[task].priority_at(at)) {
                        card.facts
                            .push(("priority", format!("{priority} there, {}", kernel.precedence.reading())));
                    }
                    if !still && !run.span.to.lost {
                        card.events = ("last events, the switch last", self.events_before(run.span));
                    }
                    cards.push(card);
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
                    let mut card = self.card(line, "interrupt", Swatch::Interrupt);
                    card.facts.extend(self.extent(interrupt.span, still).facts());
                    let within = match interrupt.depth {
                        None => "unknown since a loss".to_owned(),
                        Some(0) => "no handler".to_owned(),
                        Some(depth) => counted(depth as u64, "handler", "handlers"),
                    };
                    card.facts.push(("nested in", within));
                    cards.push(card);
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
                    let (kind, swatch) = if hold.waiting {
                        ("waited for", Swatch::Wait)
                    } else {
                        ("held", Swatch::Hold)
                    };
                    let mut card = self.card(line, kind, swatch);
                    card.facts.push(("by", self.label(Line::Task(Some(hold.task)))));
                    card.facts.extend(self.extent(hold.span, still).facts());
                    cards.push(card);
                }
            }
            Line::Section(_) | Line::Cpu | Line::Event(_) => {}
        }
        (cards, arrow)
    }

    /// What woke the activation that time ready `which` is part of, and when,
    /// and how long the task waited for its first run, added to `card`; the
    /// wake to draw, from the line of a handler or task that woke it.
    fn woken(&self, meaning: &Meaning, which: usize, card: &mut Card) -> Option<Arrow> {
        let kernel = &meaning.kernel;
        let ready = &meaning.readies[which];
        let Some(wake) = ready.wake.map(|wake| &kernel.wakes[wake]) else {
            card.facts.push(("woken by", "unknown: ready since a loss".to_owned()));
            return None;
        };
        let (cause, from) = match wake.cause {
            Cause::Handler(handler) => (self.label(Line::Handler(handler)), Some(Line::Handler(handler))),
            Cause::Task(task) => (self.label(Line::Task(task)), Some(Line::Task(task))),
            Cause::Elapsed => ("its delay elapsing".to_owned(), None),
            Cause::Unknown => ("what a loss hides".to_owned(), None),
        };
        card.facts.push(("woken by", format!("{cause} ({})", wake.reason)));
        card.facts
            .push(("woken at", self.time((wake.edge.cycles - self.origin()) as f64)));
        if let Some(first) = meaning::waited(&meaning.readies, which) {
            let waited = self.duration(first.span.to.cycles - first.span.from.cycles);
            let at_least = if first.open { "at least " } else { "" };
            card.facts.push(("waited", format!("{at_least}{waited}")));
        }
        if matches!(wake.cause, Cause::Task(_)) && wake.reason == "notified" {
            card.notes
                .push("Or a handler without Kymo's hooks, running in that task.".to_owned());
        }
        from.map(|from| Arrow {
            cycles: wake.edge.cycles,
            from,
        })
    }

    /// The load of a kernel `line` in `bin`, the bin drawn: its share of the
    /// time known, how much is known, and the longest span reaching into it.
    fn load_card(&self, meaning: &Meaning, line: Line, bin: &Drawn, open: &kymo::meaning::Spans) -> Option<Card> {
        let (kind, swatch) = match line {
            Line::Cpu => ("load", None),
            Line::Task(task) => ("load", Some(Swatch::task(meaning, task))),
            Line::Handler(_) => ("load", Some(Swatch::Interrupt)),
            Line::Mutex(_) => ("held, load", Some(Swatch::Hold)),
            Line::Section(_) | Line::Event(_) => return None,
        };
        // The CPU's line has a colour per layer, and so no swatch.
        let mut card = match swatch {
            Some(swatch) => self.card(line, kind, swatch),
            None => Card {
                title: self.label(line),
                kind,
                ..Card::default()
            },
        };
        let origin = self.origin();
        card.facts.extend([
            ("from", self.time((bin.from - origin) as f64)),
            ("to", self.time((bin.to - origin) as f64)),
            ("bin", self.duration(bin.to - bin.from)),
        ]);
        let statistic = load::statistic(line == Line::Cpu, self.binning.statistic);
        let (low, high) = bin.counted;
        let length = high - low;
        if (low, high) != (bin.from, bin.to) {
            let at = format!(
                "{} to {}",
                self.time((low - origin) as f64),
                self.time((high - origin) as f64)
            );
            card.facts.push(("sub-bin", at));
        }
        card.facts.push(("statistic", statistic.name().to_owned()));
        if bin.known == 0 {
            card.facts.push(("share", "unknown".to_owned()));
        } else {
            card.facts.push(("share", percent(bin.covered(), bin.known)));
            card.facts.push(("known", percent(bin.known, length)));
        }
        if line == Line::Cpu && bin.known > 0 {
            card.facts.push(("handlers", percent(bin.stack[0], bin.known)));
            card.facts
                .push(("other tasks", percent(bin.covered() - bin.stack[0], bin.known)));
        }
        if let Some((who, span, still)) = self.longest(meaning, line, bin.from, bin.to, open) {
            let extent = self.extent(span, still);
            let duration = extent
                .duration
                .map(|duration| format!("{duration}, "))
                .unwrap_or_default();
            card.facts
                .push(("longest", format!("{who}, {duration}from {}", extent.start)));
        }
        Some(card)
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
        open: &kymo::meaning::Spans,
    ) -> Option<(String, Span, bool)> {
        let kernel = &meaning.kernel;
        let closed = &kernel.closed;
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
                (0..kernel.tasks.list.len())
                    .map(Some)
                    .flat_map(runs)
                    .chain((0..kernel.handlers.list.len()).flat_map(handlers)),
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
            Line::Section(_) | Line::Event(_) => return None,
        };
        let (span, still, who) = candidates.max_by_key(|(span, ..)| span.to.cycles - span.from.cycles)?;
        let who = match who {
            Who::Task(which) => self.label(Line::Task(which)),
            Who::Handler(which) => self.label(Line::Handler(which)),
            Who::Holder(which) => format!("held by {}", self.label(Line::Task(Some(which)))),
        };
        Some((who, span, still))
    }

    /// Where `span` starts and ends, and how long it lasts: at a loss, the
    /// cycles of the event before it, so that the duration is a bound.
    fn extent(&self, span: Span, still: bool) -> Extent {
        let at = |edge: Edge| {
            let time = self.time((edge.cycles - self.origin()) as f64);
            if edge.lost {
                format!("a loss after {time}")
            } else {
                time
            }
        };
        Extent {
            start: at(span.from),
            end: if still {
                "open at the end".to_owned()
            } else {
                at(span.to)
            },
            duration: bounded(
                self.duration(span.to.cycles - span.from.cycles),
                span.from.lost,
                span.to.lost || still,
            ),
        }
    }

    /// The last events of `span`, the one that ends it last: time, type and
    /// fields, a subject by the name its row shows.
    fn events_before(&self, span: Span) -> Vec<String> {
        let mut lines: Vec<String> = self.trace.elements[span.from.element..=span.to.element]
            .iter()
            .rev()
            .filter_map(|element| match element {
                Element::Event { event, cycles, .. } => {
                    let time = self.time((*cycles - self.origin()) as f64);
                    let fields = self.fields(event);
                    Some(format!("{time}  {}  {fields}", event.event.name).trim_end().to_owned())
                }
                _ => None,
            })
            .take(SWITCH)
            .collect();
        lines.reverse();
        lines
    }
}

/// Where a span starts and ends, and how long it lasts when that is known.
struct Extent {
    start: String,
    end: String,
    duration: Option<String>,
}

impl Extent {
    fn facts(self) -> Vec<(&'static str, String)> {
        let mut facts = vec![("start", self.start), ("end", self.end)];
        facts.extend(self.duration.map(|duration| ("duration", duration)));
        facts
    }
}

/// A span's `duration`, as a bound when an edge is unknown: at most when it
/// starts at a loss, at least when it ends at one or is still open; none
/// when both edges are.
fn bounded(duration: String, from_unknown: bool, to_unknown: bool) -> Option<String> {
    match (from_unknown, to_unknown) {
        (false, false) => Some(duration),
        (true, false) => Some(format!("at most {duration}")),
        (false, true) => Some(format!("at least {duration}")),
        (true, true) => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_a_bound_when_an_edge_is_unknown() {
        let duration = || "5 ms".to_owned();
        assert_eq!(bounded(duration(), false, false).as_deref(), Some("5 ms"));
        assert_eq!(bounded(duration(), true, false).as_deref(), Some("at most 5 ms"));
        assert_eq!(bounded(duration(), false, true).as_deref(), Some("at least 5 ms"));
        assert_eq!(bounded(duration(), true, true), None);
    }

    #[test]
    fn a_gap_card_names_its_indexes_and_the_slots_lost() {
        let card = mark_card(&Element::Lost { from: 3, to: 7 }).expect("a card");
        assert_eq!(card.title, "Gap");
        assert_eq!(
            card.facts,
            [("indexes", "3 to 7".to_owned()), ("lost", "4 slots".to_owned())]
        );
    }

    #[test]
    fn an_extent_lists_its_duration_only_when_known() {
        let extent = |duration: Option<&str>| Extent {
            start: "1".to_owned(),
            end: "2".to_owned(),
            duration: duration.map(str::to_owned),
        };
        let names = |extent: Extent| extent.facts().into_iter().map(|(name, _)| name).collect::<Vec<_>>();
        assert_eq!(names(extent(Some("1 s"))), ["start", "end", "duration"]);
        assert_eq!(names(extent(None)), ["start", "end"]);
    }
}
