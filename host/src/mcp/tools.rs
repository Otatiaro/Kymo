//! The tools of `kymo mcp`: what each takes, and the text it answers.

use super::{Captures, Instant, Loaded, Options};
use crate::meaning::{Edge, Meaning, Span};
use crate::model::Element;
use crate::words::Words;
use crate::{check, durations, events, intervals, meaning, record, stats};
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

const TOOLS: [Tool; 9] = [
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
        name: "context",
        description: "The state after an event, as the kernel adapter and the durations know it: the \
task on the core and since when, the handlers open from the outermost, the mutexes held and waited \
for, the durations open, the tasks ready, the last wake of the task on the core, and the faults so \
far. A time gives the state after the last event at or before it.",
        arguments: &[Argument {
            name: "at",
            kind: Kind::Instant,
            required: true,
            description: "The instant: an event's index (#1234) or a time since the first event (12.5ms).",
        }],
        run: context,
    },
    Tool {
        name: "subject",
        description: "The life of a task or a handler, a line per activation or run that starts in the \
range. A task's: the wake, its reason and cause (who unblocked it), the wake latency, the response, \
gross and net times, the time preempted by tasks and by handlers, the time ready, the time blocked \
since the previous activation ended, and the period to the next wake. A handler's: gross and net \
times, depth and period. Times in cycles, then in seconds when the frequency is known.",
        arguments: &[
            Argument {
                name: "subject",
                kind: Kind::Text,
                required: true,
                description: "The task's or handler's name, as summary lists it.",
            },
            FROM,
            TO,
            LIMIT_ARGUMENT,
        ],
        run: subject,
    },
    Tool {
        name: "intervals",
        description: "The delay from each event a filter A keeps to the first event a filter B keeps \
before the next A: the count, the A with no B, those excluded by a loss or a suspect line, those cut \
by the range, then min, p50, p90, p99, max and mean, and the worst with the indexes of both ends. \
An event both filters keep ends one interval and opens the next. Filters as find's: plain words are \
alternatives, so a task's wake to its start is a=\"+task_ready +task=<address>\".",
        arguments: &[
            Argument {
                name: "a",
                kind: Kind::Text,
                required: true,
                description: "The filter of the events that open an interval.",
            },
            Argument {
                name: "b",
                kind: Kind::Text,
                required: true,
                description: "The filter of the events that close it.",
            },
            FROM,
            TO,
            argument("worst", Kind::Count, "The worst intervals given, 10 by default."),
        ],
        run: intervals,
    },
    Tool {
        name: "stats",
        description: "The statistics of kymo stats --json over a range: for each task, handler, mutex \
and duration, its count, distribution, worst instances and share, and the losses, suspect lines and \
contradictions that bound them. Needs the kernel adapter's events or durations. Over a whole \
capture of a few dozen tasks it runs to a hundred kilobytes: name the subjects summary lists.",
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
        name: "compare",
        description: "The statistics of two captures face to face, as JSON: for each group of kymo stats \
(subject, metric, detail), its count, exclusions, min, p50, p90, p99, max, mean, jitter and share in \
each capture, and the difference, other minus capture. A group only one capture has is given with \
null for the other. It does not judge: whether a difference is beyond the noise depends on the \
counts behind it. Times are in cycles; each side gives its frequency. from and to apply to both.",
        arguments: &[
            Argument {
                name: "other",
                kind: Kind::Text,
                required: true,
                description: "The path of the capture to compare with.",
            },
            argument(
                "other_elf",
                Kind::Text,
                "The ELF to decode the other capture with, instead of the one it holds or names.",
            ),
            argument(
                "other_clock",
                Kind::Count,
                "The other capture's cycle counter frequency in hertz, instead of the one recorded.",
            ),
            FROM,
            TO,
            argument(
                "subjects",
                Kind::Texts,
                "Only the groups of these subjects: tasks, handlers, mutexes, durations or values.",
            ),
        ],
        run: compare,
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

/// A time: its cycles, then in seconds when the frequency is known.
fn time(loaded: &Loaded, cycles: i64) -> String {
    match loaded.trace.frequency {
        Some(_) if cycles != 0 => format!("{cycles} ({})", stats::duration(cycles, loaded.trace.frequency)),
        _ => cycles.to_string(),
    }
}

/// Whether `subject` is named `name`: a handler also by its name without
/// the interrupt that follows it, ` (IRQ45)`.
fn named(subject: &meaning::Subject, name: &str) -> bool {
    subject
        .name
        .strip_prefix(name)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(" ("))
}

