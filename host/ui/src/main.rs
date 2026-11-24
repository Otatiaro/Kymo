//! Kymo timeline: shows a capture made by `kymo record --out`, read with the
//! ELF of the image that made it.

#[cfg(windows)]
mod background;
mod levels;
mod listing;
mod meaning;
mod names;
mod palette;
mod panel;
mod spans;
mod timeline;

use eframe::egui;
use kymo::events;
use kymo::interrupts::Interrupts;
use kymo::model::Trace;
use kymo::target::Result;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
usage: kymo-ui <elf> <capture> [--clock <Hz>] [--svd <file>] [--theme <theme>] [--measure]

Shows a capture made by kymo record --out: one row per event type of the
image, the gaps (slots lost) and the overrun writes (a write interrupted for
a full ring lap: events recorded near it may be wrong) marked across the
rows. Wheel: zoom at the pointer; drag or horizontal wheel: scroll; double
click: the whole capture. Hovering an event shows its time, index and
fields, a subject by the name its row shows.

An image with a kernel adapter (OpSy's or FreeRTOS's) shows, instead of
the event rows (its events are in the event list), lines that fill the
height the bottom panel leaves: a row per task, by its last priority,
highest first, and the idle task, where it holds the core; a row per
interrupt handler, a nested one in the lower half, the
whole row from a loss to the next task switch, the nesting unknown; a row
per mutex, held, with its owner named where it fits, or waited for
(fainter). Hovering a span shows its start, end and duration; a task's, its
priority at the pointer, left out until an event gives it and from a loss
to the next, and the last events up to the switch that ends it.

Under a task's runs, a thin grey bar marks its time ready: woken, waiting
for the core. Hovering a run or the bar shows what woke the task, why, when,
and how long it waited for its first run, with an arrow from the row of the
handler or task that woke it. A handler without Kymo's hooks is seen as the
task it interrupts.

Their first row is the CPU's load: in each bin (16 pixels wide by default,
set in the toolbar), the time handlers run, then tasks other than idle,
stacked, then the time unknown, around losses, in the losses' colour;
idle is the empty part above. Zoomed out, the other rows show their load
too, chosen in the toolbar: Auto, once a row shows more than a span per 4
pixels; Bars; or Load. A row's bar is
its share of the time known, the mean of the bin or, as chosen in the
toolbar, the median, min or max of its sub-bins; a bin with no time known
stays empty. Hovering a bin shows its share, how much of it is known, and
the longest span in it.

Below, the events of the range selected (shift and drag; Esc clears it) or
else of the view once it stands still, by time, a line each as kymo decode
prints it; a gap or an overrun write stands after the event recorded before
it, marked in its colour. Clicking a line centres the timeline on it at the
same zoom.

With a kernel adapter, a second tab shows the statistics of the kernel's
instances, as kymo stats computes them, over the same range: a row per
subject and metric, sorted by the column clicked,
the p99 and the max marked \"≥\" when instances were excluded. Clicking a
min or a max, or one of the worst instances a subject lists when clicked,
centres the timeline on that instance and frames it.

The toolbar names the capture and the range shown, the selection's or else
the view's; its × clears the selection, its Whole button shows the whole
capture. The theme follows the system's unless chosen in its menu; the
choice is kept for the next runs.

  --clock <Hz>  the cycle counter's frequency, as for kymo decode
  --svd <file>  names the interrupts the image's DWARF does not, as for
                kymo decode
  --theme <theme>
                system, light or dark, as chosen in the toolbar
  --measure     prints the time to read the capture and build the timeline,
                shows the whole capture then the narrowest view, 300 frames
                each without vsync, prints the frame times and quits
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut paths = Vec::new();
    let mut clock = None;
    let mut measure = false;
    let mut theme = None;
    let mut svd = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--clock" => clock = Some(arguments.next().ok_or("--clock needs a value")?.parse()?),
            "--svd" => svd = Some(PathBuf::from(arguments.next().ok_or("--svd needs a file")?)),
            "--measure" => measure = true,
            "--theme" => {
                theme = Some(match arguments.next().as_deref() {
                    Some("system") => egui::ThemePreference::System,
                    Some("light") => egui::ThemePreference::Light,
                    Some("dark") => egui::ThemePreference::Dark,
                    _ => return Err("--theme takes system, light or dark".into()),
                })
            }
            _ if !argument.starts_with("--") => paths.push(PathBuf::from(argument)),
            _ => return Err(format!("unknown option {argument}\n\n{USAGE}").into()),
        }
    }
    let [elf, capture] = paths.as_slice() else {
        return Err(USAGE.into());
    };
    // The trace borrows the catalogue, which lives as long as the window.
    let mut read_catalogue = events::read(elf)?;
    if let Some(svd) = svd {
        read_catalogue.interrupts = read_catalogue.interrupts.or(&Interrupts::from_svd(&svd)?);
    }
    let catalogue: &'static events::Catalogue = Box::leak(Box::new(read_catalogue));
    let bytes = std::fs::read(capture).map_err(|error| format!("{}: {error}", capture.display()))?;
    let began = Instant::now();
    let trace = Trace::read(catalogue, &bytes, clock)?;
    let read = began.elapsed();
    let name = capture.file_name().map_or_else(
        || capture.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let timeline = timeline::Timeline::new(name, catalogue, trace, measure)?;
    if measure {
        println!(
            "capture read in {:.0} ms, timeline built in {:.0} ms",
            read.as_secs_f64() * 1e3,
            (began.elapsed() - read).as_secs_f64() * 1e3
        );
    }
    let title = format!("Kymo: {}", capture.display());
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_title(title),
        ..Default::default()
    };
    if measure {
        options.wgpu_options.surface.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
        // The window measured is the size above, not the last one used.
        options.persist_window = false;
    }
    eframe::run_native(
        "kymo-ui",
        options,
        Box::new(move |context| {
            let ctx = &context.egui_ctx;
            ctx.set_fonts(fonts());
            for theme in [egui::Theme::Dark, egui::Theme::Light] {
                ctx.set_visuals_of(theme, palette::visuals(theme));
            }
            ctx.all_styles_mut(|style| {
                style.spacing.item_spacing = egui::vec2(8.0, 6.0);
                style.spacing.button_padding = egui::vec2(8.0, 3.0);
                if let Some(body) = style.text_styles.get_mut(&egui::TextStyle::Body) {
                    body.size = 13.0;
                }
            });
            if let Some(theme) = theme {
                context.egui_ctx.set_theme(theme);
            }
            #[cfg(windows)]
            background::set(context, ctx.global_style().visuals.panel_fill);
            Ok(Box::new(timeline))
        }),
    )?;
    Ok(())
}

/// Inter (SIL Open Font License, `fonts/Inter-LICENSE.txt`) first for the
/// proportional text; egui's fonts after it for the glyphs it lacks, and for
/// the monospace text.
fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".to_owned(),
        egui::FontData::from_static(include_bytes!("../fonts/Inter-Regular.ttf")).into(),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".to_owned());
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inter_is_first_for_proportional_text_only() {
        let fonts = fonts();
        let family = |family| &fonts.families[&family];
        assert_eq!(family(egui::FontFamily::Proportional)[0], "Inter");
        // egui's fonts follow, for the glyphs Inter lacks.
        assert!(family(egui::FontFamily::Proportional).len() > 1);
        assert!(!family(egui::FontFamily::Monospace).contains(&"Inter".to_owned()));
    }
}
