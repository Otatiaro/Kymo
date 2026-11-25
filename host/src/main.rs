//! Kymo command line.

use kymo::interrupts::Interrupts;
use kymo::model::{Element, Trace};
use kymo::target::{self, Backend, Connection, Result};
use kymo::{capture, check, decode, events, meaning, record, stats};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
usage: kymo events <elf> [--svd <file>]
       kymo decode <elf> <ring or capture> [--channel <ring>] [--clock <Hz>] [--meaning] [--svd <file>]
       kymo record --probe <probe-rs|jlink|stlink|qemu> --elf <file> [options]
       kymo stats <elf> <capture> [--from <s>] [--to <s>] [--clock <Hz>] [--svd <file>] [--worst <n>] [--json]
       kymo check <elf> <capture> <rules.json> [--from <s>] [--to <s>] [--clock <Hz>] [--svd <file>]

events: lists the event types of an image, read from its ELF: each type id,
the type it stands for, its size, and each field's offset, size and type.
Then the device interrupts its DWARF, or else the SVD file --svd gives,
names: `interrupt <exception> <name>` (see decode).

decode: lists the events a copy of a trace ring holds, oldest first; the
copy is the bytes of a kymo::ring object, dumped from a stopped target
running the image. Given a capture made by kymo record --out, prints what
kymo record printed while making it, but for the seconds: they are
recomputed over the whole capture. With --meaning, prints instead the
kernel's spans in a capture of an image built with a Kymo kernel adapter
(OpSy's or FreeRTOS's): a line each for a task on the core (`run`, the idle
task as `task=idle`), a handler running
(`interrupt`, `depth` handlers below it, `unknown` from a loss to the next
task switch), a mutex held (`hold`) or waited for (`wait`), from one event
index to another; `lost` for an edge hidden by a loss, `to=open` for a span
still open at the end. Then a line for each task_ready, `wake at=<index>`,
with its reason and its cause: the context it is emitted in, the innermost
handler open but the kernel's (`handler:<name>`) or else the task on the core
(`task:<name>`, `task:idle`); `elapsed` for a delay that ran out, in SysTick;
`unknown` from a loss to the next task switch. A handler not traced by the
adapter hides in the task it interrupts. Then a line for each time a task is
ready and not on the core (`ready`): from its task_ready to its first run,
then each time it is preempted, to the end of its activation. Then a line
`contradiction <index>` for each event against the order the adapter's
events keep.

A handler is named `<handler> (<interrupt>)`, once for each interrupt it
runs for. The interrupt is named by the image's IRQn_Type enumeration, from
its DWARF (SvdGenerator's or a vendor's CMSIS header), else by the SVD file
--svd gives, else IRQ<n>; the core's exceptions by the architecture.

The ring of a copy is the image's only kymo::ring object, or the one
--channel names by its qualified name (e.g. app::trace_ring) when the image
has several. A capture names the ring it was recorded from.

Each event line reads: index, cycles (the cycle counter timestamp extended
to 64 bits), seconds since the first event (- without a frequency), type,
fields. The frequency is --clock's, or the one recorded, or else measured
from the host time of the probe's passes once the events span a second.

record: prints the events of a running target as it writes them, drained
from its trace ring through the probe, and the indexes lost when the ring
overtook the probe; stops on Enter, or after --duration. A line
\"suspect <from> <to> <n>\" reports n writes overrun: interrupted for a full
ring lap while its write index went from <from> to <to>, they may have
written into events the probe read: events recorded near it may be wrong.

record options:
  --channel <ring>   the kymo::ring object to drain, as for decode
  --probe-id <id>    probe-rs: VID:PID[:SERIAL]; jlink, stlink: serial number;
                     qemu: <host>:<port>, QEMU's monitor started with
                     -qmp tcp:<host>:<port>,server=on,wait=off
  --chip <name>      target name for probe-rs and J-Link (e.g. STM32H563ZITx)
  --speed <kHz>      debug clock, or the closest the probe supports
                     below it (default 24000)
  --duration <seconds>  stop after this long
  --out <file>       also keep the recording in this capture file
  --clock <Hz>       the cycle counter's frequency, kept in the capture;
                     without it, measured

