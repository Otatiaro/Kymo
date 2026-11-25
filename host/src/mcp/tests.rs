use super::*;
use crate::capture::{Elf, Header, Record, Writer};
use crate::events::{Event, Field, Kind, Ring, Type};
use crate::{check, decode, record, stats};
use std::sync::atomic::{AtomicUsize, Ordering};

const SLOTS: u32 = 64;
/// Cycles between two indexes; the clock is 1 MHz, so 100 µs.
const STEP: u32 = 100;
const CLOCK: u64 = 1_000_000;

fn event(id: u16, name: &str, size: u64, fields: Vec<Field>) -> (u16, Event) {
    let event = Event {
        id,
        name: name.to_owned(),
        size,
        fields,
        text: None,
    };
    (id, event)
}

fn field(name: &str, offset: u64, ty: &str, size: u64, encoding: gimli::DwAte) -> Field {
    Field {
        name: name.to_owned(),
        offset,
        ty: Type {
            name: ty.to_owned(),
            size: Some(size),
            kind: Kind::Base(encoding),
        },
    }
}

/// `app::al_status` (1) with `code` and `delta`, and the edges of the
/// duration `app::work` (2 and 3).
fn catalogue() -> Catalogue {
    Catalogue {
        events: [
            event(
                1,
                "app::al_status",
                3,
                vec![
                    field("code", 0, "uint16_t", 2, gimli::DW_ATE_unsigned),
                    field("delta", 2, "int8_t", 1, gimli::DW_ATE_signed),
                ],
            ),
            event(2, "kymo::begin_of<app::work>", 0, Vec::new()),
            event(3, "kymo::end_of<app::work>", 0, Vec::new()),
        ]
        .into_iter()
        .collect(),
        rings: vec![Ring {
            name: "trace_ring".into(),
            address: 0x2000_0000,
            slots: SLOTS,
            size: 16 + u64::from(SLOTS) * 16,
            write_index_offset: 0,
            lapped_offset: 4,
            slots_offset: 16,
        }],
        symbols: Default::default(),
        interrupts: Default::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: vec!["a warning of the image".to_owned()],
    }
}

fn parse(_: &[u8], _: &str, _: Option<&Path>) -> target::Result<Catalogue> {
    Ok(catalogue())
}

/// What a test capture holds.
enum Item {
    /// An event at its index: begin_of, al_status or end_of by the index
    /// modulo 3, al_status's code the index and its delta -1.
    Event(u32),
    Lost(u32, u32),
    Suspect(u32, u32, u32),
}

/// A capture from index 0: events 0 to 29, a loss from 30 to 35, events 35 to
/// 59 with a suspect line of 40 to 41 after event 41, ended at 60.
fn items() -> Vec<Item> {
    let mut items: Vec<Item> = (0..30).map(Item::Event).collect();
    items.push(Item::Lost(30, 35));
    for index in 35..60 {
        items.push(Item::Event(index));
        if index == 41 {
            items.push(Item::Suspect(40, 41, 1));
        }
    }
    items
}

/// A capture file of its own for each test.
fn path() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("kymo-mcp-{}-{n}.kcap", std::process::id()))
}

fn write(path: &Path, items: &[Item], end: u32) {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let header = Header {
        first: 0,
        clock: Some(CLOCK),
        channel: ring.name.clone(),
        elf: Elf::Embedded(b"not read: the catalogue is the test's".to_vec()),
    };
    let mut writer = Writer::create(path, &header).unwrap();
    writer
        .write(&Record::Pass {
            host_ns: 0,
            write_index: 0,
        })
        .unwrap();
    for item in items {
        let record = match *item {
            Item::Event(index) => {
                let type_id = [2u32, 1, 3][(index % 3) as usize];
                let head = type_id | 1 << 16 | decode::lap(index, ring) << 17;
                let mut slot = head.to_le_bytes().to_vec();
                slot.extend((index * STEP).to_le_bytes());
                slot.extend((index as u16).to_le_bytes());
                slot.push(-1i8 as u8);
                slot.resize(16, 0);
                writer.write(&Record::Event { index, slots: &slot }).unwrap();
                continue;
            }
            Item::Lost(from, to) => Record::Lost { from, to },
            Item::Suspect(from, to, writers) => Record::Suspect { from, to, writers },
        };
        writer.write(&record).unwrap();
    }
    writer
        .write(&Record::Pass {
            host_ns: 1_000_000,
            write_index: end,
        })
        .unwrap();
    writer.write(&Record::End { next: end }).unwrap();
    writer.finish().unwrap();
}

