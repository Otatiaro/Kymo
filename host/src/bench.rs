//! The throughput bench: how fast a probe drains the ring of a running
//! target, how fast SWO carries the same events, what each keeps when both
//! run at once, and whether the cycle counter survives the probe leaving.

use crate::elf::Elf;
use crate::ring::{self, Drain, Layout, SLOT_BYTES};
use crate::swo;
use crate::target::{self, Connection, Result, Target};
use std::time::{Duration, Instant};

const DEMCR: u32 = 0xE000_EDFC;
const DEMCR_TRCENA: u32 = 1 << 24;
const DWT_CTRL: u32 = 0xE000_1000;
/// PCSAMPLENA, EXCTRCENA and the event counter packets.
const DWT_CTRL_TRACE_PACKETS: u32 = 1 << 12 | 0x7f << 16;
const SYST_RVR: u32 = 0xE000_E014;
const ITM_TER: u32 = 0xE000_0E00;
const ITM_TCR: u32 = 0xE000_0E80;
const ITM_LAR: u32 = 0xE000_0FB0;
const ITM_TCR_ITMENA: u32 = 1;
const ITM_TRACE_BUS_ID: u32 = 1 << 16;
const CORESIGHT_UNLOCK: u32 = 0xC5AC_CE55;
/// The stimulus port the bench image writes SWO events to.
const SWO_PORT: u32 = 1;
/// A 32-bit stimulus write is a header byte and four payload bytes, so an
/// event of four words takes 20 bytes on the wire.
const ITM_EVENT_BYTES: u32 = 20;
/// How long the SWO events of a step may take to arrive after it.
const SWO_TAIL: Duration = Duration::from_millis(500);
const TICKS_PER_SECOND: f64 = 1000.0;
/// A source passes a step if the target produced at least this share of its
/// nominal rate: below it, the core no longer keeps up and the step says
/// nothing about the probe.
const PRODUCED_SHARE: f64 = 0.98;
/// How long the probe stays away in the cycle counter check: short enough
/// for the ring to hold every event of the gap, at one per tick.
const DETACHED: Duration = Duration::from_millis(500);

pub struct Settings {
    pub step: Duration,
    pub max_events_per_tick: u32,
    /// SWO baud rate; no SWO measurement without it.
    pub swo_baud: Option<u32>,
}

struct Symbols {
    ring: Layout,
    events_per_tick: u32,
    ticks: u32,
    swo_events_per_tick: u32,
    swo_events: u32,
}

/// One event source over one step: the ring, or SWO.
#[derive(Default)]
struct Flow {
    events_per_tick: u32,
    produced_per_second: f64,
    received_per_second: f64,
    lost: u64,
    corrupt: u64,
}

impl Flow {
    /// Passes if nothing was lost and the target produced at least
    /// PRODUCED_SHARE of the rate asked, at the tick rate measured.
    fn passed(&self, ticks_per_second: f64) -> bool {
        let expected = f64::from(self.events_per_tick) * ticks_per_second;
        self.events_per_tick == 0
            || self.lost == 0 && self.corrupt == 0 && self.produced_per_second >= PRODUCED_SHARE * expected
    }
}

struct Step {
    ring: Flow,
    swo: Flow,
    passes_per_second: f64,
    /// Measured on the target: its clock may be off its nominal frequency.
    ticks_per_second: f64,
}

impl Step {
    /// A tick handler that outlasts its period drops ticks: the target no
    /// longer produces at the rate asked, whatever the sources say.
    fn passed(&self) -> bool {
        self.ticks_per_second >= PRODUCED_SHARE * TICKS_PER_SECOND
            && self.ring.passed(self.ticks_per_second)
            && self.swo.passed(self.ticks_per_second)
    }
}

/// The target, what the bench reads from it, and the SWO decoder once SWO
/// is on.
struct Bench<'a> {
    target: &'a mut dyn Target,
    symbols: &'a Symbols,
    duration: Duration,
    swo: Option<swo::Decoder>,
    bytes: Vec<u8>,
}

pub fn run(connection: &Connection, elf: &Elf, settings: &Settings) -> Result<()> {
    let symbols = Symbols {
        ring: Layout::from_symbol(elf.symbol("kymo_ring")?)?,
        events_per_tick: elf.symbol("kymo_bench_events_per_tick")?.address,
        ticks: elf.symbol("kymo_bench_ticks")?.address,
        swo_events_per_tick: elf.symbol("kymo_bench_swo_events_per_tick")?.address,
        swo_events: elf.symbol("kymo_bench_swo_events")?.address,
    };
    let mut target = target::open(connection)?;
    println!(
        "probe {}{}; ring of {} slots ({} bytes) at {:#010x}",
        connection.backend.name(),
        target::speed_text(target.as_ref()),
        symbols.ring.slots,
        symbols.ring.bytes(),
        symbols.ring.address
    );
    target.write_word(symbols.events_per_tick, 0)?;
    target.write_word(symbols.swo_events_per_tick, 0)?;
    check_running(target.as_mut(), &symbols)?;
    raw_ceiling(target.as_mut(), &symbols, settings.step)?;
    let mut bench = Bench {
        target: target.as_mut(),
        symbols: &symbols,
        duration: settings.step,
        swo: None,
        bytes: Vec::new(),
    };
    let result = measure(&mut bench, settings);
    target.write_word(symbols.events_per_tick, 0)?;
    target.write_word(symbols.swo_events_per_tick, 0)?;
    result?;
    cycle_counter(connection, target, &symbols)
}

