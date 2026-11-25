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

/// `path` as the window names a file.
fn at(path: &Path) -> String {
    path.display().to_string()
}

fn shown(app: &App) -> String {
    match &app.shown {
        Shown::Nothing => "nothing".to_owned(),
        Shown::Timeline(timeline) => format!("timeline of {}", timeline.capture()),
        Shown::Waiting(waiting) => format!("{} waits: {}", waiting.capture, waiting.why),
    }
}

#[test]
fn a_capture_dropped_or_at_an_url_shows_as_one_opened_from_the_command_line() {
    let directory = directory("drop");
    let path = directory.join("run.kcap");
    capture(&path, Elf::Embedded(b"elf".to_vec()));
    let mut opened = app();
    opened.open(&at(&path), None, None).unwrap();
    assert_eq!(shown(&opened), "timeline of run.kcap");

    let mut dropped = app();
    drop_on(&mut dropped, &[&path]);
    let server = kymo::served::serve(vec![("/captures/run.kcap".into(), std::fs::read(&path).unwrap())]);
    let mut fetched = app();
    fetched
        .open(&format!("{server}/captures/run.kcap"), None, None)
        .unwrap();
    for other in [&dropped, &fetched] {
        assert_eq!(shown(other), shown(&opened));
        assert_eq!(other.title(), opened.title());
        assert_eq!(other.error, None);
    }
}

#[test]
fn a_capture_whose_elf_is_missing_waits_for_it_dropped() {
    let directory = directory("missing");
    let path = directory.join("run.kcap");
    let elf = directory.join("gone.elf");
    capture(&path, Elf::Reference(at(&elf)));
    let mut app = app();
    drop_on(&mut app, &[&path]);
    let waits = shown(&app);
    assert!(
        waits.starts_with(&format!("{} waits: {}", at(&path), at(&elf))),
        "{waits}"
    );
    // The ELF dropped is read: the capture shows.
    std::fs::write(&elf, b"elf").unwrap();
    drop_on(&mut app, &[&elf]);
    assert_eq!(shown(&app), "timeline of run.kcap");
}

#[test]
fn an_elf_at_an_url_is_fetched_else_waited_for() {
    let directory = directory("url");
    let server = kymo::served::serve(vec![("/app.elf".into(), b"elf".to_vec())]);
    let found = directory.join("found.kcap");
    capture(&found, Elf::Reference(format!("{server}/app.elf")));
    let mut app = app();
    app.open(&at(&found), None, None).unwrap();
    assert_eq!(shown(&app), "timeline of found.kcap");

    let gone = format!("{server}/gone.elf");
    let path = directory.join("run.kcap");
    capture(&path, Elf::Reference(gone.clone()));
    app.open(&at(&path), None, None).unwrap();
    let waits = shown(&app);
    assert!(
        waits.starts_with(&format!("{} waits: {gone}", at(&path))) && waits.contains("404"),
        "{waits}"
    );
    // Its URL given, the ELF is fetched.
    app.give(&format!("{server}/app.elf"), None).unwrap();
    assert_eq!(shown(&app), "timeline of run.kcap");
    // So is the one --elf gives.
    app.open(&at(&path), Some(&format!("{server}/app.elf")), None).unwrap();
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
    app.open(&at(&path), None, None).unwrap();
    drop_on(&mut app, &[&other]);
    assert_eq!(shown(&app), "timeline of run.kcap");
    assert_eq!(app.error, Some(format!("{} is not a Kymo capture", at(&other))));
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
    let paths: Vec<_> = (0..RECENT)
        .map(|n| {
            let path = directory.join(format!("{n}.kcap"));
            capture(&path, Elf::Embedded(b"elf".to_vec()));
            at(&path)
        })
        .collect();
    let server = kymo::served::serve(vec![("/8.kcap".into(), std::fs::read(&paths[0]).unwrap())]);
    let url = format!("{server}/8.kcap");
    let mut app = app();
    for place in paths.iter().chain([&url]) {
        app.open(place, None, None).unwrap();
    }
    // Opened again, a capture comes first, once.
    app.open(&paths[3], None, None).unwrap();
    let mut saved = Saved::default();
    eframe::App::save(&mut app, &mut saved);
    let mut next = self::app();
    next.restore(|key| eframe::Storage::get_string(&saved, key));
    let names: Vec<_> = next.recent.iter().map(|place| last(place)).collect();
    assert_eq!(
        names,
        [
            "3.kcap", "8.kcap", "7.kcap", "6.kcap", "5.kcap", "4.kcap", "2.kcap", "1.kcap"
        ]
    );
    // An URL is kept as it is, a file by its absolute path.
    assert_eq!(next.recent[1], url);
    assert!(Path::new(&next.recent[0]).is_absolute());
}

#[test]
fn the_last_part_of_a_place_names_it() {
    assert_eq!(last("http://host:8080/builds/42/run.kcap"), "run.kcap");
    assert_eq!(last("https://host/builds/run.kcap/"), "run.kcap");
    assert_eq!(last(r"D:\captures\run.kcap"), "run.kcap");
    assert_eq!(last("/captures/run.kcap"), "run.kcap");
}