/// An edge as the index of its event, or a loss.
fn edge(loaded: &Loaded, edge: Edge) -> String {
    if edge.lost {
        "a loss".to_owned()
    } else {
        format!("#{}", loaded.index(edge.element))
    }
}

/// The kernel adapter's meaning of `elements`, from the capture's start.
fn kernel(loaded: &Loaded, elements: &[Element]) -> Result<Meaning<'static>, String> {
    let mut kernel = Meaning::new(loaded.catalogue, loaded.trace.first).map_err(|error| error.to_string())?;
    if !kernel.traced() {
        return Err("the image has no kernel adapter events".to_owned());
    }
    kernel.add(elements, 0);
    Ok(kernel)
}

fn context(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let loaded = load(captures, arguments)?;
    let at = arguments.instant("at")?.expect("a required argument");
    let elements = &loaded.trace.elements;
    let event = loaded.last_at(at)?.and_then(|last| {
        (0..=last)
            .rev()
            .find(|&position| matches!(elements[position], Element::Event { .. }))
            .map(|event| (last, event))
    });
    let Some((last, event)) = event else {
        return Err("no event at or before this instant".to_owned());
    };
    let Element::Event { cycles: now, .. } = elements[event] else {
        unreachable!("an event")
    };
    let elements = &elements[..=last];
    let since = |from: Edge| {
        format!(
            "since {}, {} ago",
            edge(&loaded, from),
            time(&loaded, now - from.cycles)
        )
    };
    let mut lines = vec![format!(
        "after {}",
        record::line(&loaded.catalogue.symbols, &elements[event])
    )];
    match kernel(&loaded, elements) {
        Err(error) => lines.push(format!("kernel: {error}")),
        Ok(kernel) => {
            let task = |task: Option<usize>| task.map_or("idle", |task| kernel.tasks.list[task].name.as_str());
            let open = kernel.open();
            let running = open.runs.first().map(|run| run.task);
            lines.push(match open.runs.first() {
                Some(run) => format!("core: {} {}", task(run.task), since(run.span.from)),
                None => "core: unknown, a loss hides it or no switch came yet".to_owned(),
            });
            let mut interrupts = open.interrupts.clone();
            interrupts.sort_by_key(|interrupt| interrupt.span.from.element);
            if interrupts.is_empty() {
                lines.push("handlers open: none".to_owned());
            }
            for interrupt in &interrupts {
                let depth = interrupt
                    .depth
                    .map_or_else(|| "depth unknown".to_owned(), |depth| format!("depth {depth}"));
                lines.push(format!(
                    "handler {}, {depth}, {}",
                    kernel.handlers.list[interrupt.handler].name,
                    since(interrupt.span.from)
                ));
            }
            for hold in &open.holds {
                lines.push(format!(
                    "mutex {} {} {}, {}",
                    kernel.mutexes.list[hold.mutex].name,
                    if hold.waiting { "waited for by" } else { "held by" },
                    kernel.tasks.list[hold.task].name,
                    since(hold.span.from)
                ));
            }
            let mut activations = kernel.open_activations();
            activations.sort_by_key(|activation| activation.span.from.element);
            for activation in &activations {
                let woken = match (&activation.reason, activation.cause) {
                    (Some(reason), Some(cause)) => format!("woken by {reason}, cause {}", kernel.cause(cause)),
                    _ => "ready from a loss".to_owned(),
                };
                let state = if running == Some(Some(activation.task)) {
                    "on the core"
                } else {
                    "ready"
                };
                lines.push(format!(
                    "{state}: {}, {woken}, {}",
                    task(Some(activation.task)),
                    since(activation.span.from)
                ));
            }
            if let Some(Some(on)) = running
                && let Some(wake) = kernel.wakes.iter().rev().find(|wake| wake.task == on)
            {
                lines.push(format!(
                    "last wake of {}: by {}, cause {}, at {}, {} ago",
                    task(Some(on)),
                    wake.reason,
                    kernel.cause(wake.cause),
                    edge(&loaded, wake.edge),
                    time(&loaded, now - wake.edge.cycles)
                ));
            }
            if let Some(last) = kernel.faults.last() {
                lines.push(format!(
                    "faults so far: {}, the last at {}: {}",
                    kernel.faults.len(),
                    edge(&loaded, last.edge),
                    kernel.describe_fault(last)
                ));
            }
        }
    }
    let mut pairing = durations::Pairing::new(loaded.trace.first);
    pairing.add(elements, 0);
    for placed in pairing.open() {
        lines.push(format!(
            "duration {}, lane {}, {}",
            placed.instance.subject,
            placed.lane,
            since(placed.instance.from)
        ));
    }
    Ok(lines.join("\n"))
}

