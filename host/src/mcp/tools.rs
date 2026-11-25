//! The tools of `kymo mcp`: what each takes, and the text it answers.

use super::{Captures, Instant, Loaded, Options};
use crate::model::Element;
use crate::words::Words;
use crate::{check, events, meaning, record, stats};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

/// The lines a response holds unless `limit` says otherwise.
const LIMIT: u64 = 200;
/// The elements `events` gives before and after `around` unless told.
const AROUND: u64 = 10;

/// What an argument holds.
#[derive(Clone, Copy)]
enum Kind {
    Text,
    Count,
    Instant,
    Texts,
    /// A path, or the content of a rules file.
    Rules,
}

struct Argument {
    name: &'static str,
    kind: Kind,
    required: bool,
    description: &'static str,
}

const fn argument(name: &'static str, kind: Kind, description: &'static str) -> Argument {
    Argument {
        name,
        kind,
        required: false,
        description,
    }
}

/// The arguments every tool takes: the capture, and how to read it.
const CAPTURE: [Argument; 4] = [
    Argument {
        name: "capture",
        kind: Kind::Text,
        required: true,
        description: "The path of the capture (.kcap) kymo record wrote.",
    },
    argument(
        "elf",
        Kind::Text,
        "The ELF to decode with, a path or URL, instead of the one the capture holds or names.",
    ),
    argument(
        "clock",
        Kind::Count,
        "The cycle counter's frequency in hertz, instead of the one recorded or measured.",
    ),
    argument(
        "svd",
        Kind::Text,
        "An SVD file naming the device interrupts the DWARF does not name.",
    ),
];

const FROM: Argument = argument(
    "from",
    Kind::Instant,
    "Where to start, included: an event's index (#1234) or a time since the first event (12.5ms).",
);
const TO: Argument = argument("to", Kind::Instant, "Where to stop, excluded: an index or a time.");
const LIMIT_ARGUMENT: Argument = argument("limit", Kind::Count, "The lines to give at most; 200 by default.");

/// A tool: its name, what it does, its arguments besides the capture's.
pub struct Tool {
    name: &'static str,
    description: &'static str,
    arguments: &'static [Argument],
    run: fn(&mut Captures, &Arguments) -> Result<String, String>,
}

const TOOLS: [Tool; 5] = [
    Tool {
        name: "summary",
        description: "What a capture holds: its events, slots and losses, its span and frequency, how \
it ends, the tasks, handlers and mutexes the kernel adapter names, the durations, the count of each \
event type, and the image's warnings. Start here.",
        arguments: &[],
        run: summary,
    },
    Tool {
        name: "find",
        description: "The events a filter keeps, one line each as kymo decode prints them, after their \
count. The filter is words separated by spaces: a word is looked for in the event's type and fields, \
`name=value` holds when the field `name` has the integer value (decimal or 0x), `name=` when the \
event has the field; `+word` must hold, `-word` must not, and among plain words one must.",
        arguments: &[
            Argument {
                name: "filter",
                kind: Kind::Text,
                required: true,
                description: "The words an event must match, as above.",
            },
            FROM,
            TO,
            LIMIT_ARGUMENT,
        ],
        run: find,
    },
    Tool {
        name: "events",
        description: "The elements of a capture as kymo decode prints them: each event, and each loss \
and suspect line where it stands. Either a range, from and to, or the elements around an instant. A \
filter, as find's, leaves out events; losses and suspect lines stay.",
        arguments: &[
            FROM,
            TO,
            argument(
                "around",
                Kind::Instant,
                "The instant to read around, instead of from and to.",
            ),
            argument(
                "before",
                Kind::Count,
                "With around: the lines before it, 10 by default.",
            ),
            argument(
                "after",
                Kind::Count,
                "With around: the lines after the first at it, 10 by default.",
            ),
            argument("filter", Kind::Text, "Words an event must match, as find's."),
            LIMIT_ARGUMENT,
        ],
        run: events,
    },
    Tool {
        name: "stats",
        description: "The statistics of kymo stats --json over a range: for each task, handler, mutex \
and duration, its count, distribution, worst instances and share, and the losses, suspect lines and \
contradictions that bound them. Needs the kernel adapter's events or durations.",
        arguments: &[
            FROM,
            TO,
            argument(
                "worst",
                Kind::Count,
                "The worst instances kept for each group, 10 by default.",
            ),
            argument(
                "subjects",
                Kind::Texts,
                "Only the groups of these subjects: tasks, handlers, mutexes or durations.",
            ),
        ],
        run: stats,
    },
    Tool {
        name: "check",
        description: "The rules of a rules file judged as kymo check judges them, over a range: a line \
per rule, then the total.",
        arguments: &[
            Argument {
                name: "rules",
                kind: Kind::Rules,
                required: true,
                description: "The rules file's path, its baseline read next to it; or its content, a \
JSON object, its baseline read from the working directory.",
            },
            FROM,
            TO,
        ],
        run: check,
    },
];

