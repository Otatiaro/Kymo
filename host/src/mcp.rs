//! `kymo mcp`: a Model Context Protocol server on stdio, for an agent to
//! question closed captures (ADR-095). JSON-RPC 2.0, one message per line;
//! tools only: `initialize`, `ping`, `tools/list` and `tools/call` are
//! answered, notifications read and dropped.
//!
//! Each tool takes the path of a capture, and the ELF, clock and SVD file
//! `kymo decode` takes. The captures read are kept, keyed by path, size,
//! modification time and those options: a second call does not read the
//! file again, and a file rewritten is read again. An instant is an event's
//! index, `#1234`, or a time since the first event, `12.5ms`; a range runs
//! from its first instant, included, to its second, excluded. A response
//! holds 200 lines at most unless told otherwise, and says where to go on.

mod tools;

use crate::capture;
use crate::events::{self, Catalogue};
use crate::model::{Element, Trace};
use crate::target;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

/// The protocol version answered to a client that asks for one not listed.
const PROTOCOL: &str = "2025-06-18";
/// The protocol versions the server speaks: it uses nothing they differ on.
const PROTOCOLS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// What the server tells the agent on `initialize`.
const INSTRUCTIONS: &str = "Kymo reads the trace of a Cortex-M firmware: a capture (.kcap) that \
`kymo record` wrote. Every tool takes the capture's path; the ELF is the one the capture holds or \
names, unless `elf` gives another. Start with `summary`, find events with `find`, read what \
surrounds them with `events`, and measure with `stats` and `check`. An instant is an event's index \
(`#1234`) or a time since the first event (`12.5ms`, `300us`); a range includes `from` and excludes \
`to`. Each line of events starts with `event <index>`: pass that index to the next call. A response \
cut at its limit says how many lines are left and where to go on.";

/// Reads the catalogue of an image: its bytes, its name for errors, and an
/// SVD file naming its interrupts.
pub type Parse = fn(&[u8], &str, Option<&Path>) -> target::Result<Catalogue>;

/// Answers each message of `input` on `output`, until `input` ends.
pub fn serve(input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    let mut server = Server::new(events::parse_with_svd);
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.message(&line) {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// The server: the captures it read.
pub struct Server {
    captures: Captures,
}

impl Server {
    /// A server reading catalogues with `parse`.
    pub fn new(parse: Parse) -> Self {
        Self {
            captures: Captures {
                parse,
                kept: VecDeque::new(),
            },
        }
    }

    /// The response to the message `line`, none to a notification or a
    /// response.
    pub fn message(&mut self, line: &str) -> Option<String> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => return Some(failure(Value::Null, -32700, &format!("not JSON: {error}"))),
        };
        let Some(object) = message.as_object() else {
            return Some(failure(Value::Null, -32600, "a message is a JSON object"));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            if object.contains_key("result") || object.contains_key("error") {
                return None;
            }
            return Some(failure(id.unwrap_or(Value::Null), -32600, "a request names its method"));
        };
        let id = id?;
        let params = object.get("params").unwrap_or(&Value::Null);
        let result = match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::list() })),
            "tools/call" => self.call(params),
            _ => Err((-32601, format!("no method {method}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
            Err((code, message)) => failure(id, code, &message),
        })
    }

    /// The result of `tools/call`: the tool's text, or its error as a
    /// result in error. An unknown tool is a protocol error.
    fn call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "tools/call names its tool".to_owned()))?;
        let tool = tools::find_tool(name).ok_or_else(|| (-32602, format!("no tool {name}")))?;
        let empty = Map::new();
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return Err((-32602, "the arguments are an object".to_owned())),
        };
        let captures = &mut self.captures;
        let outcome =
            std::panic::catch_unwind(AssertUnwindSafe(|| tool.call(captures, arguments))).unwrap_or_else(|panic| {
                let reason = panic
                    .downcast_ref::<&str>()
                    .map(|reason| (*reason).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                Err(format!("kymo failed on this call: {reason}"))
            });
        let (text, error) = match outcome {
            Ok(text) => (text, false),
            Err(text) => (text, true),
        };
        Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": error }))
    }
}

/// The result of `initialize`: the version asked for when the server speaks
/// it, else its own.
fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked.filter(|asked| PROTOCOLS.contains(asked)).unwrap_or(PROTOCOL);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "kymo", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// A JSON-RPC error response.
fn failure(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// The captures kept at most.
const KEPT: usize = 4;

/// What names a capture read: the file as it stood, and how it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    elf: Option<String>,
    svd: Option<PathBuf>,
    clock: Option<u64>,
}

/// How a tool asks for a capture.
pub struct Options<'a> {
    pub path: &'a str,
    pub elf: Option<&'a str>,
    pub svd: Option<&'a Path>,
    pub clock: Option<u64>,
}

/// A capture read, and where its elements stand.
pub struct Loaded {
    /// The catalogue of its image, as the trace borrows it.
    pub catalogue: &'static Catalogue,
    pub trace: Trace<'static>,
    /// The name of its image.
    pub image: String,
    /// Each element's place: an event's index, or the first index of a loss
    /// or of a suspect line, counted from the capture's first, never less
    /// than the element's before it.
    places: Vec<u64>,
    /// Each element's cycles: an event's own, a loss's or a suspect line's
    /// those of the event before it, or after it at the start.
    cycles: Vec<i64>,
    /// The cycles of the first event.
    pub origin: Option<i64>,
}

/// The captures read, the last used last.
struct Captures {
    parse: Parse,
    kept: VecDeque<(Key, Rc<Loaded>)>,
}

