//! The recorder against a simulated target whose writer runs between the
//! words the probe reads: every interleaving a probe can meet, torn slots
//! included, in a few milliseconds.

use super::{Item, Recorder};
use crate::capture::{Elf, Header, Record, Writer};
use crate::events::{Catalogue, Event, Ring};
use crate::model::{Element, Trace};
use crate::target::{Result, Target};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const SLOTS: u32 = 16;
const MAX_EVENT_SLOTS: u32 = 4;
const BASE: u32 = 0x2000_0000;
const WRITE_INDEX_OFFSET: u32 = 0;
const LAPPED_OFFSET: u32 = 4;
/// Not a word: the writer reads the write index back, and counts itself
/// lapped if the ring went past its index, here the store's value.
const READ_BACK: usize = usize::MAX;
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
    /// Whether a writer may be preempted in the middle of an event.
    preempt: bool,
    /// The index of the event the stores under way belong to.
    current: u32,
    /// The stores left of a preempted writer, its index, and how far the
    /// ring runs on before it resumes: always past a full lap.
    preempted: VecDeque<(usize, u32)>,
    preempted_index: u32,
    resume_after: u32,
    preemptions: u32,
    stale: Vec<(u32, u32, bool)>,
    /// The indexes whose slots a lapped writer's stores landed in, and that writer.
    stomped: BTreeMap<u32, Vec<u32>>,
    /// The writers whose first slot a read met before they published it.
    read_unpublished: BTreeSet<u32>,
    /// The write index the host read last.
    host_write_index: u32,
    /// Whether the writer runs in bursts that overtake the probe.
    bursts: bool,
    /// Whether the writer stays slower than the probe, quiet or steady.
    slow: bool,
    /// Word stores between two words read drawn up to this, always: a
    /// writer that steadily outruns the probe.
    fast: Option<u32>,
    /// The writers that counted themselves lapped.
    counted: Vec<u32>,
    /// Whether the core sleeps through some passes whole, from their
    /// counters on: the probe reads zeros, and the writer stands still.
    doze: bool,
    /// Whether the core sleeps for runs of at least four words read,
    /// anywhere in a pass.
    naps: bool,
    /// Whether the core sleeps through the pass under way, and the passes it slept through.
    dozing: bool,
    dozes: u32,
    /// The words the core sleeps on for.
    asleep: u32,
}

impl Simulated {
    fn new(seed: u64) -> Self {
        Self {
            memory: vec![0; ((SLOTS_OFFSET + SLOTS * 16) / 4) as usize],
            stores: VecDeque::new(),
            written: BTreeMap::new(),
            write_index: 0,
            random: seed | 1,
            rate: 0,
            reads: 0,
            stopped: false,
            preempt: false,
            current: 0,
            preempted: VecDeque::new(),
            preempted_index: 0,
            resume_after: 0,
            preemptions: 0,
            stale: Vec::new(),
            stomped: BTreeMap::new(),
            read_unpublished: BTreeSet::new(),
            host_write_index: 0,
            bursts: true,
            slow: false,
            fast: None,
            counted: Vec::new(),
            doze: false,
            naps: false,
            dozing: false,
            dozes: 0,
            asleep: 0,
        }
    }

    /// Completes the events under way, then writes nothing until restarted.
    fn stop(&mut self) {
        if self.preempted_writes() {
            self.stale.push((self.preempted_index, self.write_index, true));
            self.blame();
        }
        let preempted = std::mem::take(&mut self.preempted);
        for (word, value) in std::mem::take(&mut self.stores).into_iter().chain(preempted) {
            self.store(word, value);
        }
        self.stopped = true;
    }

