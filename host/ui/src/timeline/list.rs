//! The bottom panel: the event list of the range (ADR-072) and, with OpSy's
//! meaning, the statistics.

use super::Timeline;
use super::hover::marked;
use super::status::counted;
use super::view::centred;
use crate::listing::Entry;
use crate::palette::Palette;
use eframe::egui;
use kymo::decode;
use kymo::model::Element;

/// The width of the bar that marks a loss or a suspect line, in pixels.
const MARK: f32 = 4.0;

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
    fn list(&mut self, ui: &mut egui::Ui) {
        let reveal = self.reveal.take();
        let range = self.range();
        if !self.moving || self.listed.is_none() {
            self.listed = Some((range.from.unwrap_or_default(), range.to.unwrap_or_default()));
        }
        let (from, to) = self.listed.expect("set above");
        let entries = self.list.within(from, to);
        ui.horizontal(|ui| {
            ui.strong(self.range_title(from, to));
            ui.label(counted(entries.len() as u64, "line", "lines"));
        });
        ui.separator();
        let names = self.rows.iter().map(|row| row.name.len()).max().unwrap_or_default();
        // The widest time, the last event's.
        let time = self.time(self.last()).len();
        let line = |entry: &Entry| -> String {
            let at = self.time((entry.cycles - self.origin()) as f64);
            match &self.trace.elements[entry.element] {
                Element::Event { event, .. } => {
                    let mut fields = String::new();
                    decode::fields(&mut fields, &self.catalogue.symbols, event);
                    format!(
                        "  {at:>time$}  {:>10}  {:<names$}  {fields}",
                        event.index, event.event.name
                    )
                }
                other => format!("  {at:>time$}  {:>10}  {}", "", marked(other).unwrap_or_default()),
            }
        };
        let font = egui::TextStyle::Monospace;
        ui.label(
            egui::RichText::new(format!(
                "  {:>time$}  {:>10}  {:<names$}  fields",
                "time", "index", "type"
            ))
            .text_style(font.clone())
            .strong(),
        );
        let height = ui.spacing().interact_size.y;
        let palette = Palette::of(ui.visuals());
        let mut clicked = None;
        let mut area = egui::ScrollArea::both().id_salt("events").auto_shrink(false);
        if let Some(element) = reveal
            && let Some(n) = entries.iter().position(|entry| entry.element == element)
        {
            let spacing = ui.spacing().item_spacing.y;
            let shown = ui.available_height();
            area = area.vertical_scroll_offset(((height + spacing) * n as f32 - shown / 2.0).max(0.0));
        }
        area.show_rows(ui, height, entries.len(), |ui, rows| {
            for entry in &entries[rows] {
                let text = egui::RichText::new(line(entry)).text_style(font.clone());
                let chosen = self.chosen == Some(entry.element);
                let button = ui.add(egui::Button::selectable(chosen, text).min_size(egui::vec2(0.0, height)));
                let color = match self.trace.elements[entry.element] {
                    Element::Lost { .. } => Some(palette.loss),
                    Element::Suspect { .. } => Some(palette.suspect),
                    Element::Event { .. } => None,
                };
                if let Some(color) = color {
                    let rect = button.rect;
                    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(MARK, rect.height()));
                    ui.painter().rect_filled(bar, 0.0, color);
                }
                if button.clicked() {
                    clicked = Some(*entry);
                }
            }
        });
        if let Some(entry) = clicked {
            self.go_to_entry(entry);
        }
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
