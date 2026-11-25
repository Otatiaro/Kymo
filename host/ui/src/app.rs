//! The window: the timeline of the capture opened, or else what opens one.
//! A capture opens from the command line, the Open button (Ctrl+O), a file
//! dropped on the window or the list of recent ones; another one replaces the
//! timeline. A capture whose ELF is not found waits for it, given the same
//! ways. An error is shown above what the window shows, which stays.

use crate::timeline::{self, Timeline};
use eframe::egui;
use kymo::capture::{self, Elf};
use kymo::events::Catalogue;
use kymo::model::Trace;
use kymo::target::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How many recent captures are kept.
const RECENT: usize = 8;
/// The key the recent captures are saved under, a path a line.
const RECENTS: &str = "recent captures";

/// Reads an image's ELF: its bytes, what errors call it, and the SVD file
/// that names its interrupts.
pub type Parse = fn(&[u8], &str, Option<&Path>) -> Result<Catalogue>;

/// Where the timeline's state is saved, none in tests.
pub type Store<'a> = Option<&'a mut (dyn eframe::Storage + 'static)>;

/// What the command line sets for every capture opened.
#[derive(Default)]
pub struct Options {
    pub clock: Option<u64>,
    pub svd: Option<PathBuf>,
    pub measure: bool,
}

pub struct App {
    options: Options,
    parse: Parse,
    shown: Shown,
    /// The last error, until dismissed or another capture opens.
    error: Option<String>,
    /// The captures opened, the latest first.
    recent: Vec<PathBuf>,
    /// The file dialog to open at the next frame.
    pick: Option<Pick>,
    /// The window's title, as last set.
    titled: String,
}

enum Shown {
    Nothing,
    Timeline(Box<Timeline>),
    /// A capture read, whose ELF is not found.
    Waiting(Waiting),
}

struct Waiting {
    capture: PathBuf,
    bytes: Vec<u8>,
    /// Where the capture names its ELF, and why it is not read there.
    place: String,
    why: String,
}

#[derive(Clone, Copy)]
enum Pick {
    Capture,
    Elf,
}

/// What the toolbar's file controls ask for.
enum Request {
    Pick(Pick),
    Open(PathBuf),
}

/// The ELF of a capture, found, or named where it is not found.
enum Image {
    Found(Vec<u8>, String),
    Missing { place: String, why: String },
}

impl App {
    pub fn new(options: Options, parse: Parse) -> Self {
        Self {
            options,
            parse,
            shown: Shown::Nothing,
            error: None,
            recent: Vec::new(),
            pick: None,
            titled: String::new(),
        }
    }

    /// Restores the recent captures, after those opened already, and the
    /// timeline's state, from what `saved` gives by key.
    pub fn restore(&mut self, saved: impl Fn(&str) -> Option<String>) {
        if let Some(recent) = saved(RECENTS) {
            for path in recent.lines().filter(|line| !line.is_empty()).map(PathBuf::from) {
                if !self.recent.contains(&path) {
                    self.recent.push(path);
                }
            }
            self.recent.truncate(RECENT);
        }
        if let Shown::Timeline(timeline) = &mut self.shown {
            timeline.restore(saved);
        }
    }

    /// Opens the capture at `path`, read with the ELF at `elf`, else the one
    /// it holds or names; one whose ELF is not found waits for it. The
    /// timeline shown is saved to and the new one restored from `storage`.
    pub fn open(&mut self, path: &Path, elf: Option<&Path>, storage: Store<'_>) -> Result<()> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let image = match elf {
            Some(elf) => {
                let bytes = std::fs::read(elf).map_err(|error| format!("{}: {error}", elf.display()))?;
                Image::Found(bytes, elf.display().to_string())
            }
            None => image(&path.display().to_string(), &bytes)?,
        };
        self.remember(path);
        match image {
            Image::Found(elf, name) => {
                let timeline = self.load(path, &bytes, &elf, &name)?;
                self.show(timeline, storage);
            }
            Image::Missing { place, why } => {
                self.leave(storage);
                self.shown = Shown::Waiting(Waiting {
                    capture: path.to_owned(),
                    bytes,
                    place,
                    why,
                });
            }
        }
        self.error = None;
        Ok(())
    }

    /// Reads the capture waiting for its ELF with the one at `path`.
    fn give(&mut self, path: &Path, storage: Store<'_>) -> Result<()> {
        let Shown::Waiting(waiting) = &self.shown else {
            return Err("no capture waits for an ELF".into());
        };
        let elf = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let timeline = self.load(&waiting.capture, &waiting.bytes, &elf, &path.display().to_string())?;
        self.show(timeline, storage);
        self.error = None;
        Ok(())
    }

    fn load(&self, path: &Path, bytes: &[u8], elf: &[u8], elf_name: &str) -> Result<Timeline> {
        // The trace borrows the catalogue: each capture opened keeps its
        // catalogue until the window closes.
        let catalogue: &'static Catalogue =
            Box::leak(Box::new((self.parse)(elf, elf_name, self.options.svd.as_deref())?));
        let began = Instant::now();
        let trace = Trace::read(catalogue, bytes, self.options.clock)?;
        let read = began.elapsed();
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let timeline = Timeline::new(name, catalogue, trace, self.options.measure)?;
        if self.options.measure {
            println!(
                "capture read in {:.0} ms, timeline built in {:.0} ms",
                read.as_secs_f64() * 1e3,
                (began.elapsed() - read).as_secs_f64() * 1e3
            );
        }
        Ok(timeline)
    }

    /// Shows `timeline` in place of what is shown.
    fn show(&mut self, mut timeline: Timeline, storage: Store<'_>) {
        if let Some(storage) = storage {
            if let Shown::Timeline(shown) = &self.shown {
                shown.save(storage);
            }
            // The frames measured are those of the whole trace.
            if !self.options.measure {
                timeline.restore(|key| storage.get_string(key));
            }
        }
        self.shown = Shown::Timeline(Box::new(timeline));
    }

    /// Saves the timeline shown, before another takes its place.
    fn leave(&mut self, storage: Store<'_>) {
        if let (Shown::Timeline(timeline), Some(storage)) = (&self.shown, storage) {
            timeline.save(storage);
        }
    }

    /// Puts `path` first among the recent captures.
    fn remember(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        self.recent.retain(|recent| *recent != path);
        self.recent.insert(0, path);
        self.recent.truncate(RECENT);
    }

    /// The files dropped on the window: a capture opens; another file is the
    /// ELF of the capture waiting for one.
    fn dropped(&mut self, paths: Vec<PathBuf>, mut storage: Store<'_>) {
        for path in paths {
            let waiting = matches!(self.shown, Shown::Waiting(_));
            let result = if waiting && !starts_as_capture(&path) {
                self.give(&path, storage.as_deref_mut())
            } else {
                self.open(&path, None, storage.as_deref_mut())
            };
            self.attempt(result);
        }
    }

    /// Shows the error of `result`, if any.
    pub fn attempt(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
    }

    /// The window's title: the capture shown or waiting.
    fn title(&self) -> String {
        match &self.shown {
            Shown::Nothing => "Kymo".to_owned(),
            Shown::Timeline(timeline) => format!("Kymo: {}", timeline.capture()),
            Shown::Waiting(waiting) => format!("Kymo: {}", waiting.capture.display()),
        }
    }

    /// The file dialog `pick` asks for, and what it gives.
    fn dialog(&mut self, pick: Pick, storage: Store<'_>) {
        let dialog = rfd::FileDialog::new();
        let dialog = match pick {
            Pick::Capture => dialog.set_title("Open a capture").add_filter("Kymo capture", &["kcap"]),
            Pick::Elf => dialog
                .set_title("Choose the capture's ELF")
                .add_filter("ELF", &["elf", "axf", "out"]),
        };
        let Some(path) = dialog.add_filter("All files", &["*"]).pick_file() else {
            return;
        };
        let result = match pick {
            Pick::Capture => self.open(&path, None, storage),
            Pick::Elf => self.give(&path, storage),
        };
        self.attempt(result);
    }
}

