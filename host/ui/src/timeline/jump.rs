//! Jumps from the keyboard (ADR-100): the arrows move the view to the next or
//! previous activity of the line under the pointer, with shift to the next or
//! previous loss; `F` frames the range selected; `?` lists the shortcuts.
//!
//! A jump centres the view on the start of its target, at the same zoom. The
//! target is the first start strictly after the view's centre, or strictly
//! before it: repeated jumps always move on. Where the view could not be
//! centred on the last target, at an end of the capture, the jumps count from
//! that target while the view stays where it put it.

use super::Timeline;
use super::rows::Line;
use super::view::centred;
use crate::spans::Spans;
use eframe::egui;
use kymo::meaning::Spans as Open;
use kymo::model::Element;

/// The shortcuts, as `?` lists them: keys and what they do.
pub(super) const SHORTCUTS: [(&str, &str); 8] = [
    ("→ / ←", "next / previous activity of the line under the pointer"),
    ("Shift + → / ←", "next / previous loss"),
    ("F", "frame the range selected"),
    ("Shift + drag", "select a range"),
    ("Esc", "clear the selection"),
    ("Double click", "show the whole capture"),
    ("Wheel / Ctrl + wheel", "scroll / zoom"),
    ("?", "show or hide this list"),
];

/// The first of `starts`, sorted, strictly after `from`, or strictly before
/// it backward.
pub(super) fn target(starts: &[i64], from: f64, forward: bool) -> Option<i64> {
    if forward {
        let n = starts.partition_point(|&start| start as f64 <= from);
        starts.get(n).copied()
    } else {
        let n = starts.partition_point(|&start| (start as f64) < from);
        n.checked_sub(1).map(|n| starts[n])
    }
}

/// The starts of the rows `closed`, and those `open`, sorted together.
pub(super) fn sorted<'a>(closed: impl IntoIterator<Item = &'a Spans>, open: impl Iterator<Item = i64>) -> Vec<i64> {
    let mut starts: Vec<i64> = closed
        .into_iter()
        .flat_map(|spans| spans.starts().iter().copied())
        .chain(open)
        .collect();
    starts.sort_unstable();
    starts
}

impl Timeline {
    /// Reads the shortcuts of this frame: `line` is the line under the
    /// pointer, `open` the kernel's spans still open.
    pub(super) fn keys(&mut self, ui: &egui::Ui, line: Option<Line>, open: &Open) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (right, left, shift, frame, help) = ui.input(|input| {
            let plain = !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.command;
            (
                plain && input.key_pressed(egui::Key::ArrowRight),
                plain && input.key_pressed(egui::Key::ArrowLeft),
                input.modifiers.shift,
                plain && !input.modifiers.shift && input.key_pressed(egui::Key::F),
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Text(text) if text == "?")),
            )
        });
        if help {
            self.shortcuts = !self.shortcuts;
        }
        if frame && let Some((from, to)) = self.selection {
            self.frame(from, to);
        }
        if right == left {
            return;
        }
        let starts = if shift {
            self.losses()
        } else {
            match line {
                Some(line) => self.starts(line, open),
                None => return,
            }
        };
        self.jump(&starts, right);
    }

    /// Centres the view on the target of `starts` after or before it.
    pub(super) fn jump(&mut self, starts: &[i64], forward: bool) {
        let from = match self.jumped {
            Some((target, start, span)) if start == self.start && span == self.span => target as f64,
            _ => self.origin() as f64 + self.start + self.span / 2.0,
        };
        let Some(target) = target(starts, from, forward) else {
            return;
        };
        (self.start, self.span) = centred((target - self.origin()) as f64, self.span, self.first(), self.last());
        self.jumped = Some((target, self.start, self.span));
    }

    /// Shows `from` to `to` cycles across the view.
    fn frame(&mut self, from: i64, to: i64) {
        self.span = ((to - from) as f64).clamp(super::NARROWEST, super::view::widest(self.first(), self.last()));
        self.start = (from - self.origin()) as f64 + (to - from) as f64 / 2.0 - self.span / 2.0;
        self.clamp();
    }

    /// The starts of the losses, sorted: a loss starts at the event before it.
    fn losses(&self) -> Vec<i64> {
        let mut starts: Vec<i64> = self
            .marks()
            .filter(|mark| matches!(self.trace.elements[mark.element], Element::Lost { .. }))
            .map(|mark| mark.from)
            .collect();
        starts.sort_unstable();
        starts
    }

    /// The starts of the activities of `line`, sorted: a task's runs, a
    /// handler's interrupts, a mutex's holds, the pieces of the CPU's band,
    /// the events of an event type. Other lines have none.
    pub(super) fn starts(&self, line: Line, open: &Open) -> Vec<i64> {
        match (line, self.meaning.as_ref()) {
            (Line::Event(row), _) => self.rows[row].levels.cycles().to_vec(),
            (Line::Cpu, Some(meaning)) => meaning.band.pieces.iter().map(|piece| piece.from).collect(),
            (Line::Task(task), Some(meaning)) => {
                let closed = match task {
                    Some(task) => meaning.runs.get(task),
                    None => Some(&meaning.idle),
                };
                let open = open.runs.iter().filter(|run| run.task == task);
                sorted(closed, open.map(|run| run.span.from.cycles))
            }
            (Line::Handler(handler), Some(meaning)) => {
                let open = open.interrupts.iter().filter(|interrupt| interrupt.handler == handler);
                let depths = meaning.interrupts.get(handler).into_iter().flatten();
                sorted(depths, open.map(|interrupt| interrupt.span.from.cycles))
            }
            (Line::Mutex(mutex), Some(meaning)) => {
                let open = open.holds.iter().filter(|hold| hold.mutex == mutex);
                sorted(meaning.holds.get(mutex), open.map(|hold| hold.span.from.cycles))
            }
            _ => Vec::new(),
        }
    }

    /// The list of shortcuts, while `?` shows it.
    pub(super) fn show_shortcuts(&mut self, ctx: &egui::Context) {
        let mut open = self.shortcuts;
        egui::Window::new("Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Grid::new("shortcuts")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for (keys, does) in SHORTCUTS {
                            ui.strong(keys);
                            ui.label(does);
                            ui.end_row();
                        }
                    });
            });
        self.shortcuts = open;
    }
}

#[cfg(test)]
pub(super) mod tests;
