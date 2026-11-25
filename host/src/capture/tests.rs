use super::*;

/// A record as text, to compare.
fn describe(record: &Record) -> String {
    match *record {
        Record::Pass { host_ns, write_index } => format!("pass {host_ns} {write_index}"),
        Record::Event { index, slots } => format!("event {index} {slots:?}"),
        Record::Lost { from, to } => format!("lost {from} {to}"),
        Record::Suspect { from, to, writers } => format!("suspect {from} {to} {writers}"),
        Record::End { next } => format!("end {next}"),
    }
}

/// A capture of each kind of record, with the byte each record ends at and
/// the index after the last event or loss up to it.
fn capture(name: &str, elf: Elf) -> (Vec<u8>, usize, Vec<(usize, u32, String)>) {
    let path = std::env::temp_dir().join(format!("kymo-capture-{name}-{}.kcap", std::process::id()));
    let header = Header {
        first: 5,
        clock: Some(1_000_000),
        channel: "kymo::ring".into(),
        elf,
    };
    let slots: Vec<u8> = (0..48).collect();
    let records = [
        (
            Record::Pass {
                host_ns: 1,
                write_index: 9,
            },
            5,
        ),
        (
            Record::Event {
                index: 7,
                slots: &slots[..32],
            },
            9,
        ),
        (Record::Lost { from: 9, to: 12 }, 12),
        (
            Record::Suspect {
                from: 9,
                to: 12,
                writers: 1,
            },
            12,
        ),
        (
            Record::Pass {
                host_ns: 2,
                write_index: 13,
            },
            12,
        ),
        (
            Record::Event {
                index: 12,
                slots: &slots[32..],
            },
            13,
        ),
        (Record::End { next: 13 }, 13),
    ];
    let mut writer = Writer::create(&path, &header).unwrap();
    writer.flush().unwrap();
    let header_bytes = std::fs::metadata(&path).unwrap().len() as usize;
    let mut ends = Vec::new();
    for (record, next) in &records {
        writer.write(record).unwrap();
        writer.flush().unwrap();
        ends.push((
            std::fs::metadata(&path).unwrap().len() as usize,
            *next,
            describe(record),
        ));
    }
    writer.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    (bytes, header_bytes, ends)
}

#[test]
fn a_capture_cut_anywhere_reads_up_to_its_last_whole_record() {
    let (bytes, header_bytes, ends) = capture("cut", Elf::Embedded(vec![0x7f, b'E', b'L', b'F']));
    for length in 0..=bytes.len() {
        let read = read(&bytes[..length]);
        if length < header_bytes {
            assert!(read.is_err(), "cut at {length}, inside the header");
            continue;
        }
        let capture = read.unwrap_or_else(|error| panic!("cut at {length}: {error}"));
        assert_eq!((capture.header.first, capture.header.clock), (5, Some(1_000_000)));
        assert_eq!(capture.header.channel, "kymo::ring");
        let whole: Vec<_> = ends.iter().filter(|(end, ..)| *end <= length).collect();
        let records: Vec<_> = capture.records.iter().map(describe).collect();
        let expected: Vec<_> = whole.iter().map(|(.., text)| text.clone()).collect();
        assert_eq!(records, expected, "cut at {length}");
        let cut = if length == bytes.len() {
            None
        } else {
            let (at, next) = whole.last().map_or((header_bytes, 5), |(end, next, _)| (*end, *next));
            Some(Cut { at, next })
        };
        assert_eq!(capture.cut, cut, "cut at {length}");
    }
}

#[test]
fn an_unknown_record_tag_is_an_error() {
    let (mut bytes, header_bytes, _) = capture("tag", Elf::Reference("/images/board.elf".into()));
    bytes[header_bytes] = 6;
    assert!(read(&bytes).is_err());
}