/// The tools as `tools/list` gives them.
pub fn list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|tool| {
            let mut properties = Map::new();
            let mut required = Vec::new();
            for argument in CAPTURE.iter().chain(tool.arguments) {
                let mut schema = match argument.kind {
                    Kind::Text | Kind::Instant => json!({ "type": "string" }),
                    Kind::Count => json!({ "type": "integer", "minimum": 0 }),
                    Kind::Texts => json!({ "type": "array", "items": { "type": "string" } }),
                    Kind::Rules => json!({ "anyOf": [{ "type": "string" }, { "type": "object" }] }),
                };
                schema["description"] = argument.description.into();
                properties.insert(argument.name.to_owned(), schema);
                if argument.required {
                    required.push(argument.name);
                }
            }
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                    "additionalProperties": false,
                },
            })
        })
        .collect()
}

/// The tool named `name`.
pub fn find_tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

impl Tool {
    /// The tool's text, or its error, for `arguments`.
    pub(super) fn call(&self, captures: &mut Captures, arguments: &Map<String, Value>) -> Result<String, String> {
        let mut known = CAPTURE.iter().chain(self.arguments);
        for name in arguments.keys() {
            if !known.any(|argument| argument.name == name) {
                let names: Vec<_> = CAPTURE
                    .iter()
                    .chain(self.arguments)
                    .map(|argument| argument.name)
                    .collect();
                return Err(format!(
                    "{} takes no argument {name}: it takes {}",
                    self.name,
                    names.join(", ")
                ));
            }
            known = CAPTURE.iter().chain(self.arguments);
        }
        for argument in CAPTURE.iter().chain(self.arguments) {
            if argument.required && arguments.get(argument.name).is_none_or(Value::is_null) {
                return Err(format!("{} needs the argument {}", self.name, argument.name));
            }
        }
        (self.run)(captures, &Arguments(arguments))
    }
}

/// The arguments of a call; null is no argument.
struct Arguments<'a>(&'a Map<String, Value>);

impl Arguments<'_> {
    fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name).filter(|value| !value.is_null())
    }

    fn text(&self, name: &str) -> Result<Option<&str>, String> {
        self.get(name)
            .map(|value| value.as_str().ok_or_else(|| format!("{name} is a string")))
            .transpose()
    }

    fn count(&self, name: &str) -> Result<Option<u64>, String> {
        self.get(name)
            .map(|value| value.as_u64().ok_or_else(|| format!("{name} is an integer, 0 or more")))
            .transpose()
    }

    fn instant(&self, name: &str) -> Result<Option<Instant>, String> {
        self.text(name)?.map(Instant::parse).transpose()
    }

    fn texts(&self, name: &str) -> Result<Option<Vec<&str>>, String> {
        self.get(name)
            .map(|value| {
                value
                    .as_array()
                    .and_then(|values| values.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
                    .ok_or_else(|| format!("{name} is an array of strings"))
            })
            .transpose()
    }

    /// The filter, none when absent.
    fn words(&self, name: &str) -> Result<Words, String> {
        Ok(match self.text(name)? {
            Some(text) => Words::parse(text)?,
            None => Words::default(),
        })
    }

    fn limit(&self) -> Result<usize, String> {
        let limit = self.count("limit")?.unwrap_or(LIMIT);
        if limit == 0 {
            return Err("limit is 1 or more".to_owned());
        }
        Ok(usize::try_from(limit).unwrap_or(usize::MAX))
    }
}

