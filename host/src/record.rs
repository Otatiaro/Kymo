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
//!
//! The target counts the writers the ring lapped (ADR-054). Each pass reads
//! that count with the write index, and matches the writers counted since the
//! last pass against the dangers raised: a danger stands for at most as many
//! writers as it covers slots. A writer left over was lapped where the host
//! raised no danger, so the events recorded may hold its bytes: it is
//! reported suspect, between the write indexes of the two passes.

use crate::capture::{self, Record, Writer};
use crate::decode::{self, Decoded, Header};
use crate::events::{Catalogue, Ring};
use crate::target::{self, Result, Target};
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
    /// `writers` writers lapped while the write index went from `from` to
    /// `to`, that no danger accounts for. Placed among the items of the pass
    /// that read the count, not in index order.
    Suspect { from: u32, to: u32, writers: u32 },
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
    /// The indexes the last pass read unpublished, of an older lap: a writer
    /// the ring laps after that may never have finished.
    unpublished: Vec<u32>,
    /// The indexes settled lost while read unpublished: their writers may be
    /// under way, and a danger is raised if the ring laps them.
    skipped: Vec<u32>,
    /// The writers that may still write to slots the ring has reserved again.
    dangers: Vec<Danger>,
    raised: u32,
    lifted: u32,
    expired: u32,
    /// The count of writers lapped, and the write index, the last pass read.
    lapped: u32,
    pass_index: u32,
    suspects: u32,
}

/// The events `start..end`, read unpublished then lapped by the ring: their
/// writers were preempted for a full lap, or may have been, and may still
/// write their slots, now reserved again by newer events. `head` is the first
/// of them not yet seen published; each event seen published is done.
///
/// The host cannot tell a writer still preempted from one that finished and
/// whose first slot a newer event then overwrote: a danger expires once the
/// write index reaches `expires`, a lap after it was raised.
struct Danger {
    start: u32,
    head: u32,
    end: u32,
    expires: u32,
    /// Every writer seen done: their slots may hold others' first slots
    /// overwritten, but nothing is written to them any more.
    done: bool,
    /// The writers counted lapped it may still account for: one per slot
    /// it covers.
    credit: u32,
}

impl Danger {
    /// Whether the slots `from..end` cover a slot of the `count`-slot event at
    /// `index`, guard zone included: slot positions are compared, not indexes.
    fn covers(&self, from: u32, index: u32, count: u32, slots: u32) -> bool {
        let length = self.end.wrapping_sub(from);
        let mask = slots - 1;
        (0..count).any(|slot| length > mask || (index.wrapping_add(slot).wrapping_sub(from) & mask) < length)
    }
}

