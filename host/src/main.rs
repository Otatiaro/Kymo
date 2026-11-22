//! Kymo host.

mod bench;
mod decode;
mod elf;
mod events;
mod ring;
mod swo;
mod target;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use target::{Backend, Connection, Result};

const USAGE: &str = "\
usage: kymo events <elf>
       kymo decode <elf> <ring>
       kymo bench --probe <probe-rs|jlink|stlink> --elf <file> [options]

events: lists the event types of an image, read from its ELF: each type id,
the type it stands for, its size, and each field's offset, size and type.

decode: lists the events a copy of the trace ring holds, oldest first; the
copy is the bytes of kymo::trace_ring, dumped from a stopped target running
the image.

bench: measures how fast a probe drains the trace ring of the bench image,
and how fast SWO carries the same events with --swo-baud, and whether the
cycle counter keeps counting once the probe leaves.

bench options:
  --probe-id <id>    probe-rs: VID:PID[:SERIAL]; jlink, stlink: serial number
  --chip <name>      target name for probe-rs and J-Link (e.g. STM32H563ZITx)
  --speed <kHz>      debug clock (default 4000)
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

fn run() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("bench") => {}
        Some("events") => {
            let elf = match (arguments.next(), arguments.next()) {
                (Some(elf), None) => PathBuf::from(elf),
                _ => return Err(USAGE.into()),
            };
            events::print(&events::read(&elf)?);
            return Ok(());
        }
        Some("decode") => {
            let (elf, ring) = match (arguments.next(), arguments.next(), arguments.next()) {
                (Some(elf), Some(ring), None) => (PathBuf::from(elf), PathBuf::from(ring)),
                _ => return Err(USAGE.into()),
            };
            let catalogue = events::read(&elf)?;
            let bytes = std::fs::read(&ring).map_err(|error| format!("{}: {error}", ring.display()))?;
            let (write_index, decoded) = decode::decode(&catalogue, &bytes)?;
            decode::print(write_index, &decoded);
            return Ok(());
        }
        _ => return Err(USAGE.into()),
    }
    let mut backend = None;
    let mut probe = None;
    let mut chip = None;
    let mut elf = None;
    let mut speed_khz = 4000;
    let mut step = 2.0;
    let mut max_events_per_tick = 1024;
    let mut swo_baud = None;
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
            _ => return Err(format!("unknown option {option}\n\n{USAGE}").into()),
        }
    }
    let connection = Connection {
        backend: backend.ok_or(USAGE)?,
        probe,
        chip,
        speed_khz,
    };
    let elf = elf::Elf::read(&elf.ok_or(USAGE)?)?;
    let settings = bench::Settings {
        step: Duration::from_secs_f64(step),
        max_events_per_tick,
        swo_baud,
    };
    bench::run(&connection, &elf, &settings)
}
