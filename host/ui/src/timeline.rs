//! The timeline: one row per event type of the catalogue, time across. A row
//! draws at most one shape per pixel column, however many events fall in it,
//! so a frame costs the same at any zoom (ADR-055). Losses and suspect lines
//! are bands across the rows, between the events recorded before and after
//! them.
//!
//! An image with the OpSy adapter shows its meaning above them: a row per
//! task, the idle task, each interrupt handler and each mutex, drawn as
//! spans; the rows of event types are then folded under one line.

use crate::levels::Levels;
use crate::meaning::Meaning;
use crate::spans::{Band, Spans};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use kymo::decode;
use kymo::events::Catalogue;
use kymo::model::{Element, Trace};
use kymo::opsy::{self, Edge, Span};
use kymo::target::Result;
use std::fmt::Write;
use std::time::Instant;

/// The width of the column of row names.
const LABELS: f32 = 260.0;
const RULER: f32 = 22.0;
const ROW: f32 = 18.0;
/// The narrowest view, in cycles.
const NARROWEST: f64 = 100.0;
/// How near the pointer, in pixels, an event or a mark is hovered.
const HOVER: f32 = 4.0;
/// The ruler's spacing between ticks, in pixels, at least.
const TICK: f64 = 110.0;

const EVENT: Color32 = Color32::from_rgb(70, 130, 220);
const LOSS: Color32 = Color32::from_rgba_premultiplied(110, 25, 25, 110);
const SUSPECT: Color32 = Color32::from_rgba_premultiplied(115, 75, 15, 110);
const RUN: Color32 = Color32::from_rgb(70, 130, 220);
const IDLE: Color32 = Color32::from_rgb(125, 125, 125);
const INTERRUPT: Color32 = Color32::from_rgb(225, 140, 40);
const HOLD: Color32 = Color32::from_rgb(60, 160, 90);
const WAIT: Color32 = Color32::from_rgba_premultiplied(25, 64, 36, 100);
/// How many events a run's hover shows, the switch that ends it last.
const SWITCH: usize = 4;

pub struct Timeline {
    catalogue: &'static Catalogue,
    trace: Trace<'static>,
    /// The event types of the catalogue, a row each, in its order.
    ids: Vec<u16>,
    rows: Vec<Row>,
    /// OpSy's meaning, when the image has the adapter.
    meaning: Option<Meaning>,
    /// Whether the rows of event types are folded.
    folded: bool,
    /// The losses and suspect lines between two events.
    marks: Vec<Mark>,
    /// The losses and suspect lines after the last event, which the next
    /// event closes.
    pending: Vec<usize>,
    /// The cycles of the last event, in the trace's order.
    before: Option<i64>,
    /// The cycles of the first event, which the view counts from.
    origin: Option<i64>,
    /// The earliest and latest event, in cycles.
    bounds: Option<(i64, i64)>,
    /// The view: its left edge and its width, in cycles from the origin.
    start: f64,
    span: f64,
    /// The status line's counts, kept as elements are added.
    events: usize,
    losses: usize,
    lost: u64,
    suspects: u64,
    /// Milliseconds the last frame took to lay the timeline out.
    painted: f64,
    measure: Option<Measure>,
}

/// The events of one type, by cycles, and their elements in the trace.
struct Row {
    name: String,
    levels: Levels,
    elements: Vec<usize>,
}

/// A loss or suspect line, between the cycles of the events recorded
/// before and after it.
#[derive(Clone, Copy)]
struct Mark {
    from: i64,
    to: i64,
    element: usize,
}