impl<'a> Recorder<'a> {
    /// Starts at the current write index: events already in the ring are left
    /// out, but their writers not yet done are watched, as they may write on
    /// into the slots of the events recorded.
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
            unpublished: Vec::new(),
            skipped: Vec::new(),
            dangers: Vec::new(),
            raised: 0,
            lifted: 0,
            expired: 0,
            lapped: 0,
            pass_index: 0,
            suspects: 0,
        };
        let (lapped, write_index) = recorder.counters(target)?;
        recorder.lapped = lapped;
        recorder.pass_index = write_index;
        recorder.read(target, 0, physical)?;
        let after = recorder.counters(target)?.1;
        // The last lap, its writers given up on: a ring not yet filled once has
        // nothing before index 0.
        recorder.read_end = write_index;
        let mut index = write_index.wrapping_sub(write_index.min(ring.slots));
        recorder.unpublished = recorder.unpublished_from(index);
        while index != write_index {
            index = recorder.give_up(index, after)?;
        }
        recorder.next = write_index;
        Ok(recorder)
    }

    /// What a failed test reports of the recorder.
    #[cfg(test)]
    pub fn state(&self) -> String {
        let dangers: Vec<_> = self
            .dangers
            .iter()
            .map(|d| (d.start, d.head, d.end, d.expires, d.done))
            .collect();
        format!(
            "next {} read_end {} unpublished {:?} dangers {dangers:?}",
            self.next, self.read_end, self.unpublished
        )
    }

    /// The dangers raised, lifted, and expired unlifted, and the writers
    /// reported suspect, since recording started.
    #[cfg(test)]
    pub fn dangers(&self) -> (u32, u32, u32, u32) {
        (self.raised, self.lifted, self.expired, self.suspects)
    }

    /// The first index not yet settled: where recording started, before the
    /// first pass; where it stopped, after `finish`.
    pub fn next(&self) -> u32 {
        self.next
    }

    /// The count of writers lapped and the write index, read in one go.
    fn counters(&self, target: &mut dyn Target) -> Result<(u32, u32)> {
        let mut words = [0u32; 2];
        target.read_words(self.address + self.ring.write_index_offset as u32, &mut words)?;
        Ok((words[1], words[0]))
    }

    /// One pass: settles the last pass's read, then reads what was written
    /// since. Returns the write index it read.
    pub fn pass(&mut self, target: &mut dyn Target, items: &mut Vec<Item<'a>>) -> Result<u32> {
        let (lapped, write_index) = self.counters(target)?;
        self.settle(write_index, items)?;
        self.account(lapped, write_index, items);
        self.lift(target, write_index)?;
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
        let (lapped, write_index) = self.counters(target)?;
        self.settle(write_index, items)?;
        self.account(lapped, write_index, items);
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

    /// Settles the indexes the last pass read against `write_index`, read
    /// after them. An event is lost if the ring went past it, if its slots do
    /// not hold it whole, or if a writer may still write to them.
    fn settle(&mut self, write_index: u32, items: &mut Vec<Item<'a>>) -> Result<()> {
        for index in std::mem::take(&mut self.skipped) {
            if write_index.wrapping_sub(index) > self.ring.slots {
                self.raise(index, write_index);
            } else {
                self.skipped.push(index);
            }
        }
        let mut index = self.next;
        while (self.read_end.wrapping_sub(index) as i32) > 0 {
            if write_index.wrapping_sub(index) > self.ring.slots {
                let to = self.give_up(index, write_index)?;
                self.lost(items, index, to);
                index = to;
                continue;
            }
            let position = ((index & (self.ring.slots - 1)) * SLOT_BYTES) as usize;
            match self.header(index) {
                Header::Unwritten
                    if self
                        .dangers
                        .iter()
                        .any(|danger| danger.covers(danger.start, index, 1, self.ring.slots)) =>
                {
                    // Its first slot may have been overwritten by a writer the
                    // ring lapped; or its own writer is under way, and is
                    // watched until the ring laps it.
                    self.lost(items, index, index.wrapping_add(1));
                    self.skipped.push(index);
                    index = index.wrapping_add(1);
                }
                Header::Unwritten => break,
                Header::Tail => {
                    // A following slot of an event whose first slot was lost.
                    self.lost(items, index, index.wrapping_add(1));
                    index = index.wrapping_add(1);
                }
                Header::Head => {
                    let count = self.count(index)?;
                    let event = decode::event(self.catalogue, self.ring, index, &self.copy[position..])?;
                    let to = index.wrapping_add(count);
                    match event {
                        Some(event) if !self.endangered(index, count) => {
                            let slots = self.copy[position..position + (count * SLOT_BYTES) as usize].to_vec();
                            self.flush(items);
                            items.push(Item::Event(event, slots));
                        }
                        _ => self.lost(items, index, to),
                    }
                    index = to;
                }
            }
        }
        self.unpublished = self.unpublished_from(index);
        self.next = index;
        self.read_end = index;
        Ok(())
    }

    /// The indexes `from..read_end` read unpublished, of an older lap, by this
    /// pass or an earlier one: not yet written, or written by a writer still
    /// under way. A later read may show a newer event there instead.
    fn unpublished_from(&self, from: u32) -> Vec<u32> {
        let mut unpublished: Vec<u32> = self
            .unpublished
            .iter()
            .copied()
            .filter(|&index| (index.wrapping_sub(from) as i32) >= 0)
            .collect();
        let mut index = from;
        while (self.read_end.wrapping_sub(index) as i32) > 0 {
            if matches!(self.header(index), Header::Unwritten) && self.older(index) && !unpublished.contains(&index) {
                unpublished.push(index);
            }
            index = index.wrapping_add(1);
        }
        unpublished
    }

    /// Gives up on `index`, read by the last pass and since lapped by the
    /// ring, and returns the next index to settle. A first slot of an event
    /// read unpublished means its writer had not finished: from there to the
    /// next event read published, writers may still write, and a danger is
    /// raised over those slots.
    fn give_up(&mut self, index: u32, write_index: u32) -> Result<u32> {
        Ok(match self.header(index) {
            Header::Head => index.wrapping_add(self.count(index)?),
            // Read unpublished by the last pass, which the copy holds, or by an earlier one.
            Header::Unwritten if self.older(index) || self.unpublished.contains(&index) => {
                self.raise(index, write_index)
            }
            _ => index.wrapping_add(1),
        })
    }

    /// Raises a danger from `index`, read unpublished and since lapped, to the
    /// next event read published, at most `max_event_slots`; returns its end.
    fn raise(&mut self, index: u32, write_index: u32) -> u32 {
        let mut end = index.wrapping_add(1);
        let last = index.wrapping_add(self.ring.max_event_slots);
        while end != self.read_end && end != last && !matches!(self.header(end), Header::Head) {
            end = end.wrapping_add(1);
        }
        self.dangers.push(Danger {
            start: index,
            head: index,
            end,
            expires: write_index.wrapping_add(self.ring.slots),
            done: false,
            credit: end.wrapping_sub(index),
        });
        self.raised += 1;
        end
    }

    /// Whether the copy holds, at `index`'s position, a slot of an older lap:
    /// not yet reserved again when it was read.
    fn older(&self, index: u32) -> bool {
        let at = ((index & (self.ring.slots - 1)) * SLOT_BYTES) as usize;
        let word = u32::from_le_bytes(self.copy[at..at + 4].try_into().expect("four bytes"));
        let behind = decode::lap(index, self.ring).wrapping_sub(word >> 17) & 0x7fff;
        behind != 0 && behind < 0x4000
    }

    /// The slots of the event whose first slot, published in the copy, is at `index`.
    fn count(&self, index: u32) -> Result<u32> {
        let at = ((index & (self.ring.slots - 1)) * SLOT_BYTES) as usize;
        self.slots_of(
            index,
            u32::from_le_bytes(self.copy[at..at + 4].try_into().expect("four bytes")),
        )
    }

    /// The slots of the event at `index` whose first slot's header is `word`.
    fn slots_of(&self, index: u32, word: u32) -> Result<u32> {
        let type_id = (word & 0xffff) as u16;
        let event = self
            .catalogue
            .events
            .get(&type_id)
            .ok_or_else(|| format!("event {index}: type id {type_id} is not in the image"))?;
        Ok(decode::slots_for(event.size) as u32)
    }

    /// Whether a writer may still write to a slot of the `count`-slot event at `index`.
    fn endangered(&self, index: u32, count: u32) -> bool {
        self.dangers
            .iter()
            .any(|danger| !danger.done && danger.covers(danger.head, index, count, self.ring.slots))
    }

    /// Reads the first slot of each danger's oldest writer: once published,
    /// that writer is done. A danger with all its writers done is lifted.
    /// Called before the slots are read, so a read never meets a writer that
    /// was seen done. A danger expires a lap after it was raised.
    fn lift(&mut self, target: &mut dyn Target, write_index: u32) -> Result<()> {
        let mut dangers = std::mem::take(&mut self.dangers);
        for danger in dangers.iter_mut().filter(|danger| !danger.done) {
            while (danger.end.wrapping_sub(danger.head) as i32) > 0 {
                let position = danger.head & (self.ring.slots - 1);
                let word = target.read_word(self.address + self.ring.slots_offset as u32 + position * SLOT_BYTES)?;
                if !matches!(decode::header(word, danger.head, self.ring), Header::Head) {
                    break;
                }
                danger.head = danger.head.wrapping_add(self.slots_of(danger.head, word)?);
            }
            if (danger.end.wrapping_sub(danger.head) as i32) <= 0 {
                danger.done = true;
                self.lifted += 1;
            }
        }
        dangers.retain(|danger| {
            let live = (write_index.wrapping_sub(danger.expires) as i32) < 0;
            self.expired += u32::from(!live && !danger.done);
            live
        });
        self.dangers = dangers;
        Ok(())
    }

    /// Matches the writers counted lapped since the last pass against the
    /// dangers, oldest first, raised up to this pass; reports those left over.
    /// A danger's credit goes when it expires: a writer counted after that
    /// was held past it, out of the host's reach.
    fn account(&mut self, lapped: u32, write_index: u32, items: &mut Vec<Item<'a>>) {
        let mut writers = lapped.wrapping_sub(self.lapped);
        for danger in &mut self.dangers {
            let taken = writers.min(danger.credit);
            danger.credit -= taken;
            writers -= taken;
        }
        if writers > 0 {
            items.push(Item::Suspect {
                from: self.pass_index,
                to: write_index,
                writers,
            });
            self.suspects += writers;
        }
        self.lapped = lapped;
        self.pass_index = write_index;
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
    let mut counts = (0u64, 0u64, 0u64, 0u64);
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
                Item::Suspect { from, to, writers } => {
                    counts.3 += u64::from(*writers);
                    println!("suspect {from} {to} {writers}");
                    Record::Suspect {
                        from: *from,
                        to: *to,
                        writers: *writers,
                    }
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
                "{} events, {} slots lost, {} passes{}{}{}",
                counts.0,
                counts.1,
                counts.2,
                suspect(counts.3),
                target::speed_text(target),
                measured(&timeline)
            );
            return Ok(());
        }
    }
}

/// The writers reported suspect over a recording, for its summary.
fn suspect(writers: u64) -> String {
    match writers {
        0 => String::new(),
        _ => format!(", {writers} lapped writers no danger accounts for: events near the suspect lines may hold their bytes"),
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
            Record::Lost { .. } | Record::Suspect { .. } | Record::End { .. } => {}
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
            Record::Suspect { from, to, writers } => println!("suspect {from} {to} {writers}"),
            Record::End { next } => println!("end {next}"),
        }
    }
    let suspects = records
        .iter()
        .map(|record| match *record {
            Record::Suspect { writers, .. } => u64::from(writers),
            _ => 0,
        })
        .sum();
    eprintln!(
        "{} events{}{}",
        records
            .iter()
            .filter(|record| matches!(record, Record::Event { .. }))
            .count(),
        suspect(suspects),
        measured(&timeline)
    );
    Ok(())
}

#[cfg(test)]
mod tests;
