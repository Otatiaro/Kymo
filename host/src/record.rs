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
//! An event that reaches the end of the ring goes on at its first position.
//!
//! The target counts the writers the ring lapped (ADR-054). Each pass reads
//! that count with the write index, and matches the writers counted since the
//! last pass against the dangers raised: a danger stands for at most as many
//! writers as it covers slots. A writer left over was lapped where the host
//! raised no danger, so the events recorded may hold its bytes: it is
//! reported suspect, between the write indexes of the two passes.
//!
//! On some parts (the STM32H5 measured) the probe reads the system bus as
//! zeros while the core sleeps in WFI, the ring included. What no live ring
//! can give is dropped, and the pass counted: counters at zero once they have
//! counted, which settle nothing, or a slot's header at zero where a header
//! was read before. A read is kept up to the event before its first such
//! slot, the zeros may have started in its payload; counters at zero also
//! drop the event that holds the last slot of the read before them. The
//! slots dropped are read again by the next pass. Not seen: zeros the core
//! slept and woke on within a read's last slot, between two passes; zeros in
//! slots never written since reset; a ring read as zeros by `start`.

use crate::capture::{self, Record, Writer};
use crate::decode::{self, Decoded, Header};
use crate::events::{Catalogue, Ring};
use crate::model::{Element, Trace};
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
    address: u32,
    /// The most slots an event of the image takes.
    max_event_slots: u32,
    /// The first index not yet settled.
    next: u32,
    /// The indexes `next..read_end` were read by the last pass.
    read_end: u32,
    /// The ring's slots as last read.
    copy: Vec<u8>,
    /// The loss held back, `from..to`.
    loss: Option<(u32, u32)>,
    /// The indexes the last pass read unpublished, of an older lap: a writer
    /// the ring laps after that may never have finished.
    unpublished: Vec<u32>,
    /// The indexes settled lost while read unpublished: their writers may be
    /// under way, and a danger is raised if the ring laps them.
    skipped: Vec<u32>,
    /// The end of the last loss a pass's window cut, whose slots no pass may
    /// have read: an index read unpublished up to `max_event_slots - 1` after
    /// it may be a following slot of an event whose first slot is in the loss.
    cut: Option<u32>,
    /// The writers that may still write to slots the ring has reserved again.
    dangers: Vec<Danger>,
    /// Writers read unpublished, then read published by a pass the ring
    /// lapped them before it ended: done, but maybe lapped before they
    /// published, and counted after. Each may account for one writer counted
    /// lapped until the write index reaches its value, a lap on.
    late: Vec<u32>,
    raised: u32,
    lifted: u32,
    expired: u32,
    /// The count of writers lapped, and the write index, the last pass read.
    lapped: u32,
    pass_index: u32,
    suspects: u32,
    /// The passes that read the target as zeros, its core asleep.
    zeros: u32,
    /// The most slots a pass reads, the newest: halved when the ring lapped
    /// a read before the next pass's counters settled it, grown by an eighth
    /// when it did not, up to the ring. A probe slower than the writer still
    /// reads a share of the events, where a read of the whole backlog would
    /// be lapped every time.
    window: u32,
    /// Whether the core slept since the last pass settled: a read lapped
    /// then says nothing of the probe's pace.
    slept: bool,
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
    /// `index`: slot positions are compared, not indexes.
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
    pub fn start(target: &mut dyn Target, catalogue: &'a Catalogue, ring: &'a Ring) -> Result<Self> {
        let mut recorder = Self {
            catalogue,
            ring,
            address: u32::try_from(ring.address)?,
            max_event_slots: decode::max_event_slots(catalogue),
            next: 0,
            read_end: 0,
            copy: vec![0; (ring.slots * SLOT_BYTES) as usize],
            loss: None,
            unpublished: Vec::new(),
            skipped: Vec::new(),
            cut: None,
            dangers: Vec::new(),
            late: Vec::new(),
            raised: 0,
            lifted: 0,
            expired: 0,
            lapped: 0,
            pass_index: 0,
            suspects: 0,
            zeros: 0,
            window: ring.slots,
            slept: false,
        };
        let (lapped, write_index) = recorder.counters(target)?;
        recorder.lapped = lapped;
        recorder.pass_index = write_index;
        recorder.read(target, 0, ring.slots)?;
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

    /// The passes that read the target as zeros since recording started.
    pub fn zeros(&self) -> u32 {
        self.zeros
    }

    /// The count of writers lapped and the write index, read in one go.
    fn counters(&self, target: &mut dyn Target) -> Result<(u32, u32)> {
        let mut words = [0u32; 2];
        target.read_words(self.address + self.ring.write_index_offset as u32, &mut words)?;
        Ok((words[1], words[0]))
    }

    /// Whether counters read `lapped` and `write_index` are zeros where the
    /// ring had counted. If so, the pass is counted, and the event holding
    /// the last slot of the last pass's read is read again: the core may have
    /// fallen asleep within that slot's payload, after its header was read.
    fn read_as_zeros(&mut self, lapped: u32, write_index: u32) -> bool {
        let zeros = (write_index == 0 && self.pass_index != 0) || (lapped == 0 && self.lapped != 0);
        if zeros {
            self.zeros += 1;
            self.slept = true;
            self.read_end = self.retreat(self.read_end);
        }
        zeros
    }

    /// The first index of the event the copy holds at `end - 1`, read from
    /// `self.next` on: where a read that may end in zeros is trusted up to.
    fn retreat(&self, end: u32) -> u32 {
        let mut index = self.next;
        while (end.wrapping_sub(index) as i32) > 0 {
            let count = match self.header(index) {
                Header::Head => self.count(index).unwrap_or(1),
                _ => 1,
            };
            if (end.wrapping_sub(index.wrapping_add(count)) as i32) <= 0 {
                return index;
            }
            index = index.wrapping_add(count);
        }
        end
    }

    /// One pass: settles the last pass's read, then reads what was written
    /// since. Returns the write index it read, or nothing if it read the
    /// counters as zeros.
    pub fn pass(&mut self, target: &mut dyn Target, items: &mut Vec<Item<'a>>) -> Result<Option<u32>> {
        let (lapped, write_index) = self.counters(target)?;
        if self.read_as_zeros(lapped, write_index) {
            return Ok(None);
        }
        self.settle(write_index, items)?;
        self.account(lapped, write_index, items);
        self.lift(target, write_index)?;
        let slots = self.ring.slots;
        let window = self.window;
        if write_index.wrapping_sub(self.next) > window {
            // Indexes read unpublished and left behind: their writers may be
            // under way, watched like those settled lost. One the ring already
            // lapped, a read since dropped as zeros, is endangered at once.
            let to = write_index.wrapping_sub(window);
            let left = to.wrapping_sub(self.next);
            for index in std::mem::take(&mut self.unpublished) {
                if index.wrapping_sub(self.next) >= left {
                    self.unpublished.push(index);
                } else if write_index.wrapping_sub(index) <= slots {
                    self.skipped.push(index);
                } else {
                    self.raise(index, write_index);
                }
            }
            self.lost(items, self.next, to);
            self.next = to;
            self.cut = Some(to);
        }
        let count = write_index.wrapping_sub(self.next);
        let start = self.next & (slots - 1);
        let mut ranges = vec![(start, (start + count).min(slots))];
        if start + count > slots {
            ranges.push((0, start + count - slots));
        }
        let mut index = self.next;
        let mut end = write_index;
        for (from, to) in ranges {
            let words = self.fetch(target, from, to)?;
            match self.zeroed(index, &words) {
                Some(slot) => {
                    self.store(from, &words[..slot as usize * 4]);
                    self.zeros += 1;
                    self.slept = true;
                    end = self.retreat(index.wrapping_add(slot));
                    break;
                }
                None => self.store(from, &words),
            }
            index = index.wrapping_add(to - from);
        }
        self.read_end = end;
        Ok(Some(write_index))
    }

    /// Settles the last pass's read: call once recording ends. Returns the
    /// write index it read, or nothing if it read the counters as zeros: the
    /// last pass's read is then left unsettled.
    pub fn finish(&mut self, target: &mut dyn Target, items: &mut Vec<Item<'a>>) -> Result<Option<u32>> {
        let (lapped, write_index) = self.counters(target)?;
        if self.read_as_zeros(lapped, write_index) {
            self.flush(items);
            return Ok(None);
        }
        self.settle(write_index, items)?;
        self.account(lapped, write_index, items);
        self.flush(items);
        Ok(Some(write_index))
    }

    /// Reads the slots at positions `from..to` into the copy.
    fn read(&mut self, target: &mut dyn Target, from: u32, to: u32) -> Result<()> {
        let words = self.fetch(target, from, to)?;
        self.store(from, &words);
        Ok(())
    }

    /// The words of the slots at positions `from..to`.
    fn fetch(&self, target: &mut dyn Target, from: u32, to: u32) -> Result<Vec<u32>> {
        let mut words = vec![0u32; ((to - from) * SLOT_BYTES / 4) as usize];
        if from != to {
            let address = self.address + self.ring.slots_offset as u32 + from * SLOT_BYTES;
            target.read_words(address, &mut words)?;
        }
        Ok(words)
    }

    /// Puts `words`, the slots from position `from`, in the copy.
    fn store(&mut self, from: u32, words: &[u32]) {
        let bytes = &mut self.copy[(from * SLOT_BYTES) as usize..][..words.len() * 4];
        for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
    }

    /// The first of `words`, the slots read for the indexes from `index` on,
    /// whose header no ring can have, the core asleep: zero where the copy
    /// holds a non-zero one. A header carries its index's lap, which starts
    /// at 1, and a first slot its head bit: a written header is zero only on
    /// a following slot of type id 0 once the lap field wraps to 0, and those
    /// indexes are not judged. Counted from `words`' first slot. A run of
    /// zeros that does not reach a header escapes it: a read's last slot is
    /// covered by `retreat` when the next counters read as zeros.
    fn zeroed(&self, index: u32, words: &[u32]) -> Option<u32> {
        (0u32..)
            .zip(words.chunks(4))
            .find(|&(slot, words)| {
                let index = index.wrapping_add(slot);
                words[0] == 0 && self.word(index) != 0 && decode::lap(index, self.ring) != 0
            })
            .map(|(slot, _)| slot)
    }

    /// The header the copy holds at `index`'s position.
    fn word(&self, index: u32) -> u32 {
        let at = ((index & (self.ring.slots - 1)) * SLOT_BYTES) as usize;
        u32::from_le_bytes(self.copy[at..at + 4].try_into().expect("four bytes"))
    }

    /// What the copy says of `index`.
    fn header(&self, index: u32) -> Header {
        decode::header(self.word(index), index, self.ring)
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
        let read = self.read_end != index;
        let mut overtaken = false;
        while (self.read_end.wrapping_sub(index) as i32) > 0 {
            if write_index.wrapping_sub(index) > self.ring.slots {
                overtaken = true;
                let to = self.give_up(index, write_index)?;
                self.lost(items, index, to);
                index = to;
                continue;
            }
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
                    let event = decode::event_at(self.catalogue, self.ring, &self.copy, index)?;
                    let to = index.wrapping_add(count);
                    match event {
                        Some(event) if !self.endangered(index, count) => {
                            let slots = decode::gather(&self.copy, self.ring, index, count);
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
        if overtaken {
            if !self.slept {
                self.window = (self.window / 2).max(self.max_event_slots.min(self.ring.slots));
            }
        } else if read {
            self.window = (self.window + self.window / 8 + 1).min(self.ring.slots);
            self.slept = false;
        }
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
    /// raised over those slots. One read unpublished before, then published,
    /// may have published once lapped: it is late.
    fn give_up(&mut self, index: u32, write_index: u32) -> Result<u32> {
        Ok(match self.header(index) {
            Header::Head => {
                if self.unpublished.contains(&index) {
                    self.late.push(write_index.wrapping_add(self.ring.slots));
                }
                index.wrapping_add(self.count(index)?)
            }
            // Read unpublished by the last pass, which the copy holds, or by an earlier one.
            Header::Unwritten if self.older(index) || self.unpublished.contains(&index) => {
                self.raise(index, write_index)
            }
            _ => index.wrapping_add(1),
        })
    }

    /// Raises a danger from `index`, read unpublished and since lapped, to the
    /// next event read published, at most `max_event_slots`; returns its end.
    /// Right after a loss, `index` may be a following slot of an event whose
    /// first slot is in the loss: the danger then starts where that first slot
    /// may be, its writer's slots the loss's events, which `lift` steps over
    /// once read published.
    fn raise(&mut self, index: u32, write_index: u32) -> u32 {
        let mut end = index.wrapping_add(1);
        let last = index.wrapping_add(self.max_event_slots);
        while end != self.read_end && end != last && !matches!(self.header(end), Header::Head) {
            end = end.wrapping_add(1);
        }
        let start = match self.cut {
            Some(cut) if index.wrapping_sub(cut) < self.max_event_slots - 1 => {
                index.wrapping_sub(self.max_event_slots - 1)
            }
            _ => index,
        };
        self.dangers.push(Danger {
            start,
            head: start,
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
        let behind = decode::lap(index, self.ring).wrapping_sub(self.word(index) >> 17) & 0x7fff;
        behind != 0 && behind < 0x4000
    }

    /// The slots of the event whose first slot, published in the copy, is at `index`.
    fn count(&self, index: u32) -> Result<u32> {
        self.slots_of(index, self.word(index))
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
        self.late
            .retain(|&expires| (write_index.wrapping_sub(expires) as i32) < 0);
        Ok(())
    }

    /// Matches the writers counted lapped since the last pass against the
    /// dangers, oldest first, raised up to this pass, then the late writers;
    /// reports those left over. A credit goes when it expires: a writer
    /// counted after that was held past it, out of the host's reach.
    fn account(&mut self, lapped: u32, write_index: u32, items: &mut Vec<Item<'a>>) {
        let mut writers = lapped.wrapping_sub(self.lapped);
        for danger in &mut self.dangers {
            let taken = writers.min(danger.credit);
            danger.credit -= taken;
            writers -= taken;
        }
        let taken = writers.min(self.late.len() as u32);
        self.late.drain(..taken as usize);
        writers -= taken;
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

/// `kymo record` of `ring`: prints the events as they come, until Enter is
/// pressed on the standard input or `duration` has passed, and keeps them in the
/// capture file `out`. Their seconds are at `clock` hertz, or else at the
/// frequency measured so far (time.rs).
pub fn run(
    target: &mut dyn Target,
    catalogue: &Catalogue,
    ring: &Ring,
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
    let mut recorder = Recorder::start(target, catalogue, ring)?;
    println!("record {}", recorder.next());
    let header = capture::Header {
        first: recorder.next(),
        clock,
        channel: ring.name.clone(),
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
        let zeros = recorder.zeros();
        let write_index = if done {
            recorder.finish(target, &mut items)?
        } else {
            recorder.pass(target, &mut items)?
        };
        counts.2 += 1;
        if zeros == 0 && recorder.zeros() > 0 {
            eprintln!("warning: {}", ASLEEP);
        }
        if let Some(write_index) = write_index {
            timeline.pass(host_ns, write_index);
            if let Some(capture) = &mut capture {
                capture.write(&Record::Pass { host_ns, write_index })?;
            }
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
        if let Some(capture) = &mut capture {
            capture.flush()?;
        }
        if done {
            println!("end {}", recorder.next());
            if let Some(mut capture) = capture {
                capture.write(&Record::End { next: recorder.next() })?;
                capture.finish()?;
            }
            eprintln!(
                "{} events, {} slots lost, {} passes{}{}{}{}",
                counts.0,
                counts.1,
                counts.2,
                asleep(recorder.zeros()),
                suspect(counts.3),
                target::speed_text(target),
                measured(timeline.measured())
            );
            return Ok(());
        }
    }
}

/// What a target read as zeros means.
const ASLEEP: &str = "the target read as zeros: its core is likely asleep (WFI), which on some parts hides its memory from the probe; keep its idle awake while tracing";

/// The passes that read the target as zeros, for a recording's summary.
fn asleep(zeros: u32) -> String {
    match zeros {
        0 => String::new(),
        _ => format!(", {zeros} of them partly dropped: {ASLEEP}"),
    }
}

/// The writers reported suspect over a recording, for its summary.
fn suspect(writers: u64) -> String {
    match writers {
        0 => String::new(),
        1 => ", 1 write overrun: events near its suspect line may be wrong".to_string(),
        _ => format!(", {writers} writes overrun: events near the suspect lines may be wrong"),
    }
}

/// The frequency measured over a recording, for its summary.
fn measured(measured: Option<(f64, f64)>) -> String {
    match measured {
        Some((hz, span)) => format!(", cycle counter measured at {hz:.0} Hz over {span:.1} s"),
        None => String::new(),
    }
}

/// `kymo decode` of a capture: the lines `kymo record` printed while making
/// it, but for the seconds: they are at `clock` hertz, or the capture's, or
/// else at the frequency measured over the whole capture.
pub fn print_capture(catalogue: &Catalogue, bytes: &[u8], clock: Option<u64>) -> Result<()> {
    let trace = Trace::read(catalogue, bytes, clock)?;
    println!("record {}", trace.first);
    for element in &trace.elements {
        match element {
            Element::Event { event, cycles, seconds } => {
                println!("{}", decode::line(&catalogue.symbols, event, *cycles, *seconds));
            }
            Element::Lost { from, to } => println!("lost {from} {to}"),
            Element::Suspect { from, to, writers } => println!("suspect {from} {to} {writers}"),
        }
    }
    if let Some(next) = trace.end {
        println!("end {next}");
    }
    if let Some(cut) = &trace.cut {
        eprintln!(
            "the capture is cut short at byte {}, after index {}: read up to its last whole record",
            cut.at, cut.next
        );
    }
    eprintln!(
        "{} events{}{}",
        trace.events(),
        suspect(trace.suspects()),
        measured(trace.measured)
    );
    Ok(())
}

#[cfg(test)]
mod tests;