stats: the kernel's statistics over a capture of an image built with a Kymo
kernel adapter, or over its range from --from to --to seconds after its
first event.
For each task, of each activation, from its task_ready to the event that
makes it not ready: wake latency (in total, by wake reason, and by cause as
decode --meaning names it: handler:<name>, task:<name>, elapsed, unknown), time
preempted by tasks and by handlers, net and gross execution, response time;
and the period, from a task_ready to the next. For each handler: gross and
net duration, period. For each mutex: holds and waits, by task and in total.
A line each: the count, the min, the percentiles 50, 90 and 99 by nearest
rank, the max, the mean; the jitter of a period, max - min; the share of the
time known that a net execution takes, the time known being when the task on
the core is: a loss hides it until the next task_started or enter_idle. An
instance that touches a loss or
covers an overrun write is excluded and counted: the high values are then a
lower bound, marked >=. One cut by the range, or open at the end, is counted
as cut. --json prints the same, with each histogram and the --worst
instances (default 10), as host/schema/stats.schema.json describes. Exits
with 0 when the statistics are printed, losses or not; 1 on a capture or an
image it cannot read; 2 on a command line it cannot parse, for any command.

check: judges rules on the statistics stats gives, over the same range. The
rules file is JSON, its shape in host/schema/check.schema.json: each rule
bounds a statistic (min, p50, p90, p99, max, mean, jitter, count, share) of
a group (subject, metric, detail, kind) with min and max, as a duration with
its unit (50us, 2ms, 300cycles), a percentage or a count; or holds it within
a regression (10%, 2us) of the same statistic in the baseline, a kymo stats
--json the file names; or bounds the losses, suspects or contradictions of
the range. Excluded instances make each statistic an interval: a rule passes
when all of it keeps to the rule, fails when none does, and is unknown
otherwise. A group absent from the range fails. A line each, then a total.
Exits with 0 when every rule passes; 3 when one fails; 4 when none fails
and one is unknown; 1 on an input it cannot read or a rule it cannot judge.
";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let outcome = if arguments.next().as_deref() == Some("check") {
        match (arguments.next(), arguments.next(), arguments.next()) {
            (Some(elf), Some(copy), Some(rules)) => check(&elf, &copy, &rules, arguments),
            _ => Err(usage(USAGE)),
        }
    } else {
        run().map(|()| ExitCode::SUCCESS)
    };
    match outcome {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(if error.is::<Usage>() { 2 } else { 1 })
        }
    }
}

/// A command line that does not parse.
#[derive(Debug)]
struct Usage(String);

impl std::fmt::Display for Usage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Usage {}

fn usage(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(Usage(message.into()))
}

/// The value of `option`, parsed.
fn parsed<T: std::str::FromStr>(option: &str, value: Option<String>) -> Result<T> {
    let value = value.ok_or_else(|| usage(format!("{option} needs a value")))?;
    value
        .parse()
        .map_err(|_| usage(format!("{option}: cannot read {value}")))
}

/// The range, clock and SVD options of stats and check.
#[derive(Clone, Default)]
struct Window {
    clock: Option<u64>,
    from: Option<f64>,
    to: Option<f64>,
    svd: Option<PathBuf>,
}