/// A test capture written, removed when dropped.
struct File(PathBuf);

impl File {
    fn new() -> Self {
        let path = path();
        write(&path, &items(), 60);
        File(path)
    }

    fn name(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn server() -> Server {
    Server::new(parse)
}

/// The response to `message`, parsed.
fn ask(server: &mut Server, message: Value) -> Value {
    serde_json::from_str(&server.message(&message.to_string()).expect("a response")).unwrap()
}

/// The text of a tool's result, and whether it is an error.
fn call(server: &mut Server, tool: &str, arguments: Value) -> (String, bool) {
    let response = ask(
        server,
        json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": tool, "arguments": arguments } }),
    );
    let result = &response["result"];
    assert_eq!(result["content"][0]["type"], "text", "{response}");
    (
        result["content"][0]["text"].as_str().unwrap().to_owned(),
        result["isError"].as_bool().unwrap(),
    )
}

fn text(server: &mut Server, tool: &str, arguments: Value) -> String {
    let (text, error) = call(server, tool, arguments);
    assert!(!error, "{tool}: {text}");
    text
}

/// The lines `kymo decode` prints for the elements of the capture.
fn decoded(file: &File) -> Vec<String> {
    let catalogue = catalogue();
    let bytes = std::fs::read(&file.0).unwrap();
    let trace = Trace::read(&catalogue, &bytes, None).unwrap();
    trace
        .elements
        .iter()
        .map(|element| record::line(&catalogue.symbols, element))
        .collect()
}

fn error_code(response: &Value) -> i64 {
    response["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("an error: {response}"))
}

#[test]
fn the_protocol_answers_requests_and_drops_notifications() {
    let mut server = server();
    let initialize = |version: &str| {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": version, "capabilities": {},
                            "clientInfo": { "name": "test", "version": "0" } } })
    };
    let response = ask(&mut server, initialize("2025-03-26"));
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(response["result"]["serverInfo"]["name"], "kymo");
    assert!(response["result"]["capabilities"]["tools"].is_object());
    let response = ask(&mut server, initialize("1999-01-01"));
    assert_eq!(response["result"]["protocolVersion"], PROTOCOL);

    let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert_eq!(server.message(&notification.to_string()), None);
    let response = json!({ "jsonrpc": "2.0", "id": 3, "result": {} });
    assert_eq!(server.message(&response.to_string()), None);

    let response = ask(&mut server, json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" }));
    assert_eq!((&response["id"], &response["result"]), (&json!("a"), &json!({})));

    let response = ask(
        &mut server,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let names: Vec<_> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "summary",
            "find",
            "events",
            "context",
            "subject",
            "intervals",
            "stats",
            "compare",
            "check",
            "fault"
        ]
    );
    for tool in response["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["required"][0], "capture", "{tool}");
    }

    let unknown = ask(
        &mut server,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }),
    );
    assert_eq!((error_code(&unknown), &unknown["id"]), (-32601, &json!(4)));
    let response: Value = serde_json::from_str(&server.message("{ not json").unwrap()).unwrap();
    assert_eq!((error_code(&response), &response["id"]), (-32700, &Value::Null));
    let response: Value = serde_json::from_str(&server.message("[1, 2]").unwrap()).unwrap();
    assert_eq!(error_code(&response), -32600);
    let response = ask(&mut server, json!({ "jsonrpc": "2.0", "id": 5 }));
    assert_eq!((error_code(&response), &response["id"]), (-32600, &json!(5)));
    let response = ask(
        &mut server,
        json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": { "name": "decode" } }),
    );
    assert_eq!(error_code(&response), -32602);
    let response = ask(
        &mut server,
        json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": { "name": "summary", "arguments": [1] } }),
    );
    assert_eq!(error_code(&response), -32602);
}

