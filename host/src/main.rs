//! Kymo command line.

use kymo::target::{self, Backend, Connection, Result};
use kymo::{bench, capture, decode, elf, events, record};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
usage: kymo events <elf>
       kymo decode <elf> <ring or capture> [--channel <ring>] [--clock <Hz>]
       kymo record --probe <probe-rs|jlink|stlink|qemu> --elf <file> [options]
       kymo bench --probe <probe-rs|jlink|stlink> --elf <file> [options]

events: lists the event types of an image, read from its ELF: each type id,
the type it stands for, its size, and each field's offset, size and type.

decode: lists the events a copy of a trace ring holds, oldest first; the
copy is the bytes of a kymo::ring object, dumped from a stopped target
running the image. Given a capture made by kymo record --out, prints what
kymo record printed while making it, but for the seconds: they are
recomputed over the whole capture.

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
\"suspect <from> <to> <n>\" reports n writers the ring lapped while its write
index went from <from> to <to>, where the probe could not tell which events
they wrote into: events recorded near it may hold their bytes.

record options:
  --channel <ring>   the kymo::ring object to drain, as for decode
  --probe-id, --chip, --speed as for bench; qemu: --probe-id <host>:<port>,
                     QEMU's monitor started with -qmp tcp:<host>:<port>,server=on,wait=off
  --duration <seconds>  stop after this long
  --out <file>       also keep the recording in this capture file
  --clock <Hz>       the cycle counter's frequency, kept in the capture;
                     without it, measured

bench: measures how fast a probe drains the trace ring of the bench image,
and how fast SWO carries the same events with --swo-baud, and whether the
cycle counter keeps counting once the probe leaves.

bench options:
  --probe-id <id>    probe-rs: VID:PID[:SERIAL]; jlink, stlink: serial number
  --chip <name>      target name for probe-rs and J-Link (e.g. STM32H563ZITx)
  --speed <kHz>      debug clock, or the closest the probe supports
                     below it (default 24000)
  --step <seconds>   duration of each measurement (default 2)
  --max-rate <n>     highest rate tried, in events per tick (default 1024)
  --swo-baud <Bd>    also measure SWO at this baud rate, alone then alongside
                     SWD (probe-rs and jlink)
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The catalogue of the image at `path`, its warnings printed.
fn catalogue(path: &Path) -> Result<events::Catalogue> {
    let catalogue = events::read(path)?;
    for warning in &catalogue.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(catalogue)
}

fn run() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next();
    match command.as_deref() {
        Some("bench" | "record") => {}
        Some("events") => {
            let elf = match (arguments.next(), arguments.next()) {
                (Some(elf), None) => PathBuf::from(elf),
                _ => return Err(USAGE.into()),
            };
            events::print(&catalogue(&elf)?);
            return Ok(());
        }
        Some("decode") => {
            let (Some(elf), Some(copy)) = (arguments.next(), arguments.next()) else {
                return Err(USAGE.into());
            };
            let mut channel = None;
            let mut clock = None;
            while let Some(option) = arguments.next() {
                let value = arguments.next().ok_or_else(|| format!("{option} needs a value"))?;
                match option.as_str() {
                    "--channel" => channel = Some(value),
                    "--clock" => clock = Some(value.parse()?),
                    _ => return Err(format!("unknown option {option}\n\n{USAGE}").into()),
                }
            }
            let catalogue = catalogue(Path::new(&elf))?;
            let bytes = std::fs::read(&copy).map_err(|error| format!("{copy}: {error}"))?;
            if capture::is_capture(&bytes) {
                if channel.is_some() {
                    return Err("--channel names the ring of a copy; a capture names its own".into());
                }
                return record::print_capture(&catalogue, &bytes, clock);
            }
            let ring = catalogue.ring(channel.as_deref())?;
            let (write_index, decoded) = decode::decode(&catalogue, ring, &bytes)?;
            decode::print(&catalogue, write_index, &decoded, clock);
            return Ok(());
        }
        _ => return Err(USAGE.into()),
    }
    let mut backend = None;
    let mut probe = None;
    let mut chip = None;
    let mut elf = None;
    let mut speed_khz = 24_000;
    let mut step = 2.0;
    let mut max_events_per_tick = 1024;
    let mut swo_baud = None;
    let mut duration = None;
    let mut out = None;
    let mut clock = None;
    let mut channel = None;
    while let Some(option) = arguments.next() {
        let mut value = || arguments.next().ok_or_else(|| format!("{option} needs a value"));
        match option.as_str() {
            "--probe" => {
                let name = value()?;
                backend = Some(Backend::parse(&name).ok_or_else(|| format!("unknown probe {name}"))?);
            }
            "--probe-id" => probe = Some(value()?),
            "--chip" => chip = Some(value()?),
            "--elf" => elf = Some(PathBuf::from(value()?)),
            "--speed" => speed_khz = value()?.parse()?,
            "--step" => step = value()?.parse()?,
            "--max-rate" => max_events_per_tick = value()?.parse()?,
            "--swo-baud" => swo_baud = Some(value()?.parse()?),
            "--duration" => duration = Some(Duration::from_secs_f64(value()?.parse()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--clock" => clock = Some(value()?.parse()?),
            "--channel" => channel = Some(value()?),
            _ => return Err(format!("unknown option {option}\n\n{USAGE}").into()),
        }
    }
    let connection = Connection {
        backend: backend.ok_or(USAGE)?,
        probe,
        chip,
        speed_khz,
    };
    let elf_path = elf.ok_or(USAGE)?;
    if command.as_deref() == Some("record") {
        let catalogue = catalogue(&elf_path)?;
        let ring = catalogue.ring(channel.as_deref())?;
        let mut target = target::open(&connection)?;
        return record::run(target.as_mut(), &catalogue, ring, duration, out.as_deref(), clock);
    }
    let elf = elf::Elf::read(&elf_path)?;
    let settings = bench::Settings {
        step: Duration::from_secs_f64(step),
        max_events_per_tick,
        swo_baud,
    };
    bench::run(&connection, &elf, &settings)
}