/// The capture the arguments name.
fn load(captures: &mut Captures, arguments: &Arguments) -> Result<Rc<Loaded>, String> {
    let path = arguments.text("capture")?.expect("a required argument");
    let svd = arguments.text("svd")?.map(Path::new);
    captures.get(&Options {
        path,
        elf: arguments.text("elf")?,
        svd,
        clock: arguments.count("clock")?,
    })
}

/// The positions from `from` to `to` of the capture.
fn window(loaded: &Loaded, arguments: &Arguments) -> Result<(usize, usize), String> {
    let length = loaded.trace.elements.len();
    let from = match arguments.instant("from")? {
        Some(at) => loaded.start(at)?,
        None => 0,
    };
    let to = match arguments.instant("to")? {
        Some(at) => loaded.start(at)?,
        None => length,
    };
    Ok((from, to.max(from)))
}

/// The stats range from `from` to `to`, in cycles: open where not given.
fn range(loaded: &Loaded, arguments: &Arguments) -> Result<stats::Range, String> {
    let from = match arguments.instant("from")? {
        Some(at) => Some(loaded.cycles_of(at)?.unwrap_or(i64::MAX)),
        None => None,
    };
    let to = match arguments.instant("to")? {
        Some(at) => loaded.cycles_of(at)?,
        None => None,
    };
    Ok(stats::Range { from, to })
}

/// Whether the element at `position` is kept by `words`: losses and suspect
/// lines are.
fn kept(loaded: &Loaded, words: &Words, position: usize) -> bool {
    match &loaded.trace.elements[position] {
        Element::Event { event, .. } => words.keeps(&loaded.catalogue.symbols, event),
        _ => true,
    }
}

/// The lines of the elements at `positions`, `limit` at most. A response
/// cut says how many lines it gives of how many, and where to go on: the
/// index of the first left out, and `to` when the range has an end before
/// the capture's. It is not cut between elements of the same index, so that
/// going on from it gives each line once: it stops before them, or past
/// them when they are more than the limit.
fn page(loaded: &Loaded, positions: &[usize], limit: usize, to: Option<usize>) -> Vec<String> {
    let mut given = positions.len().min(limit);
    let split = |at: usize| at < positions.len() && loaded.same_place(positions[at - 1], positions[at]);
    let mut back = given;
    while back > 0 && split(back) {
        back -= 1;
    }
    if back > 0 {
        given = back;
    } else {
        while split(given) {
            given += 1;
        }
    }
    let mut lines: Vec<String> = positions[..given]
        .iter()
        .map(|&position| record::line(&loaded.catalogue.symbols, &loaded.trace.elements[position]))
        .collect();
    if given < positions.len() {
        let mut resume = format!("from=#{}", loaded.index(positions[given]));
        if let Some(to) = to.filter(|&to| to < loaded.trace.elements.len()) {
            resume.push_str(&format!(" to=#{}", loaded.index(to)));
        }
        lines.push(format!("{given} of {} lines; the rest with {resume}", positions.len()));
    }
    lines
}

