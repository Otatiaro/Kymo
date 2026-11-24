//! The colours of the data, per theme, apart from egui's own: the rows'
//! backgrounds are the panel's, every other line darkened or lightened by
//! egui's faint background.

use eframe::egui::{Color32, Visuals};

/// The colours the timeline draws its data with.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub event: Color32,
    pub run: Color32,
    pub idle: Color32,
    pub interrupt: Color32,
    pub hold: Color32,
    pub wait: Color32,
    /// The bands of a loss and of a suspect line, over the rows.
    pub loss: Color32,
    pub suspect: Color32,
    /// The owner's name, written on a hold.
    pub owner: Color32,
}

const LIGHT: Palette = Palette {
    event: Color32::from_rgb(70, 130, 220),
    run: Color32::from_rgb(70, 130, 220),
    idle: Color32::from_rgb(125, 125, 125),
    interrupt: Color32::from_rgb(190, 100, 10),
    hold: Color32::from_rgb(30, 120, 60),
    wait: Color32::from_rgb(80, 150, 100),
    loss: Color32::from_rgba_premultiplied(150, 0, 0, 200),
    suspect: Color32::from_rgba_premultiplied(120, 70, 0, 200),
    owner: Color32::WHITE,
};

const DARK: Palette = Palette {
    event: Color32::from_rgb(95, 155, 240),
    run: Color32::from_rgb(95, 155, 240),
    idle: Color32::from_rgb(120, 120, 125),
    interrupt: Color32::from_rgb(240, 160, 60),
    hold: Color32::from_rgb(80, 185, 110),
    wait: Color32::from_rgb(55, 125, 80),
    loss: Color32::from_rgba_premultiplied(210, 60, 60, 210),
    suspect: Color32::from_rgba_premultiplied(200, 140, 30, 200),
    owner: Color32::BLACK,
};

impl Palette {
    /// The palette of the theme `visuals` are of.
    pub fn of(visuals: &Visuals) -> Self {
        if visuals.dark_mode { DARK } else { LIGHT }
    }
}

#[cfg(test)]
mod tests;
