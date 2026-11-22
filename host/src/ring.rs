//! Draining the trace ring (see target/include/kymo/ring.hpp for the layout
//! and the protocol).
//!
//! Each pass reads the write index, settles the slots read by the previous
//! pass against it, then reads the slots written since. The index read at the
//! start of a pass is the one read right after the previous pass's slots,
//! which is what the integrity rule asks for: a slot read at index `i` is
//! intact if that index has not gone past `i` by a full ring.

use crate::elf::Symbol;
use crate::target::{Result, Target};

pub const SLOT_BYTES: u32 = 16;
const SLOT_WORDS: usize = 4;
const HEAD_BIT: u32 = 1 << 16;
const LAP_SHIFT: u32 = 17;
const LAP_MASK: u32 = 0x7fff;

/// Where the ring is and how many slots it has, from its ELF symbol.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub address: u32,
    pub slots: u32,
}

impl Layout {
    pub fn from_symbol(symbol: Symbol) -> Result<Self> {
        let slots = symbol.size.checked_sub(4).map(|bytes| bytes / SLOT_BYTES).unwrap_or(0);
        if slots < 2 || !slots.is_power_of_two() || 4 + slots * SLOT_BYTES != symbol.size {
            return Err(format!("kymo_ring has an unexpected size of {} bytes", symbol.size).into());
        }
        Ok(Self {
            address: symbol.address,
            slots,
        })
    }

    pub fn bytes(&self) -> u32 {
        4 + self.slots * SLOT_BYTES
    }

    fn slot_address(&self, index: u32) -> u32 {
        self.address + 4 + (index & (self.slots - 1)) * SLOT_BYTES
    }

    /// The header a slot holds once the event at `index` is published.
    fn header_lap(&self, index: u32) -> u32 {
        ((index >> self.slots.trailing_zeros()).wrapping_add(1) & LAP_MASK) << LAP_SHIFT
    }
}

/// The newest published event: its index and its timestamp.
pub fn newest(target: &mut dyn Target, layout: Layout) -> Result<(u32, u32)> {
    for _ in 0..100 {
        let write_index = target.read_word(layout.address)?;
        let index = write_index.wrapping_sub(1);
        let mut slot = [0u32; SLOT_WORDS];
        target.read_words(layout.slot_address(index), &mut slot)?;
        let still_there = target.read_word(layout.address)?.wrapping_sub(index) <= layout.slots;
        if still_there && slot[0] & !0xffff == layout.header_lap(index) | HEAD_BIT {
            return Ok((index, slot[1]));
        }
    }
    Err("no published event in the ring".into())
}

/// The timestamps of the published events `first..=last`, in order; `None`
/// for an event no longer or not yet in the ring.
pub fn timestamps(target: &mut dyn Target, layout: Layout, first: u32, last: u32) -> Result<Vec<Option<u32>>> {
    let mut words = vec![0u32; layout.slots as usize * SLOT_WORDS];
    target.read_words(layout.slot_address(0), &mut words)?;
    let count = last.wrapping_sub(first).wrapping_add(1);
    Ok((0..count)
        .map(|offset| {
            let index = first.wrapping_add(offset);
            let slot = (index & (layout.slots - 1)) as usize * SLOT_WORDS;
            let published = words[slot] & !0xffff == layout.header_lap(index) | HEAD_BIT;
            published.then_some(words[slot + 1])
        })
        .collect())
}

/// What a drain has seen so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    /// Events read intact.
    pub received: u64,
    /// Events overwritten before the host could read them intact.
    pub lost: u64,
    /// Events read intact whose payload is wrong: a protocol or probe fault.
    pub corrupt: u64,
    /// Bytes read from the target, write index included.
    pub bytes_read: u64,
    pub passes: u64,
}

pub struct Drain {
    layout: Layout,
    /// Next index to read.
    next: u32,
    /// Slots read by the last pass, from index `pending_first`.
    pending: Vec<u32>,
    pending_first: u32,
    pub counts: Counts,
}

impl Drain {
    /// Starts at the current write index: events already in the ring are skipped.
    pub fn start(target: &mut dyn Target, layout: Layout) -> Result<Self> {
        let next = target.read_word(layout.address)?;
        Ok(Self {
            layout,
            next,
            pending: Vec::with_capacity(layout.slots as usize * SLOT_WORDS),
            pending_first: next,
            counts: Counts::default(),
        })
    }

    /// The next index to read.
    pub fn next(&self) -> u32 {
        self.next
    }

    /// One pass; returns the write index it read.
    pub fn pass(&mut self, target: &mut dyn Target) -> Result<u32> {
        let write_index = target.read_word(self.layout.address)?;
        self.counts.bytes_read += 4;
        self.counts.passes += 1;
        self.settle(write_index);

        let mut count = write_index.wrapping_sub(self.next);
        if count > self.layout.slots {
            self.counts.lost += u64::from(count - self.layout.slots);
            self.next = write_index.wrapping_sub(self.layout.slots);
            count = self.layout.slots;
        }
        self.pending_first = self.next;
        self.pending.clear();
        if count == 0 {
            return Ok(write_index);
        }
        self.pending.resize(count as usize * SLOT_WORDS, 0);
        let start = self.next & (self.layout.slots - 1);
        let first = count.min(self.layout.slots - start) as usize;
        let (head, tail) = self.pending.split_at_mut(first * SLOT_WORDS);
        target.read_words(self.layout.slot_address(self.next), head)?;
        if !tail.is_empty() {
            target.read_words(self.layout.slot_address(0), tail)?;
        }
        self.counts.bytes_read += u64::from(count * SLOT_BYTES);
        self.next = write_index;
        Ok(write_index)
    }

    /// Settles the slots of the previous pass against the write index read
    /// after them.
    fn settle(&mut self, write_index: u32) {
        for (offset, slot) in self.pending.chunks_exact(SLOT_WORDS).enumerate() {
            let index = self.pending_first.wrapping_add(offset as u32);
            if write_index.wrapping_sub(index) > self.layout.slots {
                // Reserved again before the read ended: may be torn.
                self.counts.lost += 1;
                continue;
            }
            if slot[0] & !0xffff != self.layout.header_lap(index) | HEAD_BIT {
                // Reserved but not yet published when read: read it again.
                self.next = index;
                break;
            }
            // The bench event: type 1, payload the index and its complement.
            if slot[0] & 0xffff == 1 && slot[2] == index && slot[3] == !index {
                self.counts.received += 1;
            } else {
                self.counts.corrupt += 1;
            }
        }
        self.pending.clear();
    }

    /// Settles what the last pass read: call once the drain ends.
    pub fn finish(&mut self, target: &mut dyn Target) -> Result<()> {
        let write_index = target.read_word(self.layout.address)?;
        self.settle(write_index);
        Ok(())
    }
}
