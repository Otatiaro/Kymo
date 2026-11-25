//! The timeline: one row per event type of the catalogue, time across. A row
//! draws at most one shape per pixel column, however many events fall in it,
//! so a frame costs the same at any zoom (ADR-055). Losses and suspect lines
//! are bands across the rows, between the events recorded before and after
//! them.
//!
//! An image with a kernel adapter shows its meaning instead: a row per task,
//! the idle task, each interrupt handler and each mutex, drawn as spans; its
//! events are in the event list below. The lines stretch to fill the height
//! the bottom panel leaves.

mod draw;
mod hover;
mod list;
mod load;
mod measure;
mod rows;
mod status;
mod toolbar;
mod tree;
mod view;
mod words;

pub use status::counted;
pub use toolbar::theme_menu;
pub use view::View;

use crate::durations::Durations;
use crate::filter::Filter;
use crate::levels::Levels;
use crate::listing::{Entry, Events};
use crate::meaning::Meaning;
use crate::names;
use crate::panel::Panel;
use eframe::egui;
use kymo::events::{Catalogue, Kind};
use kymo::model::Trace;
use kymo::stats;
use kymo::target::Result;
use list::Tab;
use load::{Binning, Mode};
use measure::Measure;
use rows::{Mark, Row};
use std::collections::{BTreeSet, HashMap};
use view::{clamped, whole, widest};
use words::Words;

/// The width of the column of row names.
const LABELS: f32 = 260.0;
const RULER: f32 = 22.0;
/// The height of a line, in pixels; of a task's, and of the CPU's load.
const ROW: f32 = 22.0;
const TASK: f32 = 30.0;
const CPU: f32 = 60.0;
/// The height a line of durations takes for each lane past its first: a
/// lane's bar and the gap above it.
const LANE: f32 = 16.0;
/// The gap between two lanes' bars, in pixels.
const GAP: f32 = 2.0;
/// The bottom panel's height at first, a share of the window's.
const BOTTOM: f32 = 1.0 / 3.0;
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
    /// The kernel's meaning, when the image has an adapter.
    meaning: Option<Meaning>,
    /// The application's durations, when the image has a begin.
    durations: Option<Durations>,
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
    /// The statistics of the kernel's instances, with its meaning.
    panel: Option<Panel>,
    /// The range selected, in cycles, and where a selection being dragged
    /// started.
    selection: Option<(i64, i64)>,
    anchor: Option<i64>,
    /// The instance picked in the panel: its start and end, in cycles.
    picked: Option<(i64, i64)>,
    /// Whether the last frame moved the view or the selection.
    moving: bool,
    /// How the kernel's rows are drawn.
    mode: Mode,
    /// How the load view bins time.
    binning: Binning,
    /// The event list's lines, the range it shows, the element clicked in
    /// it and the one to scroll to.
    list: Events,
    listed: Option<(i64, i64)>,
    chosen: Option<usize>,
    reveal: Option<usize>,
    /// The bottom panel's tab.
    tab: Tab,
    /// The namespace filter, whether its panel is open, the namespaces it
    /// reads, and the event list's lines it keeps.
    filter: Filter,
    filtering: bool,
    spaces: Spaces,
    kept: Option<Kept>,
    /// The text filter of the event list.
    words: Words,
    /// The headings of the column of names folded, by section and path.
    folded: BTreeSet<String>,
}

/// The namespaces the filter reads (names.rs).
#[derive(Default)]
struct Spaces {
    /// Each subject's, by kind as `Meaning::names` orders them.
    subjects: [Vec<String>; 3],
    /// Each subject's by its address, none at address 0.
    at: HashMap<u64, String>,
    /// Each row's type's, and where its pointers are in its payload: offset
    /// and size.
    types: Vec<String>,
    pointers: Vec<Vec<(usize, usize)>>,
    /// Each type of duration's, by its line.
    durations: Vec<String>,
}

/// The event list's lines the filters keep, of a range, at a generation of
/// the namespace filter and a text of the text filter.
struct Kept {
    range: (i64, i64),
    generation: u64,
    words: String,
    entries: Vec<Entry>,
}