/// SWD alone; with SWO, SWO alone, then SWD with SWO at its highest rate.
fn measure(bench: &mut Bench, settings: &Settings) -> Result<()> {
    let max = settings.max_events_per_tick;
    println!("\nSWD alone");
    print_header();
    let swd = highest(max, |rate| bench.step(rate, 0))?;
    print_highest("SWD", swd);
    let Some(baud) = settings.swo_baud else {
        return Ok(());
    };

    let clock = tpiu_clock(bench.target)?;
    bench.target.swo_start(clock, baud)?;
    itm_bench_only(bench.target)?;
    bench.swo = Some(swo::Decoder::new());
    println!(
        "\nSWO alone, {baud} Bd from a {clock} Hz TPIU clock: {:.0} kB/s on the wire, \
         {:.0} kB/s of events at most",
        f64::from(baud) / 10.0 / 1000.0,
        f64::from(baud) / 10.0 / f64::from(ITM_EVENT_BYTES) * f64::from(SLOT_BYTES) / 1000.0
    );
    print_header();
    let swo = highest(max, |rate| bench.step(0, rate))?;
    print_highest("SWO", swo);
    if swo.0 == 0 {
        return Ok(());
    }

    // Where SWD and SWO share a bottleneck (USB, the probe, the host loop),
    // the SWD rate falls as the SWO rate rises.
    let mut frontier = Vec::new();
    for quarter in 1..=4 {
        let swo_rate = (swo.0 * quarter / 4).max(1);
        println!("\nSWD with SWO at {swo_rate} events/tick");
        print_header();
        let swd = highest(max, |rate| bench.step(rate, swo_rate))?;
        frontier.push((swo_rate, swd.0));
    }
    println!("\nwithout loss on both:");
    for (swo_rate, swd_rate) in frontier {
        println!(
            "  SWO {swo_rate:>3} events/tick ({:>4.0} kB/s) + SWD {swd_rate:>3} events/tick ({:>4.0} kB/s) = {:>4.0} kB/s",
            nominal_kb(swo_rate),
            nominal_kb(swd_rate),
            nominal_kb(swo_rate + swd_rate)
        );
    }
    Ok(())
}

/// The bench image clocks SysTick from the core clock, which also clocks
/// the TPIU.
fn tpiu_clock(target: &mut dyn Target) -> Result<u32> {
    Ok((target.read_word(SYST_RVR)? + 1) * TICKS_PER_SECOND as u32)
}

