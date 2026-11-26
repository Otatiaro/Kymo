//! The status bar, and the times and durations it and the hovers print.

use super::Timeline;
use crate::palette::Palette;
use eframe::egui;

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

    /// The status bar's segments: the counts in weak text, what may be wrong
    /// in the colour of its mark, then the frame's cost.
    pub(super) fn status(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui.visuals());
        // `text` in `color` when `warn`, weak otherwise.
        let segment = |ui: &mut egui::Ui, text: String, warn: bool, color| {
            let text = egui::RichText::new(text);
            ui.label(if warn { text.color(color) } else { text.weak() });
        };
        ui.label(counted(self.events as u64, "event", "events"));
        ui.separator();
        let gaps = format!(
            "{} ({} lost)",
            counted(self.losses as u64, "gap", "gaps"),
            counted(self.lost, "slot", "slots")
        );
        segment(ui, gaps, self.losses > 0, palette.loss);
        if self.suspects > 0 {
            ui.separator();
            let writes = counted(self.suspects, "write", "writes");
            segment(
                ui,
                format!("{writes} overrun: nearby events may be wrong"),
                true,
                palette.suspect,
            );
        }
        if let Some(meaning) = &self.meaning {
            ui.separator();
            let against = meaning.kernel.contradictions.len();
            segment(
                ui,
                format!("{against} against the kernel's order"),
                against > 0,
                palette.suspect,
            );
        }
        ui.separator();
        let frequency = match (self.frequency(), self.trace.measured) {
            (Some(hz), Some((measured, _))) if hz == measured => format!("{} Hz measured", hz.round()),
            (Some(hz), _) => format!("{} Hz", hz.round()),
            (None, _) => "no frequency: in cycles".to_owned(),
        };
        segment(ui, frequency, false, palette.loss);
        ui.separator();
        segment(ui, format!("laid out in {:.2} ms", self.painted), false, palette.loss);
        ui.separator();
        segment(ui, "? shortcuts".to_owned(), false, palette.loss);
    }

    /// `cycles` from the origin, as seconds when the frequency is known.
    pub(super) fn time(&self, cycles: f64) -> String {
        match self.frequency() {
            Some(hz) => format!("{:.9} s", cycles / hz),
            None => format!("{cycles:.0} cycles"),
        }
    }
}

/// `n` and the noun that counts it: `one` for 1, `many` otherwise.
pub fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