#[test]
fn a_tool_that_fails_answers_an_error_and_the_server_goes_on() {
    let file = File::new();
    let mut server = server();
    let failures = [
        ("summary", json!({})),
        ("summary", json!({ "capture": "D:/no/such/capture.kcap" })),
        ("summary", json!({ "capture": file.name(), "verbose": true })),
        ("summary", json!({ "capture": 5 })),
        ("events", json!({ "capture": file.name(), "from": "12 parsecs" })),
        (
            "events",
            json!({ "capture": file.name(), "around": "#3", "from": "#1" }),
        ),
        ("events", json!({ "capture": file.name(), "before": 3 })),
        ("events", json!({ "capture": file.name(), "limit": 0 })),
        ("events", json!({ "capture": file.name(), "filter": "code=x" })),
        ("find", json!({ "capture": file.name() })),
        ("find", json!({ "capture": file.name(), "filter": "-" })),
        ("stats", json!({ "capture": file.name(), "subjects": ["app::play"] })),
        ("check", json!({ "capture": file.name(), "rules": 3 })),
        (
            "check",
            json!({ "capture": file.name(), "rules": { "rules": [{ "subject": "x" }] } }),
        ),
    ];
    for (tool, arguments) in failures {
        let (answer, error) = call(&mut server, tool, arguments.clone());
        assert!(error, "{tool} {arguments}: {answer}");
        assert!(!answer.is_empty());
    }
    let not_a_capture = path();
    std::fs::write(&not_a_capture, b"kymo").unwrap();
    let (answer, error) = call(&mut server, "summary", json!({ "capture": not_a_capture }));
    let _ = std::fs::remove_file(&not_a_capture);
    assert!(error && answer.contains("not a capture"), "{answer}");
    assert!(text(&mut server, "summary", json!({ "capture": file.name() })).contains("events 55,"));
}

#[test]
fn events_gives_the_lines_of_decode() {
    let file = File::new();
    let mut server = server();
    let lines = text(&mut server, "events", json!({ "capture": file.name() }));
    assert_eq!(lines.lines().collect::<Vec<_>>(), decoded(&file));
    assert!(
        lines.contains("lost 30 35") && lines.contains("suspect 40 41 1"),
        "{lines}"
    );
}

/// Going on from where each response stops gives each line once, also where
/// the cut falls between a suspect line and the event at its index.
#[test]
fn a_response_cut_goes_on_without_a_gap_or_a_line_twice() {
    let file = File::new();
    let mut server = server();
    let all = decoded(&file);
    for limit in 1..=12 {
        let mut read = Vec::new();
        let mut from = "#0".to_owned();
        for _ in 0..100 {
            let response = text(
                &mut server,
                "events",
                json!({ "capture": file.name(), "from": from, "limit": limit }),
            );
            let mut lines: Vec<&str> = response.lines().collect();
            let last = lines.last().copied().unwrap();
            let Some((_, rest)) = last.split_once("the rest with from=") else {
                read.extend(lines.iter().map(|line| line.to_string()));
                break;
            };
            lines.pop();
            // Past the limit only to keep a suspect line with its event.
            assert!(lines.len() <= limit.max(2) && !lines.is_empty(), "{response}");
            assert!(last.starts_with(&format!("{} of ", lines.len())), "{last}");
            read.extend(lines.iter().map(|line| line.to_string()));
            from = rest.to_owned();
        }
        assert_eq!(read, all, "limit {limit}");
    }
}

#[test]
fn a_range_includes_from_and_excludes_to_by_index_or_time() {
    let file = File::new();
    let mut server = server();
    let all = decoded(&file);
    let range = |server: &mut Server, from: &str, to: &str| {
        text(
            server,
            "events",
            json!({ "capture": file.name(), "from": from, "to": to }),
        )
    };
    let by_index = range(&mut server, "#3", "#6");
    assert_eq!(by_index.lines().collect::<Vec<_>>(), all[3..6]);
    assert_eq!(range(&mut server, "300us", "0.6ms"), by_index);
    // The loss stands at its first index, before event 35.
    let around_loss = range(&mut server, "#30", "#36");
    assert_eq!(around_loss.lines().collect::<Vec<_>>(), all[30..32]);
    let to = range(&mut server, "#58", "#40");
    assert_eq!(to, "nothing in this range");
}

#[test]
fn around_gives_the_lines_on_each_side() {
    let file = File::new();
    let mut server = server();
    let all = decoded(&file);
    let around = text(
        &mut server,
        "events",
        json!({ "capture": file.name(), "around": "#10", "before": 2, "after": 1 }),
    );
    assert_eq!(around.lines().collect::<Vec<_>>(), all[8..12]);
    let filtered = text(
        &mut server,
        "events",
        json!({ "capture": file.name(), "around": "#10", "before": 1, "after": 1, "filter": "al_status" }),
    );
    let expected: Vec<_> = all.iter().filter(|line| line.contains("al_status")).collect();
    let at = expected.iter().position(|line| line.contains("code=10")).unwrap();
    assert_eq!(filtered.lines().collect::<Vec<_>>(), expected[at - 1..=at + 1]);
}