    /// Records the indexes the preempted writer's stores, about to land,
    /// belong to now: those of the lap below the write index.
    fn blame(&mut self) {
        for &(word, _) in self.preempted.iter().filter(|store| store.0 != READ_BACK) {
            let position = (word as u32 - SLOTS_OFFSET / 4) / 4 % SLOTS;
            let below = (self.write_index - position - 1) / SLOTS * SLOTS + position;
            let writers = self.stomped.entry(below).or_default();
            if !writers.contains(&self.preempted_index) {
                writers.push(self.preempted_index);
            }
        }
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
        self.current = index;
        self.write_index += count;
        self.memory[(WRITE_INDEX_OFFSET / 4) as usize] = self.write_index;
        let payload: Vec<u8> = (0..size / 4)
            .flat_map(|word| (index << 8 | word).to_le_bytes())
            .collect();
        let timestamp = !index;
        let lap = |n: u32| (((index + n) / SLOTS) + 1) & 0x7fff;
        let slot = |n: u32| ((SLOTS_OFFSET + ((index + n) % SLOTS) * 16) / 4) as usize;
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
                .push_back((slot(tail), u32::from(type_id) | lap(tail) << LAP_SHIFT));
        }
        self.stores
            .push_back((slot(0), u32::from(type_id) | HEAD_BIT | lap(0) << LAP_SHIFT));
        self.stores.push_back((READ_BACK, index));
        self.written.insert(
            index,
            Written {
                type_id,
                timestamp,
                payload,
            },
        );
    }

    /// Runs the writer for `stores` word stores. With `preempt`, a writer
    /// may stop in the middle of an event while others write past it by more
    /// than a full lap, then resume where it stopped.
    fn run(&mut self, stores: u32) {
        for _ in 0..stores {
            if self.preempt && !self.stores.is_empty() && self.preempted.is_empty() && self.random(64) == 0 {
                self.preempted = std::mem::take(&mut self.stores);
                self.preempted_index = self.current;
                // Past a lap, and done before a danger raised as soon as the
                // ring lapped it expires: the host's limits are not tested here.
                self.resume_after = SLOTS + 1 + self.random(SLOTS - MAX_EVENT_SLOTS);
                self.preemptions += 1;
            }
            if self.stores.is_empty() {
                if !self.preempted.is_empty() && self.write_index.wrapping_sub(self.preempted_index) > self.resume_after
                {
                    if self.preempted_writes() {
                        self.blame();
                        self.stale.push((self.preempted_index, self.write_index, false));
                    }
                    self.stores = std::mem::take(&mut self.preempted);
                    self.current = self.preempted_index;
                } else {
                    self.emit();
                }
            }
            let (word, value) = self.stores.pop_front().expect("an event queued");
            self.store(word, value);
        }
    }

    /// Whether the preempted writer has slots left to write, not only its read-back.
    fn preempted_writes(&self) -> bool {
        self.preempted.iter().any(|store| store.0 != READ_BACK)
    }

    /// One store of the writer, or its read-back of the write index.
    fn store(&mut self, word: usize, value: u32) {
        if word != READ_BACK {
            self.memory[word] = value;
        } else if self.write_index.wrapping_sub(value) > SLOTS {
            self.memory[(LAPPED_OFFSET / 4) as usize] += 1;
            self.counted.push(value);
        }
    }
}

impl Target for Simulated {
    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        if self.reads.is_multiple_of(500) {
            // Quiet, steady, or bursts that overtake the probe.
            let rates = if self.slow {
                2
            } else if self.bursts {
                4
            } else {
                3
            };
            self.rate = [0, 1, 3, 40][self.random(rates) as usize];
        }
        if let Some(rate) = self.fast {
            self.rate = rate;
        }
        self.reads += 1;
        if self.doze && address == BASE + WRITE_INDEX_OFFSET {
            self.dozing = self.random(4) == 0;
            self.dozes += u32::from(self.dozing);
        }
        let copy = words.len() > 1;
        for (n, word) in words.iter_mut().enumerate() {
            if self.naps && self.asleep == 0 && self.random(256) == 0 {
                self.asleep = 4 + self.random(40);
            }
            if self.dozing || self.asleep > 0 {
                self.asleep = self.asleep.saturating_sub(1);
                *word = 0;
                continue;
            }
            if !self.stopped {
                let stores = self.random(self.rate + 1);
                self.run(stores);
            }
            let at = (address - BASE) / 4 + n as u32;
            *word = self.memory[at as usize];
            if at == WRITE_INDEX_OFFSET / 4 {
                self.host_write_index = *word;
            }
            // The host copies slots, and reads a position as the index of the
            // last lap below the write index it read last; a single word read
            // is a danger's head, of an earlier lap.
            if copy && at >= SLOTS_OFFSET / 4 && (at - SLOTS_OFFSET / 4).is_multiple_of(4) {
                let position = (at - SLOTS_OFFSET / 4) / 4;
                for (writing, index) in [
                    (!self.stores.is_empty(), self.current),
                    (!self.preempted.is_empty(), self.preempted_index),
                ] {
                    // Read as `index`, and before the ring reserved its slot again.
                    let seen = index < self.host_write_index
                        && self.host_write_index - index <= SLOTS
                        && self.write_index - index <= SLOTS;
                    if writing && seen && index % SLOTS == position {
                        self.read_unpublished.insert(index);
                    }
                }
            }
        }
        Ok(())
    }
}

fn catalogue() -> Catalogue {
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
                        text: None,
                    },
                )
            })
            .collect(),
        rings: vec![Ring {
            name: "trace_ring".into(),
            address: u64::from(BASE),
            slots: SLOTS,
            size: u64::from(SLOTS_OFFSET + SLOTS * 16),
            write_index_offset: u64::from(WRITE_INDEX_OFFSET),
            lapped_offset: u64::from(LAPPED_OFFSET),
            slots_offset: u64::from(SLOTS_OFFSET),
        }],
        symbols: Default::default(),
        interrupts: Default::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }
}

