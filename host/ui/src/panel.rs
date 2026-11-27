//! The statistics panel: the groups `kymo::stats` makes of the kernel's instances
//! over a range, a table sorted by a column. A group's min, its max and its
//! worst instances each lead to the instance on the timeline.

use crate::meaning::Full;
use crate::palette::Palette;
use crate::timeline::counted;
use eframe::egui;
use egui_extras::{Column as Size, TableBuilder};
use kymo::load::Covered;
use kymo::model::Trace;
use kymo::stats::{self, Group, Instance, Point, Range, Report};
use std::collections::HashMap;

/// What the kernel found besides its instances, as the report counts it.
pub struct Found<'a> {
    pub contradictions: &'a [usize],
    /// The elements that are faults.
    pub faults: &'a [usize],
    pub known: &'a Covered,
}

/// The worst instances kept for each group, as kymo stats keeps by default.
const WORST: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Subject,
    Metric,
    Count,
    Min,
    P50,
    P99,
    Max,
    Excluded,
}

impl Column {
    pub const ALL: [Self; 8] = [
        Self::Subject,
        Self::Metric,
        Self::Count,
        Self::Min,
        Self::P50,
        Self::P99,
        Self::Max,
        Self::Excluded,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::Metric => "metric",
            Self::Count => "count",
            Self::Min => "min",
            Self::P50 => "p50",
            Self::P99 => "p99",
            Self::Max => "max",
            Self::Excluded => "excluded",
        }
    }

    fn numeric(self) -> bool {
        !matches!(self, Self::Subject | Self::Metric)
    }
}

/// Whether a subject is shown, by its kind and its name in full.
pub type Shown<'a> = &'a dyn Fn(&str, &str) -> bool;

/// What tells a group from the others, whatever the range.
type Key = (&'static str, String, &'static str, Option<String>);

fn key(group: &Group) -> Key {
    (
        group.kind,
        group.subject.clone(),
        group.metric.name,
        group.detail.clone(),
    )
}

pub struct Panel {
    instances: Vec<Instance>,
    report: Option<(Range, Report)>,
    pub sort: Column,
    pub descending: bool,
    /// The group whose worst instances are listed.
    chosen: Option<Key>,
    /// The names in full, by kind and name shown; a detail's under the kind
    /// `detail`.
    full: Full,
}

impl Panel {
    pub fn new(instances: Vec<Instance>) -> Self {
        Self {
            instances,
            report: None,
            sort: Column::Subject,
            descending: false,
            chosen: None,
            full: HashMap::new(),
        }
    }

    /// The panel, its instances named short, with their names in full
    /// by kind and name shown.
    pub fn named(instances: Vec<Instance>, full: Full) -> Self {
        Self {
            full,
            ..Self::new(instances)
        }
    }

    /// The report over `range`, computed again only when the range is not
    /// the last one's.
    pub fn report(&mut self, trace: &Trace, found: Found, range: Range) -> &Report {
        if self.report.as_ref().is_none_or(|(last, _)| *last != range) {
            let found = stats::Found {
                instances: &self.instances,
                contradictions: found.contradictions,
                faults: found.faults,
                known: found.known,
            };
            let report = stats::compute(trace, found, range, WORST);
            self.report = Some((range, report));
        }
        &self.report.as_ref().expect("computed above").1
    }

    /// The range of the report last computed.
    pub fn range(&self) -> Option<Range> {
        self.report.as_ref().map(|(range, _)| *range)
    }

    /// Sorts by `column`, the other way round when it already sorts by it;
    /// a number highest first, a name in order.
    pub fn sort_by(&mut self, column: Column) {
        if self.sort == column {
            self.descending = !self.descending;
        } else {
            self.sort = column;
            self.descending = column.numeric();
        }
    }

