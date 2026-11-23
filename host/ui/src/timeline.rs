//! The timeline: one row per event type of the catalogue, time across. A row
//! draws at most one shape per pixel column, however many events fall in it,
//! so a frame costs the same at any zoom (ADR-055). Losses and suspect lines
//! are bands across the rows, between the events recorded before and after
//! them.

use crate::levels::Levels;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use kymo::decode;
use kymo::events::Catalogue;
use kymo::model::{Element, Trace};
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

pub struct Timeline {
    catalogue: &'static Catalogue,
    trace: Trace<'static>,
    rows: Vec<Row>,
    marks: Vec<Mark>,
    /// The cycles of the first event, which the view counts from.
    origin: i64,
    /// The earliest and latest event, in cycles from `origin`.
    first: f64,
    last: f64,
    /// The view: its left edge and its width, in cycles from `origin`.
    start: f64,
    span: f64,
    /// The status line's counts, taken once: each is a pass over the trace.
    events: usize,
    losses: usize,
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
struct Mark {
    from: i64,
    to: i64,
    element: usize,
}

impl Timeline {
    pub fn new(catalogue: &'static Catalogue, trace: Trace<'static>, measure: bool) -> Self {
        let ids: Vec<u16> = catalogue.events.keys().copied().collect();
        let mut rows: Vec<Vec<(i64, usize)>> = vec![Vec::new(); ids.len()];
        let (mut marks, mut pending, mut before) = (Vec::new(), Vec::new(), None);
        let (mut lowest, mut highest) = (i64::MAX, i64::MIN);
        for (n, element) in trace.elements.iter().enumerate() {
            if let Element::Event { event, cycles, .. } = element {
                let row = ids.binary_search(&event.event.id).expect("a type of the catalogue");
                rows[row].push((*cycles, n));
                for element in pending.drain(..) {
                    let from = before.unwrap_or(*cycles);
                    marks.push(Mark {
                        from: from.min(*cycles),
                        to: from.max(*cycles),
                        element,
                    });
                }
                before = Some(*cycles);
                lowest = lowest.min(*cycles);
                highest = highest.max(*cycles);
            } else {
                pending.push(n);
            }
        }
        let after = before.unwrap_or_default();
        marks.extend(pending.into_iter().map(|element| Mark {
            from: after,
            to: after,
            element,
        }));
        marks.sort_by_key(|mark| mark.from);
        let origin = trace
            .elements
            .iter()
            .find_map(|element| match element {
                Element::Event { cycles, .. } => Some(*cycles),
                _ => None,
            })
            .unwrap_or_default();
        let rows = catalogue
            .events
            .values()
            .zip(rows)
            .map(|(event, mut events)| {
                events.sort_by_key(|event| event.0);
                Row {
                    name: event.name.clone(),
                    levels: Levels::new(events.iter().map(|event| event.0).collect()),
                    elements: events.iter().map(|event| event.1).collect(),
                }
            })
            .collect();
        let (first, last) = if lowest <= highest {
            ((lowest - origin) as f64, (highest - origin) as f64)
        } else {
            (0.0, 0.0)
        };
        let losses = trace
            .elements
            .iter()
            .filter(|element| matches!(element, Element::Lost { .. }))
            .count();
        let (events, suspects) = (trace.events(), trace.suspects());
        let mut timeline = Self {
            catalogue,
            trace,
            rows,
            marks,
            origin,
            first,
            last,
            start: 0.0,
            span: 0.0,
            events,
            losses,
            suspects,
            painted: 0.0,
            measure: measure.then(Measure::default),
        };
        timeline.whole();
        timeline
    }

    /// Shows the whole capture.
    fn whole(&mut self) {
        self.span = self.widest() / 1.05;
        self.start = self.first - (self.span * 0.025);
    }

    fn widest(&self) -> f64 {
        ((self.last - self.first) * 1.05).max(NARROWEST)
    }

    /// Zooms by `factor` about the point `at` of the view's width, 0 to 1.
    fn zoom(&mut self, at: f64, factor: f64) {
        let pivot = self.start + at * self.span;
        self.span = (self.span * factor).clamp(NARROWEST, self.widest());
        self.start = pivot - at * self.span;
    }

