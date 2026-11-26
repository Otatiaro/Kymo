//! Kymo timeline: shows a capture made by `kymo record --out`, read with the
//! ELF of the image that made it.

mod app;
#[cfg(windows)]
mod background;
mod durations;
mod filter;
mod levels;
mod listing;
mod meaning;
mod names;
mod palette;
mod panel;
mod spans;
mod timeline;
mod values;

use app::{App, Options};
use eframe::egui;
use kymo::events;
use kymo::target::Result;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: kymo-ui [<capture>] [--elf <file or URL>] [--clock <Hz>] [--svd <file>] [--theme <theme>]
               [--value <event.field>]... [--measure]

Shows a capture made by kymo record --out, read with the ELF it holds or
names, or the one --elf gives; an http(s) URL is fetched. Without a
capture, the window offers to open one: its Open button (Ctrl+O), a
capture dropped on it, its URL pasted in the URL menu, or the list of those
opened recently, kept for the next runs; another one replaces the timeline.
A capture whose ELF is not found says why, and waits for the ELF, chosen,
dropped or its URL pasted. An error shows above the window, which stays.

The timeline: one row per event type of the
image, the gaps (slots lost) marked across the rows in grey edged in red, a
red line where narrower than a pixel, and the overrun writes (a write
interrupted for a full ring lap: events recorded near it may be wrong) in
light orange. Wheel: zoom at the pointer; drag or horizontal wheel: scroll;
double click: the whole capture. Hovering an event shows its time, index
and fields, a subject by the name its row shows.

An image with a kernel adapter (OpSy's or FreeRTOS's) shows, instead of
the event rows (its events are in the event list), lines that fill the
height the bottom panel leaves: a row per task, by its last priority,
highest first, and the idle task, where it holds the core; a row per
interrupt handler, a nested one in the lower half, the
whole row from a loss to the next task switch, the nesting unknown; a row
per mutex, held, with its owner named where it fits, or waited for
(fainter). Handlers and mutexes follow their namespace, then their name.
Hovering a span shows its start, end and duration; a task's, its priority
at the pointer, left out until an event gives it and from a loss to the
next, and the last events up to the switch that ends it. An application
event (outside kymo::) is a flag at the head of the row of the context that
emitted it: the innermost handler open but the kernel's, else the task on
the core, else the CPU's row. Hovering a flag shows its time, index, fields
and context.

An image whose events include begins (kymo::begin_of, keyed_begin_of)
shows, last, a line per type of duration, by namespace: each duration a
bar, from its begin to its end, a loss or, still open, the last event.
Durations of a type open at once each take a lane, the lowest free at
their begin; a line shows up to 4 lanes, the last holding those past it.
Hovering a duration shows its start, end and duration, and the fields of
its begin.

Each numeric field of an application event (outside kymo::) is a curve,
event.field: an integer or a floating-point number, sampled at each event.
None shows at first: the toolbar's Values menu ticks those shown, kept for
the next runs, and --value shows one at launch. Each takes a line, last,
by namespace: a step from each sample to the next, cut by a loss, none
where a sample is NaN; its axis runs from the lowest to the highest value
in the view, both written at its edge. Zoomed out, each pixel column spans
the step entering it and its samples, a peak of one sample included;
zoomed in, each sample is a dot. Hovering a line shows the time, the value
held, when it was sampled and, with a kernel adapter, where.

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

The toolbar's Namespaces button opens the tree of the namespaces of the
image's subjects and event types, a box each, all ticked at first; it is
marked while one is not. Unticking a namespace unticks those nested in it,
and hides their subjects' and types' rows, the arrows to those rows, their
subjects' statistics, and the events whose type and subjects named are all
unticked; the CPU's and idle's rows stay. Its \"only\" button ticks it alone,
with its nested ones; All ticks them all. The namespaces unticked are kept
for the next runs, those of the image.

The toolbar names the capture and the range shown, the selection's or else
the view's; its × clears the selection, its Whole button shows the whole
capture. The theme follows the system's unless chosen in its menu; the
choice is kept for the next runs.

  --clock <Hz>  the cycle counter's frequency, as for kymo decode
  --svd <file>  names the interrupts the image's DWARF does not, as for
                kymo decode
  --theme <theme>
                system, light or dark, as chosen in the toolbar
  --value <event.field>
                shows that curve, as ticked in the Values menu; repeated,
                each one named
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
    let mut places = Vec::new();
    let mut elf = None;
    let mut clock = None;
    let mut measure = false;
    let mut theme = None;
    let mut svd = None;
    let mut values = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--clock" => clock = Some(arguments.next().ok_or("--clock needs a value")?.parse()?),
            "--elf" => elf = Some(arguments.next().ok_or("--elf needs a file or an URL")?),
            "--svd" => svd = Some(PathBuf::from(arguments.next().ok_or("--svd needs a file")?)),
            "--measure" => measure = true,
            "--value" => values.push(arguments.next().ok_or("--value needs a curve, as event.field")?),
            "--theme" => {
                theme = Some(match arguments.next().as_deref() {
                    Some("system") => egui::ThemePreference::System,
                    Some("light") => egui::ThemePreference::Light,
                    Some("dark") => egui::ThemePreference::Dark,
                    _ => return Err("--theme takes system, light or dark".into()),
                })
            }
            _ if !argument.starts_with("--") => places.push(argument),
            _ => return Err(format!("unknown option {argument}\n\n{USAGE}").into()),
        }
    }
    let capture = match places.as_slice() {
        [] if !measure && elf.is_none() => None,
        [capture] => Some(capture.clone()),
        _ => return Err(USAGE.into()),
    };
    if capture.is_none() && !values.is_empty() {
        return Err("--value needs a capture".into());
    }
    let mut app = App::new(Options { clock, svd, measure }, events::parse_with_svd);
    // Measured, the capture opens before the window, which an error does not open.
    if measure && let Some(capture) = &capture {
        app.open(capture, elf.as_deref(), None)?;
        app.choose(&values)?;
    }
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_title("Kymo"),
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
            if !measure {
                // An error opening the capture shows in the window.
                if let Some(capture) = &capture {
                    let result = app.open(capture, elf.as_deref(), None);
                    app.attempt(result);
                }
                if let Some(storage) = context.storage {
                    app.restore(|key| storage.get_string(key));
                }
                // The curves named show with those kept from the last runs.
                if capture.is_some() {
                    let result = app.choose(&values);
                    app.attempt(result);
                }
            }
            Ok(Box::new(app))
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