/// Whether the file at `path` begins as a capture does.
fn starts_as_capture(path: &Path) -> bool {
    use std::io::Read;
    let mut start = [0; 8];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut start))
        .is_ok_and(|()| capture::is_capture(&start))
}

/// The ELF of the capture `bytes`, called `name`: the one it holds, or the
/// file it names.
fn image(name: &str, bytes: &[u8]) -> Result<Image> {
    if !capture::is_capture(bytes) {
        return Err(format!("{name} is not a Kymo capture").into());
    }
    Ok(match capture::header(bytes)?.elf {
        Elf::Embedded(elf) => Image::Found(elf, format!("the ELF in {name}")),
        Elf::Reference(place) if capture::is_url(&place) => Image::Missing {
            place,
            why: "this kymo-ui does not fetch URLs".to_owned(),
        },
        Elf::Reference(place) => match std::fs::read(&place) {
            Ok(elf) => Image::Found(elf, place),
            Err(error) => Image::Missing {
                place,
                why: error.to_string(),
            },
        },
    })
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let recent: Vec<_> = self.recent.iter().map(|path| path.display().to_string()).collect();
        storage.set_string(RECENTS, recent.join("\n"));
        if let Shown::Timeline(timeline) = &self.shown {
            timeline.save(storage);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect()
        });
        self.dropped(dropped, frame.storage_mut());
        let open = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::O);
        if ctx.input_mut(|input| input.consume_shortcut(&open)) {
            self.pick = Some(Pick::Capture);
        }
        if let Some(pick) = self.pick.take() {
            self.dialog(pick, frame.storage_mut());
        }
        if self.title() != self.titled {
            self.titled = self.title();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.titled.clone()));
        }
        if let Some(error) = &self.error {
            let mut dismissed = false;
            egui::Panel::top("error").show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("×").on_hover_text("Dismiss").clicked() {
                        dismissed = true;
                    }
                    ui.colored_label(ui.visuals().error_fg_color, error);
                });
            });
            if dismissed {
                self.error = None;
            }
        }
        let mut request = None;
        let Self { shown, recent, .. } = self;
        let mut files = |ui: &mut egui::Ui| files(ui, recent, &mut request);
        let choose = match shown {
            Shown::Timeline(timeline) => {
                timeline.ui(ui, frame, &mut files);
                false
            }
            Shown::Nothing => empty(ui, &mut files, None),
            Shown::Waiting(waiting) => empty(ui, &mut files, Some(waiting)),
        };
        if choose {
            self.pick = Some(Pick::Elf);
        }
        match request {
            Some(Request::Pick(pick)) => self.pick = Some(pick),
            Some(Request::Open(path)) => {
                let result = self.open(&path, None, frame.storage_mut());
                self.attempt(result);
            }
            None => {}
        }
        let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
        if hovering {
            drop_hint(&ctx, matches!(self.shown, Shown::Waiting(_)));
        }
    }
}

