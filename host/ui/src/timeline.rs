//! The timeline: one row per event type of the catalogue, time across. A row
//! draws at most one shape per pixel column, however many events fall in it,
//! so a frame costs the same at any zoom (ADR-055). Losses and suspect lines
//! are bands across the rows, between the events recorded before and after
//! them.
//!
//! An image with the OpSy adapter shows its meaning above them: a row per
//! task, the idle task, each interrupt handler and each mutex, drawn as
//! spans; the rows of event types are then folded under one line.

mod draw;
mod hover;
mod list;
mod load;
mod measure;
mod rows;
mod status;
mod toolbar;
mod view;

pub use status::counted;
pub use view::View;

use crate::levels::Levels;
use crate::listing::Events;
use crate::meaning::Meaning;
use crate::panel::Panel;
use eframe::egui;
use kymo::events::Catalogue;
use kymo::model::Trace;
use kymo::stats;
use kymo::target::Result;
use list::Tab;
use load::Mode;
use measure::Measure;
use rows::{Mark, Row};
use view::{clamped, whole, widest};

/// The width of the column of row names.
const LABELS: f32 = 260.0;
const RULER: f32 = 22.0;
const ROW: f32 = 22.0;
/// The bottom panel's least height at first, in pixels.
const BOTTOM: f32 = 160.0;
/// The narrowest view, in cycles.
const NARROWEST: f64 = 100.0;
/// How near the pointer, in pixels, an event or a mark is hovered.
const HOVER: f32 = 4.0;
/// The ruler's spacing between ticks, in pixels, at least.
const TICK: f64 = 110.0;

/// How many events a run's hover shows, the switch that ends it last.
const SWITCH: usize = 4;

pub struct Timeline {
    /// The capture's file name.
    capture: String,
    catalogue: &'static Catalogue,
    trace: Trace<'static>,
    /// The event types of the catalogue, a row each, in its order.
    ids: Vec<u16>,
    rows: Vec<Row>,
    /// OpSy's meaning, when the image has the adapter.
    meaning: Option<Meaning>,
    /// Whether the rows of event types are folded.
    folded: bool,
    /// Whether the bottom panel still takes the height the lines leave: until
    /// it is resized by hand.
    fit: bool,
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
    /// The statistics of OpSy's instances, with OpSy's meaning.
    panel: Option<Panel>,
    /// The range selected, in cycles, and where a selection being dragged
    /// started.
    selection: Option<(i64, i64)>,
    anchor: Option<i64>,
    /// The instance picked in the panel: its start and end, in cycles.
    picked: Option<(i64, i64)>,
    /// Whether the last frame moved the view or the selection.
    moving: bool,
    /// How OpSy's rows are drawn.
    mode: Mode,
    /// The event list's lines, the range it shows, the element clicked in
    /// it and the one to scroll to.
    list: Events,
    listed: Option<(i64, i64)>,
    chosen: Option<usize>,
    reveal: Option<usize>,
    /// The bottom panel's tab.
    tab: Tab,
}

impl Timeline {
    pub fn new(capture: String, catalogue: &'static Catalogue, trace: Trace<'static>, measure: bool) -> Result<Self> {
        let meaning = Meaning::new(catalogue, trace.first)?;
        let mut timeline = Self {
            capture,
            folded: meaning.is_some(),
            fit: true,
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
            panel: None,
            selection: None,
            anchor: None,
            picked: None,
            moving: false,
            mode: Mode::Auto,
            list: Events::default(),
            listed: None,
            chosen: None,
            reveal: None,
            tab: Tab::Events,
        };
        timeline.add(0);
        timeline.panel = timeline.meaning.as_ref().map(|meaning| {
            let (instances, full) = meaning.shorten(kymo::meaning::instances(&meaning.kernel));
            Panel::named(instances, full)
        });
        timeline.whole();
        Ok(timeline)
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
}

impl eframe::App for Timeline {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.measure.is_some() {
            self.measure_frame(ui.ctx(), frame.info().cpu_usage.unwrap_or_default());
        }
        let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6));
        egui::Panel::top("toolbar").frame(frame).show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| self.status(ui));
        });
        // The timeline as tall as its lines, the panel the rest, until the
        // panel is resized: its size egui keeps is dropped every frame before.
        let lines = RULER + self.lines().len() as f32 * ROW + ROW;
        let fitted = (ui.available_height() - lines).max(BOTTOM);
        let id = ui.scope_id().with("bottom");
        if self.fit {
            ui.ctx().data_mut(|data| data.remove::<egui::PanelState>(id));
        }
        egui::Panel::bottom("bottom")
            .resizable(true)
            .default_size(fitted)
            .show(ui, |ui| self.bottom(ui));
        if egui::PanelState::load(ui.ctx(), id).is_some_and(|state| (state.size().y - fitted).abs() > 0.5) {
            self.fit = false;
        }
        egui::CentralPanel::default().show(ui, |ui| self.timeline(ui));
        // The panel came before the view moved: it shows the new range next frame.
        if self.behind() {
            ui.ctx().request_repaint();
        }
    }
}

impl Timeline {
    /// The range the statistics are of: the selection, or else the view.
    fn range(&self) -> stats::Range {
        let (from, to) = self.selection.unwrap_or_else(|| {
            let origin = self.origin() as f64;
            (
                (origin + self.start).round() as i64,
                (origin + self.start + self.span).round() as i64,
            )
        });
        stats::Range {
            from: Some(from),
            to: Some(to),
        }
    }

    /// The panel, of the range the view had when it last stood still: over
    /// the whole capture, a report costs more than a frame of the timeline.
    fn statistics(&mut self, ui: &mut egui::Ui) {
        let (Some(mut panel), Some(meaning)) = (self.panel.take(), &self.meaning) else {
            return;
        };
        let range = self.range();
        if !self.moving || panel.range().is_none() {
            panel.report(
                &self.trace,
                &meaning.kernel.contradictions,
                &meaning.load.cpu.known,
                range,
            );
        }
        let shown = panel.range().expect("a report computed");
        let (from, to) = (shown.from.unwrap_or_default(), shown.to.unwrap_or_default());
        let title = self.range_title(from, to);
        let picked = panel.show(ui, &title);
        self.panel = Some(panel);
        if let Some(instance) = picked {
            self.go_to(instance.from.cycles, instance.to.cycles);
        }
    }

    /// Centres the view on `from` to `to`, a third of its width, and frames
    /// it.
    fn go_to(&mut self, from: i64, to: i64) {
        let width = (to - from) as f64;
        self.span = (width * 3.0).clamp(NARROWEST, widest(self.first(), self.last()));
        self.start = (from - self.origin()) as f64 + width / 2.0 - self.span / 2.0;
        self.clamp();
        self.picked = Some((from, to));
    }
}
