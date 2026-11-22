//! The throughput bench: how fast a probe drains the ring of a running
//! target, and whether the cycle counter survives the probe leaving.

use crate::elf::Elf;
use crate::ring::{self, Drain, Layout, SLOT_BYTES};
use crate::target::{self, Connection, Result, Target};
use std::time::{Duration, Instant};

const DEMCR: u32 = 0xE000_EDFC;
const DEMCR_TRCENA: u32 = 1 << 24;
const DWT_CTRL: u32 = 0xE000_1000;
const TICKS_PER_SECOND: f64 = 1000.0;
/// A step passes if the target produced at least this share of its nominal
/// rate: below it, the core no longer keeps up and the step says nothing
/// about the probe.
const PRODUCED_SHARE: f64 = 0.98;
/// How long the probe stays away in the cycle counter check: short enough
/// for the ring to hold every event of the gap, at one per tick.
const DETACHED: Duration = Duration::from_millis(500);

pub struct Settings {
    pub step: Duration,
    pub max_events_per_tick: u32,
}

struct Symbols {
    ring: Layout,
    events_per_tick: u32,
    ticks: u32,
}

struct Step {
    events_per_tick: u32,
    produced_per_second: f64,
    received_per_second: f64,
    lost: u64,
    corrupt: u64,
    passes_per_second: f64,
    /// Measured on the target: its clock may be off its nominal frequency.
    ticks_per_second: f64,
}

impl Step {
    fn passed(&self) -> bool {
        let expected = f64::from(self.events_per_tick) * self.ticks_per_second;
        self.lost == 0 && self.corrupt == 0 && self.produced_per_second >= PRODUCED_SHARE * expected
    }
}

pub fn run(connection: &Connection, elf: &Elf, settings: &Settings) -> Result<()> {
    let symbols = Symbols {
        ring: Layout::from_symbol(elf.symbol("kymo_ring")?)?,
        events_per_tick: elf.symbol("kymo_bench_events_per_tick")?.address,
        ticks: elf.symbol("kymo_bench_ticks")?.address,
    };
    println!(
        "probe {} at {} kHz; ring of {} slots ({} bytes) at {:#010x}",
        connection.backend.name(),
        connection.speed_khz,
        symbols.ring.slots,
        symbols.ring.bytes(),
        symbols.ring.address
    );

    let mut target = target::open(connection)?;
    target.write_word(symbols.events_per_tick, 0)?;
    check_running(target.as_mut(), &symbols)?;
    raw_ceiling(target.as_mut(), &symbols, settings.step)?;
    let result = steps(target.as_mut(), &symbols, settings);
    target.write_word(symbols.events_per_tick, 0)?;
    result?;
    cycle_counter(connection, target, &symbols)
}

fn check_running(target: &mut dyn Target, symbols: &Symbols) -> Result<()> {
    let before = target.read_word(symbols.ticks)?;
    std::thread::sleep(Duration::from_millis(50));
    let after = target.read_word(symbols.ticks)?;
    if after == before {
        return Err("kymo_bench_ticks does not advance: is the bench image running?".into());
    }
    Ok(())
}

/// Reading the whole ring back to back, and the round trip of one word.
fn raw_ceiling(target: &mut dyn Target, symbols: &Symbols, duration: Duration) -> Result<()> {
    let mut words = vec![0u32; symbols.ring.bytes() as usize / 4];
    let (reads, elapsed) = repeat(duration, || target.read_words(symbols.ring.address, &mut words))?;
    println!(
        "raw read of {} bytes: {:.1} kB/s ({:.0} reads/s)",
        symbols.ring.bytes(),
        reads as f64 * f64::from(symbols.ring.bytes()) / elapsed / 1000.0,
        reads as f64 / elapsed
    );
    let (reads, elapsed) = repeat(duration, || target.read_word(symbols.ring.address).map(|_| ()))?;
    println!(
        "one-word read: {:.0} reads/s, {:.0} us each",
        reads as f64 / elapsed,
        elapsed / reads as f64 * 1e6
    );
    Ok(())
}

fn repeat(duration: Duration, mut operation: impl FnMut() -> Result<()>) -> Result<(u64, f64)> {
    let start = Instant::now();
    let mut count = 0u64;
    while start.elapsed() < duration {
        operation()?;
        count += 1;
    }
    Ok((count, start.elapsed().as_secs_f64()))
}

/// Doubles the rate until a step fails, then bisects to the highest rate
/// drained without loss, within 1/16.
fn steps(target: &mut dyn Target, symbols: &Symbols, settings: &Settings) -> Result<()> {
    println!("events/tick  nominal kB/s  produced ev/s  received ev/s   lost  corrupt  passes/s");
    let mut passed = 0;
    let mut failed = None;
    let mut rate = 1;
    while rate <= settings.max_events_per_tick {
        if step(target, symbols, rate, settings.step)?.passed() {
            passed = rate;
            rate *= 2;
        } else {
            failed = Some(rate);
            break;
        }
    }
    if let Some(mut failed) = failed {
        while failed - passed > 1.max(passed / 16) {
            let middle = passed + (failed - passed) / 2;
            if step(target, symbols, middle, settings.step)?.passed() {
                passed = middle;
            } else {
                failed = middle;
            }
        }
    }
    match (passed, failed) {
        (0, _) => println!("no rate drained without loss"),
        (_, Some(_)) => println!(
            "highest rate without loss: {passed} events/tick, {:.0} kB/s",
            nominal_kb(passed)
        ),
        (_, None) => println!(
            "no loss up to the bench limit of {passed} events/tick ({:.0} kB/s)",
            nominal_kb(passed)
        ),
    }
    Ok(())
}