/// Every index from where recording starts to where it stops is either in
/// an event read as written or in a loss, once; whenever the writer stops,
/// two passes settle everything it wrote; and the run went through losses,
/// every type, and events whose slots run across the end of the ring.
#[test]
fn records_what_was_written_or_reports_it_lost() {
    records_with(false, true, 20);
}

/// The same, with writers preempted for more than a full lap: the slots they
/// reserved are reserved again by newer events before they write them. The
/// host keeps up, so every lapped writer it reports suspect is one it never
/// read unpublished.
#[test]
fn records_what_was_written_with_writers_preempted_for_a_lap() {
    records_with(true, false, 1000);
}

/// The same, with bursts that overtake the probe: lapped writers the host
/// never read unpublished are reported suspect.
#[test]
fn reports_suspect_the_lapped_writers_a_host_fallen_behind_missed() {
    records_with(true, true, 200);
}

/// A target whose core sleeps through whole passes, or through runs of words
/// anywhere in a pass, the probe reading zeros: those reads are dropped, each
/// pass that met them counted, and every event is read back as written, with
/// no suspect line. Whole passes asleep are counted one each. What a read
/// met asleep is read again, later: on this small ring, the ring may lap
/// it meanwhile, or lap a writer read unpublished before the core fell
/// asleep, whose slots are then endangered for a lap: no more than three
/// slots lost in two hundred shown.
#[test]
fn drops_what_it_reads_of_a_target_asleep() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let mut awake = 0;
    for (doze, naps) in [(false, false), (true, false), (false, true)] {
        let (mut zeros, mut dropped, mut shown) = (0, 0, 0);
        for seed in 1..=50u64 {
            let mut target = Simulated::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            target.slow = true;
            target.run(1000);
            let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
            let mut expected = recorder.next();
            let mut items = Vec::new();
            (target.doze, target.naps) = (doze, naps);
            for _ in 0..2000 {
                recorder.pass(&mut target, &mut items).unwrap();
            }
            // A nap under way runs to its end: a read it cut is read again.
            (target.doze, target.naps, target.dozing) = (false, false, false);
            while target.asleep > 0 {
                recorder.pass(&mut target, &mut items).unwrap();
            }
            target.stop();
            recorder.pass(&mut target, &mut items).unwrap();
            recorder.pass(&mut target, &mut items).unwrap();
            recorder.finish(&mut target, &mut items).unwrap();
            assert_eq!(recorder.next(), target.write_index, "seed {seed}: {}", recorder.state());
            for item in &items {
                match item {
                    Item::Event(event, _) => {
                        assert_eq!(event.index, expected, "seed {seed}: event");
                        let written = &target.written[&event.index];
                        assert_eq!(
                            (event.event.id, event.timestamp, &event.payload),
                            (written.type_id, written.timestamp, &written.payload),
                            "seed {seed}: event {}",
                            event.index
                        );
                        expected = event.index + slots(written.type_id);
                        shown += slots(written.type_id);
                    }
                    Item::Lost { from, to } => {
                        assert_eq!(*from, expected, "seed {seed}: loss");
                        dropped += to - from;
                        expected = *to;
                    }
                    Item::Suspect { from, to, writers } => panic!("seed {seed}: suspect {from} {to} {writers}"),
                }
            }
            assert_eq!(expected, target.write_index, "seed {seed}: end");
            if doze {
                assert_eq!(recorder.zeros(), target.dozes, "seed {seed}: passes asleep");
            }
            zeros += recorder.zeros();
        }
        eprintln!("doze {doze} naps {naps}: {zeros} passes dropped, slots lost {dropped}, shown {shown}");
        if doze || naps {
            assert!(zeros > 0, "doze {doze} naps {naps}: no pass met the core asleep");
            assert!(
                dropped * 200 <= shown * 3,
                "doze {doze} naps {naps}: {dropped} slots lost, {shown} shown, {awake} lost awake"
            );
        } else {
            assert_eq!(zeros, 0, "awake: passes dropped");
            awake = dropped;
        }
    }
}

