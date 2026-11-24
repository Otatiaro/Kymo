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
    [("light", visuals(Theme::Light)), ("dark", visuals(Theme::Dark))]
}

/// The text, strong and weak, reads on the rows and on the surface by 4.5:1
/// at least, the WCAG minimum for small text.
#[test]
fn the_text_reads_on_every_background() {
    let pairs = themes().into_iter().flat_map(|(theme, visuals)| {
        let palette = Palette::of(&visuals);
        let [panel, stripe] = backgrounds(&visuals);
        let surface = over(palette.surface, [0.0; 3]);
        let texts = [
            ("text", visuals.text_color()),
            ("strong", visuals.strong_text_color()),
            ("weak", visuals.weak_text_color()),
        ];
        [("panel", panel), ("stripe", stripe), ("surface", surface)]
            .into_iter()
            .flat_map(move |(place, background)| {
                texts.into_iter().map(move |(name, color)| {
                    (
                        format!("{theme} {name} on {place}"),
                        contrast(over(color, background), background),
                    )
                })
            })
    });
    assert_eq!(below(4.5, pairs), Vec::<String>::new());
}

/// The loss and suspect colours, written as warnings in the status bar,
/// read on the panel by 4.5:1 at least.
#[test]
fn the_warnings_read_on_the_panel() {
    let pairs = themes().into_iter().flat_map(|(theme, visuals)| {
        let palette = Palette::of(&visuals);
        let [panel, _] = backgrounds(&visuals);
        [("loss", palette.loss), ("suspect", palette.suspect)]
            .into_iter()
            .map(move |(name, color)| (format!("{theme} {name}"), contrast(over(color, panel), panel)))
    });
    assert_eq!(below(4.5, pairs), Vec::<String>::new());
}

/// The tasks' colours, written as names in the list, read on the rows by
/// 4.5:1 at least: every shade, not a sample.
#[test]
fn the_tasks_read_on_the_rows() {
    let pairs = themes().into_iter().flat_map(|(theme, visuals)| {
        let palette = Palette::of(&visuals);
        backgrounds(&visuals)
            .into_iter()
            .enumerate()
            .flat_map(move |(row, background)| {
                palette.tasks.into_iter().enumerate().map(move |(shade, color)| {
                    (
                        format!("{theme} task {shade} on row {row}"),
                        contrast(over(color, background), background),
                    )
                })
            })
    });
    assert_eq!(below(4.5, pairs), Vec::<String>::new());
}

/// A free task's shade comes from its name alone, the same from a run to
/// the next, and names spread over the shades.
#[test]
fn a_task_shade_comes_from_its_name() {
    let name = "app::(anonymous namespace)::g_logger";
    assert_eq!(Shade::of(name, &[]), Shade::of(name, &[]));
    // FNV-1a of "a" is 0xaf63dc4c8601ec8c.
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(Shade::of("a", &[]), Shade::from(0xaf63_dc4c_8601_ec8c));
    let shades: std::collections::HashSet<_> = (0..64).map(|n| Shade::of(&format!("app::task_{n}"), &[])).collect();
    assert_eq!(shades.len(), Shade::COUNT);
}

/// A task whose shade is taken takes the one its hash hashed again picks,
/// when free.
#[test]
fn a_taken_shade_hashes_again() {
    let own = Shade::of("a", &[]);
    let again = fnv1a(&0xaf63_dc4c_8601_ec8c_u64.to_le_bytes());
    assert_ne!(Shade::from(again), own, "the test needs a second shade");
    assert_eq!(Shade::of("a", &[own]), Shade::from(again));
}

/// Up to `Shade::COUNT` tasks, each has its own shade, the last one free
/// included; past them, a task shares its name's.
#[test]
fn every_task_has_its_own_shade() {
    let mut taken = Vec::new();
    for n in 0..Shade::COUNT {
        taken.push(Shade::of(&format!("app::task_{n}"), &taken));
    }
    let distinct: std::collections::HashSet<_> = taken.iter().collect();
    assert_eq!(distinct.len(), Shade::COUNT);
    let all_but = |free: usize| (0..Shade::COUNT).filter(|&n| n != free).map(Shade).collect::<Vec<_>>();
    for free in 0..Shade::COUNT {
        assert_eq!(Shade::of("a", &all_but(free)), Shade(free));
    }
    assert_eq!(Shade::of("a", &taken), Shade::of("a", &[]));
}

/// The tasks' colours differ from each other.
#[test]
fn the_tasks_differ() {
    for palette in [LIGHT, DARK] {
        let distinct: std::collections::HashSet<_> = palette.tasks.iter().collect();
        assert_eq!(distinct.len(), Shade::COUNT);
    }
}

/// The colours the data is drawn with, by name, the tasks' among them.
fn data(palette: &Palette) -> Vec<(&'static str, Color32)> {
    const TASKS: [&str; Shade::COUNT] = [
        "task 0", "task 1", "task 2", "task 3", "task 4", "task 5", "task 6", "task 7", "task 8", "task 9", "task 10",
        "task 11", "task 12", "task 13", "task 14", "task 15",
    ];
    let tasks = TASKS.into_iter().zip(palette.tasks);
    [
        ("event", palette.event),
        ("run", palette.run),
        ("idle", palette.idle),
        ("interrupt", palette.interrupt),
        ("hold", palette.hold),
        ("wait", palette.wait),
        ("ready", palette.ready),
        ("wake", palette.wake),
        ("unknown", palette.unknown),
        ("loss", palette.loss),
        ("suspect", palette.suspect),
    ]
    .into_iter()
    .chain(tasks)
    .collect()
}

/// The pairs below `minimum`, each with its ratio.
fn below(minimum: f64, pairs: impl Iterator<Item = (String, f64)>) -> Vec<String> {
    pairs
        .filter(|(_, ratio)| *ratio < minimum)
        .map(|(name, ratio)| format!("{name} {ratio:.2}:1"))
        .collect()
}

/// Every colour of the data, over either background of its rows, stands out
/// of it by 3:1 at least, the WCAG minimum for a graphical object. The loss
/// and suspect colours are those of their marks in the ruler, on the panel.
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

/// Under a loss's or a suspect line's band, the data still stands out of
/// the row by 3:1 at least.
#[test]
fn the_data_reads_through_a_band() {
    let pairs = themes().into_iter().flat_map(|(theme, visuals)| {
        let palette = Palette::of(&visuals);
        let bands = [("loss", palette.loss), ("suspect", palette.suspect)];
        backgrounds(&visuals)
            .into_iter()
            .enumerate()
            .flat_map(move |(row, background)| {
                bands.into_iter().flat_map(move |(band, mark)| {
                    let band_color = mark.gamma_multiply(BAND);
                    data(&palette)
                        .into_iter()
                        .filter(|(name, _)| !matches!(*name, "loss" | "suspect"))
                        .map(move |(name, color)| {
                            let under = over(band_color, over(color, background));
                            (
                                format!("{theme} {name} under {band} on row {row}"),
                                contrast(under, over(band_color, background)),
                            )
                        })
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
    assert_eq!(Palette::of(&visuals(Theme::Light)).idle, LIGHT.idle);
    assert_eq!(Palette::of(&visuals(Theme::Dark)).idle, DARK.idle);
}
