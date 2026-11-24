//! The status bar, and the times and durations it and the hovers print.

use super::Timeline;
use eframe::egui;
use std::fmt::Write;

impl Timeline {
    /// `cycles` as a duration, in seconds when the frequency is known.
    pub(super) fn duration(&self, cycles: i64) -> String {
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

    pub(super) fn status(&self) -> String {
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
    pub(super) fn time(&self, cycles: f64) -> String {
        match self.frequency() {
            Some(hz) => format!("{:.9} s", cycles / hz),
            None => format!("{cycles:.0} cycles"),
        }
    }
}

/// The theme's buttons, right to left, in words: egui's own draw icons the
/// default fonts have no glyph for.
pub(super) fn theme_choice(ui: &mut egui::Ui) {
    let chosen = ui.ctx().options(|options| options.theme_preference);
    let mut choice = chosen;
    for (preference, name) in [
        (egui::ThemePreference::Dark, "Dark"),
        (egui::ThemePreference::Light, "Light"),
        (egui::ThemePreference::System, "System"),
    ] {
        ui.selectable_value(&mut choice, preference, name);
    }
    if choice != chosen {
        ui.ctx().set_theme(choice);
    }
}

/// `n` and the noun that counts it: `one` for 1, `many` otherwise.
pub fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