#[test]
fn the_header_holds_the_elf_embedded_or_named() {
    for elf in [
        Elf::Embedded((0..=255).collect()),
        Elf::Reference("D:\\images\\board.elf".into()),
        Elf::Reference("https://example.com/builds/42/board.elf".into()),
    ] {
        let path = std::env::temp_dir().join(format!("kymo-capture-elf-{}.kcap", std::process::id()));
        let written = Header {
            first: 5,
            clock: None,
            channel: "kymo::ring".into(),
            elf,
        };
        Writer::create(&path, &written).unwrap().finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let read = header(&bytes).unwrap();
        assert_eq!((read.first, read.clock, &read.channel), (5, None, &written.channel));
        assert_eq!(read.elf, written.elf);
        assert!(super::read(&bytes).unwrap().records.is_empty());
    }
}

/// A file of the temporary directory holding `bytes`, its path as text.
fn file(name: &str, bytes: &[u8]) -> String {
    let path = std::env::temp_dir().join(format!("kymo-capture-{name}-{}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn the_elf_comes_from_the_capture_unless_given() {
    let embedded = capture("embedded", Elf::Embedded(b"embedded".to_vec())).0;
    let (elf, name) = super::elf("run.kcap", &embedded, None).unwrap();
    assert_eq!(
        (elf.as_slice(), name.as_str()),
        (&b"embedded"[..], "the ELF in run.kcap")
    );

    let given = file("given.elf", b"given");
    assert_eq!(
        super::elf("run.kcap", &embedded, Some(&given)).unwrap(),
        (b"given".to_vec(), given.clone())
    );

    let named = file("named.elf", b"named");
    let referenced = capture("referenced", Elf::Reference(named.clone())).0;
    assert_eq!(
        super::elf("run.kcap", &referenced, None).unwrap(),
        (b"named".to_vec(), named.clone())
    );
    assert_eq!(super::elf("run.kcap", &referenced, Some(&given)).unwrap().0, b"given");

    std::fs::remove_file(&named).unwrap();
    let missing = super::elf("run.kcap", &referenced, None).unwrap_err().to_string();
    assert!(missing.contains(&named) && missing.contains("--elf"), "{missing}");

    let ring = super::elf("ring.bin", &[0; 64], None).unwrap_err().to_string();
    assert!(ring.contains("--elf"), "{ring}");
    assert_eq!(super::elf("ring.bin", &[0; 64], Some(&given)).unwrap().0, b"given");
    std::fs::remove_file(&given).unwrap();
}

#[test]
fn an_elf_at_an_url_is_fetched() {
    let server = crate::served::serve(vec![("/board.elf".into(), b"served".to_vec())]);
    let at = format!("{server}/board.elf");
    let gone = format!("{server}/gone.elf");
    let embedded = capture("fetched", Elf::Embedded(b"embedded".to_vec())).0;
    assert_eq!(
        super::elf("run.kcap", &embedded, Some(&at)).unwrap(),
        (b"served".to_vec(), at.clone())
    );

    let referenced = capture("remote", Elf::Reference(at.clone())).0;
    assert_eq!(
        super::elf("run.kcap", &referenced, None).unwrap(),
        (b"served".to_vec(), at.clone())
    );

    let missing = capture("missing", Elf::Reference(gone.clone())).0;
    let missing = super::elf("run.kcap", &missing, None).unwrap_err().to_string();
    assert!(
        missing.contains(&gone) && missing.contains("404") && missing.contains("--elf"),
        "{missing}"
    );
    let given = super::elf("run.kcap", &embedded, Some(&gone)).unwrap_err().to_string();
    assert!(given.contains(&gone) && given.contains("404"), "{given}");
}

#[test]
fn a_file_at_an_url_is_fetched_whole_however_large() {
    let large: Vec<u8> = (0..12 << 20).map(|n: u32| n as u8).collect();
    let server = crate::served::serve(vec![("/large.elf".into(), large.clone())]);
    assert!(fetch(&format!("{server}/large.elf")).unwrap() == large);
}

#[test]
fn a_reference_names_a_file_by_its_absolute_path() {
    let url = "http://example.com/builds/42/board.elf";
    assert_eq!(reference(url).unwrap(), url);
    let relative = reference(&format!("build{}board.elf", std::path::MAIN_SEPARATOR)).unwrap();
    let expected = std::env::current_dir().unwrap().join("build").join("board.elf");
    assert_eq!(relative, expected.to_string_lossy());
    assert!(std::path::Path::new(&relative).is_absolute());
}
