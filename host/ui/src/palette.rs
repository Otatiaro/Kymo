//! The colours of the data and egui's visuals, per theme: the rows'
//! backgrounds are the panel's, every other row striped by the faint
//! background; the column of names and the sections sit on the surface.

use eframe::egui::{Color32, CornerRadius, Stroke, Theme, Visuals};

/// The colours the timeline draws its data with.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub event: Color32,
    pub idle: Color32,
    pub interrupt: Color32,
    pub hold: Color32,
    pub wait: Color32,
    /// The thin bar under a task's runs: its time ready.
    pub ready: Color32,
    /// The arrow from what woke a task to the task.
    pub wake: Color32,
    /// A loss and a suspect line: their marks in the ruler, and, at `BAND`'s
    /// opacity, their bands over the rows.
    pub loss: Color32,
    pub suspect: Color32,
    /// The owner's name, written on a hold.
    pub owner: Color32,
    /// The column of names and the sections' rows.
    pub surface: Color32,
    /// The tasks' colours, a task taking the one its `Shade` picks: eight
    /// hues from olive to violet, clear of the handlers' orange, the losses'
    /// red and the wake's magenta, at two lightnesses per theme.
    pub tasks: [Color32; Shade::COUNT],
}

/// A task's place in the palette's `tasks`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shade(usize);

/// The tries of a hash chain before a task takes the first shade free.
const TRIES: usize = 64;

impl Shade {
    pub const COUNT: usize = 16;

    /// The shade of the task named `name` in full, the shades `taken` by
    /// the tasks before it: the one FNV-1a of the name picks, else the hash
    /// hashed again until a free one, else the first free one; with none
    /// free, the name's.
    pub fn of(name: &str, taken: &[Self]) -> Self {
        let mut hash = fnv1a(name.as_bytes());
        let own = Self::from(hash);
        for _ in 0..TRIES {
            if !taken.contains(&Self::from(hash)) {
                return Self::from(hash);
            }
            hash = fnv1a(&hash.to_le_bytes());
        }
        (0..Self::COUNT)
            .map(Self)
            .find(|shade| !taken.contains(shade))
            .unwrap_or(own)
    }

    fn from(hash: u64) -> Self {
        Self((hash % Self::COUNT as u64) as usize)
    }
}

/// FNV-1a, 64 bits: the same across runs and platforms.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

const LIGHT: Palette = Palette {
    event: Color32::from_rgb(0x2f, 0x6f, 0xdb),
    idle: Color32::from_rgb(0x76, 0x7f, 0x8a),
    interrupt: Color32::from_rgb(0xc4, 0x61, 0x0a),
    hold: Color32::from_rgb(0x1a, 0x7f, 0x4b),
    wait: Color32::from_rgb(0x4e, 0x94, 0x70),
    ready: Color32::from_rgb(0x6e, 0x77, 0x81),
    wake: Color32::from_rgb(0xa2, 0x1c, 0xaf),
    loss: Color32::from_rgb(0xc6, 0x28, 0x28),
    suspect: Color32::from_rgb(0xb4, 0x5f, 0x06),
    owner: Color32::WHITE,
    surface: Color32::from_rgb(0xf0, 0xf2, 0xf5),
    // OKLCH hues 110 to 300 at lightness 0.40 then 0.52, chroma up to 0.13
    // within sRGB.
    tasks: [
        Color32::from_rgb(0x4b, 0x4b, 0x00),
        Color32::from_rgb(0x22, 0x56, 0x00),
        Color32::from_rgb(0x02, 0x55, 0x3b),
        Color32::from_rgb(0x02, 0x53, 0x51),
        Color32::from_rgb(0x00, 0x50, 0x62),
        Color32::from_rgb(0x00, 0x4b, 0x7b),
        Color32::from_rgb(0x32, 0x3e, 0x8c),
        Color32::from_rgb(0x54, 0x32, 0x82),
        Color32::from_rgb(0x6d, 0x6e, 0x00),
        Color32::from_rgb(0x41, 0x79, 0x26),
        Color32::from_rgb(0x02, 0x7c, 0x58),
        Color32::from_rgb(0x07, 0x79, 0x75),
        Color32::from_rgb(0x00, 0x75, 0x8d),
        Color32::from_rgb(0x08, 0x6e, 0xae),
        Color32::from_rgb(0x51, 0x61, 0xb3),
        Color32::from_rgb(0x75, 0x55, 0xa8),
    ],
};

const DARK: Palette = Palette {
    event: Color32::from_rgb(0x5b, 0x9b, 0xf0),
    idle: Color32::from_rgb(0x74, 0x7c, 0x88),
    interrupt: Color32::from_rgb(0xf0, 0xa2, 0x4a),
    hold: Color32::from_rgb(0x3f, 0xb9, 0x7f),
    wait: Color32::from_rgb(0x35, 0x8a, 0x62),
    ready: Color32::from_rgb(0x8b, 0x95, 0xa1),
    wake: Color32::from_rgb(0xe8, 0x79, 0xf9),
    loss: Color32::from_rgb(0xf0, 0x64, 0x64),
    suspect: Color32::from_rgb(0xe0, 0xa0, 0x30),
    owner: Color32::BLACK,
    surface: Color32::from_rgb(0x1e, 0x21, 0x27),
    // OKLCH hues 110 to 300 at lightness 0.66 then 0.80, chroma up to 0.12
    // within sRGB.
    tasks: [
        Color32::from_rgb(0x97, 0x98, 0x36),
        Color32::from_rgb(0x6e, 0xa3, 0x58),
        Color32::from_rgb(0x38, 0xa9, 0x7f),
        Color32::from_rgb(0x0c, 0xa7, 0xa3),
        Color32::from_rgb(0x07, 0xa3, 0xc2),
        Color32::from_rgb(0x4c, 0x99, 0xd7),
        Color32::from_rgb(0x7a, 0x8c, 0xdb),
        Color32::from_rgb(0x9e, 0x80, 0xd1),
        Color32::from_rgb(0xc2, 0xc5, 0x64),
        Color32::from_rgb(0x98, 0xd0, 0x82),
        Color32::from_rgb(0x6a, 0xd6, 0xa9),
        Color32::from_rgb(0x46, 0xd6, 0xd1),
        Color32::from_rgb(0x50, 0xd0, 0xf1),
        Color32::from_rgb(0x82, 0xc5, 0xfe),
        Color32::from_rgb(0xa8, 0xba, 0xff),
        Color32::from_rgb(0xc9, 0xac, 0xff),
    ],
};

/// The opacity of a loss's or a suspect line's band over the rows: the data
/// stays readable through it.
pub const BAND: f32 = 0.15;

impl Palette {
    /// The palette of the theme `visuals` are of.
    pub fn of(visuals: &Visuals) -> Self {
        if visuals.dark_mode { DARK } else { LIGHT }
    }

    /// The colour of a task of shade `shade`.
    pub fn task(&self, shade: Shade) -> Color32 {
        self.tasks[shade.0]
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
