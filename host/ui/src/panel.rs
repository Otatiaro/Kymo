//! The statistics panel: the groups `kymo::stats` makes of OpSy's instances
//! over a range, a table sorted by a column. A group's min, its max and its
//! worst instances each lead to the instance on the timeline.

use crate::timeline::counted;
use eframe::egui;
use kymo::load::Covered;
use kymo::model::Trace;
use kymo::stats::{self, Group, Instance, Point, Range, Report};

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
}

impl Panel {
    pub fn new(instances: Vec<Instance>) -> Self {
        Self {
            instances,
            report: None,
            sort: Column::Subject,
            descending: false,
            chosen: None,
        }
    }

    /// The report over `range`, computed again only when the range is not
    /// the last one's.
    pub fn report(&mut self, trace: &Trace, contradictions: &[usize], known: &Covered, range: Range) -> &Report {
        if self.report.as_ref().is_none_or(|(last, _)| *last != range) {
            let found = stats::Found {
                instances: &self.instances,
                contradictions,
                known,
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

    /// The groups of `report` in the table's order; equals keep the
    /// report's.
    pub fn order(&self, report: &Report) -> Vec<usize> {
        let mut order: Vec<usize> = (0..report.groups.len()).collect();
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

    /// The table, then the chosen group's worst instances. Returns the
    /// instance clicked, if any.
    pub fn show(&mut self, ui: &mut egui::Ui, title: &str) -> Option<Instance> {
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
                "{} known, {}, {} overrun writes, {} against OpSy's order",
                stats::duration(report.known, frequency),
                counted(report.losses as u64, "loss", "losses"),
                report.suspects,
                report.contradictions
            ));
        });
        ui.separator();
        let order = self.order(report);
        let chosen_group = self
            .chosen
            .as_ref()
            .and_then(|chosen| report.groups.iter().position(|group| key(group) == *chosen));
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
        egui::ScrollArea::both()
            .id_salt("table")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("groups")
                    .striped(true)
                    .num_columns(Column::ALL.len())
                    .spacing([16.0, 2.0])
                    .show(ui, |ui| {
                        for column in Column::ALL {
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
                        }
                        ui.end_row();
                        for &n in &order {
                            let group = &report.groups[n];
                            let picked = chosen_group == Some(n);
                            if ui.selectable_label(picked, &group.subject).clicked() {
                                chosen = Some(key(group));
                            }
                            ui.label(metric(group));
                            for column in &Column::ALL[2..] {
                                let text = cell(group, *column, frequency);
                                match point(group, *column) {
                                    Some(point) => {
                                        if ui.link(text).on_hover_text(at(point)).clicked() {
                                            clicked = Some((n, point.clone()));
                                        }
                                    }
                                    None if *column == Column::Excluded => {
                                        ui.label(text).on_hover_text(format!(
                                            "{} at a loss, {} on an overrun write, {} cut by the range",
                                            group.excluded_loss, group.excluded_suspect, group.cut
                                        ));
                                    }
                                    None => {
                                        ui.label(text);
                                    }
                                }
                            }
                            ui.end_row();
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

/// Where an instance starts: its event's index, and its instant.
fn at(point: &Point) -> String {
    match point.seconds {
        Some(seconds) => format!("#{} at {seconds:.6} s", point.index),
        None => format!("#{}", point.index),
    }
}

#[cfg(test)]
mod tests;