fn summary(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let loaded = load(captures, arguments)?;
    let trace = &loaded.trace;
    let catalogue = loaded.catalogue;
    let mut lines = vec![format!("image {}", loaded.image)];
    let (taken, lost) = trace.slots();
    let losses = trace
        .elements
        .iter()
        .filter(|element| matches!(element, Element::Lost { .. }))
        .count();
    let suspect_lines = trace
        .elements
        .iter()
        .filter(|element| matches!(element, Element::Suspect { .. }))
        .count();
    lines.push(format!(
        "events {}, slots {taken}, slots lost {lost}, losses {losses}, suspect lines {suspect_lines}, suspect writers {}",
        trace.events(),
        trace.suspects()
    ));
    lines.push(match (trace.end, &trace.cut) {
        (Some(end), _) => format!("indexes #{} to #{end}, where recording stopped", trace.first),
        (None, Some(cut)) => format!(
            "indexes from #{}; the capture is cut short, before #{}",
            trace.first, cut.next
        ),
        (None, None) => format!("indexes from #{}; the capture is cut short", trace.first),
    });
    let last = trace.elements.iter().rev().find_map(|element| match element {
        Element::Event { cycles, .. } => Some(*cycles),
        _ => None,
    });
    if let (Some(origin), Some(last)) = (loaded.origin, last) {
        lines.push(format!(
            "{} from the first event to the last",
            stats::duration(last - origin, trace.frequency)
        ));
    }
    lines.push(match (trace.frequency, trace.measured) {
        (None, _) => "no frequency: times are in cycles; give the clock to have seconds".to_owned(),
        (Some(hz), Some((measured, span))) if hz == measured => {
            format!("frequency {hz:.0} Hz, measured over {span:.3} s")
        }
        (Some(hz), Some((measured, _))) => format!("frequency {hz:.0} Hz; measured {measured:.0} Hz"),
        (Some(hz), None) => format!("frequency {hz:.0} Hz"),
    });
    match meaning::Meaning::new(catalogue, trace.first) {
        Err(error) => lines.push(format!("kernel: {error}")),
        Ok(kernel) if !kernel.traced() => lines.push("kernel: no adapter events".to_owned()),
        Ok(mut kernel) => {
            kernel.add(&trace.elements, 0);
            let names = |subjects: &meaning::Subjects| {
                let names: Vec<_> = subjects.list.iter().map(|subject| subject.name.as_str()).collect();
                if names.is_empty() {
                    "none".to_owned()
                } else {
                    names.join(", ")
                }
            };
            lines.push(format!("tasks: {}", names(&kernel.tasks)));
            lines.push(format!("handlers: {}", names(&kernel.handlers)));
            lines.push(format!("mutexes: {}", names(&kernel.mutexes)));
            lines.push(format!(
                "{} elements contradict the adapter's order",
                kernel.contradictions.len()
            ));
        }
    }
    let mut durations: Vec<String> = catalogue
        .events
        .values()
        .filter_map(|event| events::Duration::of(&event.name).map(|duration| duration.of))
        .collect();
    durations.sort();
    durations.dedup();
    if !durations.is_empty() {
        lines.push(format!("durations: {}", durations.join(", ")));
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for element in &trace.elements {
        if let Element::Event { event, .. } = element {
            *counts.entry(event.event.name.as_str()).or_default() += 1;
        }
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
    lines.push("events by type:".to_owned());
    lines.extend(counts.iter().map(|(name, count)| format!("  {count} {name}")));
    lines.extend(catalogue.warnings.iter().map(|warning| format!("warning: {warning}")));
    Ok(lines.join("\n"))
}

fn find(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let loaded = load(captures, arguments)?;
    let words = arguments.words("filter")?;
    if !words.active() {
        return Err("the filter leaves out nothing: events reads a range whole".to_owned());
    }
    let (from, to) = window(&loaded, arguments)?;
    let positions: Vec<usize> = (from..to)
        .filter(|&position| {
            matches!(loaded.trace.elements[position], Element::Event { .. }) && kept(&loaded, &words, position)
        })
        .collect();
    let mut lines = vec![match positions.len() {
        1 => "1 event matches".to_owned(),
        count => format!("{count} events match"),
    }];
    lines.extend(page(&loaded, &positions, arguments.limit()?, Some(to)));
    Ok(lines.join("\n"))
}

fn events(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let loaded = load(captures, arguments)?;
    let words = arguments.words("filter")?;
    let limit = arguments.limit()?;
    let length = loaded.trace.elements.len();
    let (positions, to) = match arguments.instant("around")? {
        Some(at) => {
            if arguments.get("from").is_some() || arguments.get("to").is_some() {
                return Err("around takes the place of from and to".to_owned());
            }
            let count = |name| -> Result<usize, String> {
                let count = arguments.count(name)?.unwrap_or(AROUND);
                Ok(usize::try_from(count).unwrap_or(usize::MAX))
            };
            let (before, after) = (count("before")?, count("after")?);
            let at = loaded.start(at)?;
            let mut positions: Vec<usize> = (0..at)
                .rev()
                .filter(|&position| kept(&loaded, &words, position))
                .take(before)
                .collect();
            positions.reverse();
            positions.extend(
                (at..length)
                    .filter(|&position| kept(&loaded, &words, position))
                    .take(after.saturating_add(1)),
            );
            let to = positions.last().map_or(at, |&last| last + 1);
            (positions, to)
        }
        None => {
            if arguments.get("before").is_some() || arguments.get("after").is_some() {
                return Err("before and after go with around".to_owned());
            }
            let (from, to) = window(&loaded, arguments)?;
            let positions = (from..to).filter(|&position| kept(&loaded, &words, position)).collect();
            (positions, to)
        }
    };
    if positions.is_empty() {
        return Ok("nothing in this range".to_owned());
    }
    Ok(page(&loaded, &positions, limit, Some(to)).join("\n"))
}

fn stats(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let loaded = load(captures, arguments)?;
    let range = range(&loaded, arguments)?;
    let worst = arguments.count("worst")?.unwrap_or(10);
    let worst = usize::try_from(worst).unwrap_or(usize::MAX);
    let mut report =
        stats::of_trace(loaded.catalogue, &loaded.trace, range, worst).map_err(|error| error.to_string())?;
    if let Some(subjects) = arguments.texts("subjects")? {
        let unknown: Vec<_> = subjects
            .iter()
            .filter(|subject| !report.groups.iter().any(|group| group.subject == **subject))
            .collect();
        if !unknown.is_empty() {
            let mut known: Vec<_> = report.groups.iter().map(|group| group.subject.as_str()).collect();
            known.sort();
            known.dedup();
            let unknown: Vec<_> = unknown.iter().map(|subject| **subject).collect();
            return Err(format!(
                "no group of {}; the subjects are {}",
                unknown.join(", "),
                known.join(", ")
            ));
        }
        report.groups.retain(|group| subjects.contains(&group.subject.as_str()));
    }
    Ok(stats::json(&report))
}

fn check(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let (text, name, folder) = match arguments.get("rules") {
        Some(Value::String(path)) => {
            let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
            let folder = Path::new(path).parent().unwrap_or(Path::new("")).to_path_buf();
            (text, path.clone(), folder)
        }
        Some(rules @ Value::Object(_)) => (rules.to_string(), "rules".to_owned(), Default::default()),
        _ => return Err("rules is a path or a JSON object".to_owned()),
    };
    let (rules, baseline) = check::load(&text, &name, &folder)?;
    let loaded = load(captures, arguments)?;
    let range = range(&loaded, arguments)?;
    let report = stats::of_trace(loaded.catalogue, &loaded.trace, range, 1).map_err(|error| error.to_string())?;
    let verdicts = check::judge(&report, &rules, baseline.as_ref()).map_err(|error| format!("{name}: {error}"))?;
    let mut lines: Vec<String> = verdicts.iter().map(|verdict| verdict.line.clone()).collect();
    lines.push(check::summary(&verdicts));
    Ok(lines.join("\n"))
}
