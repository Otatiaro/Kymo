//! Recording: the events of a running target, drained from its trace ring
//! through the probe as they are written.
//!
//! Each pass reads the write index, settles what the previous pass read
//! against it, then reads the slots written since. The ring's rule
//! (target/include/kymo/ring.hpp) decides what a read holds: an event read
//! with its first slot at index `i` is intact if the write index read after it
//! has not gone past `i` by a full ring. Its following slots are published
//! before its first one, and read after it, so a first slot found published
//! vouches for the rest. An index the ring went past before it could be read
//! is lost; a first slot not yet published is read again by the next pass.

use crate::capture::{self, Record, Writer};
use crate::decode::{self, Decoded, Header};
use crate::events::{Catalogue, Ring};
use crate::target::{Result, Target};
use crate::time::Timeline;
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SLOT_BYTES: u32 = 16;

/// What a recording yields, in index order.
pub enum Item<'a> {
    /// An event, and its slots as read.
    Event(Decoded<'a>, Vec<u8>),
    /// The indexes `from..to`, overwritten before they could be read intact.
    Lost { from: u32, to: u32 },
}

pub struct Recorder<'a> {
    catalogue: &'a Catalogue,
    ring: &'a Ring,
    /// The address of kymo::trace_ring.
    address: u32,
    /// The first index not yet settled.
    next: u32,
    /// The indexes `next..read_end` were read by the last pass.
    read_end: u32,
    /// The ring's slots as last read, the guard zone included.
    copy: Vec<u8>,
    /// The loss held back, `from..to`.
    loss: Option<(u32, u32)>,
}

impl<'a> Recorder<'a> {
    /// Starts at the current write index: events already in the ring are left out.
    pub fn start(target: &mut dyn Target, catalogue: &'a Catalogue, address: u32) -> Result<Self> {
        let ring = catalogue.ring.as_ref().ok_or("the image has no kymo::trace_ring")?;
        let physical = ring.slots + ring.max_event_slots - 1;
        let mut recorder = Self {
            catalogue,
            ring,
            address,
            next: 0,
            read_end: 0,
            copy: vec![0; (physical * SLOT_BYTES) as usize],
            loss: None,
        };
        recorder.next = recorder.write_index(target)?;
        recorder.read_end = recorder.next;
        Ok(recorder)
    }

    /// The first index not yet settled: where recording started, before the
    /// first pass; where it stopped, after `finish`.
    pub fn next(&self) -> u32 {
        self.next
    }

    fn write_index(&self, target: &mut dyn Target) -> Result<u32> {
        target.read_word(self.address + self.ring.write_index_offset as u32)
    }

    /// One pass: settles the last pass's read, then reads what was written
    /// since. Returns the write index it read.
    pub fn pass(&mut self, target: &mut dyn Target, items: &mut Vec<Item<'a>>) -> Result<u32> {
        let write_index = self.write_index(target)?;
        self.settle(write_index, items)?;
        let slots = self.ring.slots;
        if write_index.wrapping_sub(self.next) > slots {
            self.lost(items, self.next, write_index.wrapping_sub(slots));
            self.next = write_index.wrapping_sub(slots);
        }
        let count = write_index.wrapping_sub(self.next);
        let start = self.next & (slots - 1);
        let guard = self.ring.max_event_slots - 1;
        if start + count <= slots {
            self.read(target, start, start + count)?;
            if start < guard && count > 0 {
                // Where the first indexes may lie instead: the following
                // slots of an event that started near the end.
                self.read(target, slots, slots + guard)?;
            }
        } else {
            // Up to the end of the guard zone, where the following slots of
            // an event that starts near the end lie.
            self.read(target, start, slots + guard)?;
            self.read(target, 0, start + count - slots)?;
        }
        self.read_end = write_index;
        Ok(write_index)
    }

    /// Settles the last pass's read: call once recording ends. Returns the
    /// write index it read.
    pub fn finish(&mut self, target: &mut dyn Target, items: &mut Vec<Item<'a>>) -> Result<u32> {
        let write_index = self.write_index(target)?;
        self.settle(write_index, items)?;
        self.flush(items);
        Ok(write_index)
    }