#[test]
fn find_counts_and_lists_the_events_its_filter_keeps() {
    let file = File::new();
    let mut server = server();
    let all = decoded(&file);
    let found = text(
        &mut server,
        "find",
        json!({ "capture": file.name(), "filter": "code=0x2b" }),
    );
    let lines: Vec<_> = found.lines().collect();
    assert_eq!(lines[0], "1 event matches");
    assert_eq!(lines[1..], [all.iter().find(|line| line.contains("code=43")).unwrap()]);

    let found = text(
        &mut server,
        "find",
        json!({ "capture": file.name(), "filter": "+al_status -code=4", "limit": 3 }),
    );
    let lines: Vec<_> = found.lines().collect();
    assert_eq!(lines[0], "17 events match");
    assert_eq!(lines[1..4], [&all[1], &all[7], &all[10]]);
    assert_eq!(lines[4], "3 of 17 lines; the rest with from=#13");

    let found = text(
        &mut server,
        "find",
        json!({ "capture": file.name(), "filter": "end_of", "from": "#3", "to": "#9" }),
    );
    assert_eq!(found.lines().collect::<Vec<_>>(), ["2 events match", &all[5], &all[8]]);
}

#[test]
fn a_capture_rewritten_is_read_again() {
    let file = File::new();
    let mut server = server();
    assert!(text(&mut server, "summary", json!({ "capture": file.name() })).contains("indexes #0 to #60"));
    let fewer: Vec<Item> = (0..12).map(Item::Event).collect();
    write(&file.0, &fewer, 12);
    let summary = text(&mut server, "summary", json!({ "capture": file.name() }));
    assert!(summary.contains("events 12, slots 12,"), "{summary}");
    assert!(summary.contains("indexes #0 to #12"), "{summary}");
}

#[test]
fn summary_tells_what_the_capture_holds() {
    let file = File::new();
    let mut server = server();
    let summary = text(&mut server, "summary", json!({ "capture": file.name() }));
    for line in [
        "events 55, slots 55, slots lost 5, losses 1, suspect lines 1, suspect writers 1",
        "indexes #0 to #60, where recording stopped",
        "5.90 ms from the first event to the last",
        "frequency 1000000 Hz",
        "kernel: no adapter events",
        "durations: app::work",
        "  19 kymo::end_of<app::work>",
        "  18 app::al_status",
        "  18 kymo::begin_of<app::work>",
        "warning: a warning of the image",
    ] {
        assert!(summary.lines().any(|candidate| candidate == line), "{line}\n{summary}");
    }
}

/// `stats` is `kymo stats --json`, `check` `kymo check`, over the same range.
#[test]
fn stats_and_check_give_what_the_command_line_prints() {
    let file = File::new();
    let mut server = server();
    let catalogue = catalogue();
    let bytes = std::fs::read(&file.0).unwrap();
    let trace = Trace::read(&catalogue, &bytes, None).unwrap();
    let report = |from, to| stats::of_trace(&catalogue, &trace, stats::Range { from, to }, 10).unwrap();

    let whole = text(&mut server, "stats", json!({ "capture": file.name() }));
    assert_eq!(whole, stats::json(&report(None, None)));
    let range = text(
        &mut server,
        "stats",
        json!({ "capture": file.name(), "from": "#6", "to": "2ms" }),
    );
    assert_eq!(range, stats::json(&report(Some(600), Some(2000))));
    assert_ne!(range, whole);
    let subjects = text(
        &mut server,
        "stats",
        json!({ "capture": file.name(), "subjects": ["app::work"] }),
    );
    assert_eq!(subjects, whole);

    let rules = json!({ "rules": [
        { "subject": "app::work", "metric": "duration", "stat": "max", "max": "150us" },
        { "subject": "app::work", "metric": "duration", "stat": "max", "max": "250us" },
    ] });
    let judged = text(&mut server, "check", json!({ "capture": file.name(), "rules": rules }));
    let (read, _) = check::rules(&rules.to_string()).unwrap();
    let verdicts = check::judge(&report(None, None), &read, None).unwrap();
    let mut expected: Vec<String> = verdicts.iter().map(|verdict| verdict.line.clone()).collect();
    expected.push(check::summary(&verdicts));
    assert_eq!(judged, expected.join("\n"));
    assert_eq!(check::code(&verdicts), 3, "{judged}");

    let written = path().with_extension("json");
    std::fs::write(&written, rules.to_string()).unwrap();
    let from_file = text(
        &mut server,
        "check",
        json!({ "capture": file.name(), "rules": written }),
    );
    let _ = std::fs::remove_file(&written);
    assert_eq!(from_file, judged);
}

