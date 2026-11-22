//! The recorder against a simulated target whose writer runs between the
//! words the probe reads: every interleaving a probe can meet, torn slots
//! included, in a few milliseconds.

use super::{Item, Recorder};
use crate::events::{Catalogue, Event, Ring};
use crate::target::{Result, Target};
use std::collections::{BTreeMap, VecDeque};

const SLOTS: u32 = 16;
const MAX_EVENT_SLOTS: u32 = 4;
const BASE: u32 = 0x2000_0000;
const WRITE_INDEX_OFFSET: u32 = 0;
const SLOTS_OFFSET: u32 = 16;
const HEAD_BIT: u32 = 1 << 16;
const LAP_SHIFT: u32 = 17;
/// The event types: id and size in bytes, one, two and four slots.
const TYPES: [(u16, u32); 3] = [(1, 4), (2, 16), (3, 44)];

/// What the writer put at an index: its type, timestamp and payload.
struct Written {
    type_id: u16,
    timestamp: u32,
    payload: Vec<u8>,
}

/// A target running ring.hpp's writer, a word store at a time, between the
/// words the probe reads: reserve, the first slot's words, each following
/// slot's words then header, the first slot's header last.
struct Simulated {
    memory: Vec<u32>,
    /// Word stores of the events reserved, in the writer's order.
    stores: VecDeque<(usize, u32)>,
    written: BTreeMap<u32, Written>,
    write_index: u32,
    random: u64,
    /// Word stores between two words read: drawn up to this, which swings
    /// between quiet and bursts.
    rate: u32,
    reads: u64,
    stopped: bool,
}

impl Simulated {
    fn new(seed: u64) -> Self {
        let physical = SLOTS + MAX_EVENT_SLOTS - 1;
        Self {
            memory: vec![0; ((SLOTS_OFFSET + physical * 16) / 4) as usize],
            stores: VecDeque::new(),
            written: BTreeMap::new(),
            write_index: 0,
            random: seed | 1,
            rate: 0,
            reads: 0,
            stopped: false,
        }
    }

    /// Completes the event under way, then writes nothing until restarted.
    fn stop(&mut self) {
        while let Some((word, value)) = self.stores.pop_front() {
            self.memory[word] = value;
        }
        self.stopped = true;
    }

    fn random(&mut self, below: u32) -> u32 {
        // xorshift64
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        (self.random % u64::from(below)) as u32
    }

    /// Reserves the next event and queues its word stores.
    fn emit(&mut self) {
        let (type_id, size) = TYPES[self.random(3) as usize];
        let count = if size <= 8 { 1 } else { 1 + (size - 8).div_ceil(12) };
        let index = self.write_index;
        self.write_index += count;
        self.memory[(WRITE_INDEX_OFFSET / 4) as usize] = self.write_index;
        let payload: Vec<u8> = (0..size / 4)
            .flat_map(|word| (index << 8 | word).to_le_bytes())
            .collect();
        let timestamp = !index;
        let lap = ((index / SLOTS) + 1) & 0x7fff;
        let slot = |n: u32| ((SLOTS_OFFSET + ((index % SLOTS) + n) * 16) / 4) as usize;
        let word = |offset: u32| u32::from_le_bytes(payload[offset as usize..][..4].try_into().unwrap());
        self.stores.push_back((slot(0) + 1, timestamp));
        for offset in (0..size.min(8)).step_by(4) {
            self.stores
                .push_back((slot(0) + 2 + (offset / 4) as usize, word(offset)));
        }
        for tail in 1..count {
            let from = 8 + (tail - 1) * 12;
            for offset in (from..size.min(from + 12)).step_by(4) {
                self.stores
                    .push_back((slot(tail) + 1 + ((offset - from) / 4) as usize, word(offset)));
            }
            self.stores
                .push_back((slot(tail), u32::from(type_id) | lap << LAP_SHIFT));
        }
        self.stores
            .push_back((slot(0), u32::from(type_id) | HEAD_BIT | lap << LAP_SHIFT));
        self.written.insert(
            index,
            Written {
                type_id,
                timestamp,
                payload,
            },
        );
    }