    /// Reads the slots at positions `from..to`.
    fn read(&mut self, target: &mut dyn Target, from: u32, to: u32) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let mut words = vec![0u32; ((to - from) * SLOT_BYTES / 4) as usize];
        let address = self.address + self.ring.slots_offset as u32 + from * SLOT_BYTES;
        target.read_words(address, &mut words)?;
        let bytes = &mut self.copy[(from * SLOT_BYTES) as usize..(to * SLOT_BYTES) as usize];
        for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        Ok(())
    }

    /// What the copy says of `index`. An index among the first positions,
    /// whose slot there is of an older lap, may be the following slot of an
    /// event that started near the end of the previous lap: that slot lies in
    /// the guard zone, and carries the lap of that event's first slot.
    fn header(&self, index: u32) -> Header {
        let word = |position: u32| {
            let at = (position * SLOT_BYTES) as usize;
            u32::from_le_bytes(self.copy[at..at + 4].try_into().expect("four bytes"))
        };
        let position = index & (self.ring.slots - 1);
        match decode::header(word(position), index, self.ring) {
            Header::Unwritten if position < self.ring.max_event_slots - 1 => {
                // An index of the previous lap: the guard zone follows its last position.
                let previous_lap = index.wrapping_sub(position + 1);
                match decode::header(word(self.ring.slots + position), previous_lap, self.ring) {
                    Header::Tail => Header::Tail,
                    _ => Header::Unwritten,
                }
            }
            header => header,
        }
    }

    /// Settles the indexes the last pass read against `write_index`, read after them.
    fn settle(&mut self, write_index: u32, items: &mut Vec<Item<'a>>) -> Result<()> {
        let mut index = self.next;
        while (self.read_end.wrapping_sub(index) as i32) > 0 {
            if write_index.wrapping_sub(index) > self.ring.slots {
                self.lost(items, index, index.wrapping_add(1));
                index = index.wrapping_add(1);
                continue;
            }
            let position = ((index & (self.ring.slots - 1)) * SLOT_BYTES) as usize;
            match self.header(index) {
                Header::Unwritten => break,
                Header::Tail => {
                    // A following slot of an event whose first slot was lost.
                    self.lost(items, index, index.wrapping_add(1));
                    index = index.wrapping_add(1);
                }
                Header::Head => {
                    let event =
                        decode::event(self.catalogue, self.ring, index, &self.copy[position..])?.ok_or_else(|| {
                            format!("event {index}: published, yet its following slots do not carry its header")
                        })?;
                    let count = decode::slots_for(event.event.size) as u32;
                    let slots = self.copy[position..position + (count * SLOT_BYTES) as usize].to_vec();
                    index = index.wrapping_add(count);
                    self.flush(items);
                    items.push(Item::Event(event, slots));
                }
            }
        }
        self.next = index;
        self.read_end = index;
        Ok(())
    }

    /// Records `from..to` as lost. The loss is held back until an event
    /// follows or recording ends: the passes after may extend it.
    fn lost(&mut self, items: &mut Vec<Item<'a>>, from: u32, to: u32) {
        match self.loss {
            Some((start, end)) if end == from => self.loss = Some((start, to)),
            _ => {
                self.flush(items);
                self.loss = Some((from, to));
            }
        }
    }

    /// Hands out the loss held back.
    fn flush(&mut self, items: &mut Vec<Item<'a>>) {
        if let Some((from, to)) = self.loss.take() {
            items.push(Item::Lost { from, to });
        }
    }
}