#[test]
fn instants_read_an_index_or_a_time_with_its_unit() {
    assert_eq!(Instant::parse("#12"), Ok(Instant::Index(12)));
    assert_eq!(Instant::parse(" 2s "), Ok(Instant::Seconds(2.0)));
    assert_eq!(Instant::parse("12.5ms"), Ok(Instant::Seconds(0.0125)));
    assert_eq!(Instant::parse("3 us"), Ok(Instant::Seconds(3e-6)));
    assert_eq!(Instant::parse("3µs"), Ok(Instant::Seconds(3e-6)));
    assert_eq!(Instant::parse("40ns"), Ok(Instant::Seconds(40e-9)));
    for wrong in ["12", "#-1", "#", "ms", "inf s", "1e999s", "3 parsecs"] {
        assert!(Instant::parse(wrong).is_err(), "{wrong}");
    }
}

/// `context` gives the event an instant falls on or after, and what is open
/// there: here no kernel, a duration open from its begin_of to its end_of.
#[test]
fn context_tells_what_is_open_at_an_instant() {
    let file = File::new();
    let mut server = server();
    let all = decoded(&file);
    let at = |server: &mut Server, at: &str| text(server, "context", json!({ "capture": file.name(), "at": at }));
    let open = at(&mut server, "#19");
    assert_eq!(
        open.lines().collect::<Vec<_>>(),
        [
            format!("after {}", all[19]),
            "kernel: the image has no kernel adapter events".to_owned(),
            "duration app::work, lane 0, since #18, 100 (100 µs) ago".to_owned(),
        ]
    );
    // An instant falls on the event at its cycles, or after the one before.
    assert_eq!(at(&mut server, "1.9ms"), open);
    assert_eq!(at(&mut server, "1.95ms"), open);
    let closed = at(&mut server, "#20");
    assert_eq!(closed.lines().next(), Some(format!("after {}", all[20]).as_str()));
    assert!(!closed.contains("duration"), "{closed}");
}

/// `intervals` gives `intervals::of` over the capture: begin_of to end_of,
/// one excluded by the suspect line, over a range or the whole.
#[test]
fn intervals_measures_from_an_event_to_the_next_of_another() {
    let file = File::new();
    let mut server = server();
    let whole = text(
        &mut server,
        "intervals",
        json!({ "capture": file.name(), "a": "begin_of", "b": "end_of", "worst": 2 }),
    );
    assert_eq!(
        whole.lines().collect::<Vec<_>>(),
        [
            "17 intervals; 0 A without a B before the next A; excluded 0 by a loss, 1 by a suspect line; 0 cut",
            "min 200 (200 µs) at #0",
            "p50 200 (200 µs)",
            "p90 200 (200 µs)",
            "p99 200 (200 µs)",
            "max 200 (200 µs) at #0",
            "mean 200.0 cycles (200 µs)",
            "worst:",
            "  #0 to #2: 200 (200 µs)",
            "  #3 to #5: 200 (200 µs)",
        ]
    );
    let range = text(
        &mut server,
        "intervals",
        json!({ "capture": file.name(), "a": "begin_of", "b": "end_of", "from": "#5", "to": "1.2ms" }),
    );
    assert!(
        range.starts_with(
            "2 intervals; 0 A without a B before the next A; excluded 0 by a loss, 0 by a suspect line; 1 cut\n"
        ),
        "{range}"
    );
    let (error, failed) = call(
        &mut server,
        "intervals",
        json!({ "capture": file.name(), "a": "begin_of", "b": " " }),
    );
    assert!(failed);
    assert_eq!(error, "a and b each need a word");
}

