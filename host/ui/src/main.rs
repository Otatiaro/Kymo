//! Kymo timeline: shows a capture made by `kymo record --out`, read with the
//! ELF of the image that made it.

mod levels;
mod meaning;
mod names;
mod palette;
mod panel;
mod spans;
mod timeline;

use eframe::egui;
use kymo::events;
use kymo::model::Trace;
use kymo::target::Result;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
usage: kymo-ui <elf> <capture> [--clock <Hz>] [--theme <theme>] [--measure]

Shows a capture made by kymo record --out: one row per event type of the
image, the gaps (slots lost) and the overrun writes (a write interrupted for
a full ring lap: events recorded near it may be wrong) marked across the
rows. Wheel: zoom at the pointer; drag or horizontal wheel: scroll; double
click: the whole capture. Hovering an event shows its line as kymo decode
prints it.

An image with the OpSy adapter shows, above the event rows, folded then: a
row per task, by its last priority, and the idle task, where it holds the
core; a row per interrupt handler, a nested one in the lower half, the
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

Their first row is the CPU's load: in each bin of a few pixels, the share of
the time known that handlers run, then tasks other than idle, stacked; the
time unknown, around losses, hatched above. Zoomed out, the other rows show
their load too, chosen in the status bar: Auto, once a row shows more than
a span per 4 pixels; Bars; or Load. A line marks a bin's busiest sub-bin.
Hovering a bin shows its share, how much of it is known, and the longest
span in it.

Below, the statistics of OpSy's instances, as kymo stats computes them, over
the range selected (shift and drag; Esc clears it) or else the view once it
stands still: a row per subject and metric, sorted by the column clicked,
the p99 and the max marked \"≥\" when instances were excluded. Clicking a
min or a max, or one of the worst instances a subject lists when clicked,
centres the timeline on that instance and frames it.

The theme follows the system's unless chosen in the status bar; the choice
is kept for the next runs.

  --clock <Hz>  the cycle counter's frequency, as for kymo decode
  --theme <theme>
                system, light or dark, as chosen in the status bar
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
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--clock" => clock = Some(arguments.next().ok_or("--clock needs a value")?.parse()?),
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
    let catalogue: &'static events::Catalogue = Box::leak(Box::new(events::read(elf)?));
    let bytes = std::fs::read(capture).map_err(|error| format!("{}: {error}", capture.display()))?;
    let began = Instant::now();
    let trace = Trace::read(catalogue, &bytes, clock)?;
    let read = began.elapsed();
    let timeline = timeline::Timeline::new(catalogue, trace, measure)?;
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
            if let Some(theme) = theme {
                context.egui_ctx.set_theme(theme);
            }
            Ok(Box::new(timeline))
        }),
    )?;
    Ok(())
}
