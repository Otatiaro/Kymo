use super::*;
use kymo::capture::{Header, Writer};
use kymo::events::Ring;
use std::collections::{BTreeMap, HashMap};

/// A catalogue of one ring, `os`, whatever the ELF.
fn parse(_: &[u8], _: &str, _: Option<&Path>) -> Result<Catalogue> {
    Ok(Catalogue {
        events: BTreeMap::new(),
        rings: vec![Ring {
            name: "os".to_owned(),
            address: 0x2000_0000,
            slots: 16,
            size: 16 * 32 + 16,
            write_index_offset: 0,
            lapped_offset: 4,
            slots_offset: 16,
        }],
        symbols: Default::default(),
        interrupts: Default::default(),
        warnings: Vec::new(),
    })
}

/// A directory of its own for the test `name`.
fn directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("kymo-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

/// A capture of no event at `path`, its ELF `elf`.
fn capture(path: &Path, elf: Elf) {
    let header = Header {
        first: 0,
        clock: None,
        channel: "os".to_owned(),
        elf,
    };
    Writer::create(path, &header).unwrap().finish().unwrap();
}

fn app() -> App {
    App::new(Options::default(), parse)
}

/// Drops `paths` on the window of `app`.
fn drop_on(app: &mut App, paths: &[&Path]) {
    app.dropped(paths.iter().map(|path| path.to_path_buf()).collect(), None);
}

fn shown(app: &App) -> String {
    match &app.shown {
        Shown::Nothing => "nothing".to_owned(),
        Shown::Timeline(timeline) => format!("timeline of {}", timeline.capture()),
        Shown::Waiting(waiting) => format!("{} waits for {}", waiting.capture.display(), waiting.place),
    }
}

#[test]
fn a_capture_dropped_shows_as_one_opened_from_the_command_line() {
    let directory = directory("drop");
    let path = directory.join("run.kcap");
    capture(&path, Elf::Embedded(b"elf".to_vec()));
    let mut opened = app();
    opened.open(&path, None, None).unwrap();
    let mut dropped = app();
    drop_on(&mut dropped, &[&path]);
    assert_eq!(shown(&opened), "timeline of run.kcap");
    assert_eq!(shown(&dropped), shown(&opened));
    assert_eq!(dropped.title(), opened.title());
    assert_eq!(dropped.error, None);
}

#[test]
fn a_capture_whose_elf_is_missing_waits_for_it_dropped() {
    let directory = directory("missing");
    let path = directory.join("run.kcap");
    let elf = directory.join("gone.elf");
    capture(&path, Elf::Reference(elf.display().to_string()));
    let mut app = app();
    drop_on(&mut app, &[&path]);
    assert_eq!(shown(&app), format!("{} waits for {}", path.display(), elf.display()));
    // The ELF dropped is read: the capture shows.
    std::fs::write(&elf, b"elf").unwrap();
    drop_on(&mut app, &[&elf]);
    assert_eq!(shown(&app), "timeline of run.kcap");
}

#[test]
fn a_capture_naming_an_url_waits_for_its_elf() {
    let directory = directory("url");
    let path = directory.join("run.kcap");
    capture(&path, Elf::Reference("https://example.com/app.elf".to_owned()));
    let mut app = app();
    app.open(&path, None, None).unwrap();
    assert_eq!(
        shown(&app),
        format!("{} waits for https://example.com/app.elf", path.display())
    );
    // --elf replaces the reference.
    let elf = directory.join("app.elf");
    std::fs::write(&elf, b"elf").unwrap();
    app.open(&path, Some(&elf), None).unwrap();
    assert_eq!(shown(&app), "timeline of run.kcap");
}

#[test]
fn an_error_leaves_the_timeline_shown() {
    let directory = directory("error");
    let path = directory.join("run.kcap");
    capture(&path, Elf::Embedded(b"elf".to_vec()));
    let other = directory.join("notes.txt");
    std::fs::write(&other, b"not a capture").unwrap();
    let mut app = app();
    app.open(&path, None, None).unwrap();
    drop_on(&mut app, &[&other]);
    assert_eq!(shown(&app), "timeline of run.kcap");
    assert_eq!(app.error, Some(format!("{} is not a Kymo capture", other.display())));
}

#[derive(Default)]
struct Saved(HashMap<String, String>);

impl eframe::Storage for Saved {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }

    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }

    fn flush(&mut self) {}
}

#[test]
fn the_recent_captures_are_kept_for_the_next_runs_the_latest_first() {
    let directory = directory("recent");
    let paths: Vec<_> = (0..RECENT + 1)
        .map(|n| {
            let path = directory.join(format!("{n}.kcap"));
            capture(&path, Elf::Embedded(b"elf".to_vec()));
            path
        })
        .collect();
    let mut app = app();
    for path in &paths {
        app.open(path, None, None).unwrap();
    }
    // Opened again, a capture comes first, once.
    app.open(&paths[3], None, None).unwrap();
    let mut saved = Saved::default();
    eframe::App::save(&mut app, &mut saved);
    let mut next = self::app();
    next.restore(|key| eframe::Storage::get_string(&saved, key));
    let names: Vec<_> = next
        .recent
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "3.kcap", "8.kcap", "7.kcap", "6.kcap", "5.kcap", "4.kcap", "2.kcap", "1.kcap"
        ]
    );
}