impl Window {
    /// Takes `option` and its value if it is one of the window's.
    fn take(&mut self, option: &str, arguments: &mut impl Iterator<Item = String>) -> Result<bool> {
        match option {
            "--from" => self.from = Some(parsed(option, arguments.next())?),
            "--to" => self.to = Some(parsed(option, arguments.next())?),
            "--clock" => self.clock = Some(parsed(option, arguments.next())?),
            "--svd" => self.svd = Some(parsed(option, arguments.next())?),
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// The kernel's statistics of the capture `copy` of the image `elf`.
fn stats(elf: &str, copy: &str, mut arguments: impl Iterator<Item = String>) -> Result<()> {
    let mut window = Window::default();
    let mut worst = 10;
    let mut json = false;
    while let Some(option) = arguments.next() {
        if window.take(&option, &mut arguments)? {
            continue;
        }
        match option.as_str() {
            "--json" => json = true,
            "--worst" => worst = parsed(&option, arguments.next())?,
            _ => return Err(usage(format!("unknown option {option}\n\n{USAGE}"))),
        }
    }
    let report = report(elf, copy, &window, worst)?;
    if json {
        println!("{}", stats::json(&report));
    } else {
        for line in stats::text(&report) {
            println!("{line}");
        }
    }
    Ok(())
}

/// The rules of the file `rules` judged on the capture `copy` of the image
/// `elf`: the exit code says how they stand.
fn check(elf: &str, copy: &str, rules: &str, mut arguments: impl Iterator<Item = String>) -> Result<ExitCode> {
    let mut window = Window::default();
    while let Some(option) = arguments.next() {
        if !window.take(&option, &mut arguments)? {
            return Err(usage(format!("unknown option {option}\n\n{USAGE}")));
        }
    }
    let text = std::fs::read_to_string(rules).map_err(|error| format!("{rules}: {error}"))?;
    let (rules_read, baseline) = check::rules(&text).map_err(|error| format!("{rules}: {error}"))?;
    let baseline = match baseline {
        None => None,
        Some(path) => {
            let path = Path::new(rules).parent().unwrap_or(Path::new("")).join(path);
            let text = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            Some(check::baseline(&text).map_err(|error| format!("{}: {error}", path.display()))?)
        }
    };
    let report = report(elf, copy, &window, 1)?;
    let verdicts =
        check::judge(&report, &rules_read, baseline.as_ref()).map_err(|error| format!("{rules}: {error}"))?;
    for verdict in &verdicts {
        println!("{}", verdict.line);
    }
    println!("{}", check::summary(&verdicts));
    Ok(ExitCode::from(check::code(&verdicts)))
}

/// The statistics of the capture `copy` of the image `elf`, over the window.
fn report(elf: &str, copy: &str, window: &Window, worst: usize) -> Result<stats::Report> {
    let Window { clock, from, to, svd } = window;
    let (clock, from, to) = (*clock, *from, *to);
    let catalogue = catalogue(Path::new(elf), svd.as_deref())?;
    let bytes = std::fs::read(copy).map_err(|error| format!("{copy}: {error}"))?;
    if !capture::is_capture(&bytes) {
        return Err(format!("{copy} is not a capture of kymo record").into());
    }
    let trace = Trace::read(&catalogue, &bytes, clock)?;
    let mut kernel = meaning::Meaning::new(&catalogue, trace.first)?;
    if !kernel.traced() {
        return Err("the image has no kernel adapter events".into());
    }
    kernel.add(&trace.elements, 0);
    let origin = trace.elements.iter().find_map(|element| match element {
        Element::Event { cycles, .. } => Some(*cycles),
        _ => None,
    });
    let cycles = |seconds: Option<f64>| -> Result<Option<i64>> {
        let Some(seconds) = seconds else {
            return Ok(None);
        };
        let hz = trace
            .frequency
            .ok_or("--from and --to are in seconds: give the capture's frequency with --clock")?;
        Ok(Some(origin.unwrap_or(0) + (seconds * hz).round() as i64))
    };
    let range = stats::Range {
        from: cycles(from)?,
        to: cycles(to)?,
    };
    let found = stats::Found {
        instances: &meaning::instances(&kernel),
        contradictions: &kernel.contradictions,
        known: &kernel.known(),
    };
    Ok(stats::compute(&trace, found, range, worst))
}

/// The catalogue of the image at `path`, its warnings printed, the
/// interrupts its DWARF does not name named by the SVD file `svd`.
fn catalogue(path: &Path, svd: Option<&Path>) -> Result<events::Catalogue> {
    let mut catalogue = events::read(path)?;
    if let Some(svd) = svd {
        catalogue.interrupts = catalogue.interrupts.or(&Interrupts::from_svd(svd)?);
    }
    for warning in &catalogue.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(catalogue)
}

fn run() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next();
    match command.as_deref() {
        Some("record") => {}
        Some("events") => {
            let elf = PathBuf::from(arguments.next().ok_or_else(|| usage(USAGE))?);
            let svd = match (arguments.next().as_deref(), arguments.next()) {
                (None, _) => None,
                (Some("--svd"), value) => Some(parsed::<PathBuf>("--svd", value)?),
                _ => return Err(usage(USAGE)),
            };
            if arguments.next().is_some() {
                return Err(usage(USAGE));
            }
            events::print(&catalogue(&elf, svd.as_deref())?);
            return Ok(());
        }
        Some("decode") => {
            let (Some(elf), Some(copy)) = (arguments.next(), arguments.next()) else {
                return Err(usage(USAGE));
            };
            let mut channel = None;
            let mut clock = None;
            let mut spans = false;
            let mut svd = None;
            while let Some(option) = arguments.next() {
                if option == "--meaning" {
                    spans = true;
                    continue;
                }
                let value = arguments.next();
                match option.as_str() {
                    "--channel" => channel = Some(parsed::<String>(&option, value)?),
                    "--clock" => clock = Some(parsed(&option, value)?),
                    "--svd" => svd = Some(parsed::<PathBuf>(&option, value)?),
                    _ => return Err(usage(format!("unknown option {option}\n\n{USAGE}"))),
                }
            }
            let catalogue = catalogue(Path::new(&elf), svd.as_deref())?;
            let bytes = std::fs::read(&copy).map_err(|error| format!("{copy}: {error}"))?;
            if capture::is_capture(&bytes) {
                if channel.is_some() {
                    return Err("--channel names the ring of a copy; a capture names its own".into());
                }
                if spans {
                    let trace = Trace::read(&catalogue, &bytes, clock)?;
                    let mut kernel = meaning::Meaning::new(&catalogue, trace.first)?;
                    if !kernel.traced() {
                        return Err("the image has no kernel adapter events".into());
                    }
                    kernel.add(&trace.elements, 0);
                    for line in meaning::lines(&trace, &kernel) {
                        println!("{line}");
                    }
                    return Ok(());
                }
                return record::print_capture(&catalogue, &bytes, clock);
            }
            if spans {
                return Err("--meaning reads a capture, not a copy of a ring".into());
            }
            let ring = catalogue.ring(channel.as_deref())?;
            let (write_index, decoded) = decode::decode(&catalogue, ring, &bytes)?;
            decode::print(&catalogue, write_index, &decoded, clock);
            return Ok(());
        }
        Some("stats") => {
            let (Some(elf), Some(copy)) = (arguments.next(), arguments.next()) else {
                return Err(usage(USAGE));
            };
            return stats(&elf, &copy, arguments);
        }
        _ => return Err(usage(USAGE)),
    }
    let mut backend = None;
    let mut probe = None;
    let mut chip = None;
    let mut elf = None;
    let mut speed_khz = 24_000;
    let mut duration = None;
    let mut out = None;
    let mut clock = None;
    let mut channel = None;
    while let Some(option) = arguments.next() {
        let value = arguments.next();
        let given = || -> Result<String> { parsed(&option, value.clone()) };
        match option.as_str() {
            "--probe" => {
                let name = given()?;
                backend = Some(Backend::parse(&name).ok_or_else(|| usage(format!("unknown probe {name}")))?);
            }
            "--probe-id" => probe = Some(given()?),
            "--chip" => chip = Some(given()?),
            "--elf" => elf = Some(PathBuf::from(given()?)),
            "--speed" => speed_khz = parsed(&option, value)?,
            "--duration" => duration = Some(Duration::from_secs_f64(parsed(&option, value)?)),
            "--out" => out = Some(PathBuf::from(given()?)),
            "--clock" => clock = Some(parsed(&option, value)?),
            "--channel" => channel = Some(given()?),
            _ => return Err(usage(format!("unknown option {option}\n\n{USAGE}"))),
        }
    }
    let connection = Connection {
        backend: backend.ok_or_else(|| usage(USAGE))?,
        probe,
        chip,
        speed_khz,
    };
    let elf_path = elf.ok_or_else(|| usage(USAGE))?;
    let catalogue = catalogue(&elf_path, None)?;
    let ring = catalogue.ring(channel.as_deref())?;
    let mut target = target::open(&connection)?;
    record::run(target.as_mut(), &catalogue, ring, duration, out.as_deref(), clock)
}
