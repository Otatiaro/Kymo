use super::*;

/// `color`, premultiplied, drawn over the opaque `background`.
fn over(color: Color32, background: [f64; 3]) -> [f64; 3] {
    let [r, g, b, a] = color.to_array().map(f64::from);
    let rest = 1.0 - a / 255.0;
    [
        r + background[0] * rest,
        g + background[1] * rest,
        b + background[2] * rest,
    ]
}

/// The relative luminance of an sRGB colour, as WCAG 2 defines it.
fn luminance(color: [f64; 3]) -> f64 {
    let linear = |channel: f64| {
        let c = channel.min(255.0) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = color.map(linear);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The WCAG 2 contrast ratio between two opaque colours.
fn contrast(one: [f64; 3], other: [f64; 3]) -> f64 {
    let (one, other) = (luminance(one), luminance(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// The rows' two backgrounds in a theme: the panel, and the panel under
/// egui's faint background, every other row.
fn backgrounds(visuals: &Visuals) -> [[f64; 3]; 2] {
    let panel = over(visuals.panel_fill, [0.0; 3]);
    [panel, over(visuals.faint_bg_color, panel)]
}

fn themes() -> [(&'static str, Visuals); 2] {
    [("light", Visuals::light()), ("dark", Visuals::dark())]
}

/// The colours the data is drawn with, by name.
fn data(palette: &Palette) -> [(&'static str, Color32); 9] {
    [
        ("event", palette.event),
        ("run", palette.run),
        ("idle", palette.idle),
        ("interrupt", palette.interrupt),
        ("hold", palette.hold),
        ("wait", palette.wait),
        ("unknown", palette.unknown),
        ("loss", palette.loss),
        ("suspect", palette.suspect),
    ]
}

/// The pairs below `minimum`, each with its ratio.
fn below(minimum: f64, pairs: impl Iterator<Item = (String, f64)>) -> Vec<String> {
    pairs
        .filter(|(_, ratio)| *ratio < minimum)
        .map(|(name, ratio)| format!("{name} {ratio:.2}:1"))
        .collect()
}

/// Every colour of the data, over either background of its rows, stands out
/// of it by 3:1 at least, the WCAG minimum for a graphical object.
#[test]
fn the_data_stands_out_of_the_rows() {
    let pairs = themes().into_iter().flat_map(|(theme, visuals)| {
        let palette = Palette::of(&visuals);
        backgrounds(&visuals)
            .into_iter()
            .enumerate()
            .flat_map(move |(row, background)| {
                data(&palette).into_iter().map(move |(name, color)| {
                    (
                        format!("{theme} {name} on row {row}"),
                        contrast(over(color, background), background),
                    )
                })
            })
    });
    assert_eq!(below(3.0, pairs), Vec::<String>::new());
}

/// The owner's name stands out of the hold it is written on by 4.5:1 at
/// least, the WCAG minimum for small text.
#[test]
fn the_owner_reads_on_its_hold() {
    let pairs = themes().into_iter().flat_map(|(theme, visuals)| {
        let palette = Palette::of(&visuals);
        backgrounds(&visuals)
            .into_iter()
            .enumerate()
            .map(move |(row, background)| {
                let hold = over(palette.hold, background);
                (
                    format!("{theme} owner on row {row}"),
                    contrast(over(palette.owner, hold), hold),
                )
            })
    });
    assert_eq!(below(4.5, pairs), Vec::<String>::new());
}

/// The palette follows the theme.
#[test]
fn each_theme_has_its_palette() {
    assert_eq!(Palette::of(&Visuals::light()).idle, LIGHT.idle);
    assert_eq!(Palette::of(&Visuals::dark()).idle, DARK.idle);
}