/// The toolbar's file controls: Open and the recent captures.
fn files(ui: &mut egui::Ui, recent: &[PathBuf], request: &mut Option<Request>) {
    if ui
        .button("Open…")
        .on_hover_text("Open a capture (Ctrl+O), or drop one on the window")
        .clicked()
    {
        *request = Some(Request::Pick(Pick::Capture));
    }
    ui.add_enabled_ui(!recent.is_empty(), |ui| {
        ui.menu_button("Recent", |ui| {
            if let Some(path) = recent_list(ui, recent) {
                *request = Some(Request::Open(path));
                ui.close();
            }
        });
    });
}

/// The recent captures, a button each; the one clicked.
fn recent_list(ui: &mut egui::Ui, recent: &[PathBuf]) -> Option<PathBuf> {
    let mut chosen = None;
    for path in recent {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        if ui.button(name).on_hover_text(path.display().to_string()).clicked() {
            chosen = Some(path.clone());
        }
    }
    chosen
}

/// The window without a timeline: how to open a capture, or what the
/// capture waiting for its ELF names; whether its ELF is to be chosen.
fn empty(ui: &mut egui::Ui, files: &mut dyn FnMut(&mut egui::Ui), waiting: Option<&Waiting>) -> bool {
    let mut choose = false;
    let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 6));
    egui::Panel::top("toolbar").frame(frame).show(ui, |ui| {
        ui.horizontal(|ui| {
            files(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), timeline::theme_menu);
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(ui.available_height() / 3.0);
        ui.vertical_centered(|ui| match waiting {
            None => {
                ui.heading("Open a capture");
                ui.label("Open… (Ctrl+O), drop a .kcap on the window, or pick a recent one.");
            }
            Some(waiting) => {
                ui.heading(format!("{}: its ELF is not found", waiting.capture.display()));
                ui.label(format!("The capture names {}: {}.", waiting.place, waiting.why));
                ui.add_space(8.0);
                choose = ui.button("Choose the ELF…").clicked();
                ui.label("the one it was recorded with, or drop it on the window");
            }
        });
    });
    choose
}

/// Over the window while files are dragged over it: what dropping does.
fn drop_hint(ctx: &egui::Context, waiting: bool) {
    let text = match waiting {
        true => "Drop the ELF, or another capture",
        false => "Drop a capture to open it",
    };
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop"));
    let painter = ctx.layer_painter(layer);
    let screen = ctx.content_rect();
    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(20.0),
        egui::Color32::WHITE,
    );
}

#[cfg(test)]
mod tests;