/// A writer that steadily writes one to three slots while the probe reads
/// one: each pass reads no more than the probe can read before the ring laps
/// it, so the recording keeps a share of the events, each read back as
/// written, and reports the rest lost. A read of the whole backlog would be
/// lapped every time, and keep next to nothing.
#[test]
fn keeps_a_share_of_what_a_writer_outrunning_the_probe_writes() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    for rate in [2, 3, 4, 6] {
        let (mut dropped, mut shown) = (0, 0);
        for seed in 1..=50u64 {
            let mut target = Simulated::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            target.run(1000);
            let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
            let mut expected = recorder.next();
            let mut items = Vec::new();
            target.fast = Some(rate);
            for _ in 0..2000 {
                recorder.pass(&mut target, &mut items).unwrap();
            }
            target.stop();
            recorder.pass(&mut target, &mut items).unwrap();
            recorder.pass(&mut target, &mut items).unwrap();
            recorder.finish(&mut target, &mut items).unwrap();
            for item in &items {
                match item {
                    Item::Event(event, _) => {
                        assert_eq!(event.index, expected, "seed {seed}: event");
                        let written = &target.written[&event.index];
                        assert_eq!(
                            (event.event.id, event.timestamp, &event.payload),
                            (written.type_id, written.timestamp, &written.payload),
                            "seed {seed}: event {}",
                            event.index
                        );
                        expected = event.index + slots(written.type_id);
                        shown += slots(written.type_id);
                    }
                    Item::Lost { from, to } => {
                        assert_eq!(*from, expected, "seed {seed}: loss");
                        dropped += to - from;
                        expected = *to;
                    }
                    Item::Suspect { from, to, writers } => panic!("seed {seed}: suspect {from} {to} {writers}"),
                }
            }
            assert_eq!(expected, target.write_index, "seed {seed}: end");
        }
        eprintln!("rate {rate}: slots lost {dropped}, shown {shown}");
        assert!(
            shown * 25 >= dropped,
            "rate {rate}: {shown} slots shown, {dropped} lost"
        );
    }
}

/// A zero header where a slot was read written is the core asleep, but in a
/// lap whose field wraps to 0, where a following slot of type id 0 is zero.
#[test]
fn judges_a_zero_header_but_where_the_lap_field_wraps() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let mut target = Simulated::new(1);
    target.run(1000);
    let recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
    let zeros = [0u32; 8];
    let wraps = 0x8000 * SLOTS - SLOTS;
    assert_eq!(super::decode::lap(wraps, ring), 0);
    assert!((0..2).all(|slot| recorder.word(wraps + slot) != 0));
    assert_eq!(recorder.zeroed(wraps - SLOTS, &zeros), Some(0));
    assert_eq!(recorder.zeroed(wraps, &zeros), None);
    assert_eq!(recorder.zeroed(wraps - 1, &zeros), Some(0));
    assert_eq!(recorder.zeroed(wraps + SLOTS - 1, &zeros), Some(1));
}

/// A ring whose memory a script changes when the host reads the write index
/// for the `at`-th time since recording started, before its next read.
struct Scripted {
    memory: Vec<u32>,
    counter_reads: u32,
    at: u32,
    change: Vec<(usize, u32)>,
}

impl Target for Scripted {
    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        let at = ((address - BASE) / 4) as usize;
        if at != (WRITE_INDEX_OFFSET / 4) as usize && self.counter_reads == self.at {
            for (word, value) in std::mem::take(&mut self.change) {
                self.memory[word] = value;
            }
        }
        if at == (WRITE_INDEX_OFFSET / 4) as usize {
            self.counter_reads += 1;
        }
        words.copy_from_slice(&self.memory[at..at + words.len()]);
        Ok(())
    }
}

/// A writer the host read unpublished, lapped while a pass read its slot,
/// which it published before counting itself lapped: that pass reads it
/// published, raises no danger, and the writer counted after is not suspect.
#[test]
fn credits_a_writer_read_unpublished_then_published_once_lapped() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let mut target = Scripted {
        memory: vec![0; ((SLOTS_OFFSET + SLOTS * 16) / 4) as usize],
        counter_reads: 0,
        at: 2,
        change: Vec::new(),
    };
    // Recording starts at the third lap, the second written whole with
    // one-slot events.
    let writer = 2 * SLOTS;
    target.memory[(WRITE_INDEX_OFFSET / 4) as usize] = writer;
    for position in 0..SLOTS {
        target.memory[((SLOTS_OFFSET + position * 16) / 4) as usize] = 1 | HEAD_BIT | 2 << LAP_SHIFT;
    }
    let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
    target.counter_reads = 0;
    // The writer reserves its slot; two passes read it unpublished.
    target.memory[(WRITE_INDEX_OFFSET / 4) as usize] = writer + 1;
    // During the second pass's read of the slots, the ring laps the writer
    // and a newer event reserves its position; the writer publishes, then
    // counts itself lapped.
    let head = 1 | HEAD_BIT | (writer / SLOTS + 1) << LAP_SHIFT;
    target.change = vec![
        ((WRITE_INDEX_OFFSET / 4) as usize, writer + SLOTS + 2),
        ((SLOTS_OFFSET / 4) as usize, head),
        ((LAPPED_OFFSET / 4) as usize, 1),
    ];
    let mut items = Vec::new();
    for _ in 0..3 {
        recorder.pass(&mut target, &mut items).unwrap();
    }
    assert!(target.change.is_empty(), "the ring changed during the second pass");
    assert_eq!(recorder.dangers().0, 0, "dangers raised: {}", recorder.state());
    assert!(
        !items.iter().any(|item| matches!(item, Item::Suspect { .. })),
        "{}",
        recorder.state()
    );
    assert!(
        !items
            .iter()
            .any(|item| matches!(item, Item::Event(event, _) if event.index == writer)),
        "the writer was lapped, its slot may hold a newer event"
    );
}

