//! A frame of the timeline: the view moved by the pointer, then every line drawn.

use super::load::{self, Drawn};
use super::rows::Line;
use super::view::{View, ticks};
use super::{LABELS, ROW, RULER, Timeline};
use crate::palette::{BAND, Palette};
use crate::spans::{Band, Spans};
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke};
use kymo::load::Covered;
use kymo::model::Element;
use kymo::opsy::Span;
use std::time::Instant;

impl Timeline {
    pub(super) fn timeline(&mut self, ui: &mut egui::Ui) {
        let began = Instant::now();
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let area = Rect::from_min_max(egui::pos2(rect.left() + LABELS, rect.top() + RULER), rect.max);
        let width = f64::from(area.width().max(1.0));
        if response.double_clicked() {
            self.whole();
        }
        // Dragged with shift held, a range is selected; else the view scrolls.
        let at = |timeline: &Self, x: f32| {
            timeline.origin() + (timeline.start + f64::from(x - area.left()) / width * timeline.span).round() as i64
        };
        if response.drag_started()
            && ui.input(|input| input.modifiers.shift)
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.anchor = Some(at(self, pointer.x));
        }
        self.moving = response.dragged() && response.drag_delta() != egui::Vec2::ZERO;
        if response.dragged() {
            match (self.anchor, response.interact_pointer_pos()) {
                (Some(anchor), Some(pointer)) => {
                    let to = at(self, pointer.x);
                    self.selection = (to != anchor).then(|| (anchor.min(to), anchor.max(to)));
                }
                _ => self.start -= f64::from(response.drag_delta().x) * self.span / width,
            }
        }
        if response.drag_stopped() {
            self.anchor = None;
        }
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.selection = None;
            self.picked = None;
        }
        if response.hovered() {
            let (scroll, zoom) = ui.input(|input| (input.smooth_scroll_delta, input.zoom_delta()));
            self.moving |= scroll != egui::Vec2::ZERO || zoom != 1.0;
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
        let palette = Palette::of(visuals);
        let labels = ui.painter_at(Rect::from_min_max(
            egui::pos2(rect.left(), area.top()),
            egui::pos2(area.left() - 4.0, rect.bottom()),
        ));
        let font = FontId::proportional(13.0);
        let heading = FontId::proportional(11.0);
        let strong = visuals.strong_text_color();
        let shown = lines.len().min(((area.height() / ROW).ceil() as usize).max(1));
        let bottom = (area.top() + lines.len() as f32 * ROW).min(area.bottom());

        // The backgrounds first: the column of names and the headers on the
        // surface, every other data row striped; then the grid, under the data.
        painter.rect_filled(
            Rect::from_min_max(rect.min, egui::pos2(area.left(), rect.bottom())),
            0.0,
            palette.surface,
        );
        for (n, &line) in lines.iter().enumerate().take(shown) {
            let top = area.top() + n as f32 * ROW;
            let header = matches!(line, Line::Section(_) | Line::Fold);
            if header || n % 2 == 1 {
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(area.left(), top), egui::pos2(area.right(), top + ROW)),
                    0.0,
                    if header { palette.surface } else { faint },
                );
            }
            if header {
                painter.line_segment(
                    [
                        egui::pos2(rect.left(), top + ROW - 0.5),
                        egui::pos2(area.right(), top + ROW - 0.5),
                    ],
                    Stroke::new(1.0, grid),
                );
            }
        }
        let ticks = ticks(view.start, view.span, view.width, self.frequency());
        for (at, _) in &ticks {
            let x = view.x(*at);
            painter.line_segment(
                [egui::pos2(x, rect.top() + RULER - 6.0), egui::pos2(x, bottom)],
                Stroke::new(1.0, grid),
            );
        }
        painter.line_segment(
            [
                egui::pos2(area.left() - 0.5, rect.top()),
                egui::pos2(area.left() - 0.5, rect.bottom()),
            ],
            Stroke::new(1.0, grid),
        );

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
        // The load drawn, by line: the bins the pointer reads.
        let mut drawn_rows: Vec<(usize, Vec<Drawn>)> = Vec::new();
        // Whether each label shown was cut.
        let mut elided: Vec<bool> = Vec::new();
        let capture = self.bounds.unwrap_or_default();
        let loaded = self
            .meaning
            .as_ref()
            .is_some_and(|meaning| self.mode.load(load::busiest(meaning, &view), width));
        for (n, &line) in lines.iter().enumerate() {
            let top = area.top() + n as f32 * ROW;
            if top > area.bottom() {
                break;
            }
            let (font, color) = match line {
                Line::Section(_) => (heading.clone(), strong),
                Line::Fold => (font.clone(), strong),
                _ => (font.clone(), text),
            };
            // Cut with an ellipsis at the column's edge.
            let x = rect.left() + 8.0 + self.indent(line);
            let mut job = egui::text::LayoutJob::simple_singleline(self.label(line), font, color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(area.left() - 8.0 - x);
            let galley = labels.layout_job(job);
            elided.push(galley.elided);
            labels.galley(egui::pos2(x, top + (ROW - galley.size().y) / 2.0), galley, color);
            if line == Line::Fold {
                // Pointing right folded, down unfolded.
                let (x, y) = (rect.left() + 13.0, top + ROW / 2.0);
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
                labels.add(egui::Shape::convex_polygon(points, strong, egui::Stroke::NONE));
            }
            let (high, low) = (top + 4.0, top + ROW - 4.0);
            let meaning = self.meaning.as_ref();
            if let Some(meaning) = meaning {
                let rows = &meaning.load;
                let known = &rows.cpu.known;
                let row = |covered: Option<&Covered>, color: Color32| {
                    covered.map(|covered| (load::row(&view, capture, covered, known), [color; 2]))
                };
                let drawn = match line {
                    Line::Cpu => Some((load::cpu(&view, capture, &rows.cpu), [palette.interrupt, palette.run])),
                    Line::Task(task) if loaded => row(
                        task.map_or(Some(&rows.idle), |task| rows.tasks.get(task)),
                        if task.is_some() { palette.run } else { palette.idle },
                    ),
                    Line::Handler(handler) if loaded => row(rows.handlers.get(handler), palette.interrupt),
                    Line::Mutex(mutex) if loaded => row(rows.holds.get(mutex), palette.hold),
                    _ => None,
                };
                if let Some((bins, colors)) = drawn {
                    load::paint(bars.mesh, &bins, area, (high, low), colors, palette.unknown);
                    drawn_rows.push((n, bins));
                    continue;
                }
            }
            match line {
                Line::Section(_) | Line::Fold | Line::Cpu => {}
                Line::Event(row) => {
                    for (from, to) in self.rows[row].levels.columns(&view) {
                        bars.mesh.add_colored_rect(
                            Rect::from_min_max(egui::pos2(from, high), egui::pos2(to, low)),
                            palette.event,
                        );
                    }
                }
                Line::Task(task) => {
                    let Some(meaning) = meaning else { continue };
                    // The time ready, a thin bar at the foot of the row: the
                    // runs, never at the same cycles, are drawn over it.
                    if let Some(ready) = task.and_then(|task| meaning.ready.get(task)) {
                        bars.paint(Some(ready), std::iter::empty(), low - READY, low, palette.ready);
                    }
                    let spans = task.map_or(Some(&meaning.idle), |task| meaning.runs.get(task));
                    let open = open.runs.iter().filter(|run| run.task == task).map(|run| run.span);
                    bars.paint(
                        spans,
                        open,
                        high,
                        low,
                        if task.is_some() { palette.run } else { palette.idle },
                    );
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
                        bars.paint(depths.get(depth), open, high, low, palette.interrupt);
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
                        bars.paint(Some(waits), open, high, low, palette.wait);
                    }
                    // Waiters not yet in a row: their waits are all open.
                    for hold in holders.clone().filter(|hold| {
                        hold.waiting
                            && !meaning
                                .waits
                                .get(mutex)
                                .is_some_and(|waits| waits.iter().any(|wait| wait.0 == hold.task))
                    }) {
                        bars.paint(None, std::iter::once(hold.span), high, low, palette.wait);
                    }
                    let open: Vec<_> = holders.filter(|hold| !hold.waiting).collect();
                    let closed = meaning.holds.get(mutex);
                    let middle = top + ROW / 2.0;
                    for band in bars.paint(closed, open.iter().map(|hold| hold.span), high, low, palette.hold) {
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
            // Small enough that the descenders stay inside the bar's 14 pixels.
            let small = FontId::proportional(11.0);
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
                        palette.owner,
                    );
                }
            }
        }
        // A loss or a suspect line: a light band over the rows, the data
        // readable through it, and a solid mark in the ruler.
        let mut flags = Vec::new();
        for (kind, color) in [(false, palette.loss), (true, palette.suspect)] {
            let marks = self
                .marks()
                .filter(|mark| matches!(self.trace.elements[mark.element], Element::Suspect { .. }) == kind);
            for (from, to) in view.bands(marks) {
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(from, area.top()), egui::pos2(to, bottom)),
                    0.0,
                    color.gamma_multiply(BAND),
                );
                flags.push(((from + to) / 2.0, color));
            }
        }
        // The range selected, shaded; the instance picked, framed: each at
        // least a pixel wide.
        let columns = |(from, to): (i64, i64)| {
            let from = view.column(from).max(area.left() - 2.0);
            (from, view.column(to).ceil().max(from + 1.0).min(area.right() + 2.0))
        };
        let selected = visuals.selection.bg_fill;
        if let Some(selection) = self.selection {
            let (from, to) = columns(selection);
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(from, area.top()), egui::pos2(to, bottom)),
                0.0,
                selected.gamma_multiply(0.3),
            );
        }
        if let Some(picked) = self.picked {
            let (from, to) = columns(picked);
            painter.rect_stroke(
                Rect::from_min_max(egui::pos2(from - 1.0, area.top()), egui::pos2(to + 1.0, bottom)),
                0.0,
                Stroke::new(2.0, selected),
                egui::StrokeKind::Inside,
            );
        }
        // The ruler's labels, over the bands and the selection.
        let small = FontId::proportional(11.0);
        for (at, label) in ticks {
            painter.text(
                egui::pos2(view.x(at) + 3.0, rect.top() + 2.0),
                Align2::LEFT_TOP,
                label,
                small.clone(),
                visuals.weak_text_color(),
            );
        }
        painter.line_segment(
            [
                egui::pos2(area.left(), rect.top() + RULER),
                egui::pos2(area.right(), rect.top() + RULER),
            ],
            Stroke::new(1.0, grid),
        );
        // Pointing down at the rule, over the ruler's labels.
        let y = rect.top() + RULER;
        for (x, color) in flags {
            let points = vec![
                egui::pos2(x - FLAG, y - FLAG),
                egui::pos2(x + FLAG, y - FLAG),
                egui::pos2(x, y),
            ];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        self.painted = began.elapsed().as_secs_f64() * 1e3;
        // A label cut or shortened, in full.
        if !response.dragged()
            && let Some(pointer) = response.hover_pos()
            && pointer.x < area.left()
            && pointer.y >= area.top()
        {
            let n = ((pointer.y - area.top()) / ROW) as usize;
            if let Some(&line) = lines.get(n) {
                let full = self.full_label(line);
                if elided.get(n).copied().unwrap_or_default() || full != self.label(line) {
                    response.clone().on_hover_text_at_pointer(full);
                }
            }
        }
        if !response.dragged()
            && let Some(pointer) = response.hover_pos()
            && let Some((hovered, arrow)) = self.hovered(&view, area, pointer, &lines, &open, &drawn_rows)
        {
            // The wake, from the line of what woke the task to the task's
            // line, both shown; at the wake's column.
            let row = |line: Line| lines.iter().position(|&other| other == line);
            let to = lines
                .get(((pointer.y - area.top()) / ROW) as usize)
                .and_then(|&line| row(line));
            if let Some((arrow, from, to)) = arrow.and_then(|arrow| Some((arrow, row(arrow.from)?, to?))) {
                let middle = |n: usize| area.top() + (n as f32 + 0.5) * ROW;
                let x = view.column(arrow.cycles) + 0.5;
                if x >= area.left() && x < area.right() {
                    let (from, to) = (middle(from), middle(to));
                    painter.arrow(
                        egui::pos2(x, from),
                        egui::vec2(0.0, to - from),
                        Stroke::new(1.5, palette.wake),
                    );
                }
            }
            response.on_hover_ui_at_pointer(|ui| {
                ui.label(egui::RichText::new(hovered).monospace());
            });
        }
    }
}

/// Half the width, and the height, of a loss's mark in the ruler, in pixels.
const FLAG: f32 = 4.0;

/// The height of the bar of a task's time ready, in pixels.
const READY: f32 = 3.0;

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