/// `kymo record`: prints the events as they come, until Enter is pressed on
/// the standard input or `duration` has passed, and keeps them in the
/// capture file `out`. Their seconds are at `clock` hertz, or else at the
/// frequency measured so far (time.rs).
pub fn run(
    target: &mut dyn Target,
    catalogue: &Catalogue,
    address: u32,
    duration: Option<Duration>,
    out: Option<&Path>,
    clock: Option<u64>,
) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            if std::io::stdin().lock().read_line(&mut line).is_ok_and(|read| read > 0) {
                stop.store(true, Ordering::Relaxed);
            }
        });
    }
    let started = Instant::now();
    let mut recorder = Recorder::start(target, catalogue, address)?;
    println!("record {}", recorder.next());
    let header = capture::Header {
        first: recorder.next(),
        clock,
    };
    let mut capture = out.map(|out| Writer::create(out, &header)).transpose()?;
    let mut timeline = Timeline::new(clock);
    let mut items = Vec::new();
    let mut counts = (0u64, 0u64, 0u64);
    loop {
        let done = stop.load(Ordering::Relaxed) || duration.is_some_and(|duration| started.elapsed() >= duration);
        let host_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos() as u64);
        let write_index = if done {
            recorder.finish(target, &mut items)?
        } else {
            recorder.pass(target, &mut items)?
        };
        counts.2 += 1;
        timeline.pass(host_ns, write_index);
        if let Some(capture) = &mut capture {
            capture.write(&Record::Pass { host_ns, write_index })?;
        }
        for item in items.drain(..) {
            let record = match &item {
                Item::Event(event, slots) => {
                    counts.0 += 1;
                    let cycles = timeline.event(event.index, event.timestamp);
                    let seconds = timeline.seconds(cycles, timeline.frequency());
                    println!("{}", decode::line(&catalogue.symbols, event, cycles, seconds));
                    Record::Event {
                        index: event.index,
                        slots,
                    }
                }
                Item::Lost { from, to } => {
                    counts.1 += u64::from(to.wrapping_sub(*from));
                    println!("lost {from} {to}");
                    Record::Lost { from: *from, to: *to }
                }
            };
            if let Some(capture) = &mut capture {
                capture.write(&record)?;
            }
        }
        if done {
            println!("end {}", recorder.next());
            if let Some(mut capture) = capture {
                capture.write(&Record::End { next: recorder.next() })?;
                capture.finish()?;
            }
            eprintln!(
                "{} events, {} slots lost, {} passes{}",
                counts.0,
                counts.1,
                counts.2,
                measured(&timeline)
            );
            return Ok(());
        }
    }
}

/// The frequency measured over a recording, for its summary.
fn measured(timeline: &Timeline) -> String {
    match timeline.measured() {
        Some((hz, span)) => format!(", cycle counter measured at {hz:.0} Hz over {span:.1} s"),
        None => String::new(),
    }
}

/// `kymo decode` of a capture: the lines `kymo record` printed while making
/// it, but for the seconds: they are at `clock` hertz, or the capture's, or
/// else at the frequency measured over the whole capture.
pub fn print_capture(catalogue: &Catalogue, bytes: &[u8], clock: Option<u64>) -> Result<()> {
    let ring = catalogue.ring.as_ref().ok_or("the image has no kymo::trace_ring")?;
    let (header, records) = capture::read(bytes)?;
    let mut timeline = Timeline::new(clock.or(header.clock));
    let mut events = Vec::new();
    for record in &records {
        match *record {
            Record::Pass { host_ns, write_index } => timeline.pass(host_ns, write_index),
            Record::Event { index, slots } => {
                let event = decode::event(catalogue, ring, index, slots)?
                    .ok_or_else(|| format!("event {index}: its slots in the capture do not hold it whole"))?;
                let cycles = timeline.event(index, event.timestamp);
                events.push((event, cycles));
            }
            Record::Lost { .. } | Record::End { .. } => {}
        }
    }
    let frequency = timeline.frequency();
    let mut events = events.iter();
    println!("record {}", header.first);
    for record in &records {
        match *record {
            Record::Pass { .. } => {}
            Record::Event { .. } => {
                let (event, cycles) = events.next().expect("one event per event record");
                let seconds = timeline.seconds(*cycles, frequency);
                println!("{}", decode::line(&catalogue.symbols, event, *cycles, seconds));
            }
            Record::Lost { from, to } => println!("lost {from} {to}"),
            Record::End { next } => println!("end {next}"),
        }
    }
    eprintln!(
        "{} events{}",
        records
            .iter()
            .filter(|record| matches!(record, Record::Event { .. }))
            .count(),
        measured(&timeline)
    );
    Ok(())
}

#[cfg(test)]
mod tests;