    /// The groups of `report` in the table's order, those whose subject
    /// `shown` hides left out; equals keep the report's.
    pub fn order(&self, report: &Report, shown: Shown) -> Vec<usize> {
        let mut order: Vec<usize> = (0..report.groups.len())
            .filter(|&n| {
                let group = &report.groups[n];
                let full = self.full.get(&(group.kind, group.subject.clone()));
                shown(group.kind, full.unwrap_or(&group.subject))
            })
            .collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&report.groups[a], &report.groups[b]);
            let ordering = match self.sort {
                Column::Subject => a.subject.cmp(&b.subject),
                Column::Metric => (a.metric.name, &a.detail).cmp(&(b.metric.name, &b.detail)),
                column => sorted(a, column).cmp(&sorted(b, column)),
            };
            if self.descending { ordering.reverse() } else { ordering }
        });
        order
    }

    /// The instance that `point` of `group` starts.
    pub fn instance(&self, group: &Group, point: &Point) -> Option<&Instance> {
        self.instances.iter().find(|instance| {
            instance.from.element == point.element
                && instance.kind == group.kind
                && instance.metric == group.metric
                && instance.detail == group.detail
                && instance.subject == group.subject
        })
    }

    /// The faults of the range, each described, the table of the groups
    /// `shown` keeps, then the chosen group's worst instances. Returns the
    /// instance clicked, if any.
    pub fn show(&mut self, ui: &mut egui::Ui, title: &str, faults: &[String], shown: Shown) -> Option<Instance> {
        let Some((_, report)) = &self.report else {
            return None;
        };
        let frequency = report.frequency;
        let mut clicked: Option<(usize, Point)> = None;
        let mut sort = None;
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.strong(title);
            ui.label(format!(
                "{} known, {}, {} overrun writes, {} against the kernel's order, {}",
                stats::duration(report.known, frequency),
                counted(report.losses as u64, "loss", "losses"),
                report.suspects,
                report.contradictions,
                counted(report.faults as u64, "fault", "faults")
            ));
        });
        for fault in faults {
            ui.horizontal(|ui| {
                let (chip, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(chip, 2.0, Palette::of(ui.visuals()).loss);
                ui.label(egui::RichText::new(fault).monospace());
            });
        }
        ui.separator();
        let order = self.order(report, shown);
        let chosen_group = self
            .chosen
            .as_ref()
            .and_then(|chosen| report.groups.iter().position(|group| key(group) == *chosen))
            .filter(|n| order.contains(n));
        if let Some(n) = chosen_group {
            egui::Panel::right("worst")
                .resizable(true)
                .default_size(340.0)
                .show(ui, |ui| {
                    let group = &report.groups[n];
                    ui.strong(format!("{} {}", group.subject, metric(group)));
                    let Some(distribution) = &group.distribution else {
                        ui.label("no instance in the range");
                        return;
                    };
                    ui.label(format!("the {} worst, then the min:", distribution.worst.len()));
                    egui::ScrollArea::vertical().id_salt("worst").show(ui, |ui| {
                        for point in distribution.worst.iter().chain([&distribution.min]) {
                            let text = format!("{}  at {}", stats::duration(point.value, frequency), at(point));
                            if ui.link(text).clicked() {
                                clicked = Some((n, point.clone()));
                            }
                        }
                    });
                });
        }
        let height = ui.spacing().interact_size.y;
        // A cell's text on one line, centred in its height; a numeric
        // column's cells, header included, at its right.
        let aligned = |ui: &mut egui::Ui, column: Column, add: &mut dyn FnMut(&mut egui::Ui)| {
            let layout = if column.numeric() {
                egui::Layout::right_to_left(egui::Align::Center)
            } else {
                egui::Layout::left_to_right(egui::Align::Center)
            };
            ui.with_layout(layout, add);
        };
        let mut table = TableBuilder::new(ui)
            .id_salt("groups")
            .striped(true)
            .resizable(true)
            .auto_shrink(false)
            .column(Size::auto().at_least(80.0).clip(true))
            .column(Size::auto().at_least(80.0).clip(true));
        for _ in &Column::ALL[2..] {
            table = table.column(Size::auto().at_least(60.0));
        }
        table
            .header(height, |mut header| {
                for column in Column::ALL {
                    header.col(|ui| {
                        aligned(ui, column, &mut |ui| {
                            let sorting = self.sort == column;
                            // Room for the triangle, drawn: the fonts have no arrow.
                            let name = if sorting {
                                format!("{}     ", column.name())
                            } else {
                                column.name().to_owned()
                            };
                            let button = ui.add(egui::Button::new(name).selected(sorting));
                            if sorting {
                                triangle(ui, button.rect, self.descending);
                            }
                            if button.clicked() {
                                sort = Some(column);
                            }
                        });
                    });
                }
            })
            .body(|body| {
                body.rows(height, order.len(), |mut row| {
                    let n = order[row.index()];
                    let group = &report.groups[n];
                    let picked = chosen_group == Some(n);
                    row.set_selected(picked);
                    row.col(|ui| {
                        aligned(ui, Column::Subject, &mut |ui| {
                            let subject = ui.selectable_label(picked, &group.subject);
                            let subject = match self.full.get(&(group.kind, group.subject.clone())) {
                                Some(full) => subject.on_hover_text(full),
                                None => subject,
                            };
                            if subject.clicked() {
                                chosen = Some(key(group));
                            }
                        });
                    });
                    row.col(|ui| {
                        aligned(ui, Column::Metric, &mut |ui| {
                            let detail = group
                                .detail
                                .as_ref()
                                .and_then(|detail| self.full.get(&("detail", detail.clone())));
                            let label = ui.label(metric(group));
                            if let Some(full) = detail {
                                label.on_hover_text(full);
                            }
                        });
                    });
                    for &column in &Column::ALL[2..] {
                        row.col(|ui| {
                            aligned(ui, column, &mut |ui| {
                                let text = cell(group, column, frequency);
                                match point(group, column) {
                                    Some(point) => {
                                        if ui.link(text).on_hover_text(at(point)).clicked() {
                                            clicked = Some((n, point.clone()));
                                        }
                                    }
                                    None if column == Column::Excluded => {
                                        ui.label(text).on_hover_text(format!(
                                            "{} at a loss, {} on an overrun write, {} cut by the range",
                                            group.excluded_loss, group.excluded_suspect, group.cut
                                        ));
                                    }
                                    None => {
                                        ui.label(text);
                                    }
                                }
                            });
                        });
                    }
                });
            });
        if let Some(column) = sort {
            self.sort_by(column);
        }
        if chosen.is_some() {
            self.chosen = chosen;
        }
        let (n, point) = clicked?;
        let group = &self.report.as_ref()?.1.groups[n];
        self.instance(group, &point).cloned()
    }
}

