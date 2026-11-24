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
fn capture(name: &str) -> (Vec<u8>, usize, Vec<(usize, u32, String)>) {
    let path = std::env::temp_dir().join(format!("kymo-capture-{name}-{}.kcap", std::process::id()));
    let header = Header {
        first: 5,
        clock: Some(1_000_000),
        channel: "kymo::ring".into(),
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
    let (bytes, header_bytes, ends) = capture("cut");
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
    let (mut bytes, header_bytes, _) = capture("tag");
    bytes[header_bytes] = 6;
    assert!(read(&bytes).is_err());
}