impl Timeline {
    pub fn new(capture: String, catalogue: &'static Catalogue, trace: Trace<'static>, measure: bool) -> Result<Self> {
        let meaning = Meaning::new(catalogue, trace.first)?;
        let mut timeline = Self {
            capture,
            meaning,
            durations: Durations::new(catalogue, trace.first),
            catalogue,
            trace,
            ids: catalogue.events.keys().copied().collect(),
            rows: names::types(
                &catalogue
                    .events
                    .values()
                    .map(|event| event.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .into_iter()
            .map(|name| Row {
                name,
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
            binning: Binning::default(),
            list: Events::default(),
            listed: None,
            chosen: None,
            reveal: None,
            tab: Tab::Events,
            filter: Filter::default(),
            filtering: false,
            spaces: Spaces::default(),
            kept: None,
            words: Words::default(),
            folded: BTreeSet::new(),
        };
        timeline.add(0);
        timeline.panel = timeline.meaning.as_ref().map(|meaning| {
            let (instances, full) = meaning.shorten(kymo::meaning::instances(&meaning.kernel));
            Panel::named(instances, full)
        });
        timeline.spaces = Spaces::of(catalogue, timeline.meaning.as_ref(), timeline.durations.as_ref());
        let spaces = &timeline.spaces;
        timeline.filter = Filter::new(
            spaces
                .types
                .iter()
                .chain(spaces.subjects.iter().flatten())
                .chain(&spaces.durations)
                .map(String::as_str),
        );
        timeline.whole();
        Ok(timeline)
    }

    /// Restores the namespaces unticked, those this image has, and the
    /// headings folded, from what saved gives by key.
    pub fn restore(&mut self, saved: impl Fn(&str) -> Option<String>) {
        if let Some(filter) = saved(FILTER) {
            self.filter.restore(&filter);
        }
        if let Some(folded) = saved(FOLDED) {
            self.folded = folded.lines().map(str::to_owned).collect();
        }
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

impl Spaces {
    fn of(catalogue: &Catalogue, meaning: Option<&Meaning>, durations: Option<&Durations>) -> Self {
        let mut spaces = Self::default();
        if let Some(durations) = durations {
            spaces.durations = durations.types.iter().map(|of| names::namespace(of)).collect();
        }
        for event in catalogue.events.values() {
            spaces.types.push(names::namespace(&event.name));
            let pointer = |field: &&kymo::events::Field| {
                matches!(field.ty.kind, Kind::Pointer(_)) && field.ty.size.is_some_and(|size| (1..=8).contains(&size))
            };
            let pointers = event.fields.iter().filter(pointer);
            let pointers = pointers.map(|field| (field.offset as usize, field.ty.size.unwrap_or_default() as usize));
            spaces.pointers.push(pointers.collect());
        }
        if let Some(meaning) = meaning {
            let kernel = &meaning.kernel;
            let lists = [&kernel.tasks.list, &kernel.handlers.list, &kernel.mutexes.list];
            for (spaces_of, list) in spaces.subjects.iter_mut().zip(lists) {
                for subject in list {
                    let space = names::namespace(&subject.name);
                    if subject.address != 0 {
                        spaces.at.insert(subject.address, space.clone());
                    }
                    spaces_of.push(space);
                }
            }
        }
        spaces
    }
}

/// The key the namespaces unticked are saved under.
const FILTER: &str = "namespaces unticked";
/// The key the headings folded are saved under.
const FOLDED: &str = "namespaces folded";

impl Timeline {
    /// The capture's file name.
    pub fn capture(&self) -> &str {
        &self.capture
    }

    /// Saves the namespaces unticked and the headings folded.
    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(FILTER, self.filter.saved());
        storage.set_string(FOLDED, self.folded.iter().cloned().collect::<Vec<_>>().join("\n"));
    }

    /// The timeline in `ui`, the window's controls `files` draws first in
    /// its toolbar.
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, files: &mut dyn FnMut(&mut egui::Ui)) {
        if self.measure.is_some() {
            self.measure_frame(ui.ctx(), frame.info().cpu_usage.unwrap_or_default());
        }
        let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6));
        egui::Panel::top("toolbar")
            .frame(frame)
            .show(ui, |ui| self.toolbar(ui, files));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| self.status(ui));
        });
        if self.filtering {
            egui::Panel::left("namespaces")
                .resizable(true)
                .default_size(220.0)
                .show(ui, |ui| self.filter.show(ui));
        }
        // A third of the window at first; the timeline's lines fill the rest.
        egui::Panel::bottom("lower")
            .resizable(true)
            .default_size(ui.available_height() * BOTTOM)
            .show(ui, |ui| self.bottom(ui));
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
        let picked = panel.show(ui, &title, &|kind, name| self.subject_shown(kind, name));
        self.panel = Some(panel);
        if let Some(instance) = picked {
            self.go_to(instance.from.cycles, instance.to.cycles);
        }
    }

    /// Whether the filter shows the subject of `kind`, as
    /// `kymo::meaning::instances` names the kinds, named `name` in full; one
    /// of another kind, or unknown, it does.
    fn subject_shown(&self, kind: &str, name: &str) -> bool {
        let Some(meaning) = &self.meaning else {
            return true;
        };
        let kernel = &meaning.kernel;
        let (n, list) = match kind {
            "task" => (0, &kernel.tasks.list),
            "handler" => (1, &kernel.handlers.list),
            "mutex" => (2, &kernel.mutexes.list),
            _ => return true,
        };
        list.iter()
            .position(|subject| subject.name == name)
            .and_then(|at| self.spaces.subjects[n].get(at))
            .is_none_or(|space| self.filter.shown(space))
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
