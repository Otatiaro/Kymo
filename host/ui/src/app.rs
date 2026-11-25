//! The window: the timeline of the capture opened, or else what opens one.
//! A capture opens from the command line, the Open button (Ctrl+O), a file
//! dropped on the window, its URL pasted or the list of recent ones; another
//! one replaces the timeline. A capture whose ELF is not found waits for it,
//! given the same ways. An error is shown above what the window shows, which stays.

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
/// The key the recent captures are saved under, a path or an URL a line.
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
    /// The captures opened, paths or URLs, the latest first.
    recent: Vec<String>,
    /// The URL of a capture to open, as typed.
    url: String,
    /// The URL of the ELF of the capture waiting for one, as typed.
    elf_url: String,
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
    capture: String,
    bytes: Vec<u8>,
    /// Why its ELF is not read where the capture names it.
    why: String,
}

#[derive(Clone, Copy)]
enum Pick {
    Capture,
    Elf,
}

/// What the file controls ask for: a file dialog, a capture to open, or the
/// ELF of the capture waiting for one.
enum Request {
    Pick(Pick),
    Open(String),
    Give(String),
}

/// The ELF of a capture, found, or why it is not found where the capture
/// names it.
enum Image {
    Found(Vec<u8>, String),
    Missing(String),
}

impl App {
    pub fn new(options: Options, parse: Parse) -> Self {
        Self {
            options,
            parse,
            shown: Shown::Nothing,
            error: None,
            recent: Vec::new(),
            url: String::new(),
            elf_url: String::new(),
            pick: None,
            titled: String::new(),
        }
    }

    /// Restores the recent captures, after those opened already, and the
    /// timeline's state, from what `saved` gives by key.
    pub fn restore(&mut self, saved: impl Fn(&str) -> Option<String>) {
        if let Some(recent) = saved(RECENTS) {
            for place in recent.lines().filter(|line| !line.is_empty()) {
                if !self.recent.iter().any(|recent| recent == place) {
                    self.recent.push(place.to_owned());
                }
            }
            self.recent.truncate(RECENT);
        }
        if let Shown::Timeline(timeline) = &mut self.shown {
            timeline.restore(saved);
        }
    }

    /// Shows the curves `names` on the timeline shown; an error names the
    /// first its image does not declare.
    pub fn choose(&mut self, names: &[String]) -> Result<()> {
        match &mut self.shown {
            Shown::Timeline(timeline) => timeline.choose(names),
            _ if names.is_empty() => Ok(()),
            _ => Err("--value needs a capture read with its ELF".into()),
        }
    }

    /// Opens the capture at `place`, a file or an URL, read with the ELF at
    /// `elf`, else the one it holds or names; one whose ELF is not found
    /// waits for it. The timeline shown is saved to and the new one restored
    /// from `storage`.
    pub fn open(&mut self, place: &str, elf: Option<&str>, storage: Store<'_>) -> Result<()> {
        let bytes = capture::fetch(place)?;
        let image = match elf {
            Some(elf) => Image::Found(capture::fetch(elf)?, elf.to_owned()),
            None => image(place, &bytes)?,
        };
        self.remember(place);
        match image {
            Image::Found(elf, name) => {
                let timeline = self.load(place, &bytes, &elf, &name)?;
                self.show(timeline, storage);
            }
            Image::Missing(why) => {
                self.leave(storage);
                self.shown = Shown::Waiting(Waiting {
                    capture: place.to_owned(),
                    bytes,
                    why,
                });
            }
        }
        self.error = None;
        Ok(())
    }

    /// Reads the capture waiting for its ELF with the one at `place`, a file
    /// or an URL.
    fn give(&mut self, place: &str, storage: Store<'_>) -> Result<()> {
        let Shown::Waiting(waiting) = &self.shown else {
            return Err("no capture waits for an ELF".into());
        };
        let elf = capture::fetch(place)?;
        let timeline = self.load(&waiting.capture, &waiting.bytes, &elf, place)?;
        self.show(timeline, storage);
        self.error = None;
        Ok(())
    }