    fn clamp(&mut self) {
        let low = self.first - self.span / 2.0;
        let high = (self.last - self.span / 2.0).max(low);
        self.start = self.start.clamp(low, high);
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

        let view = View {
            origin: self.origin,
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

        // The events of all rows in one mesh: their rectangles sit on whole
        // pixels and need none of the anti-aliasing a shape each would get.
        let mut events = egui::Mesh::default();
        for (n, row) in self.rows.iter().enumerate() {
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
                &row.name,
                font.clone(),
                text,
            );
            for (from, to) in row.levels.columns(&view) {
                events.add_colored_rect(
                    Rect::from_min_max(egui::pos2(from, top + 3.0), egui::pos2(to, top + ROW - 3.0)),
                    EVENT,
                );
            }
        }
        painter.add(egui::Shape::mesh(events));
        let bottom = (area.top() + self.rows.len() as f32 * ROW).min(area.bottom());
        for (kind, color) in [(false, LOSS), (true, SUSPECT)] {
            let marks = self
                .marks
                .iter()
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
            && let Some(hovered) = self.hovered(&view, area, pointer)
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
        // Ticks in seconds when the frequency is known, else in cycles.
        let hz = self.frequency().unwrap_or(1.0);
        let wanted = TICK * view.span / view.width / hz;
        let magnitude = 10f64.powf(wanted.log10().floor());
        let step = [1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .map(|n| n * magnitude)
            .find(|step| *step >= wanted)
            .unwrap_or(10.0 * magnitude);
        let (unit, suffix) = match self.frequency() {
            None => (1.0, " cycles"),
            Some(_) if step < 1e-6 => (1e-9, " ns"),
            Some(_) if step < 1e-3 => (1e-6, " µs"),
            Some(_) if step < 1.0 => (1e-3, " ms"),
            Some(_) => (1.0, " s"),
        };
        let decimals = (-(step / unit).log10().floor()).max(0.0) as usize;
        let font = FontId::proportional(11.0);
        let mut tick = (view.start / hz / step).ceil();
        loop {
            let at = tick * step * hz;
            if at > view.start + view.span {
                break;
            }
            let x = view.x(at);
            painter.line_segment(
                [egui::pos2(x, top + RULER - 6.0), egui::pos2(x, bottom)],
                Stroke::new(1.0, grid),
            );
            painter.text(
                egui::pos2(x + 3.0, top + 2.0),
                Align2::LEFT_TOP,
                format!("{:.decimals$}{suffix}", tick * step / unit),
                font.clone(),
                text,
            );
            tick += 1.0;
        }
        painter.line_segment(
            [
                egui::pos2(area.left(), top + RULER),
                egui::pos2(area.right(), top + RULER),
            ],
            Stroke::new(1.0, grid),
        );
    }

    /// What the pointer is over: the nearest event of its row, and the marks
    /// at its column.
    fn hovered(&self, view: &View, area: Rect, pointer: Pos2) -> Option<String> {
        if pointer.x < area.left() || pointer.y < area.top() {
            return None;
        }
        let low = view.cycles(pointer.x - HOVER);
        let high = view.cycles(pointer.x + HOVER);
        let mut text = String::new();
        if let Some(row) = self.rows.get(((pointer.y - area.top()) / ROW) as usize) {
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
        for mark in self
            .marks
            .iter()
            .filter(|mark| mark.from <= high && mark.to >= low)
            .take(8)
        {
            if !text.is_empty() {
                text.push('\n');
            }
            match self.trace.elements[mark.element] {
                Element::Lost { from, to } => {
                    let _ = write!(text, "lost {from} {to}: {} indexes", to.wrapping_sub(from));
                }
                Element::Suspect { from, to, writers } => {
                    let _ = write!(
                        text,
                        "suspect {from} {to}: {writers} lapped writers no danger accounts for"
                    );
                }
                Element::Event { .. } => {}
            }
        }
        (!text.is_empty()).then_some(text)
    }

    fn status(&self) -> String {
        let mut status = format!(
            "{} events, {} losses, {} suspect writers",
            self.events, self.losses, self.suspects
        );
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

    /// The cycles, absolute, at pixel `x`.
    pub fn cycles(&self, x: f32) -> i64 {
        self.origin + (self.start + f64::from(x - self.left) / self.width * self.span).round() as i64
    }

    /// The marks in view, at least a pixel wide, merged where they touch.
    fn bands<'m>(&self, marks: impl Iterator<Item = &'m Mark>) -> Vec<(f32, f32)> {
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
            let middle = (self.first + self.last) / 2.0;
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