impl Captures {
    /// The capture `options` name, read now unless it is kept as it stands.
    ///
    /// A trace borrows its catalogue, which is leaked: each capture read
    /// keeps its catalogue to the end of the server, as each capture opened
    /// in kymo-ui does.
    fn get(&mut self, options: &Options) -> Result<Rc<Loaded>, String> {
        let path = options.path;
        let metadata = std::fs::metadata(path).map_err(|error| format!("{path}: {error}"))?;
        let key = Key {
            path: std::fs::canonicalize(path).map_err(|error| format!("{path}: {error}"))?,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            elf: options.elf.map(str::to_owned),
            svd: options.svd.map(Path::to_path_buf),
            clock: options.clock,
        };
        if let Some(at) = self.kept.iter().position(|(kept, _)| *kept == key) {
            let kept = self.kept.remove(at).expect("a kept capture");
            let loaded = kept.1.clone();
            self.kept.push_back(kept);
            return Ok(loaded);
        }
        let loaded = Rc::new(self.read(options).map_err(|error| error.to_string())?);
        if self.kept.len() == KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back((key, loaded.clone()));
        Ok(loaded)
    }

    fn read(&self, options: &Options) -> target::Result<Loaded> {
        let path = options.path;
        let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
        if !capture::is_capture(&bytes) {
            return Err(format!("{path} is not a capture of kymo record").into());
        }
        let (image, name) = capture::elf(path, &bytes, options.elf)?;
        let catalogue: &'static Catalogue = Box::leak(Box::new((self.parse)(&image, &name, options.svd)?));
        let trace = Trace::read(catalogue, &bytes, options.clock)?;
        let mut places = Vec::with_capacity(trace.elements.len());
        let mut cycles = Vec::with_capacity(trace.elements.len());
        let mut place = 0;
        let mut last = None;
        for element in &trace.elements {
            let (index, at) = match element {
                Element::Event { event, cycles, .. } => (event.index, Some(*cycles)),
                Element::Lost { from, .. } | Element::Suspect { from, .. } => (*from, None),
            };
            place = u64::from(index.wrapping_sub(trace.first)).max(place);
            places.push(place);
            last = at.or(last);
            cycles.push(last.unwrap_or(i64::MIN));
        }
        let origin = trace.elements.iter().find_map(|element| match element {
            Element::Event { cycles, .. } => Some(*cycles),
            _ => None,
        });
        // The elements before the first event take its cycles.
        for at in cycles.iter_mut().take_while(|at| **at == i64::MIN) {
            *at = origin.unwrap_or(0);
        }
        Ok(Loaded {
            catalogue,
            trace,
            image: name,
            places,
            cycles,
            origin,
        })
    }
}

/// An instant of a capture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Instant {
    /// An event's index.
    Index(u32),
    /// Seconds since the first event.
    Seconds(f64),
}

impl Instant {
    /// `#1234`, or a number and a unit: `s`, `ms`, `us` or `µs`, `ns`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if let Some(index) = text.strip_prefix('#') {
            return index
                .parse()
                .map(Instant::Index)
                .map_err(|_| format!("{text}: not an event's index"));
        }
        let units = [("ms", 1e-3), ("us", 1e-6), ("µs", 1e-6), ("ns", 1e-9), ("s", 1.0)];
        units
            .iter()
            .find_map(|(unit, scale)| {
                let number = text.strip_suffix(unit)?.trim_end();
                let value: f64 = number.parse().ok().filter(|value: &f64| value.is_finite())?;
                Some(Instant::Seconds(value * scale))
            })
            .ok_or_else(|| format!("{text}: an instant is an index, #1234, or a time with its unit, 12.5ms"))
    }
}

impl Loaded {
    /// The cycles of a time since the first event, as `kymo stats --from`
    /// counts them.
    fn time(&self, seconds: f64) -> Result<i64, String> {
        let hz = self
            .trace
            .frequency
            .ok_or("a time needs the capture's frequency: give the clock, in hertz")?;
        Ok(self.origin.unwrap_or(0) + (seconds * hz).round() as i64)
    }

    /// The position of the first element at or after `at`.
    pub fn start(&self, at: Instant) -> Result<usize, String> {
        Ok(match at {
            Instant::Index(index) => {
                let place = u64::from(index.wrapping_sub(self.trace.first));
                self.places.partition_point(|&other| other < place)
            }
            Instant::Seconds(seconds) => {
                let cycles = self.time(seconds)?;
                self.cycles.partition_point(|&other| other < cycles)
            }
        })
    }

    /// The position of the last element at or before `at`: none before the
    /// first.
    pub fn last_at(&self, at: Instant) -> Result<Option<usize>, String> {
        let after = match at {
            Instant::Index(index) => {
                let place = u64::from(index.wrapping_sub(self.trace.first));
                self.places.partition_point(|&other| other <= place)
            }
            Instant::Seconds(seconds) => {
                let cycles = self.time(seconds)?;
                self.cycles.partition_point(|&other| other <= cycles)
            }
        };
        Ok(after.checked_sub(1))
    }

    /// The cycles of `at`: those of the first element at or after an index,
    /// none past the last element.
    pub fn cycles_of(&self, at: Instant) -> Result<Option<i64>, String> {
        match at {
            Instant::Index(_) => Ok(self.cycles.get(self.start(at)?).copied()),
            Instant::Seconds(seconds) => self.time(seconds).map(Some),
        }
    }

    /// The index an element is given by, to go on from it.
    pub fn index(&self, position: usize) -> u32 {
        self.trace.first.wrapping_add(self.places[position] as u32)
    }

    /// Whether the elements at `left` and `right` share their place.
    pub fn same_place(&self, left: usize, right: usize) -> bool {
        self.places[left] == self.places[right]
    }
}

#[cfg(test)]
mod tests;