    /// Runs the writer for `stores` word stores.
    fn run(&mut self, stores: u32) {
        for _ in 0..stores {
            if self.stores.is_empty() {
                self.emit();
            }
            let (word, value) = self.stores.pop_front().expect("an event queued");
            self.memory[word] = value;
        }
    }
}

impl Target for Simulated {
    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        if self.reads.is_multiple_of(500) {
            // Quiet, steady, or bursts that overtake the probe.
            self.rate = [0, 1, 3, 40][self.random(4) as usize];
        }
        self.reads += 1;
        for (n, word) in words.iter_mut().enumerate() {
            if !self.stopped {
                let stores = self.random(self.rate + 1);
                self.run(stores);
            }
            *word = self.memory[((address - BASE) / 4) as usize + n];
        }
        Ok(())
    }

    fn write_word(&mut self, _address: u32, _value: u32) -> Result<()> {
        Err("read only".into())
    }
}

fn catalogue() -> Catalogue {
    let physical = SLOTS + MAX_EVENT_SLOTS - 1;
    Catalogue {
        events: TYPES
            .iter()
            .map(|&(id, size)| {
                (
                    id,
                    Event {
                        id,
                        name: format!("type{id}"),
                        size: u64::from(size),
                        fields: Vec::new(),
                    },
                )
            })
            .collect(),
        ring: Some(Ring {
            slots: SLOTS,
            max_event_slots: MAX_EVENT_SLOTS,
            size: u64::from(SLOTS_OFFSET + physical * 16),
            write_index_offset: u64::from(WRITE_INDEX_OFFSET),
            slots_offset: u64::from(SLOTS_OFFSET),
        }),
        symbols: Default::default(),
    }
}

/// Every index from where recording starts to where it stops is either in
/// an event read as written or in a loss, once; whenever the writer stops,
/// two passes settle everything it wrote; and the run went through losses,
/// every type, and events whose slots run into the guard zone.
#[test]
fn records_what_was_written_or_reports_it_lost() {
    let catalogue = catalogue();
    for seed in 1..=20u64 {
        let mut target = Simulated::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        target.run(1000);
        let mut recorder = Recorder::start(&mut target, &catalogue, BASE).unwrap();
        let mut expected = recorder.next();
        let mut items = Vec::new();
        for _ in 0..50 {
            target.stopped = false;
            for _ in 0..100 {
                recorder.pass(&mut target, &mut items).unwrap();
            }
            target.stop();
            recorder.pass(&mut target, &mut items).unwrap();
            recorder.pass(&mut target, &mut items).unwrap();
            assert_eq!(recorder.next(), target.write_index, "seed {seed}: the writer stopped");
        }
        recorder.finish(&mut target, &mut items).unwrap();
        let (mut events, mut lost, mut guard) = ([0u32; 3], 0, 0);
        let mut previous_lost = false;
        for item in &items {
            match item {
                Item::Lost { from, to } => {
                    assert_eq!(*from, expected, "seed {seed}: loss");
                    assert!(!previous_lost, "seed {seed}: a loss right after a loss, from {from}");
                    previous_lost = true;
                    assert!(to > from, "seed {seed}: empty loss");
                    lost += 1;
                    expected = *to;
                }
                Item::Event(event, _) => {
                    assert_eq!(event.index, expected, "seed {seed}: event");
                    previous_lost = false;
                    let written = &target.written[&event.index];
                    assert_eq!(event.event.id, written.type_id, "seed {seed}: event {}", event.index);
                    assert_eq!(event.timestamp, written.timestamp, "seed {seed}: event {}", event.index);
                    assert_eq!(event.payload, written.payload, "seed {seed}: event {}", event.index);
                    let count = super::decode::slots_for(event.event.size) as u32;
                    events[usize::from(written.type_id - 1)] += 1;
                    guard += u32::from(event.index % SLOTS + count > SLOTS);
                    expected = event.index + count;
                }
            }
        }
        assert_eq!(expected, recorder.next(), "seed {seed}: end");
        assert!(
            lost > 0 && guard > 0 && events.iter().all(|&count| count > 0),
            "seed {seed}: coverage {events:?} {lost} {guard}"
        );
    }
}