/// The value a numeric column sorts by: none, lowest, for a group without
/// an instance counted.
fn sorted(group: &Group, column: Column) -> Option<i64> {
    let distribution = group.distribution.as_ref();
    match column {
        Column::Count => Some(group.count as i64),
        Column::Excluded => Some((group.excluded_loss + group.excluded_suspect) as i64),
        Column::Min => distribution.map(|distribution| distribution.min.value),
        Column::P50 => distribution.map(|distribution| distribution.p50),
        Column::P99 => distribution.map(|distribution| distribution.p99),
        Column::Max => distribution.map(|distribution| distribution.max.value),
        Column::Subject | Column::Metric => None,
    }
}

/// The instance a cell leads to: the min's or the max's.
pub fn point(group: &Group, column: Column) -> Option<&Point> {
    let distribution = group.distribution.as_ref()?;
    match column {
        Column::Min => Some(&distribution.min),
        Column::Max => Some(&distribution.max),
        _ => None,
    }
}

/// A cell of a numeric column: the p99 and the max marked "≥" when
/// instances were excluded, as they may have been higher.
pub fn cell(group: &Group, column: Column, frequency: Option<f64>) -> String {
    let bound = if group.lower_bound() { "≥ " } else { "" };
    match column {
        Column::Count => group.count.to_string(),
        Column::Excluded => (group.excluded_loss + group.excluded_suspect).to_string(),
        Column::Subject | Column::Metric => String::new(),
        column => match sorted(group, column) {
            None => "-".to_owned(),
            Some(value) => {
                let high = matches!(column, Column::P99 | Column::Max);
                format!("{}{}", if high { bound } else { "" }, stats::duration(value, frequency))
            }
        },
    }
}

/// The sort's triangle at the right of a column's button: pointing down
/// when the highest comes first.
fn triangle(ui: &egui::Ui, rect: egui::Rect, descending: bool) {
    let centre = egui::pos2(rect.right() - 12.0, rect.center().y);
    let (tip, base) = if descending { (4.0, -3.0) } else { (-4.0, 3.0) };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            centre + egui::vec2(0.0, tip),
            centre + egui::vec2(-4.0, base),
            centre + egui::vec2(4.0, base),
        ],
        ui.visuals().widgets.active.fg_stroke.color,
        egui::Stroke::NONE,
    ));
}

fn metric(group: &Group) -> String {
    match &group.detail {
        Some(detail) => format!("{} ({detail})", group.metric.name),
        None => group.metric.name.to_owned(),
    }
}

/// Where an instance starts: its event's index, and its instant; in a trace
/// of SWO alone, its instant, or its cycles without a frequency.
fn at(point: &Point) -> String {
    match (point.index, point.seconds) {
        (Some(index), Some(seconds)) => format!("#{index} at {seconds:.6} s"),
        (Some(index), None) => format!("#{index}"),
        (None, Some(seconds)) => format!("at {seconds:.6} s"),
        (None, None) => format!("at {} cycles", point.cycles),
    }
}

#[cfg(test)]
mod tests;