/// The values of the instances `kind` of `name` gives, by the element they
/// start at, then by metric: their totals, without detail.
fn values_by_start<'a>(
    instances: &'a [stats::Instance],
    kind: &str,
    name: &str,
) -> BTreeMap<usize, BTreeMap<&'a str, i64>> {
    let mut values: BTreeMap<usize, BTreeMap<&str, i64>> = BTreeMap::new();
    for instance in instances {
        if instance.kind == kind && instance.subject == name && instance.detail.is_none() {
            values
                .entry(instance.from.element)
                .or_default()
                .insert(instance.metric.name, instance.value);
        }
    }
    values
}

/// `lines`, `limit` at most, the footer saying where to go on: `resume`
/// gives the position of each line's start.
fn cut(loaded: &Loaded, mut lines: Vec<String>, starts: &[usize], limit: usize, what: &str) -> Vec<String> {
    if lines.len() > limit {
        let total = lines.len();
        lines.truncate(limit);
        lines.push(format!(
            "{limit} of {total} {what}; the rest with from=#{}",
            loaded.index(starts[limit])
        ));
    }
    lines
}

fn subject(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let loaded = load(captures, arguments)?;
    let name = arguments.text("subject")?.expect("a required argument");
    let (from, to) = window(&loaded, arguments)?;
    let limit = arguments.limit()?;
    let kernel = kernel(&loaded, &loaded.trace.elements)?;
    let instances = meaning::instances(&kernel);
    let inside = |span: &Span| (from..to).contains(&span.from.element);
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    if let Some(task) = kernel.tasks.list.iter().position(|subject| subject.name == name) {
        let values = values_by_start(&instances, "task", name);
        let mut ready: BTreeMap<usize, i64> = BTreeMap::new();
        for part in meaning::ready(&kernel).iter().filter(|part| part.task == task) {
            *ready.entry(part.activation.from.element).or_default() += part.span.to.cycles - part.span.from.cycles;
        }
        let open = kernel.open_activations();
        let mut activations: Vec<_> = kernel
            .activations
            .iter()
            .map(|activation| (activation, false))
            .chain(open.iter().map(|activation| (activation, true)))
            .filter(|(activation, _)| activation.task == task)
            .collect();
        activations.sort_by_key(|(activation, _)| activation.span.from.element);
        let mut ran = 0;
        let mut worst: [Option<(i64, Edge)>; 2] = [None, None];
        let mut previous: Option<Edge> = None;
        for (activation, open) in activations {
            let span = activation.span;
            let blocked = previous.map(|end| span.from.cycles - end.cycles);
            previous = Some(span.to);
            if !inside(&span) {
                continue;
            }
            let metrics = values.get(&span.from.element);
            let metric = |name: &str| metrics.and_then(|metrics| metrics.get(name)).copied();
            let mut line = match (&activation.reason, activation.cause) {
                (Some(reason), Some(cause)) => format!(
                    "{} woken by {reason}, cause {}",
                    edge(&loaded, span.from),
                    kernel.cause(cause)
                ),
                _ => format!("{} ready from a loss", edge(&loaded, span.from)),
            };
            let latency = metric("wake_latency");
            match latency {
                Some(latency) => {
                    ran += 1;
                    for (slot, value) in [(0, latency), (1, metric("response").unwrap_or(0))] {
                        if worst[slot].is_none_or(|(most, _)| value > most) {
                            worst[slot] = Some((value, span.from));
                        }
                    }
                }
                None => line.push_str(", never ran"),
            }
            let parts = [
                ("latency", latency),
                ("response", metric("response")),
                ("gross", metric("gross")),
                ("net", metric("net")),
                ("preempted by tasks", metric("preempted_by_tasks")),
                ("by handlers", metric("preempted_by_handlers")),
                ("ready", ready.get(&span.from.element).copied()),
                ("blocked before", blocked),
                ("period", metric("period")),
            ];
            for (label, value) in parts {
                if let Some(value) = value {
                    line.push_str(&format!(", {label} {}", time(&loaded, value)));
                }
            }
            if open {
                line.push_str(&format!(", still open at {}", edge(&loaded, span.to)));
            } else {
                line.push_str(&format!(", ends {}", edge(&loaded, span.to)));
            }
            lines.push(line);
            starts.push(span.from.element);
        }
        let priority = kernel.tasks.list[task]
            .priority
            .map_or_else(String::new, |priority| format!(", priority {priority}"));
        let mut head = format!("task {name}{priority}: {} activations, {ran} ran", lines.len());
        for (label, worst) in ["worst latency", "worst response"].iter().zip(worst) {
            if let Some((value, at)) = worst {
                head.push_str(&format!("; {label} {} at {}", time(&loaded, value), edge(&loaded, at)));
            }
        }
        let mut out = vec![head];
        out.extend(cut(&loaded, lines, &starts, limit, "activations"));
        return Ok(out.join("\n"));
    }
    if let Some(handler) = kernel.handlers.list.iter().position(|subject| named(subject, name)) {
        let name = kernel.handlers.list[handler].name.as_str();
        let values = values_by_start(&instances, "handler", name);
        let open = kernel.open();
        let mut runs: Vec<_> = kernel
            .closed
            .interrupts
            .iter()
            .map(|interrupt| (interrupt, false))
            .chain(open.interrupts.iter().map(|interrupt| (interrupt, true)))
            .filter(|(interrupt, _)| interrupt.handler == handler && inside(&interrupt.span))
            .collect();
        runs.sort_by_key(|(interrupt, _)| interrupt.span.from.element);
        let mut worst: Option<(i64, Edge)> = None;
        for (interrupt, open) in runs {
            let span = interrupt.span;
            let metrics = values.get(&span.from.element);
            let metric = |name: &str| metrics.and_then(|metrics| metrics.get(name)).copied();
            let gross = span.to.cycles - span.from.cycles;
            if worst.is_none_or(|(most, _)| gross > most) {
                worst = Some((gross, span.from));
            }
            let mut line = format!("{} gross {}", edge(&loaded, span.from), time(&loaded, gross));
            if let Some(net) = metric("net") {
                line.push_str(&format!(", net {}", time(&loaded, net)));
            }
            line.push_str(&match interrupt.depth {
                Some(depth) => format!(", depth {depth}"),
                None => ", depth unknown".to_owned(),
            });
            if let Some(period) = metric("period") {
                line.push_str(&format!(", period {}", time(&loaded, period)));
            }
            line.push_str(&if open {
                format!(", still open at {}", edge(&loaded, span.to))
            } else {
                format!(", ends {}", edge(&loaded, span.to))
            });
            lines.push(line);
            starts.push(span.from.element);
        }
        let mut head = format!("handler {name}: {} runs", lines.len());
        if let Some((value, at)) = worst {
            head.push_str(&format!(
                "; worst gross {} at {}",
                time(&loaded, value),
                edge(&loaded, at)
            ));
        }
        let mut out = vec![head];
        out.extend(cut(&loaded, lines, &starts, limit, "runs"));
        return Ok(out.join("\n"));
    }
    let names = |subjects: &meaning::Subjects| {
        subjects
            .list
            .iter()
            .map(|subject| subject.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(format!(
        "no task or handler {name}; the tasks are {}; the handlers are {}",
        names(&kernel.tasks),
        names(&kernel.handlers)
    ))
}

fn intervals(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let loaded = load(captures, arguments)?;
    let (a, b) = (arguments.words("a")?, arguments.words("b")?);
    if !a.active() || !b.active() {
        return Err("a and b each need a word".to_owned());
    }
    let range = range(&loaded, arguments)?;
    let worst = arguments.count("worst")?.unwrap_or(10);
    let worst = usize::try_from(worst).unwrap_or(usize::MAX);
    let found = intervals::of(&loaded.trace, &loaded.catalogue.symbols, &a, &b, range, worst);
    let mut lines = vec![format!(
        "{} intervals; {} A without a B before the next A; excluded {} by a loss, {} by a suspect line; {} cut",
        found.count, found.without, found.excluded_loss, found.excluded_suspect, found.cut
    )];
    let Some(distribution) = &found.distribution else {
        lines.push("no interval counted".to_owned());
        return Ok(lines.join("\n"));
    };
    let value = |value| time(&loaded, value);
    lines.push(format!(
        "min {} at #{}",
        value(distribution.min.value),
        distribution.min.index
    ));
    lines.push(format!("p50 {}", value(distribution.p50)));
    lines.push(format!("p90 {}", value(distribution.p90)));
    lines.push(format!("p99 {}", value(distribution.p99)));
    lines.push(format!(
        "max {} at #{}",
        value(distribution.max.value),
        distribution.max.index
    ));
    lines.push(format!(
        "mean {:.1} cycles{}",
        distribution.mean,
        loaded.trace.frequency.map_or_else(String::new, |_| format!(
            " ({})",
            stats::duration(distribution.mean.round() as i64, loaded.trace.frequency)
        ))
    ));
    lines.push("worst:".to_owned());
    for point in &distribution.worst {
        lines.push(format!(
            "  #{} to #{}: {}",
            point.index,
            found.ends[&point.element],
            value(point.value)
        ));
    }
    Ok(lines.join("\n"))
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

/// The fields of a group `compare` gives for each capture and their
/// difference; `min` and `max` also give the index of their instance.
const COMPARED: [&str; 11] = [
    "count",
    "excluded_loss",
    "excluded_suspect",
    "cut",
    "min",
    "p50",
    "p90",
    "p99",
    "max",
    "mean",
    "jitter",
];

fn compare(captures: &mut Captures, arguments: &Arguments) -> Result<String, String> {
    let mine = load(captures, arguments)?;
    let other = captures.get(&Options {
        path: arguments.text("other")?.expect("a required argument"),
        elf: arguments.text("other_elf")?,
        svd: arguments.text("svd")?.map(Path::new),
        clock: arguments.count("other_clock")?,
    })?;
    let side = |loaded: &Loaded| -> Result<Value, String> {
        let range = range(loaded, arguments)?;
        let report = stats::of_trace(loaded.catalogue, &loaded.trace, range, 0).map_err(|error| error.to_string())?;
        Ok(serde_json::from_str(&stats::json(&report)).expect("the JSON stats writes"))
    };
    let (mine, other) = (side(&mine)?, side(&other)?);
    let key = |group: &Value| ["kind", "subject", "metric", "detail"].map(|field| group[field].to_string());
    let groups = |report: &Value| report["groups"].as_array().cloned().unwrap_or_default();
    let (my_groups, other_groups) = (groups(&mine), groups(&other));
    let subjects = arguments.texts("subjects")?;
    if let Some(subjects) = &subjects {
        let named = |subject: &&str| {
            my_groups
                .iter()
                .chain(&other_groups)
                .any(|group| group["subject"] == **subject)
        };
        let unknown: Vec<&str> = subjects.iter().filter(|subject| !named(subject)).copied().collect();
        if !unknown.is_empty() {
            return Err(format!("no group of {} in either capture", unknown.join(", ")));
        }
    }
    let wanted = |group: &Value| {
        subjects
            .as_ref()
            .is_none_or(|subjects| subjects.iter().any(|subject| group["subject"] == *subject))
    };
    let mut pairs: Vec<(&Value, Option<&Value>, Option<&Value>)> = Vec::new();
    for group in my_groups.iter().filter(|group| wanted(group)) {
        let twin = other_groups.iter().find(|candidate| key(candidate) == key(group));
        pairs.push((group, Some(group), twin));
    }
    for group in other_groups.iter().filter(|group| wanted(group)) {
        if !my_groups.iter().any(|candidate| key(candidate) == key(group)) {
            pairs.push((group, None, Some(group)));
        }
    }
    let compared: Vec<Value> = pairs
        .into_iter()
        .map(|(group, mine, other)| {
            let (mine, other) = (mine.map(compared_side), other.map(compared_side));
            let difference = mine.as_ref().zip(other.as_ref()).map(|(mine, other)| {
                let fields: Map<String, Value> = COMPARED
                    .iter()
                    .chain(&["share"])
                    .map(|&field| (field.to_owned(), minus(&other[field], &mine[field])))
                    .collect();
                Value::Object(fields)
            });
            json!({
                "kind": group["kind"],
                "subject": group["subject"],
                "metric": group["metric"],
                "detail": group["detail"],
                "capture": mine,
                "other": other,
                "difference": difference,
            })
        })
        .collect();
    let bounds = |report: &Value| {
        let fields: Map<String, Value> = [
            "frequency",
            "from",
            "to",
            "losses",
            "suspects",
            "contradictions",
            "faults",
        ]
        .iter()
        .map(|&field| (field.to_owned(), report[field].clone()))
        .collect();
        Value::Object(fields)
    };
    Ok(json!({
        "schema": "kymo-compare",
        "capture": bounds(&mine),
        "other": bounds(&other),
        "groups": compared,
    })
    .to_string())
}

/// A group of the stats JSON as `compare` gives it for one capture.
fn compared_side(group: &Value) -> Value {
    let distribution = &group["distribution"];
    let mut side = Map::new();
    for field in COMPARED {
        let value = match field {
            "count" | "excluded_loss" | "excluded_suspect" | "cut" | "jitter" => group[field].clone(),
            "min" | "max" => {
                side.insert(format!("{field}_at"), distribution[field]["index"].clone());
                distribution[field]["value"].clone()
            }
            _ => distribution[field].clone(),
        };
        side.insert(field.to_owned(), value);
    }
    side.insert("share".to_owned(), group["share"].clone());
    Value::Object(side)
}

/// `a - b`, an integer when both are; null when either is not a number.
fn minus(a: &Value, b: &Value) -> Value {
    match (a.as_i64(), b.as_i64()) {
        (Some(a), Some(b)) => json!(a.saturating_sub(b)),
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => json!(a - b),
            _ => Value::Null,
        },
    }
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