/// `compare` sets each group of one capture's stats against the same group of
/// the other's, and gives other minus capture; a group one lacks has null.
#[test]
fn compare_gives_each_group_of_two_captures_and_their_difference() {
    let file = File::new();
    let fewer = File(path());
    write(&fewer.0, &(0..12).map(Item::Event).collect::<Vec<_>>(), 12);
    let statuses = File(path());
    write(&statuses.0, &[Item::Event(1), Item::Event(4), Item::Event(7)], 8);
    let mut server = server();
    let mut compare = |other: &File, subjects: Option<&[&str]>| -> Value {
        let mut arguments = json!({ "capture": file.name(), "other": other.name() });
        if let Some(subjects) = subjects {
            arguments["subjects"] = json!(subjects);
        }
        serde_json::from_str(&text(&mut server, "compare", arguments)).unwrap()
    };

    let same = compare(&file, None);
    assert_eq!(same["capture"], same["other"]);
    let groups = same["groups"].as_array().unwrap();
    assert!(!groups.is_empty());
    for group in groups {
        assert_eq!(group["capture"], group["other"], "{group}");
        for (field, difference) in group["difference"].as_object().unwrap() {
            assert!(
                difference.is_null() || difference.as_f64() == Some(0.0),
                "{field}: {group}"
            );
        }
    }

    let whole = &groups.iter().find(|group| group["subject"] == "app::work").unwrap()["capture"];
    let compared = compare(&fewer, Some(&["app::work"]));
    let groups = compared["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{compared}");
    let other = &groups[0]["other"];
    assert_eq!((other["count"].as_i64(), other["p50"].as_i64()), (Some(4), Some(200)));
    assert_eq!((other["min_at"].as_i64(), other["max_at"].as_i64()), (Some(0), Some(0)));
    let difference = &groups[0]["difference"];
    assert_eq!(difference["count"].as_i64(), Some(4 - whole["count"].as_i64().unwrap()));
    assert_eq!(difference["p50"].as_i64(), Some(200 - whole["p50"].as_i64().unwrap()));
    assert_eq!(compared["other"]["losses"], 0);
    assert_eq!(compared["capture"]["losses"], 1);

    let lacking = compare(&statuses, Some(&["app::work"]));
    let group = &lacking["groups"][0];
    assert_eq!(group["capture"]["count"], whole["count"]);
    assert!(group["other"].is_null() && group["difference"].is_null(), "{group}");
    let arguments = json!({
        "capture": statuses.name(), "other": file.name(), "other_clock": 2_000_000, "subjects": ["app::work"]
    });
    let lacked: Value = serde_json::from_str(&text(&mut server, "compare", arguments)).unwrap();
    let group = &lacked["groups"][0];
    assert_eq!(group["other"]["count"], whole["count"]);
    assert!(group["capture"].is_null() && group["difference"].is_null(), "{group}");
    assert_eq!(lacked["other"]["frequency"], 2_000_000.0);
    assert_ne!(lacked["capture"]["frequency"], 2_000_000.0);

    let (error, failed) = call(
        &mut server,
        "compare",
        json!({ "capture": file.name(), "other": statuses.name(), "subjects": ["app::rest"] }),
    );
    assert!(failed);
    assert_eq!(error, "no group of app::rest in either capture");
}