/// A four-slot writer the ring lapped before the host read its first slot,
/// whose following slots a pass read unpublished, right after a loss. Lapped,
/// it writes its first slot's words into a newer event's slot, which the host
/// reads before the writer writes that slot's header: the danger raised from
/// its following slots also covers the slots of the loss before them it may
/// have reserved, and the newer event is reported lost, not read as intact.
#[test]
fn endangers_the_lost_slots_before_a_following_slot_read_unpublished() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let header = |type_id: u32, index: u32| type_id | HEAD_BIT | ((index / SLOTS + 1) & 0x7fff) << LAP_SHIFT;
    let slot = |index: u32| ((SLOTS_OFFSET + index % SLOTS * 16) / 4) as usize;
    let write_index = (WRITE_INDEX_OFFSET / 4) as usize;
    let mut target = Scripted {
        memory: vec![0; ((SLOTS_OFFSET + SLOTS * 16) / 4) as usize],
        counter_reads: 0,
        at: u32::MAX,
        change: Vec::new(),
    };
    // Recording starts at the third lap, the second written whole with
    // one-slot events.
    let start = 2 * SLOTS;
    target.memory[write_index] = start;
    for index in SLOTS..start {
        target.memory[slot(index)] = header(1, index);
    }
    let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
    // The writer reserves four slots, the rest are one-slot events, past a
    // lap: the first pass reads the last lap's worth, from the writer's
    // second slot, all published but the writer's.
    let writer = start + 2;
    let end = writer + SLOTS + 2;
    for index in (start..end).filter(|index| !(writer..writer + 4).contains(index)) {
        target.memory[slot(index)] = header(1, index);
    }
    // The newer event in the writer's first slot, lapped: the writer has
    // written its words there, not yet its header.
    let newer = writer + SLOTS;
    target.memory[slot(newer) + 2] = 0xdead_beef;
    target.memory[write_index] = end;
    let mut items = Vec::new();
    recorder.pass(&mut target, &mut items).unwrap();
    // The ring laps the writer's following slots read unpublished.
    for index in end..end + 2 {
        target.memory[slot(index)] = header(1, index);
    }
    target.memory[write_index] = end + 2;
    recorder.pass(&mut target, &mut items).unwrap();
    recorder.pass(&mut target, &mut items).unwrap();
    assert!(recorder.dangers().0 > 0, "no danger raised: {}", recorder.state());
    assert!(
        !items
            .iter()
            .any(|item| matches!(item, Item::Event(event, _) if event.index == newer)),
        "event {newer} read as intact, the lapped writer's words in it: {}",
        recorder.state()
    );
}

/// A read the ring laps before the next pass settles it halves the window,
/// unless the core slept in between: that lap says nothing of the probe's pace.
#[test]
fn halves_the_window_on_a_lapped_read_but_after_a_sleep() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let header = |index: u32| 1 | HEAD_BIT | ((index / SLOTS + 1) & 0x7fff) << LAP_SHIFT;
    let slot = |index: u32| ((SLOTS_OFFSET + index % SLOTS * 16) / 4) as usize;
    let write_index = (WRITE_INDEX_OFFSET / 4) as usize;
    for asleep in [false, true] {
        let mut target = Scripted {
            memory: vec![0; ((SLOTS_OFFSET + SLOTS * 16) / 4) as usize],
            counter_reads: 0,
            at: u32::MAX,
            change: Vec::new(),
        };
        // Recording starts at the third lap, the second written whole with
        // one-slot events.
        let start = 2 * SLOTS;
        let mut written = start;
        let mut write = |target: &mut Scripted, to: u32| {
            for index in written..to {
                target.memory[slot(index)] = header(index);
            }
            written = to;
            target.memory[write_index] = to;
        };
        write(&mut target, start);
        for index in SLOTS..start {
            target.memory[slot(index)] = header(index);
        }
        let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
        let mut items = Vec::new();
        // A pass reads four events.
        write(&mut target, start + 4);
        recorder.pass(&mut target, &mut items).unwrap();
        if asleep {
            target.memory[write_index] = 0;
            assert_eq!(recorder.pass(&mut target, &mut items).unwrap(), None);
        }
        // The ring laps them before the next pass settles them.
        write(&mut target, start + SLOTS + 8);
        recorder.pass(&mut target, &mut items).unwrap();
        let expected = if asleep { SLOTS } else { SLOTS / 2 };
        assert_eq!(recorder.window, expected, "asleep {asleep}: {}", recorder.state());
    }
}