    fn load(&self, place: &str, bytes: &[u8], elf: &[u8], elf_name: &str) -> Result<Timeline> {
        // The trace borrows the catalogue: each capture opened keeps its
        // catalogue until the window closes.
        let catalogue: &'static Catalogue =
            Box::leak(Box::new((self.parse)(elf, elf_name, self.options.svd.as_deref())?));
        let began = Instant::now();
        let trace = Trace::read(catalogue, bytes, self.options.clock)?;
        let read = began.elapsed();
        let timeline = Timeline::new(last(place).to_owned(), catalogue, trace, self.options.measure)?;
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

    /// Puts `place` first among the recent captures, a file by its absolute
    /// path.
    fn remember(&mut self, place: &str) {
        let place = match capture::is_url(place) {
            true => place.to_owned(),
            false => std::path::absolute(place).map_or_else(|_| place.to_owned(), |path| path.display().to_string()),
        };
        self.recent.retain(|recent| *recent != place);
        self.recent.insert(0, place);
        self.recent.truncate(RECENT);
    }

    /// The files dropped on the window: a capture opens; another file is the
    /// ELF of the capture waiting for one.
    fn dropped(&mut self, paths: Vec<PathBuf>, mut storage: Store<'_>) {
        for path in paths {
            let waiting = matches!(self.shown, Shown::Waiting(_));
            let place = path.display().to_string();
            let result = if waiting && !starts_as_capture(&path) {
                self.give(&place, storage.as_deref_mut())
            } else {
                self.open(&place, None, storage.as_deref_mut())
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
            Shown::Waiting(waiting) => format!("Kymo: {}", waiting.capture),
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
        let place = path.display().to_string();
        let result = match pick {
            Pick::Capture => self.open(&place, None, storage),
            Pick::Elf => self.give(&place, storage),
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
/// file or URL it names.
fn image(name: &str, bytes: &[u8]) -> Result<Image> {
    if !capture::is_capture(bytes) {
        return Err(format!("{name} is not a Kymo capture").into());
    }
    Ok(match capture::header(bytes)?.elf {
        Elf::Embedded(elf) => Image::Found(elf, format!("the ELF in {name}")),
        Elf::Reference(place) => match capture::fetch(&place) {
            Ok(elf) => Image::Found(elf, place),
            Err(error) => Image::Missing(error.to_string()),
        },
    })
}

/// The last part of the path or URL `place`: a file's name.
fn last(place: &str) -> &str {
    place.rsplit(['/', '\\']).find(|part| !part.is_empty()).unwrap_or(place)
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(RECENTS, self.recent.join("\n"));
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
        let Self {
            shown,
            recent,
            url,
            elf_url,
            ..
        } = self;
        let mut files = |ui: &mut egui::Ui| files(ui, recent, url, &mut request);
        let asked = match shown {
            Shown::Timeline(timeline) => {
                timeline.ui(ui, frame, &mut files);
                None
            }
            Shown::Nothing => empty(ui, &mut files, None),
            Shown::Waiting(waiting) => empty(ui, &mut files, Some((waiting, elf_url))),
        };
        let result = match asked.or(request) {
            Some(Request::Pick(pick)) => {
                self.pick = Some(pick);
                Ok(())
            }
            Some(Request::Open(place)) => self.open(&place, None, frame.storage_mut()),
            Some(Request::Give(place)) => self.give(&place, frame.storage_mut()),
            None => Ok(()),
        };
        self.attempt(result);
        let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
        if hovering {
            drop_hint(&ctx, matches!(self.shown, Shown::Waiting(_)));
        }
    }
}

/// The toolbar's file controls: Open, the URL of a capture, and the recent
/// captures.
fn files(ui: &mut egui::Ui, recent: &[String], url: &mut String, request: &mut Option<Request>) {
    if ui
        .button("Open…")
        .on_hover_text("Open a capture (Ctrl+O), or drop one on the window")
        .clicked()
    {
        *request = Some(Request::Pick(Pick::Capture));
    }
    ui.menu_button("URL", |ui| {
        ui.label("The URL of a capture");
        if let Some(url) = url_field(ui, url, "Open") {
            *request = Some(Request::Open(url));
            ui.close();
        }
    });
    ui.add_enabled_ui(!recent.is_empty(), |ui| {
        ui.menu_button("Recent", |ui| {
            if let Some(place) = recent_list(ui, recent) {
                *request = Some(Request::Open(place));
                ui.close();
            }
        });
    });
}

/// A field to paste an URL in and its button, labelled `go`; the URL, once
/// the button is clicked or Enter pressed.
fn url_field(ui: &mut egui::Ui, url: &mut String, go: &str) -> Option<String> {
    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(url)
                .hint_text("https://…")
                .desired_width(360.0),
        );
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let clicked = ui.add_enabled(!url.trim().is_empty(), egui::Button::new(go)).clicked();
        ((entered || clicked) && !url.trim().is_empty()).then(|| url.trim().to_owned())
    })
    .inner
}

/// The recent captures, a button each; the one clicked.
fn recent_list(ui: &mut egui::Ui, recent: &[String]) -> Option<String> {
    let mut chosen = None;
    for place in recent {
        if ui.button(last(place)).on_hover_text(place).clicked() {
            chosen = Some(place.clone());
        }
    }
    chosen
}

/// The window without a timeline: how to open a capture, or why the capture
/// waiting for its ELF does not find it, with the URL of its ELF as typed;
/// what is asked for the capture waiting.
fn empty(
    ui: &mut egui::Ui,
    files: &mut dyn FnMut(&mut egui::Ui),
    waiting: Option<(&Waiting, &mut String)>,
) -> Option<Request> {
    let mut asked = None;
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
                ui.label("Open… (Ctrl+O), drop a .kcap on the window, paste its URL, or pick a recent one.");
            }
            Some((waiting, elf_url)) => {
                ui.heading(format!("{}: its ELF is not found", waiting.capture));
                ui.label(format!("{}.", waiting.why));
                ui.add_space(8.0);
                if ui.button("Choose the ELF…").clicked() {
                    asked = Some(Request::Pick(Pick::Elf));
                }
                ui.label("the one it was recorded with, drop it on the window, or paste its URL:");
                ui.scope(|ui| {
                    // The field and its button, centred together.
                    ui.set_max_width(440.0);
                    if let Some(url) = url_field(ui, elf_url, "Fetch") {
                        asked = Some(Request::Give(url));
                    }
                });
            }
        });
    });
    asked
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
