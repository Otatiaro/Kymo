//! Kymo timeline: shows a capture made by `kymo record --out`, read with the
//! ELF of the image that made it.

mod levels;
mod timeline;

use eframe::egui;
use kymo::events;
use kymo::model::Trace;
use kymo::target::Result;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
usage: kymo-ui <elf> <capture> [--clock <Hz>] [--measure]

Shows a capture made by kymo record --out: one row per event type of the
image, the losses and suspect lines marked across the rows. Wheel: zoom at
the pointer; drag or horizontal wheel: scroll; double click: the whole
capture. Hovering an event shows its line as kymo decode prints it.

  --clock <Hz>  the cycle counter's frequency, as for kymo decode
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
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--clock" => clock = Some(arguments.next().ok_or("--clock needs a value")?.parse()?),
            "--measure" => measure = true,
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
    let timeline = timeline::Timeline::new(catalogue, trace, measure);
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
    }
    eframe::run_native("kymo-ui", options, Box::new(move |_| Ok(Box::new(timeline))))?;
    Ok(())
}