/// A one-slot writer read unpublished, whose next read the core's sleep
/// dropped: the pass after finds the ring a lap past it and cuts it out of
/// its window. Lapped, the writer writes its word into the newer event that
/// reserved its slot again, then counts itself lapped: a danger covers that
/// event, which is reported lost, not read as intact.
#[test]
fn endangers_a_writer_read_unpublished_then_cut_a_lap_on_after_a_sleep() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let header = |type_id: u32, index: u32| type_id | HEAD_BIT | ((index / SLOTS + 1) & 0x7fff) << LAP_SHIFT;
    let slot = |index: u32| ((SLOTS_OFFSET + index % SLOTS * 16) / 4) as usize;
    let write_index = (WRITE_INDEX_OFFSET / 4) as usize;
    let lapped = (LAPPED_OFFSET / 4) as usize;
    let mut target = Scripted {
        memory: vec![0; ((SLOTS_OFFSET + SLOTS * 16) / 4) as usize],
        counter_reads: 0,
        at: u32::MAX,
        change: Vec::new(),
    };
    // Recording starts at the third lap, the second written whole with
    // one-slot events.
    let writer = 2 * SLOTS;
    target.memory[write_index] = writer;
    for index in SLOTS..writer {
        target.memory[slot(index)] = header(1, index);
    }
    let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
    let mut items = Vec::new();
    // The writer reserves its slot; two passes read it unpublished.
    target.memory[write_index] = writer + 1;
    recorder.pass(&mut target, &mut items).unwrap();
    recorder.pass(&mut target, &mut items).unwrap();
    // The core sleeps: the counters read as zeros.
    target.memory[write_index] = 0;
    assert_eq!(recorder.pass(&mut target, &mut items).unwrap(), None);
    // Awake, the ring a lap past the writer: newer one-slot events, the one
    // in the writer's slot holding the writer's word, its header not yet.
    let end = writer + SLOTS + 2;
    for index in writer + 1..end {
        target.memory[slot(index)] = header(1, index);
    }
    let newer = writer + SLOTS;
    target.memory[slot(newer) + 2] = 0xdead_beef;
    target.memory[write_index] = end;
    recorder.pass(&mut target, &mut items).unwrap();
    // The writer publishes, then counts itself lapped.
    target.memory[slot(newer)] = header(1, writer);
    target.memory[lapped] = 1;
    recorder.pass(&mut target, &mut items).unwrap();
    let suspect = items.iter().any(|item| matches!(item, Item::Suspect { .. }));
    assert!(
        !items
            .iter()
            .any(|item| matches!(item, Item::Event(event, _) if event.index == newer)),
        "event {newer} read as intact, the lapped writer's word in it; dangers {:?}, suspect line {suspect}: {}",
        recorder.dangers(),
        recorder.state()
    );
}