/// `fault` decodes the fault it is asked for, gives the state after it and
/// the lines before it; a capture without one, or a number past the last,
/// is an error.
#[test]
fn fault_gives_the_fault_its_state_and_what_came_before() {
    let capture = File(path());
    let mut thread = crate::opsy::tests::fault(0xffff_fffd, 0);
    thread[4] = 0x1234;
    let mut handler = crate::opsy::tests::fault(0xffff_fff1, 45);
    handler[3] = 0x4000_0008;
    write_kernel_from(
        &capture.0,
        5000,
        &[
            ("task_ready", &[TASK, NOTIFIED]),
            ("task_started", &[TASK]),
            ("fault", &thread),
            ("isr_enter", &[HANDLER, 45]),
            ("fault", &handler),
        ],
    );
    let mut server = Server::new(kernel_parse);
    let first = text(&mut server, "fault", json!({ "capture": capture.name(), "before": 1 }));
    assert_eq!(
        first.lines().collect::<Vec<_>>(),
        [
            "fault 1 of 2 at #2, 200 (200 µs) after the first event",
            "task: 0x20000100",
            "bits: PRECISERR,BFARVALID",
            "bfar: 0x40000000",
            "pc: 0x08000124",
            "lr: 0x00000000",
            "stack: psp",
            "sp: 0x00000000",
            "registers: r0=0x00001234 r1=0x00000000 r2=0x00000000 r3=0x00000000 r12=0x00000000 \
lr=0x00000000 pc=0x08000124 xpsr=0x01000000 cfsr=0x00008200 hfsr=0x00000000 exc_return=0xfffffffd",
            "context:",
            "  core: 0x20000100 since #1, 100 (100 µs) ago",
            "  handlers open: none",
            "  on the core: 0x20000100, woken by notified, cause unknown, since #0, 200 (200 µs) ago",
            "  last wake of 0x20000100: by notified, cause unknown, at #0, 200 (200 µs) ago",
            "the 1 lines before it:",
            "  event 1 5100 0.000100000 kymo::opsy::task_started task=0x20000100",
        ]
    );
    let second = text(&mut server, "fault", json!({ "capture": capture.name(), "number": 2 }));
    let lines: Vec<&str> = second.lines().collect();
    assert_eq!(
        lines[0], "fault 2 of 2 at #12, 1200 (1.20 ms) after the first event",
        "{second}"
    );
    assert_eq!(lines[1], "handler: 0x08000401 (IRQ29)", "{second}");
    assert_eq!(lines[3], "bfar: 0x40000008", "{second}");
    assert!(second.contains("\nthe 4 lines before it:\n"), "{second}");
    assert!(
        second.contains("\n  handler 0x08000401 (IRQ29), depth 0, since #11"),
        "{second}"
    );

    let alone = text(&mut server, "fault", json!({ "capture": capture.name(), "before": 0 }));
    assert!(alone.ends_with("\nnothing before it"), "{alone}");

    let (error, failed) = call(&mut server, "fault", json!({ "capture": capture.name(), "number": 3 }));
    assert!(failed);
    assert_eq!(error, "number is 1 to 2");
    let (error, failed) = call(&mut server, "fault", json!({ "capture": capture.name(), "number": 0 }));
    assert!(failed);
    assert_eq!(error, "number is 1 to 2");
    let none = File(path());
    write_kernel(&none.0, &[("task_started", &[TASK])]);
    let (error, failed) = call(&mut server, "fault", json!({ "capture": none.name() }));
    assert!(failed);
    assert_eq!(error, "the capture holds no fault");
}

/// The adapter's catalogue of OpSy's tests, with the ring of the test
/// captures.
fn kernel_catalogue() -> Catalogue {
    Catalogue {
        rings: catalogue().rings,
        ..crate::opsy::tests::catalogue()
    }
}

fn kernel_parse(_: &[u8], _: &str, _: Option<&Path>) -> target::Result<Catalogue> {
    Ok(kernel_catalogue())
}

const TASK: u32 = 0x2000_0100;
const OTHER: u32 = 0x2000_0200;
const HANDLER: u32 = 0x0800_0401;
const NOTIFIED: u32 = 0;

/// A capture of the adapter's events `steps`, each a hook (or `fault`) and
/// its fields, at index 0 on, each index `STEP` cycles after the one before,
/// the first at cycle 0.
fn write_kernel(path: &Path, steps: &[(&str, &[u32])]) {
    write_kernel_from(path, 0, steps);
}

/// The capture of `write_kernel`, its first event at cycle `start`.
fn write_kernel_from(path: &Path, start: u32, steps: &[(&str, &[u32])]) {
    let catalogue = kernel_catalogue();
    let ring = &catalogue.rings[0];
    let header = Header {
        first: 0,
        clock: Some(CLOCK),
        channel: ring.name.clone(),
        elf: Elf::Embedded(b"not read: the catalogue is the test's".to_vec()),
    };
    let mut writer = Writer::create(path, &header).unwrap();
    writer
        .write(&Record::Pass {
            host_ns: 0,
            write_index: 0,
        })
        .unwrap();
    let mut index = 0;
    for &(hook, fields) in steps {
        let name = match hook {
            "fault" => crate::fault::EVENT.to_owned(),
            hook => format!("kymo::opsy::{hook}"),
        };
        let event = catalogue.events.values().find(|event| event.name == name).unwrap();
        let count = decode::slots_for(event.size) as u32;
        let mut payload: Vec<u8> = fields.iter().flat_map(|field| field.to_le_bytes()).collect();
        payload.resize(8 + 12 * (count as usize - 1), 0);
        let head = u32::from(event.id) | 1 << 16 | decode::lap(index, ring) << 17;
        let mut slots = head.to_le_bytes().to_vec();
        slots.extend((start + index * STEP).to_le_bytes());
        slots.extend(&payload[..8]);
        for (slot, words) in (1..).zip(payload[8..].chunks(12)) {
            let tail = u32::from(event.id) | decode::lap(index + slot, ring) << 17;
            slots.extend(tail.to_le_bytes());
            slots.extend(words);
        }
        writer.write(&Record::Event { index, slots: &slots }).unwrap();
        index += count;
    }
    let end = index;
    writer
        .write(&Record::Pass {
            host_ns: 1_000_000,
            write_index: end,
        })
        .unwrap();
    writer.write(&Record::End { next: end }).unwrap();
    writer.finish().unwrap();
}

