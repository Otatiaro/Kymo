//! The bottom panel: the event list of the range (ADR-072) and, with the kernel's
//! meaning, the statistics.

use super::hover::marked;
use super::status::counted;
use super::view::centred;
use super::{Kept, Timeline};
use crate::filter::Filter;
use crate::listing::Entry;
use crate::palette::{Palette, Shade};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use kymo::decode::{self, Decoded};
use kymo::events::Pointee;
use kymo::model::Element;
use std::cell::RefCell;
use std::ops::Range;

/// Whether `filter` keeps an event of a type in the namespace `space`, whose
/// pointers point at subjects in the namespaces `subjects`: one of them
/// ticked is enough.
fn kept<'a>(filter: &Filter, space: &str, mut subjects: impl Iterator<Item = &'a str>) -> bool {
    filter.shown(space) || subjects.any(|subject| filter.shown(subject))
}

/// The width of the bar that marks a loss or a suspect line, in pixels.
const MARK: f32 = 4.0;
/// The room a cell leaves around its text, in pixels.
const CELL: f32 = 16.0;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Tab {
    Events,
    Statistics,
}

impl Timeline {
    /// The tabs, when there is more than the list, then the tab chosen.
    pub(super) fn bottom(&mut self, ui: &mut egui::Ui) {
        if self.panel.is_some() {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Events, "Events");
                ui.selectable_value(&mut self.tab, Tab::Statistics, "Statistics");
            });
            ui.separator();
        }
        match self.tab {
            Tab::Statistics if self.panel.is_some() => self.statistics(ui),
            _ => self.list(ui),
        }
    }

    /// Whether the panel shows another range than the timeline's, for the
    /// next frame to show it.
    pub(super) fn behind(&self) -> bool {
        let range = self.range();
        let range = (range.from.unwrap_or_default(), range.to.unwrap_or_default());
        match self.tab {
            Tab::Statistics if self.panel.is_some() => self
                .panel
                .as_ref()
                .is_some_and(|panel| panel.range() != Some(self.range())),
            _ => self.listed != Some(range),
        }
    }

    /// The range's title: the selection's or the view's, from and to.
    pub(super) fn range_title(&self, from: i64, to: i64) -> String {
        format!(
            "{} {} to {}",
            if self.selection == Some((from, to)) {
                "Selection"
            } else {
                "View (shift-drag to select, Esc to clear)"
            },
            self.time((from - self.origin()) as f64),
            self.time((to - self.origin()) as f64)
        )
    }

    /// The elements of the range the view had when it last stood still, a
    /// line each, only those scrolled to written. Clicking one goes to it.
    /// An event's fields, a subject by the name its row shows, anything else
    /// as decode names it.
    pub(super) fn fields(&self, event: &Decoded) -> String {
        self.fields_and_tasks(event).0
    }

    /// `fields`, and where in them each task is named, with its shade.
    fn fields_and_tasks(&self, event: &Decoded) -> (String, Vec<(Range<usize>, Shade)>) {
        let symbols = &self.catalogue.symbols;
        let tasks = RefCell::new(Vec::new());
        let shown = |out: &mut String, pointee: &Pointee, address: u64| {
            let meaning = self.meaning.as_ref();
            let from = out.len();
            match meaning.and_then(|meaning| meaning.shown_at(address)) {
                Some(name) => out.push_str(name),
                None => match symbols.name(pointee, address) {
                    Some(name) => out.push_str(&name),
                    None => return false,
                },
            }
            if let Some(meaning) = meaning
                && let Some(task) = meaning.task_at(address)
            {
                tasks.borrow_mut().push((from..out.len(), meaning.shades[task]));
            }
            true
        };
        let mut fields = String::new();
        decode::fields_named(&mut fields, &shown, event);
        (fields, tasks.into_inner())
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let reveal = self.reveal.take();
        let range = self.range();
        if !self.moving || self.listed.is_none() {
            self.listed = Some((range.from.unwrap_or_default(), range.to.unwrap_or_default()));
        }
        let (from, to) = self.listed.expect("set above");
        self.keep(from, to);
        let entries = match &self.kept {
            Some(kept) => &kept.entries,
            None => self.list.within(from, to),
        };
        ui.horizontal(|ui| {
            ui.strong(self.range_title(from, to));
            ui.label(counted(entries.len() as u64, "line", "lines"));
            if self.kept.is_some() {
                ui.weak("of the namespaces ticked");
            }
        });
        ui.separator();
        let symbols = &self.catalogue.symbols;
        // A line's cells: index, type, fields; a loss's or a suspect line's
        // text in the fields, and the type and fields in full when they
        // differ.
        type Fields = (String, Vec<(Range<usize>, Shade)>);
        let cells = |entry: &Entry| -> (String, String, Fields, Option<String>) {
            match &self.trace.elements[entry.element] {
                Element::Event { event, .. } => {
                    let (fields, tasks) = self.fields_and_tasks(event);
                    let row = self
                        .ids
                        .binary_search(&event.event.id)
                        .expect("an event of the catalogue");
                    let kind = self.rows[row].name.clone();
                    let mut in_full = String::new();
                    decode::fields(&mut in_full, symbols, event);
                    let full = (kind != event.event.name || fields != in_full)
                        .then(|| format!("{}  {in_full}", event.event.name).trim_end().to_owned());
                    (event.index.to_string(), kind, (fields, tasks), full)
                }
                other => (
                    String::new(),
                    String::new(),
                    (marked(other).unwrap_or_default(), Vec::new()),
                    None,
                ),
            }
        };
        let height = ui.spacing().interact_size.y;
        let palette = Palette::of(ui.visuals());
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let digit = ui.fonts_mut(|fonts| fonts.glyph_width(&font, '0'));
        // The widest time, the last event's, and the widest index.
        let time = digit * self.time(self.last()).len() as f32 + CELL;
        let index = digit * 10.0 + CELL;
        let names = digit * self.rows.iter().map(|row| row.name.len()).max().unwrap_or_default() as f32 + CELL;
        let text = |ui: &mut egui::Ui, text: String, strong: bool| {
            let text = egui::RichText::new(text).font(font.clone());
            let text = if strong { text.strong() } else { text };
            ui.add(egui::Label::new(text).selectable(false).extend());
        };
        // The fields, each task named in its colour.
        let fields = |ui: &mut egui::Ui, (fields, tasks): Fields| {
            let plain = egui::TextFormat::simple(font.clone(), ui.visuals().text_color());
            let mut job = egui::text::LayoutJob::default();
            let mut at = 0;
            for (range, shade) in tasks {
                job.append(&fields[at..range.start], 0.0, plain.clone());
                let color = palette.task(shade);
                job.append(&fields[range.clone()], 0.0, egui::TextFormat { color, ..plain.clone() });
                at = range.end;
            }
            job.append(&fields[at..], 0.0, plain);
            ui.add(egui::Label::new(job).selectable(false).extend());
        };
        // A cell's text on one line, centred in its height; a number's at
        // the right.
        let right = egui::Layout::right_to_left(egui::Align::Center);
        let left = egui::Layout::left_to_right(egui::Align::Center);
        let mut clicked = None;
        let mut table = TableBuilder::new(ui)
            .id_salt("events")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .auto_shrink(false)
            .cell_layout(left)
            .column(Column::initial(time).at_least(CELL))
            .column(Column::initial(index).at_least(CELL))
            .column(Column::initial(names).at_least(CELL).clip(true))
            .column(Column::remainder().clip(true));
        if let Some(element) = reveal
            && let Some(n) = entries.iter().position(|entry| entry.element == element)
        {
            table = table.scroll_to_row(n, Some(egui::Align::Center));
        }
        table
            .header(height, |mut header| {
                header.col(|ui| {
                    ui.with_layout(right, |ui| text(ui, "time".to_owned(), true));
                });
                header.col(|ui| {
                    ui.with_layout(right, |ui| text(ui, "index".to_owned(), true));
                });
                header.col(|ui| {
                    text(ui, "type".to_owned(), true);
                });
                header.col(|ui| {
                    text(ui, "fields".to_owned(), true);
                });
            })
            .body(|body| {
                body.rows(height, entries.len(), |mut row| {
                    let entry = entries[row.index()];
                    row.set_selected(self.chosen == Some(entry.element));
                    let (index, kind, line, full) = cells(&entry);
                    let mark = match self.trace.elements[entry.element] {
                        Element::Lost { .. } => Some(palette.loss),
                        Element::Suspect { .. } => Some(palette.suspect),
                        Element::Event { .. } => None,
                    };
                    let at = self.time((entry.cycles - self.origin()) as f64);
                    row.col(|ui| {
                        if let Some(mark) = mark {
                            let cell = ui.max_rect();
                            let bar = egui::Rect::from_min_size(cell.min, egui::vec2(MARK, cell.height()));
                            ui.painter().rect_filled(bar, 0.0, mark);
                        }
                        ui.with_layout(right, |ui| text(ui, at, false));
                    });
                    row.col(|ui| {
                        ui.with_layout(right, |ui| text(ui, index, false));
                    });
                    row.col(|ui| {
                        text(ui, kind, false);
                    });
                    row.col(|ui| {
                        fields(ui, line);
                    });
                    let response = row.response();
                    let response = match full {
                        Some(full) => response.on_hover_text(full),
                        None => response,
                    };
                    if response.clicked() {
                        clicked = Some(entry);
                    }
                });
            });
        if let Some(entry) = clicked {
            self.go_to_entry(entry);
        }
    }

    /// The lines from `from` to `to` cycles the namespace filter keeps, none
    /// when it keeps all; computed again only when the range or the filter
    /// changed.
    fn keep(&mut self, from: i64, to: i64) {
        if !self.filter.active() {
            self.kept = None;
            return;
        }
        let generation = self.filter.generation();
        if self
            .kept
            .as_ref()
            .is_some_and(|kept| kept.range == (from, to) && kept.generation == generation)
        {
            return;
        }
        let entries = self.list.within(from, to);
        let entries = entries.iter().copied().filter(|entry| self.entry_kept(entry)).collect();
        self.kept = Some(Kept {
            range: (from, to),
            generation,
            entries,
        });
    }

    /// Whether the filter keeps the line of `entry`: a loss or a suspect
    /// line always, an event when its type's namespace is ticked or that of
    /// a subject one of its pointers points at.
    fn entry_kept(&self, entry: &Entry) -> bool {
        let Element::Event { event, .. } = &self.trace.elements[entry.element] else {
            return true;
        };
        let row = self
            .ids
            .binary_search(&event.event.id)
            .expect("an event of the catalogue");
        let spaces = &self.spaces;
        let subjects = spaces.pointers[row].iter().filter_map(|&(offset, size)| {
            let bytes = event.payload.get(offset..offset + size)?;
            let address = bytes
                .iter()
                .rev()
                .fold(0u64, |value, byte| value << 8 | u64::from(*byte));
            spaces.at.get(&address).map(String::as_str)
        });
        kept(&self.filter, &spaces.types[row], subjects)
    }

    /// Centres the view on `entry` at the same zoom, marks it on the
    /// timeline and in the list, and scrolls the list to it.
    fn go_to_entry(&mut self, entry: Entry) {
        let at = (entry.cycles - self.origin()) as f64;
        (self.start, self.span) = centred(at, self.span, self.first(), self.last());
        self.picked = Some((entry.cycles, entry.cycles));
        self.chosen = Some(entry.element);
        self.reveal = Some(entry.element);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event stays when its type's namespace is ticked, or that of one
    /// of the subjects it names; else it leaves.
    #[test]
    fn an_event_stays_by_its_type_or_a_subject() {
        let mut filter = Filter::new(["kymo::opsy", "app::sensors", "app::comms"]);
        filter.toggle(filter.node("kymo").unwrap());
        filter.toggle(filter.node("app::comms").unwrap());
        assert!(kept(&filter, "app::sensors", std::iter::empty()));
        assert!(kept(&filter, "kymo::opsy", ["app::comms", "app::sensors"].into_iter()));
        assert!(!kept(&filter, "kymo::opsy", ["app::comms"].into_iter()));
        assert!(!kept(&filter, "kymo::opsy", std::iter::empty()));
    }
}
