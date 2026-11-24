//! The colours of the data and egui's visuals, per theme: the rows'
//! backgrounds are the panel's, every other row striped by the faint
//! background; the column of names and the sections sit on the surface.

use eframe::egui::{Color32, CornerRadius, Stroke, Theme, Visuals};

/// The colours the timeline draws its data with.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub event: Color32,
    pub run: Color32,
    pub idle: Color32,
    pub interrupt: Color32,
    pub hold: Color32,
    pub wait: Color32,
    /// The thin bar under a task's runs: its time ready.
    pub ready: Color32,
    /// The arrow from what woke a task to the task.
    pub wake: Color32,
    /// The hatching of the time unknown, in the load view.
    pub unknown: Color32,
    /// The bands of a loss and of a suspect line, over the rows.
    pub loss: Color32,
    pub suspect: Color32,
    /// The owner's name, written on a hold.
    pub owner: Color32,
    /// The column of names and the sections' rows.
    pub surface: Color32,
}

const LIGHT: Palette = Palette {
    event: Color32::from_rgb(0x2f, 0x6f, 0xdb),
    run: Color32::from_rgb(0x2f, 0x6f, 0xdb),
    idle: Color32::from_rgb(0x84, 0x8d, 0x98),
    interrupt: Color32::from_rgb(0xc4, 0x61, 0x0a),
    hold: Color32::from_rgb(0x1a, 0x7f, 0x4b),
    wait: Color32::from_rgb(0x4e, 0x94, 0x70),
    ready: Color32::from_rgb(0x6e, 0x77, 0x81),
    wake: Color32::from_rgb(0xa2, 0x1c, 0xaf),
    unknown: Color32::from_rgb(0x6e, 0x77, 0x81),
    loss: Color32::from_rgba_premultiplied(150, 0, 0, 200),
    suspect: Color32::from_rgba_premultiplied(120, 70, 0, 200),
    owner: Color32::WHITE,
    surface: Color32::from_rgb(0xf0, 0xf2, 0xf5),
};

const DARK: Palette = Palette {
    event: Color32::from_rgb(0x5b, 0x9b, 0xf0),
    run: Color32::from_rgb(0x5b, 0x9b, 0xf0),
    idle: Color32::from_rgb(0x66, 0x6e, 0x7a),
    interrupt: Color32::from_rgb(0xf0, 0xa2, 0x4a),
    hold: Color32::from_rgb(0x3f, 0xb9, 0x7f),
    wait: Color32::from_rgb(0x35, 0x8a, 0x62),
    ready: Color32::from_rgb(0x8b, 0x95, 0xa1),
    wake: Color32::from_rgb(0xe8, 0x79, 0xf9),
    unknown: Color32::from_rgb(0x9a, 0xa3, 0xae),
    loss: Color32::from_rgba_premultiplied(210, 60, 60, 210),
    suspect: Color32::from_rgba_premultiplied(200, 140, 30, 200),
    owner: Color32::BLACK,
    surface: Color32::from_rgb(0x1e, 0x21, 0x27),
};

impl Palette {
    /// The palette of the theme `visuals` are of.
    pub fn of(visuals: &Visuals) -> Self {
        if visuals.dark_mode { DARK } else { LIGHT }
    }
}

/// egui's visuals for `theme`: the backgrounds, text, grid and widgets the
/// palette is chosen against.
pub fn visuals(theme: Theme) -> Visuals {
    let (mut visuals, background, stripe, extreme, grid, text, weak) = match theme {
        Theme::Dark => (
            Visuals::dark(),
            Color32::from_rgb(0x16, 0x18, 0x1c),
            Color32::from_rgb(0x1b, 0x1e, 0x23),
            Color32::from_rgb(0x10, 0x12, 0x15),
            Color32::from_rgb(0x2e, 0x33, 0x3a),
            Color32::from_rgb(0xe6, 0xe8, 0xeb),
            Color32::from_rgb(0x9a, 0xa3, 0xae),
        ),
        Theme::Light => (
            Visuals::light(),
            Color32::WHITE,
            Color32::from_rgb(0xf6, 0xf8, 0xfa),
            Color32::WHITE,
            Color32::from_rgb(0xd8, 0xde, 0xe4),
            Color32::from_rgb(0x1f, 0x23, 0x28),
            Color32::from_rgb(0x59, 0x63, 0x6e),
        ),
    };
    visuals.panel_fill = background;
    visuals.window_fill = background;
    visuals.faint_bg_color = stripe;
    visuals.extreme_bg_color = extreme;
    visuals.weak_text_color = Some(weak);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, grid);
    visuals.widgets.noninteractive.fg_stroke.color = text;
    let radius = CornerRadius::same(4);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
    }
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = CornerRadius::same(6);
    visuals
}

#[cfg(test)]
mod tests;