/// ITM down to the bench port, without timestamps, synchronization or DWT
/// packets, whatever the probe library enabled: the SWO bandwidth goes to
/// the events.
fn itm_bench_only(target: &mut dyn Target) -> Result<()> {
    target.write_word(ITM_LAR, CORESIGHT_UNLOCK)?;
    target.write_word(ITM_TCR, ITM_TCR_ITMENA | ITM_TRACE_BUS_ID)?;
    target.write_word(ITM_TER, 1 << SWO_PORT)?;
    let control = target.read_word(DWT_CTRL)?;
    target.write_word(DWT_CTRL, control & !DWT_CTRL_TRACE_PACKETS)?;
    Ok(())
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
/// that passes, within 1/16. Returns it, and whether some rate failed.
fn highest(max: u32, mut step: impl FnMut(u32) -> Result<Step>) -> Result<(u32, bool)> {
    let mut passed = 0;
    let mut failed = None;
    let mut rate = 1;
    while rate <= max {
        if step(rate)?.passed() {
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
            if step(middle)?.passed() {
                passed = middle;
            } else {
                failed = middle;
            }
        }
    }
    Ok((passed, failed.is_some()))
}

fn print_header() {
    println!("ring ev/tick  SWO ev/tick  ring kB/s  ring lost  SWO wire  SWO kB/s  SWO lost  corrupt  passes/s");
}

fn print_highest(what: &str, (passed, limited): (u32, bool)) {
    match (passed, limited) {
        (0, _) => println!("{what}: no rate without loss"),
        (_, true) => println!(
            "{what}: highest rate without loss {passed} events/tick, {:.0} kB/s",
            nominal_kb(passed)
        ),
        (_, false) => println!(
            "{what}: no loss up to the bench limit of {passed} events/tick ({:.0} kB/s)",
            nominal_kb(passed)
        ),
    }
}

fn nominal_kb(events_per_tick: u32) -> f64 {
    f64::from(events_per_tick) * TICKS_PER_SECOND * f64::from(SLOT_BYTES) / 1000.0
}

impl Bench<'_> {
    /// Decodes what SWO captured, if it is on.
    fn read_swo(&mut self) -> Result<()> {
        if let Some(decoder) = &mut self.swo {
            self.bytes.clear();
            self.target.swo_read(&mut self.bytes)?;
            decoder.push(&self.bytes);
        }
        Ok(())
    }

    /// Waits for `duration`, reading SWO meanwhile if it is on.
    fn idle(&mut self, duration: Duration) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < duration {
            self.read_swo()?;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    fn step(&mut self, ring_rate: u32, swo_rate: u32) -> Result<Step> {
        let symbols = self.symbols;
        self.target.write_word(symbols.events_per_tick, ring_rate)?;
        self.target.write_word(symbols.swo_events_per_tick, swo_rate)?;
        self.idle(Duration::from_millis(100))?;
        let ticks_before = self.target.read_word(symbols.ticks)?;
        let swo_first = self.target.read_word(symbols.swo_events)?;
        if let Some(decoder) = &mut self.swo {
            decoder.restart(swo_first);
        }
        let mut drain = Drain::start(self.target, symbols.ring)?;
        let first = drain.next();
        let start = Instant::now();
        while start.elapsed() < self.duration {
            if ring_rate != 0 {
                drain.pass(self.target)?;
            }
            self.read_swo()?;
        }
        // Write index, ticks and SWO count read back to back, as at the start.
        let last = self.target.read_word(symbols.ring.address)?;
        let ticks = self.target.read_word(symbols.ticks)?.wrapping_sub(ticks_before);
        let swo_last = self.target.read_word(symbols.swo_events)?;
        let elapsed = start.elapsed().as_secs_f64();
        drain.finish(self.target)?;
        // Every byte captured during the step, whether it decodes or not.
        let swo_wire = self.swo.as_ref().map_or(0, |decoder| decoder.counts.bytes);
        let swo = self.swo_flow(swo_rate, swo_first, swo_last, elapsed)?;

        let counts = drain.counts;
        let result = Step {
            ring: Flow {
                events_per_tick: ring_rate,
                produced_per_second: f64::from(last.wrapping_sub(first)) / elapsed,
                received_per_second: counts.received as f64 / elapsed,
                lost: counts.lost,
                corrupt: counts.corrupt,
            },
            swo,
            passes_per_second: counts.passes as f64 / elapsed,
            ticks_per_second: f64::from(ticks) / elapsed,
        };
        let kb = |events: f64| events * f64::from(SLOT_BYTES) / 1000.0;
        println!(
            "{:>12}  {:>11}  {:>9.0}  {:>9}  {:>8.0}  {:>8.0}  {:>8}  {:>7}  {:>8.0}{}",
            ring_rate,
            swo_rate,
            kb(result.ring.received_per_second),
            result.ring.lost,
            swo_wire as f64 / elapsed / 1000.0,
            kb(result.swo.received_per_second),
            result.swo.lost,
            result.ring.corrupt + result.swo.corrupt,
            result.passes_per_second,
            if result.passed() { "" } else { "  <- fails" }
        );
        Ok(result)
    }

    /// The SWO events produced in `first..last`: reads on until they have
    /// all arrived, or SWO_TAIL has passed.
    fn swo_flow(&mut self, rate: u32, first: u32, last: u32, elapsed: f64) -> Result<Flow> {
        let produced = u64::from(last.wrapping_sub(first));
        let Some(decoder) = &mut self.swo else {
            return Ok(Flow::default());
        };
        decoder.until(last);
        let tail = Instant::now();
        while tail.elapsed() < SWO_TAIL
            && self
                .swo
                .as_ref()
                .is_some_and(|decoder| decoder.counts.received < produced)
        {
            self.read_swo()?;
        }
        let counts = self.swo.as_ref().map(|decoder| decoder.counts).unwrap_or_default();
        Ok(Flow {
            events_per_tick: rate,
            produced_per_second: produced as f64 / elapsed,
            received_per_second: counts.received as f64 / elapsed,
            lost: produced.saturating_sub(counts.received),
            corrupt: counts.corrupt + counts.overflows,
        })
    }
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
        "\ncycle counter: {attached:.0} cycles/tick attached, {across:.0} cycles/tick across {} ticks detached",
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
            if demcr_reattached & DEMCR_TRCENA == 0 {
                "; TRCENA is clear"
            } else {
                ""
            }
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
            if missing > 0 {
                format!(" ({missing} events no longer in the ring)")
            } else {
                String::new()
            }
        ),
        _ => println!("  no tick with a still timestamp in the ring ({missing} events no longer in it)"),
    }
}
