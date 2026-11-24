//! The toolbar: the capture, the range shown, and the view's controls.

use super::Timeline;
use super::load::Mode;
use eframe::egui;

impl Timeline {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(&self.capture);
            ui.separator();
            let origin = self.origin();
            let selection = self
                .selection
                .map(|(from, to)| ((from - origin) as f64, (to - origin) as f64));
            let view = (self.start, self.start + self.span);
            ui.label(range(selection, view, |cycles| self.time(cycles)));
            if selection.is_some() && ui.button("×").on_hover_text("Clear the selection (Esc)").clicked() {
                self.selection = None;
                self.picked = None;
            }
            ui.separator();
            if ui
                .button("Whole")
                .on_hover_text("Show the whole capture (double click)")
                .clicked()
            {
                self.whole();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme_menu(ui);
                if self.meaning.is_some() {
                    ui.separator();
                    mode_choice(ui, &mut self.mode);
                }
            });
        });
    }
}

/// The range shown, from and to: the selection when there is one, else the
/// view.
fn range(selection: Option<(f64, f64)>, view: (f64, f64), time: impl Fn(f64) -> String) -> String {
    let (name, (from, to)) = match selection {
        Some(selection) => ("Selection", selection),
        None => ("View", view),
    };
    format!("{name} {} to {}", time(from), time(to))
}

/// The theme's choices, in a menu named after the one chosen.
fn theme_menu(ui: &mut egui::Ui) {
    let choices = [
        (egui::ThemePreference::System, "System"),
        (egui::ThemePreference::Light, "Light"),
        (egui::ThemePreference::Dark, "Dark"),
    ];
    let chosen = ui.ctx().options(|options| options.theme_preference);
    let name = choices
        .iter()
        .find(|(preference, _)| *preference == chosen)
        .map_or("", |(_, name)| name);
    ui.menu_button(format!("Theme: {name}"), |ui| {
        let mut choice = chosen;
        for (preference, name) in choices {
            ui.selectable_value(&mut choice, preference, name);
        }
        if choice != chosen {
            ui.ctx().set_theme(choice);
            ui.close();
        }
    });
}

/// The modes of OpSy's rows, right to left.
fn mode_choice(ui: &mut egui::Ui, mode: &mut Mode) {
    for (choice, name) in [(Mode::Load, "Load"), (Mode::Bars, "Bars"), (Mode::Auto, "Auto")] {
        ui.selectable_value(mode, choice, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(cycles: f64) -> String {
        format!("{cycles}")
    }

    #[test]
    fn the_range_is_the_selection_when_there_is_one() {
        assert_eq!(range(Some((3.0, 4.0)), (1.0, 9.0), time), "Selection 3 to 4");
        assert_eq!(range(None, (1.0, 9.0), time), "View 1 to 9");
    }
}