/// A line of the timeline, top to bottom.
#[derive(Clone, Copy, PartialEq)]
enum Line {
    Section(&'static str),
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
    pub fn new(catalogue: &'static Catalogue, trace: Trace<'static>, measure: bool) -> Result<Self> {
        let meaning = Meaning::new(catalogue, trace.first)?;
        let mut timeline = Self {
            folded: meaning.is_some(),
            meaning,
            catalogue,
            trace,
            ids: catalogue.events.keys().copied().collect(),
            rows: catalogue
                .events
                .values()
                .map(|event| Row {
                    name: event.name.clone(),
                    levels: Levels::default(),
                    elements: Vec::new(),
                })
                .collect(),
            marks: Vec::new(),
            pending: Vec::new(),
            before: None,
            origin: None,
            bounds: None,
            start: 0.0,
            span: 0.0,
            events: 0,
            losses: 0,
            lost: 0,
            suspects: 0,
            painted: 0.0,
            measure: measure.then(Measure::default),
        };
        timeline.add(0);
        timeline.whole();
        Ok(timeline)
    }

    /// Takes in the trace's elements from `from` on. Each row's new events,
    /// sorted, come after those it holds.
    fn add(&mut self, from: usize) {
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
        if let Some(meaning) = &mut self.meaning {
            meaning.add(&self.trace.elements, from);
        }
    }

    /// The lines: OpSy's sections, the tasks by priority, highest first,
    /// then the rows of event types unless folded; those rows alone without
    /// OpSy.
    fn lines(&self) -> Vec<Line> {
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
        let mut lines = vec![Line::Section("TASKS")];
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

    fn label(&self, line: Line) -> String {
        let shown = |kind: usize, n: usize| {
            let meaning = self.meaning.as_ref().expect("a subject's line has OpSy's meaning");
            &meaning.names[kind][n]
        };
        match line {
            Line::Section(name) => name.to_owned(),
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
    fn marks(&self) -> impl Iterator<Item = Mark> + '_ {
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

    fn origin(&self) -> i64 {
        self.origin.unwrap_or_default()
    }

    /// The earliest and latest event, in cycles from the origin.
    fn first(&self) -> f64 {
        self.bounds.map_or(0.0, |(lowest, _)| (lowest - self.origin()) as f64)
    }

    fn last(&self) -> f64 {
        self.bounds.map_or(0.0, |(_, highest)| (highest - self.origin()) as f64)
    }
    /// Shows the whole capture.
    fn whole(&mut self) {
        (self.start, self.span) = whole(self.first(), self.last());
    }

    /// Zooms by `factor` about the point `at` of the view's width, 0 to 1.
    fn zoom(&mut self, at: f64, factor: f64) {
        let pivot = self.start + at * self.span;
        self.span = (self.span * factor).clamp(NARROWEST, widest(self.first(), self.last()));
        self.start = pivot - at * self.span;
    }

    fn clamp(&mut self) {
        self.start = clamped(self.start, self.span, self.first(), self.last());
    }

    fn frequency(&self) -> Option<f64> {
        self.trace.frequency
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let began = Instant::now();
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let area = Rect::from_min_max(egui::pos2(rect.left() + LABELS, rect.top() + RULER), rect.max);
        let width = f64::from(area.width().max(1.0));
        if response.double_clicked() {
            self.whole();
        }
        if response.dragged() {
            self.start -= f64::from(response.drag_delta().x) * self.span / width;
        }
        if response.hovered() {
            let (scroll, zoom) = ui.input(|input| (input.smooth_scroll_delta, input.zoom_delta()));
            if let Some(pointer) = response.hover_pos() {
                let at = (f64::from(pointer.x - area.left()) / width).clamp(0.0, 1.0);
                self.zoom(at, (-f64::from(scroll.y) * 0.002).exp() / f64::from(zoom));
            }
            self.start -= f64::from(scroll.x) * self.span / width;
        }
        self.clamp();
        let mut lines = self.lines();
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && pointer.y >= area.top()
            && lines.get(((pointer.y - area.top()) / ROW) as usize) == Some(&Line::Fold)
        {
            self.folded = !self.folded;
            lines = self.lines();
        }
        let open = self
            .meaning
            .as_ref()
            .map(|meaning| meaning.opsy.open())
            .unwrap_or_default();

        let view = View {
            origin: self.origin(),
            start: self.start,
            span: self.span,
            left: area.left(),
            width,
        };
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        let text = visuals.text_color();
        let faint = visuals.faint_bg_color;
        let grid = visuals.widgets.noninteractive.bg_stroke.color;
        let labels = ui.painter_at(Rect::from_min_max(
            egui::pos2(rect.left(), area.top()),
            egui::pos2(area.left() - 4.0, rect.bottom()),
        ));
        let font = FontId::proportional(12.0);

        // The events and spans of all rows in one mesh: their rectangles sit
        // on whole pixels and need none of the anti-aliasing a shape each
        // would get.
        let mut mesh = egui::Mesh::default();
        let mut bars = Bars {
            mesh: &mut mesh,
            view: &view,
            area,
        };
        // A mutex's holders, named in their bars wide enough.
        let mut owners: Vec<(f32, f32, f32, usize)> = Vec::new();
        for (n, &line) in lines.iter().enumerate() {
            let top = area.top() + n as f32 * ROW;
            if top > area.bottom() {
                break;
            }
            if n % 2 == 1 {
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(rect.left(), top), egui::pos2(area.right(), top + ROW)),
                    0.0,
                    faint,
                );
            }
            labels.text(
                egui::pos2(rect.left() + 4.0, top + ROW / 2.0),
                Align2::LEFT_CENTER,
                self.label(line),
                font.clone(),
                text,
            );
            if line == Line::Fold {
                // Pointing right folded, down unfolded.
                let (x, y) = (rect.left() + 9.0, top + ROW / 2.0);
                let points = if self.folded {
                    vec![
                        egui::pos2(x - 3.0, y - 4.0),
                        egui::pos2(x + 3.0, y),
                        egui::pos2(x - 3.0, y + 4.0),
                    ]
                } else {
                    vec![
                        egui::pos2(x - 4.0, y - 3.0),
                        egui::pos2(x + 4.0, y - 3.0),
                        egui::pos2(x, y + 3.0),
                    ]
                };
                labels.add(egui::Shape::convex_polygon(points, text, egui::Stroke::NONE));
            }
            let (high, low) = (top + 3.0, top + ROW - 3.0);
            let meaning = self.meaning.as_ref();
            match line {
                Line::Section(_) | Line::Fold => {}
                Line::Event(row) => {
                    for (from, to) in self.rows[row].levels.columns(&view) {
                        bars.mesh
                            .add_colored_rect(Rect::from_min_max(egui::pos2(from, high), egui::pos2(to, low)), EVENT);
                    }
                }
                Line::Task(task) => {
                    let Some(meaning) = meaning else { continue };
                    let spans = task.map_or(Some(&meaning.idle), |task| meaning.runs.get(task));
                    let open = open.runs.iter().filter(|run| run.task == task).map(|run| run.span);
                    bars.paint(spans, open, high, low, if task.is_some() { RUN } else { IDLE });
                }
                Line::Handler(handler) => {
                    let Some(meaning) = meaning else { continue };
                    // Depth 0, or unknown, fills the row; a nested handler,
                    // the half below.
                    let middle = top + ROW / 2.0;
                    let depths = meaning.interrupts.get(handler).map_or(&[][..], Vec::as_slice);
                    let row = |interrupt: &kymo::opsy::Interrupt| interrupt.depth.unwrap_or(0);
                    let deepest = depths.len().max(1 + open.interrupts.iter().map(row).max().unwrap_or(0));
                    for depth in 0..deepest {
                        let open = open
                            .interrupts
                            .iter()
                            .filter(|interrupt| interrupt.handler == handler && row(interrupt) == depth)
                            .map(|interrupt| interrupt.span);
                        let high = if depth == 0 { high } else { middle };
                        bars.paint(depths.get(depth), open, high, low, INTERRUPT);
                    }
                }
                Line::Mutex(mutex) => {
                    let Some(meaning) = meaning else { continue };
                    let holders = open.holds.iter().filter(|hold| hold.mutex == mutex);
                    for (task, waits) in meaning.waits.get(mutex).into_iter().flatten() {
                        let open = holders
                            .clone()
                            .filter(|hold| hold.waiting && hold.task == *task)
                            .map(|hold| hold.span);
                        bars.paint(Some(waits), open, high, low, WAIT);
                    }
                    // Waiters not yet in a row: their waits are all open.
                    for hold in holders.clone().filter(|hold| {
                        hold.waiting
                            && !meaning
                                .waits
                                .get(mutex)
                                .is_some_and(|waits| waits.iter().any(|wait| wait.0 == hold.task))
                    }) {
                        bars.paint(None, std::iter::once(hold.span), high, low, WAIT);
                    }
                    let open: Vec<_> = holders.filter(|hold| !hold.waiting).collect();
                    let closed = meaning.holds.get(mutex);
                    let middle = top + ROW / 2.0;
                    for band in bars.paint(closed, open.iter().map(|hold| hold.span), high, low, HOLD) {
                        // A band of more spans than pixels is not walked, so
                        // that a frame costs at most its width: a long hold
                        // among them is named once zoomed in.
                        if band.spans.len() as f32 > band.to - band.from {
                            continue;
                        }
                        for n in band.spans {
                            let (from, to, which) = closed.expect("bands come from spans").columns(n, &view);
                            owners.push((from, to, middle, meaning.opsy.closed.holds[which].task));
                        }
                    }
                    for hold in open {
                        let (from, to) = open_columns(hold.span, &view);
                        owners.push((from, to, middle, hold.task));
                    }
                }
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        if let Some(meaning) = &self.meaning {
            // Small enough that the descenders stay inside the bar's 12 pixels.
            let small = FontId::proportional(10.0);
            for (from, to, y, owner) in owners {
                let name = &meaning.names[0][owner];
                let (from, to) = (from.max(area.left()), to.min(area.right()));
                // An estimate of the name's width: a narrower bar stays bare.
                if to - from >= 6.0 * name.chars().count() as f32 + 6.0 {
                    painter.text(
                        egui::pos2(from + 3.0, y - 1.0),
                        Align2::LEFT_CENTER,
                        name,
                        small.clone(),
                        Color32::WHITE,
                    );
                }
            }
        }
        let bottom = (area.top() + lines.len() as f32 * ROW).min(area.bottom());
        for (kind, color) in [(false, LOSS), (true, SUSPECT)] {
            let marks = self
                .marks()
                .filter(|mark| matches!(self.trace.elements[mark.element], Element::Suspect { .. }) == kind);
            for (from, to) in view.bands(marks) {
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(from, area.top()), egui::pos2(to, bottom)),
                    0.0,
                    color,
                );
            }
        }
        self.ruler(&painter, &view, area, rect.top(), bottom, text, grid);
        self.painted = began.elapsed().as_secs_f64() * 1e3;
        if !response.dragged()
            && let Some(pointer) = response.hover_pos()
            && let Some(hovered) = self.hovered(&view, area, pointer, &lines, &open)
        {
            response.on_hover_ui_at_pointer(|ui| {
                ui.label(egui::RichText::new(hovered).monospace());
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ruler(
        &self,
        painter: &egui::Painter,
        view: &View,
        area: Rect,
        top: f32,
        bottom: f32,
        text: Color32,
        grid: Color32,
    ) {
        let font = FontId::proportional(11.0);
        for (at, label) in ticks(view.start, view.span, view.width, self.frequency()) {
            let x = view.x(at);
            painter.line_segment(
                [egui::pos2(x, top + RULER - 6.0), egui::pos2(x, bottom)],
                Stroke::new(1.0, grid),
            );
            painter.text(
                egui::pos2(x + 3.0, top + 2.0),
                Align2::LEFT_TOP,
                label,
                font.clone(),
                text,
            );
        }
        painter.line_segment(
            [
                egui::pos2(area.left(), top + RULER),
                egui::pos2(area.right(), top + RULER),
            ],
            Stroke::new(1.0, grid),
        );
    }

    /// What the pointer is over: the nearest event of its row, or the span,
    /// and the marks at its column.
    fn hovered(&self, view: &View, area: Rect, pointer: Pos2, lines: &[Line], open: &opsy::Spans) -> Option<String> {
        if pointer.x < area.left() || pointer.y < area.top() {
            return None;
        }
        let low = view.cycles(pointer.x - HOVER);
        let high = view.cycles(pointer.x + HOVER);
        let mut text = String::new();
        let line = lines.get(((pointer.y - area.top()) / ROW) as usize).copied();
        if let (Some(meaning), Some(line)) = (&self.meaning, line) {
            text = self.span_text(meaning, line, low, high, open);
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
            match self.trace.elements[mark.element] {
                Element::Lost { from, to } => {
                    let slots = counted(u64::from(to.wrapping_sub(from)), "slot", "slots");
                    let _ = write!(text, "gap between indexes {from} and {to}: {slots} lost");
                }
                Element::Suspect { from, to, writers } => {
                    let writes = counted(u64::from(writers), "write", "writes");
                    let _ = write!(
                        text,
                        "write overrun between indexes {from} and {to}: {writes} interrupted for a full ring lap; \
                         events recorded near it may be wrong"
                    );
                }
                Element::Event { .. } => {}
            }
        }
        (!text.is_empty()).then_some(text)
    }

    /// The span of an OpSy `line` from `low` to `high` cycles, described;
    /// empty when there is none.
    fn span_text(&self, meaning: &Meaning, line: Line, low: i64, high: i64, open: &opsy::Spans) -> String {
        let opsy = &meaning.opsy;
        let closed = &opsy.closed;
        let near = |span: &Span| span.from.cycles <= high && span.to.cycles >= low;
        let named = |task: Option<usize>| task.map_or("idle", |task| opsy.tasks.list[task].name.as_str());
        let mut text = String::new();
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
                if let Some((run, still)) = found {
                    let _ = write!(text, "{}\n{}", named(run.task), self.extent(run.span, still));
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
            Line::Section(_) | Line::Fold | Line::Event(_) => {}
        }
        text
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

    /// `cycles` as a duration, in seconds when the frequency is known.
    fn duration(&self, cycles: i64) -> String {
        let Some(hz) = self.frequency() else {
            return format!("{cycles} cycles");
        };
        let seconds = cycles as f64 / hz;
        let (unit, suffix) = match seconds.abs() {
            s if s < 1e-6 => (1e-9, "ns"),
            s if s < 1e-3 => (1e-6, "µs"),
            s if s < 1.0 => (1e-3, "ms"),
            _ => (1.0, "s"),
        };
        format!("{:.3} {suffix} ({cycles} cycles)", seconds / unit)
    }

    fn status(&self) -> String {
        let mut status = format!(
            "{}, {} ({} lost)",
            counted(self.events as u64, "event", "events"),
            counted(self.losses as u64, "gap", "gaps"),
            counted(self.lost, "slot", "slots")
        );
        if self.suspects > 0 {
            let writes = counted(self.suspects, "write", "writes");
            let _ = write!(status, ", {writes} overrun: nearby events may be wrong");
        }
        if let Some(meaning) = &self.meaning {
            let _ = write!(status, ", {} against OpSy's order", meaning.opsy.contradictions.len());
        }
        match (self.frequency(), self.trace.measured) {
            (Some(hz), Some((measured, _))) if hz == measured => {
                let _ = write!(status, ", {} Hz measured", hz.round());
            }
            (Some(hz), _) => {
                let _ = write!(status, ", {} Hz", hz.round());
            }
            (None, _) => status.push_str(", no frequency: in cycles"),
        }
        let _ = write!(
            status,
            "  |  view {} to {}  |  timeline laid out in {:.2} ms",
            self.time(self.start),
            self.time(self.start + self.span),
            self.painted
        );
        status
    }

    /// `cycles` from the origin, as seconds when the frequency is known.
    fn time(&self, cycles: f64) -> String {
        match self.frequency() {
            Some(hz) => format!("{:.9} s", cycles / hz),
            None => format!("{cycles:.0} cycles"),
        }
    }
}

impl eframe::App for Timeline {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.measure.is_some() {
            self.measure_frame(ui.ctx(), frame.info().cpu_usage.unwrap_or_default());
        }
        egui::Panel::bottom("status").show(ui, |ui| ui.label(self.status()));
        egui::CentralPanel::default().show(ui, |ui| self.timeline(ui));
    }
}

/// The view's mapping between cycles and pixels.
pub struct View {
    /// The cycles the view counts from.
    pub origin: i64,
    /// The left edge and the width, in cycles from `origin`.
    pub start: f64,
    pub span: f64,
    /// The left edge and the width, in pixels.
    pub left: f32,
    pub width: f64,
}

impl View {
    /// The pixel of `cycles` from the origin.
    pub fn x(&self, cycles: f64) -> f32 {
        self.left + ((cycles - self.start) / self.span * self.width) as f32
    }

    /// The pixel column of `cycles`, absolute.
    pub fn column(&self, cycles: i64) -> f32 {
        self.x((cycles - self.origin) as f64).floor()
    }

    /// The cycles, absolute, at pixel `x`.
    pub fn cycles(&self, x: f32) -> i64 {
        self.origin + (self.start + f64::from(x - self.left) / self.width * self.span).round() as i64
    }

    /// The marks in view, at least a pixel wide, merged where they touch.
    fn bands(&self, marks: impl Iterator<Item = Mark>) -> Vec<(f32, f32)> {
        let (low, high) = (
            self.cycles(self.left),
            self.origin + (self.start + self.span).ceil() as i64,
        );
        let mut bands: Vec<(f32, f32)> = Vec::new();
        for mark in marks.filter(|mark| mark.to >= low && mark.from <= high) {
            let from = self.x((mark.from - self.origin) as f64).floor().max(self.left - 1.0);
            let to = self.x((mark.to - self.origin) as f64).ceil().max(from + 1.0);
            match bands.last_mut() {
                Some(band) if from <= band.1 => band.1 = band.1.max(to),
                _ => bands.push((from, to)),
            }
        }
        bands
    }
}

/// The widest view of a capture from `first` to `last` cycles.
fn widest(first: f64, last: f64) -> f64 {
    ((last - first) * 1.05).max(NARROWEST)
}

/// `n` and the noun that counts it: `one` for 1, `many` otherwise.
fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The view of the whole capture from `first` to `last` cycles: its left
/// edge and its width, a margin after the last event.
fn whole(first: f64, last: f64) -> (f64, f64) {
    (first, widest(first, last))
}

/// `start` within the bounds a view of `span` cycles may start at: no
/// earlier than the first event, no later than half a view before the last.
fn clamped(start: f64, span: f64, first: f64, last: f64) -> f64 {
    start.clamp(first, (last - span / 2.0).max(first))
}

/// The ruler's ticks in a view from `start` over `span` cycles, `width`
/// pixels wide: each one's cycles and label, in seconds when the frequency is
/// known, else in cycles.
fn ticks(start: f64, span: f64, width: f64, frequency: Option<f64>) -> Vec<(f64, String)> {
    let hz = frequency.unwrap_or(1.0);
    let wanted = TICK * span / width / hz;
    let magnitude = 10f64.powf(wanted.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|n| n * magnitude)
        .find(|step| *step >= wanted)
        .unwrap_or(10.0 * magnitude);
    let (unit, suffix) = match frequency {
        None => (1.0, " cycles"),
        Some(_) if step < 1e-6 => (1e-9, " ns"),
        Some(_) if step < 1e-3 => (1e-6, " µs"),
        Some(_) if step < 1.0 => (1e-3, " ms"),
        Some(_) => (1.0, " s"),
    };
    let decimals = (-(step / unit).log10().floor()).max(0.0) as usize;
    let mut ticks = Vec::new();
    let mut tick = (start / hz / step).ceil();
    loop {
        let at = tick * step * hz;
        if at > start + span {
            break;
        }
        // Adding zero turns the -0 of a tick at the origin into 0.
        ticks.push((at, format!("{:.decimals$}{suffix}", tick * step / unit + 0.0)));
        tick += 1.0;
    }
    ticks
}

/// The rectangles of a frame's rows, in one mesh.
struct Bars<'a> {
    mesh: &'a mut egui::Mesh,
    view: &'a View,
    area: Rect,
}

/// The pixel columns `from..to` of an open span.
fn open_columns(span: Span, view: &View) -> (f32, f32) {
    (
        view.column(span.from.cycles),
        view.column(span.to.cycles.max(span.from.cycles)) + 1.0,
    )
}

impl Bars<'_> {
    /// Paints a row's closed spans, then its `open` ones, which start after
    /// them, from `high` to `low`; returns the closed spans' bands.
    fn paint(
        &mut self,
        closed: Option<&Spans>,
        open: impl Iterator<Item = Span>,
        high: f32,
        low: f32,
        color: Color32,
    ) -> Vec<Band> {
        let view = self.view;
        let bands = closed.map(|spans| spans.bands(view)).unwrap_or_default();
        let mut painted: Vec<(f32, f32)> = bands.iter().map(|band| (band.from, band.to)).collect();
        let right = view.left + view.width as f32;
        for span in open {
            let (from, to) = open_columns(span, view);
            if to < view.left || from >= right {
                continue;
            }
            match painted.last_mut() {
                Some(last) if from <= last.1 => last.1 = last.1.max(to),
                _ => painted.push((from, to)),
            }
        }
        for (from, to) in painted {
            self.mesh.add_colored_rect(
                Rect::from_min_max(
                    egui::pos2(from.max(self.area.left()), high),
                    egui::pos2(to.min(self.area.right()), low),
                ),
                color,
            );
        }
        bands
    }
}

/// `--measure`: frames drawn without vsync, the whole capture then the
/// narrowest view at its middle, after a warm-up each.
#[derive(Default)]
struct Measure {
    frame: u32,
    last: Option<Instant>,
    /// Per view: each frame's interval, its time on the CPU as eframe
    /// counts it (all but the wait to present), and the time to lay the
    /// timeline out, in milliseconds.
    samples: [Vec<[f64; 3]>; 2],
}

const WARM_UP: u32 = 30;
const FRAMES: u32 = 300;

impl Timeline {
    fn measure_frame(&mut self, ctx: &egui::Context, cpu: f32) {
        let painted = self.painted;
        let Some(measure) = self.measure.as_mut() else {
            return;
        };
        let now = Instant::now();
        let phase = WARM_UP + FRAMES;
        if let Some(last) = measure.last {
            let frame = measure.frame - 1;
            if frame % phase >= WARM_UP {
                measure.samples[(frame / phase) as usize].push([
                    (now - last).as_secs_f64() * 1e3,
                    f64::from(cpu) * 1e3,
                    painted,
                ]);
            }
        }
        measure.last = Some(now);
        let frame = measure.frame;
        measure.frame += 1;
        ctx.request_repaint();
        if frame == 0 {
            self.whole();
        } else if frame == phase {
            let middle = (self.first() + self.last()) / 2.0;
            self.span = NARROWEST;
            self.start = middle - NARROWEST / 2.0;
        } else if frame == 2 * phase {
            for (name, samples) in ["whole capture", "narrowest view"].iter().zip(&measure.samples) {
                let column = |n: usize| {
                    let mut sorted: Vec<f64> = samples.iter().map(|sample| sample[n]).collect();
                    sorted.sort_by(f64::total_cmp);
                    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
                    format!("median {:.2} p99 {:.2} max {:.2}", at(0.5), at(0.99), at(1.0))
                };
                println!(
                    "{name} ({} frames, ms): frame {}; cpu {}; timeline {}",
                    samples.len(),
                    column(0),
                    column(1),
                    column(2)
                );
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole view shows every event and no time before the first.
    #[test]
    fn the_whole_view_starts_at_the_first_event_and_shows_the_last() {
        for (first, last) in [(0.0, 1e6), (-500.0, 2e9), (0.0, 10.0), (3.0, 3.0)] {
            let (start, span) = whole(first, last);
            assert_eq!(start, first, "{first} {last}");
            assert!(start + span >= last, "{first} {last}: ends at {}", start + span);
        }
    }

    /// Panning or zooming never shows time before the first event.
    #[test]
    fn a_view_never_starts_before_the_first_event() {
        for start in [-1e9, -1.0, 0.0] {
            assert_eq!(clamped(start, 1000.0, 0.0, 1e6), 0.0);
        }
        assert_eq!(clamped(5e5, 1000.0, 0.0, 1e6), 5e5);
    }

    /// A tick at the origin reads 0, not -0, from a view starting just
    /// before it, in seconds and in cycles.
    #[test]
    fn no_tick_reads_minus_zero() {
        for frequency in [Some(250e6), None] {
            let ticks = ticks(-3.0, 1e6, 1000.0, frequency);
            assert!(!ticks.is_empty());
            for (_, label) in ticks {
                assert!(!label.starts_with("-0"), "{label}");
            }
        }
    }
}