fn nominal_kb(events_per_tick: u32) -> f64 {
    f64::from(events_per_tick) * TICKS_PER_SECOND * f64::from(SLOT_BYTES) / 1000.0
}

fn step(target: &mut dyn Target, symbols: &Symbols, events_per_tick: u32, duration: Duration) -> Result<Step> {
    target.write_word(symbols.events_per_tick, events_per_tick)?;
    std::thread::sleep(Duration::from_millis(100));
    let ticks_before = target.read_word(symbols.ticks)?;
    let mut drain = Drain::start(target, symbols.ring)?;
    let first = drain.next();
    let start = Instant::now();
    while start.elapsed() < duration {
        drain.pass(target)?;
    }
    // Write index and ticks read back to back, as at the start.
    let last = target.read_word(symbols.ring.address)?;
    let ticks = target.read_word(symbols.ticks)?.wrapping_sub(ticks_before);
    let elapsed = start.elapsed().as_secs_f64();
    drain.finish(target)?;
    let counts = drain.counts;
    let result = Step {
        events_per_tick,
        produced_per_second: f64::from(last.wrapping_sub(first)) / elapsed,
        received_per_second: counts.received as f64 / elapsed,
        lost: counts.lost,
        corrupt: counts.corrupt,
        passes_per_second: counts.passes as f64 / elapsed,
        ticks_per_second: f64::from(ticks) / elapsed,
    };
    println!(
        "{:>11}  {:>12.0}  {:>13.0}  {:>13.0}  {:>5}  {:>7}  {:>8.0}{}",
        result.events_per_tick,
        nominal_kb(events_per_tick),
        result.produced_per_second,
        result.received_per_second,
        result.lost,
        result.corrupt,
        result.passes_per_second,
        if result.passed() { "" } else { "  <- fails" }
    );
    Ok(result)
}

/// Cycles per event between two newest events, at one event per tick.
fn cycles_per_tick(from: (u32, u32), to: (u32, u32)) -> f64 {
    f64::from(to.1.wrapping_sub(from.1)) / f64::from(to.0.wrapping_sub(from.0))
}

/// Whether DWT_CYCCNT, which timestamps every event, keeps counting while no
/// probe is attached. The target emits one event per SysTick tick, and
/// SysTick does not depend on the debug logic: the timestamps the target
/// wrote must advance by as many cycles per tick across the gap as while
/// attached. The timestamps are read rather than DWT_CYCCNT itself, which a
/// probe library may cache (the J-Link library does, for one-word reads).
fn cycle_counter(connection: &Connection, mut target: Box<dyn Target>, symbols: &Symbols) -> Result<()> {
    target.write_word(symbols.events_per_tick, 1)?;
    std::thread::sleep(Duration::from_millis(50));
    let e0 = ring::newest(target.as_mut(), symbols.ring)?;
    std::thread::sleep(Duration::from_millis(200));
    let e1 = ring::newest(target.as_mut(), symbols.ring)?;
    let attached = cycles_per_tick(e0, e1);
    let demcr_attached = target.read_word(DEMCR)?;
    let control_attached = target.read_word(DWT_CTRL)?;

    drop(target);
    std::thread::sleep(DETACHED);
    let mut target = target::open(connection)?;
    let e2 = ring::newest(target.as_mut(), symbols.ring)?;
    let demcr_reattached = target.read_word(DEMCR)?;
    let control_reattached = target.read_word(DWT_CTRL)?;
    target.write_word(symbols.events_per_tick, 0)?;
    let gap = ring::timestamps(target.as_mut(), symbols.ring, e1.0, e2.0)?;
    let across = cycles_per_tick(e1, e2);

    println!(
        "cycle counter: {attached:.0} cycles/tick attached, {across:.0} cycles/tick across {} ticks detached",
        e2.0.wrapping_sub(e1.0)
    );
    println!(
        "  DEMCR {demcr_attached:#010x}, DWT_CTRL {control_attached:#010x} attached; \
         DEMCR {demcr_reattached:#010x}, DWT_CTRL {control_reattached:#010x} after reattaching"
    );
    if attached == 0.0 {
        println!("the timestamps do not advance while the probe is attached");
        return Ok(());
    }
    let share = across / attached;
    if share > 0.99 {
        println!("the cycle counter kept counting while detached");
    } else {
        println!(
            "the cycle counter stopped while detached ({:.0} % of the expected cycles){}",
            share * 100.0,
            if demcr_reattached & DEMCR_TRCENA == 0 { "; TRCENA is clear" } else { "" }
        );
        print_frozen(&gap, attached);
    }
    Ok(())
}

/// Where in the gap the timestamps stood still: ticks whose timestamp did
/// not move from the previous one, counted from the last event read
/// before detaching.
fn print_frozen(gap: &[Option<u32>], cycles_per_tick: f64) {
    let moved = |pair: &[Option<u32>]| match pair {
        [Some(a), Some(b)] => Some(f64::from(b.wrapping_sub(*a)) > cycles_per_tick / 2.0),
        _ => None,
    };
    let still: Vec<usize> = gap
        .windows(2)
        .enumerate()
        .filter_map(|(tick, pair)| (moved(pair) == Some(false)).then_some(tick + 1))
        .collect();
    let missing = gap.iter().filter(|stamp| stamp.is_none()).count();
    match (still.first(), still.last()) {
        (Some(first), Some(last)) => println!(
            "  timestamps stood still on {} of {} ticks, from tick {first} to tick {last}{}",
            still.len(),
            gap.len() - 1,
            if missing > 0 { format!(" ({missing} events no longer in the ring)") } else { String::new() }
        ),
        _ => println!("  no tick with a still timestamp in the ring ({missing} events no longer in it)"),
    }
}