/// `subject` lists a task's activations and a handler's runs, each with its
/// metrics; a name neither is an error that names them.
#[test]
fn subject_lists_a_task_s_activations_and_a_handler_s_runs() {
    let path = path();
    let file = File(path.clone());
    write_kernel(
        &path,
        &[
            ("task_started", &[OTHER]),
            ("isr_enter", &[HANDLER, 61]),
            ("task_ready", &[TASK, NOTIFIED]),
            ("isr_exit", &[HANDLER]),
            ("task_started", &[TASK]),
            ("task_wait", &[TASK]),
            ("task_started", &[OTHER]),
            ("isr_enter", &[HANDLER, 61]),
            ("task_ready", &[TASK, NOTIFIED]),
            ("isr_exit", &[HANDLER]),
            ("task_started", &[TASK]),
            ("task_wait", &[TASK]),
            ("task_started", &[OTHER]),
        ],
    );
    let mut server = Server::new(kernel_parse);
    let subject = |server: &mut Server, arguments: Value| {
        let mut arguments = arguments;
        arguments["capture"] = json!(file.name());
        text(server, "subject", arguments)
    };
    let task = subject(&mut server, json!({ "subject": "0x20000100" }));
    let cause = "woken by notified, cause handler:0x08000401 (TIM2), latency 200 (200 µs), response 300 (300 µs), \
gross 100 (100 µs), net 100 (100 µs), preempted by tasks 0, by handlers 0, ready 200 (200 µs)";
    assert_eq!(
        task.lines().collect::<Vec<_>>(),
        [
            "task 0x20000100: 2 activations, 2 ran; worst latency 200 (200 µs) at #2; worst response 300 (300 µs) at #2"
                .to_owned(),
            format!("#2 {cause}, period 600 (600 µs), ends #5"),
            format!("#8 {cause}, blocked before 300 (300 µs), ends #11"),
        ]
    );
    let window = subject(
        &mut server,
        json!({ "subject": "0x20000100", "from": "#3", "to": "#9" }),
    );
    assert_eq!(
        window.lines().nth(1),
        Some(format!("#8 {cause}, blocked before 300 (300 µs), ends #11").as_str())
    );
    assert_eq!(window.lines().count(), 2, "{window}");

    let runs = [
        "handler 0x08000401 (TIM2): 2 runs; worst gross 200 (200 µs) at #1",
        "#1 gross 200 (200 µs), net 200 (200 µs), depth 0, period 600 (600 µs), ends #3",
        "#7 gross 200 (200 µs), net 200 (200 µs), depth 0, ends #9",
    ];
    for name in ["0x08000401 (TIM2)", "0x08000401"] {
        let handler = subject(&mut server, json!({ "subject": name }));
        assert_eq!(handler.lines().collect::<Vec<_>>(), runs, "{name}");
    }

    let (error, failed) = call(
        &mut server,
        "subject",
        json!({ "capture": file.name(), "subject": "0x0800" }),
    );
    assert!(failed);
    assert_eq!(
        error,
        "no task or handler 0x0800; the tasks are 0x20000200, 0x20000100; the handlers are 0x08000401 (TIM2)"
    );
}

/// Each line read on stdin answered on stdout, blank lines skipped.
#[test]
fn serve_answers_line_by_line() {
    let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n\
{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
    let mut output = Vec::new();
    serve(input.as_bytes(), &mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(
        output,
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n"
    );
}