/// A capture of a simulated recording with losses and suspect lines, read
/// back by the model: the events, losses and suspect lines in the places the
/// recording yielded them, the events dated in index order from their
/// timestamps, at the capture's frequency.
#[test]
fn a_capture_reads_back_as_the_recording_yielded_it() {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let path = std::env::temp_dir().join(format!("kymo-model-{}.kcap", std::process::id()));
    let clock = 1_000_000;
    let (mut losses, mut suspects) = (0, 0);
    for seed in 1..=50u64 {
        let mut target = Simulated::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        target.preempt = true;
        target.run(1000);
        let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
        let header = Header {
            first: recorder.next(),
            clock: Some(clock),
            channel: ring.name.clone(),
            elf: Elf::Reference(String::new()),
        };
        let mut writer = Writer::create(&path, &header).unwrap();
        let (mut items, mut yielded) = (Vec::new(), Vec::new());
        for pass in 0..2000u64 {
            let write_index = if pass < 1999 {
                recorder.pass(&mut target, &mut items).unwrap()
            } else {
                target.stop();
                recorder.finish(&mut target, &mut items).unwrap()
            }
            .expect("a target awake");
            writer
                .write(&Record::Pass {
                    host_ns: 1_000 * pass,
                    write_index,
                })
                .unwrap();
            for item in items.drain(..) {
                let record = match &item {
                    Item::Event(event, slots) => Record::Event {
                        index: event.index,
                        slots,
                    },
                    Item::Lost { from, to } => Record::Lost { from: *from, to: *to },
                    Item::Suspect { from, to, writers } => Record::Suspect {
                        from: *from,
                        to: *to,
                        writers: *writers,
                    },
                };
                writer.write(&record).unwrap();
                yielded.push(item);
            }
        }
        writer.write(&Record::End { next: recorder.next() }).unwrap();
        writer.finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let trace = Trace::read(&catalogue, &bytes, None).unwrap();
        assert_eq!(trace.first, header.first, "seed {seed}");
        assert_eq!(trace.end, Some(recorder.next()), "seed {seed}");
        assert!(trace.cut.is_none(), "seed {seed}");
        assert_eq!(trace.frequency, Some(clock as f64), "seed {seed}");
        assert_eq!(trace.elements.len(), yielded.len(), "seed {seed}");
        let mut previous: Option<(u32, i64)> = None;
        let mut origin = None;
        for (element, item) in trace.elements.iter().zip(&yielded) {
            match (element, item) {
                (Element::Event { event, cycles, seconds }, Item::Event(recorded, _)) => {
                    assert_eq!(
                        (event.index, event.event.id, event.timestamp, &event.payload),
                        (recorded.index, recorded.event.id, recorded.timestamp, &recorded.payload),
                        "seed {seed}"
                    );
                    if let Some((timestamp, before)) = previous {
                        let delta = i64::from(event.timestamp.wrapping_sub(timestamp) as i32);
                        assert_eq!(*cycles, before + delta, "seed {seed}: event {}", event.index);
                    }
                    previous = Some((event.timestamp, *cycles));
                    let origin = *origin.get_or_insert(*cycles);
                    assert_eq!(*seconds, Some((cycles - origin) as f64 / clock as f64), "seed {seed}");
                }
                (Element::Lost { from, to }, Item::Lost { from: f, to: t }) => {
                    assert_eq!((from, to), (f, t), "seed {seed}");
                    losses += 1;
                }
                (
                    Element::Suspect { from, to, writers },
                    Item::Suspect {
                        from: f,
                        to: t,
                        writers: w,
                    },
                ) => {
                    assert_eq!((from, to, writers), (f, t, w), "seed {seed}");
                    suspects += 1;
                }
                _ => panic!("seed {seed}: an element out of place"),
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    assert!(losses > 0 && suspects > 0, "{losses} losses, {suspects} suspect lines");
}

/// The slots of an event of type `type_id`.
fn slots(type_id: u16) -> u32 {
    let size = TYPES.iter().find(|t| t.0 == type_id).expect("a type").1;
    super::decode::slots_for(u64::from(size)) as u32
}

fn records_with(preempt: bool, bursts: bool, seeds: u64) {
    let catalogue = catalogue();
    let ring = &catalogue.rings[0];
    let (mut all_suspects, mut silent) = (0, 0);
    for seed in 1..=seeds {
        let mut target = Simulated::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        target.preempt = preempt;
        target.bursts = bursts;
        target.run(1000);
        // Recording starts while a writer is preempted, its slots not yet
        // written: the start must raise its danger.
        while preempt && target.preempted.is_empty() {
            target.run(1);
        }
        let mut recorder = Recorder::start(&mut target, &catalogue, ring).unwrap();
        let mut expected = recorder.next();
        let mut items = Vec::new();
        let mut blocked = 0;
        for _ in 0..50 {
            target.stopped = false;
            for _ in 0..100 {
                recorder.pass(&mut target, &mut items).unwrap();
            }
            target.stop();
            recorder.pass(&mut target, &mut items).unwrap();
            recorder.pass(&mut target, &mut items).unwrap();
            let next = recorder.next();
            if next != target.write_index {
                // Accepted only within ADR-054's limits, a host fallen behind or
                // a writer held past its danger's expiry: a lapped writer the
                // host did not record overwrote the first slot at `next`, which
                // waits for the ring to lap it: that writer's slot at `next`'s
                // position carries `next`'s lap.
                let header = |position: u32| target.memory[((SLOTS_OFFSET + position * 16) / 4) as usize];
                let unseen = |index: u32| {
                    !items
                        .iter()
                        .any(|item| matches!(item, Item::Event(event, _) if event.index == index))
                };
                let stomped = target.stale.iter().any(|&(index, _, _)| {
                    (0..slots(target.written[&index].type_id)).any(|slot| {
                        let position = (index + slot) % SLOTS;
                        position == next % SLOTS
                            && header(position) >> LAP_SHIFT == (((index + slot) / SLOTS) + 1) & 0x7fff
                    }) && index < next
                        && unseen(index)
                        && !target.read_unpublished.contains(&index)
                });
                assert!(
                    stomped,
                    "seed {seed}: the writer stopped, next {next} write index {} header {:#x} stale {:?} {}",
                    target.write_index,
                    header(next % SLOTS),
                    target
                        .stale
                        .iter()
                        .filter(|s| next.abs_diff(s.0) < 4 * SLOTS)
                        .collect::<Vec<_>>(),
                    recorder.state()
                );
                blocked += 1;
            }
        }
        recorder.finish(&mut target, &mut items).unwrap();
        let (mut events, mut lost, mut across, mut dropped, mut shown, mut wrong) = ([0u32; 3], 0, 0, 0u32, 0u32, 0);
        let mut suspects = 0;
        let mut previous_lost = false;
        for item in &items {
            match item {
                Item::Suspect { writers, .. } => suspects += writers,
                Item::Lost { from, to } => {
                    assert_eq!(*from, expected, "seed {seed}: loss");
                    assert!(!previous_lost, "seed {seed}: a loss right after a loss, from {from}");
                    previous_lost = true;
                    assert!(to > from, "seed {seed}: empty loss");
                    lost += 1;
                    dropped += to - from;
                    expected = *to;
                }
                Item::Event(event, _) => {
                    assert_eq!(event.index, expected, "seed {seed}: event");
                    previous_lost = false;
                    let written = &target.written[&event.index];
                    let count = super::decode::slots_for(event.event.size) as u32;
                    if event.event.id != written.type_id
                        || event.timestamp != written.timestamp
                        || event.payload != written.payload
                    {
                        // Accepted only from a lapped writer the host never read
                        // unpublished: a host fallen behind, ADR-054's limit.
                        let writers: Vec<u32> = (event.index..event.index + count)
                            .flat_map(|index| target.stomped.get(&index).into_iter().flatten().copied())
                            .collect();
                        let unread = !writers.is_empty()
                            && writers.iter().all(|writer| !target.read_unpublished.contains(writer));
                        assert!(
                            unread,
                            "seed {seed}: event {} writers {writers:?} read {:?} written {:?} {}",
                            event.index,
                            event.payload,
                            written.payload,
                            recorder.state()
                        );
                        wrong += 1;
                    }
                    events[usize::from(written.type_id - 1)] += 1;
                    across += u32::from(event.index % SLOTS + count > SLOTS);
                    shown += count;
                    expected = event.index + count;
                }
            }
        }
        assert_eq!(expected, recorder.next(), "seed {seed}: end");
        assert!(
            lost > 0 && across > 0 && events.iter().all(|&count| count > 0),
            "seed {seed}: coverage {events:?} {lost} {across} dangers {:?}",
            recorder.dangers()
        );
        assert_eq!(preempt, target.preemptions > 0, "seed {seed}: preemptions");
        // A writer reported suspect is one the host never read unpublished,
        // or one lapped once it had published, which wrote nothing stale: the
        // host may have read it published, and raised no danger.
        let unread = target
            .counted
            .iter()
            .filter(|writer| {
                !target.read_unpublished.contains(writer) || !target.stale.iter().any(|stale| stale.0 == **writer)
            })
            .count() as u32;
        assert!(
            suspects <= unread,
            "seed {seed}: {suspects} suspect, {} counted, {unread} never read unpublished {}",
            target.counted.len(),
            recorder.state()
        );
        all_suspects += suspects;
        silent += u32::from(wrong > 0 && suspects == 0);
        let (raised, lifted, expired, _) = recorder.dangers();
        eprintln!(
            "seed {seed}: preemptions {} dangers raised {raised} lifted {lifted} expired {expired} losses {lost} (indexes lost {dropped}, shown {shown}) blocked {blocked} wrong unread {wrong} counted {} unread {unread} suspect {suspects}",
            target.preemptions,
            target.counted.len()
        );
    }
    eprintln!("suspect writers {all_suspects}, seeds with a wrong event and no suspect {silent}");
    assert_eq!(preempt, all_suspects > 0, "suspect writers");
}

/// Registers read one word at a time; reading any other address fails.
struct Registers(BTreeMap<u32, u32>);

impl Target for Registers {
    fn read_words(&mut self, address: u32, words: &mut [u32]) -> Result<()> {
        for (n, word) in words.iter_mut().enumerate() {
            let at = address + 4 * n as u32;
            *word = *self.0.get(&at).ok_or_else(|| format!("read at {at:#x}"))?;
        }
        Ok(())
    }
}

#[test]
fn the_dwt_is_read_by_architecture() {
    use crate::capture::{Comparator, Dwt};
    let comparator = |comp, mask, function| Comparator { comp, mask, function };
    // ARMv8-M has no DWT_MASKn: reading one would fail.
    let mut armv8m = Registers(BTreeMap::from([
        (0xe000_1fbc, 0x4770_1a02),
        (0xe000_1000, 2 << 28 | 1),
        (0xe000_1020, 0x2000_0014),
        (0xe000_1028, 0x0812),
        (0xe000_1030, 0x2000_0100),
        (0xe000_1038, 0x0412),
    ]));
    assert_eq!(
        super::read_dwt(&mut armv8m, 2_000_000).unwrap(),
        Dwt {
            devarch: 0x4770_1a02,
            ctrl: 2 << 28 | 1,
            baud: 2_000_000,
            comparators: vec![comparator(0x2000_0014, 0, 0x0812), comparator(0x2000_0100, 0, 0x0412)],
        }
    );
    let mut armv7m = Registers(BTreeMap::from([
        (0xe000_1fbc, 0),
        (0xe000_1000, 1 << 28),
        (0xe000_1020, 0x2000_0014),
        (0xe000_1024, 2),
        (0xe000_1028, 0b0011),
    ]));
    assert_eq!(
        super::read_dwt(&mut armv7m, 115_200).unwrap().comparators,
        [comparator(0x2000_0014, 2, 0b0011)]
    );
}
