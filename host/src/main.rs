//! Kymo host.

mod bench;
mod capture;
mod decode;
mod elf;
mod events;
mod record;
mod ring;
mod swo;
mod symbols;
mod target;
mod time;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use target::{Backend, Connection, Result};

const USAGE: &str = "\
usage: kymo events <elf>
       kymo decode <elf> <ring or capture> [--clock <Hz>]
       kymo record --probe <probe-rs|jlink|stlink|qemu> --elf <file> [options]
       kymo bench --probe <probe-rs|jlink|stlink> --elf <file> [options]

events: lists the event types of an image, read from its ELF: each type id,
the type it stands for, its size, and each field's offset, size and type.

decode: lists the events a copy of the trace ring holds, oldest first; the
copy is the bytes of kymo::trace_ring, dumped from a stopped target running
the image. Given a capture made by kymo record --out, prints what kymo
record printed while making it, but for the seconds: they are recomputed
over the whole capture.

Each event line reads: index, cycles (the cycle counter timestamp extended
to 64 bits), seconds since the first event (- without a frequency), type,
fields. The frequency is --clock's, or the one recorded, or else measured
from the host time of the probe's passes once the events span a second.

record: prints the events of a running target as it writes them, drained
from its trace ring through the probe, and the indexes lost when the ring
overtook the probe; stops on Enter, or after --duration.

record options:
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
            events::print(&events::read(&elf)?);
            return Ok(());
        }
        Some("decode") => {
            let arguments: Vec<String> = arguments.collect();
            let (elf, ring, clock) = match arguments.as_slice() {
                [elf, ring] => (elf, ring, None),
                [elf, ring, option, hz] if option == "--clock" => (elf, ring, Some(hz.parse()?)),
                _ => return Err(USAGE.into()),
            };
            let catalogue = events::read(&PathBuf::from(elf))?;
            let bytes = std::fs::read(ring).map_err(|error| format!("{ring}: {error}"))?;
            if capture::is_capture(&bytes) {
                return record::print_capture(&catalogue, &bytes, clock);
            }
            let (write_index, decoded) = decode::decode(&catalogue, &bytes)?;
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
        let catalogue = events::read(&elf_path)?;
        let ring = catalogue.ring.as_ref().ok_or("the image has no kymo::trace_ring")?;
        let symbol = elf::Elf::read(&elf_path)?.symbol("_ZN4kymo10trace_ringE")?;
        if u64::from(symbol.size) != ring.size {
            return Err(format!(
                "kymo::trace_ring: {} bytes in the symbol table, {} in DWARF",
                symbol.size, ring.size
            )
            .into());
        }
        let mut target = target::open(&connection)?;
        return record::run(
            target.as_mut(),
            &catalogue,
            symbol.address,
            duration,
            out.as_deref(),
            clock,
        );
    }
    let elf = elf::Elf::read(&elf_path)?;
    let settings = bench::Settings {
        step: Duration::from_secs_f64(step),
        max_events_per_tick,
        swo_baud,
    };
    bench::run(&connection, &elf, &settings)
}
