//! Time: an event's timestamp is the low 32 bits of the target's cycle
//! counter. A timeline extends the timestamps of a run of events to 64 bits,
//! and turns them into seconds since the first.
//!
//! Two consecutive events are the signed 32-bit difference of their
//! timestamps apart: an event preempted between reserving its slots and
//! taking its timestamp may be slightly older than the one before it. Over a
//! recording, the passes also tell when each event was written: it lies after
//! the last pass whose write index was not past it, and before the first pass
//! whose write index was, and is taken to be halfway. Once the frequency is
//! known, the host time between two events gives the turns of the counter
//! between them, however many there are.
//!
//! The frequency is the one given, or else measured: the slope of the events'
//! cycles against their host time, once they span a second.

use std::collections::VecDeque;

/// One turn of the 32-bit counter.
const TURN: f64 = 4_294_967_296.0;
/// The host time the events must span before their frequency is measured.
const MEASURED_SPAN_NS: f64 = 1e9;

pub struct Timeline {
    given: Option<f64>,
    /// The host time of the first pass, which host times count from.
    base_ns: Option<u64>,
    /// The passes that may still bound an event: host time from `base_ns`,
    /// write index read.
    passes: VecDeque<(f64, u32)>,
    last: Option<Last>,
    /// The cycles of the first event.
    origin: i64,
    fit: Fit,
}

#[derive(Clone, Copy)]
struct Last {
    timestamp: u32,
    cycles: i64,
    host: Option<f64>,
}

/// The least-squares slope of cycles against host time, kept as running
/// means and sums of products of deviations.
#[derive(Default)]
struct Fit {
    count: f64,
    first: f64,
    last: f64,
    mean_host: f64,
    mean_cycles: f64,
    products: f64,
    squares: f64,
}

impl Fit {
    fn add(&mut self, host: f64, cycles: f64) {
        if self.count == 0.0 {
            self.first = host;
        }
        self.last = host;
        self.count += 1.0;
        let deviation = host - self.mean_host;
        self.mean_host += deviation / self.count;
        self.mean_cycles += (cycles - self.mean_cycles) / self.count;
        self.products += deviation * (cycles - self.mean_cycles);
        self.squares += deviation * (host - self.mean_host);
    }

    /// Cycles per second, once the events span `MEASURED_SPAN_NS`.
    fn frequency(&self) -> Option<f64> {
        let hz = self.products / self.squares * 1e9;
        (self.last - self.first >= MEASURED_SPAN_NS && hz > 0.0).then_some(hz)
    }
}

impl Timeline {
    /// A timeline at `clock` hertz, or at the frequency it measures.
    pub fn new(clock: Option<u64>) -> Self {
        Self {
            given: clock.map(|hz| hz as f64),
            base_ns: None,
            passes: VecDeque::new(),
            last: None,
            origin: 0,
            fit: Fit::default(),
        }
    }

    /// A pass of a recording: its host time, and the write index it read.
    pub fn pass(&mut self, host_ns: u64, write_index: u32) {
        let base = *self.base_ns.get_or_insert(host_ns);
        let host = host_ns.saturating_sub(base) as f64;
        // Of the passes that read one write index, the first is the best
        // upper bound and the last the best lower bound.
        let same = |pass: Option<&(f64, u32)>| pass.is_some_and(|pass| pass.1 == write_index);
        if same(self.passes.back()) && same(self.passes.iter().nth_back(1)) {
            self.passes.pop_back();
        }
        self.passes.push_back((host, write_index));
    }

    /// The host time the event at `index` was written, as the passes bound it.
    fn host(&mut self, index: u32) -> Option<f64> {
        let past = |pass: &(f64, u32)| (pass.1.wrapping_sub(index) as i32) > 0;
        while self.passes.get(1).is_some_and(|pass| !past(pass)) {
            self.passes.pop_front();
        }
        match (self.passes.front(), self.passes.get(1)) {
            (Some(first), _) if past(first) => Some(first.0),
            (Some(lower), Some(upper)) => Some((lower.0 + upper.0) / 2.0),
            _ => None,
        }
    }

    /// The cycles of the event at `index`, its timestamp extended to 64 bits;
    /// events come in index order.
    pub fn event(&mut self, index: u32, timestamp: u32) -> i64 {
        let host = self.host(index);
        let cycles = match self.last {
            None => {
                self.origin = i64::from(timestamp);
                self.origin
            }
            Some(last) => {
                let delta = i64::from(timestamp.wrapping_sub(last.timestamp) as i32);
                let turns = match (self.frequency(), host, last.host) {
                    (Some(hz), Some(now), Some(then)) => ((now - then) * hz / 1e9 - delta as f64) / TURN,
                    _ => 0.0,
                };
                last.cycles + delta + ((turns.round() as i64) << 32)
            }
        };
        if let Some(host) = host {
            self.fit.add(host, cycles as f64);
        }
        self.last = Some(Last {
            timestamp,
            cycles,
            host,
        });
        cycles
    }

    /// The frequency given, or else the one measured so far.
    pub fn frequency(&self) -> Option<f64> {
        self.given.or_else(|| self.fit.frequency())
    }

    /// The frequency measured over the events so far, and the seconds they span.
    pub fn measured(&self) -> Option<(f64, f64)> {
        self.fit
            .frequency()
            .map(|hz| (hz, (self.fit.last - self.fit.first) / 1e9))
    }

    /// The seconds from the first event to `cycles`, at `frequency`.
    pub fn seconds(&self, cycles: i64, frequency: Option<f64>) -> Option<f64> {
        frequency.map(|hz| (cycles - self.origin) as f64 / hz)
    }
}

#[cfg(test)]
mod tests;
