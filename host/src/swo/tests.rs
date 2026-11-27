use super::*;
use crate::capture::Comparator;
use crate::decode::Decoded;
use crate::events::Event;
use crate::interrupts::Interrupts;
use crate::symbols::Symbols;
use crate::values::Number;

/// An ITM stream built packet by packet.
#[derive(Default)]
struct Stream(Vec<u8>);

impl Stream {
    /// A data value written through `comparator`, of `size` bytes.
    fn value(mut self, comparator: u8, value: u32, size: u8) -> Self {
        let code = match size {
            1 => 1,
            2 => 2,
            _ => 3,
        };
        let discriminator = 0x10 | comparator << 1 | 1;
        self.0.push(discriminator << 3 | 0b100 | code);
        self.0.extend(&value.to_le_bytes()[..usize::from(size)]);
        self
    }

    /// An anchor through comparator 0: a 4-byte data value.
    fn anchor(self, timestamp: u32) -> Self {
        self.value(0, timestamp, 4)
    }

    /// A local timestamp `cycles` after the one before.
    fn stamp(mut self, cycles: u32) -> Self {
        let mut delta = cycles - 1;
        self.0.push(0xc0);
        loop {
            let low = (delta & 0x7f) as u8;
            delta >>= 7;
            if delta == 0 {
                self.0.push(low);
                return self;
            }
            self.0.push(0x80 | low);
        }
    }

    /// A local timestamp `cycles` after the one before, delayed: its control 1.
    fn delayed(mut self, cycles: u32) -> Self {
        self = self.stamp(cycles);
        let at = self.0.iter().rposition(|&byte| byte == 0xc0).expect("a timestamp");
        self.0[at] = 0xd0;
        self
    }

    /// An exception packet: hardware source 1, its number and its function.
    fn exception(mut self) -> Self {
        self.0.extend([0x0e, 0x2d, 0x10]);
        self
    }

    fn overflow(mut self) -> Self {
        self.0.push(0x70);
        self
    }

    /// A zero that starts no synchronisation packet.
    fn malformed(mut self) -> Self {
        self.0.push(0x00);
        self
    }
}

const LEVEL: Watch = Watch {
    comparator: 1,
    address: 0x2000_0100,
    size: 2,
};

fn expected(anchors: &[(i64, u32)]) -> Vec<Expected> {
    anchors
        .iter()
        .enumerate()
        .map(|(element, &(cycles, timestamp))| Expected {
            element,
            cycles,
            timestamp,
        })
        .collect()
}

#[test]
fn a_sample_is_dated_from_the_anchor_before_it() {
    let stream = Stream::default()
        .anchor(0x100)
        .stamp(10)
        .value(1, 42, 2)
        .stamp(300)
        .anchor(0x200)
        .stamp(700);
    let dated = date(&stream.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(
        dated,
        Dated {
            samples: vec![(0, 42, 1_300)],
            span: Some((1_000, 2_000)),
            anchors: 2,
            ..Dated::default()
        }
    );
}

#[test]
fn the_itm_clock_counts_each_delta_plus_one() {
    // Ten timestamps of 100 cycles between the anchors: the sum of the deltas
    // alone falls 10 cycles short, beyond the tolerance.
    let mut stream = Stream::default().anchor(0x100);
    for _ in 0..10 {
        stream = stream.stamp(100);
    }
    let stream = stream.value(1, 1, 2).stamp(50).anchor(0x200).stamp(50);
    let dated = date(&stream.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(dated.gaps, []);
    assert_eq!(dated.samples, [(0, 1, 1_950)]);
}

#[test]
fn incoherent_anchors_are_a_gap() {
    let stream = Stream::default()
        .anchor(0x100)
        .stamp(10)
        .value(1, 42, 2)
        .stamp(300)
        .anchor(0x200)
        .stamp(700 + TOLERANCE as u32 + 1);
    let dated = date(&stream.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(dated.gaps, [(1_000, 2_000)]);
    assert_eq!(dated.samples, []);
    assert_eq!(dated.dropped, [(0, 1_000)]);

    let within = Stream::default()
        .anchor(0x100)
        .stamp(10)
        .anchor(0x200)
        .stamp(1_000 + TOLERANCE as u32);
    let dated = date(&within.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(dated.gaps, []);
}

#[test]
fn an_overflow_between_anchors_is_a_gap() {
    let stream = Stream::default()
        .anchor(0x100)
        .stamp(10)
        .value(1, 42, 2)
        .overflow()
        .stamp(300)
        .anchor(0x200)
        .stamp(700);
    let dated = date(&stream.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(dated.overflows, 1);
    assert_eq!(dated.gaps, [(1_000, 2_000)]);
    assert_eq!(dated.dropped, [(0, 1_000)]);
}

#[test]
fn a_malformed_packet_between_anchors_is_a_gap_but_no_overflow() {
    let stream = Stream::default()
        .anchor(0x100)
        .stamp(10)
        .value(1, 42, 2)
        .malformed()
        .stamp(300)
        .anchor(0x200)
        .stamp(700);
    let dated = date(&stream.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(dated.overflows, 0);
    assert_eq!(dated.gaps, [(1_000, 2_000)]);
    assert_eq!(dated.dropped, [(0, 1_000)]);
}

#[test]
fn an_anchor_timestamp_seen_again_matches_the_later_write() {
    // The ring's 32-bit timestamp comes back to a value it carried before: the
    // third anchor matches its own write, not the first one.
    let stream = Stream::default()
        .anchor(0x100)
        .stamp(10)
        .anchor(0x200)
        .stamp(1_000)
        .value(1, 42, 2)
        .stamp(300)
        .anchor(0x100)
        .stamp(700);
    let dated = date(
        &stream.0,
        0,
        &[LEVEL],
        &expected(&[(1_000, 0x100), (2_000, 0x200), (3_000, 0x100)]),
    );
    assert_eq!(dated.gaps, []);
    assert_eq!(dated.samples, [(0, 42, 2_300)]);
}

/// Three anchors, at 1000, 1500 and 2000 cycles, the middle one misdated by
/// 200 cycles through `middle`, a sample on each side of it.
fn around_a_misdated_anchor(middle: impl FnOnce(Stream) -> Stream) -> Dated {
    let stream = Stream::default().anchor(0x100).stamp(10).value(1, 42, 2).stamp(300);
    let stream = middle(stream.anchor(0x180))
        .value(1, 43, 2)
        .stamp(100)
        .anchor(0x200)
        .stamp(200);
    date(
        &stream.0,
        0,
        &[LEVEL],
        &expected(&[(1_000, 0x100), (1_500, 0x180), (2_000, 0x200)]),
    )
}

#[test]
fn an_anchor_with_a_delayed_or_shared_timestamp_dates_nothing() {
    for (what, middle) in [
        (
            "delayed",
            (|stream: Stream| stream.delayed(400)) as fn(Stream) -> Stream,
        ),
        ("shared with an exception", |stream| stream.exception().stamp(400)),
        ("shared with a sample", |stream| stream.value(1, 7, 2).stamp(400)),
    ] {
        let dated = around_a_misdated_anchor(middle);
        assert_eq!(dated.gaps, [], "{what}");
        assert_eq!((dated.anchors, dated.set_aside), (2, 1), "{what}");
        assert_eq!(dated.span, Some((1_000, 2_000)), "{what}");
        assert_eq!(dated.samples.first(), Some(&(0, 42, 1_300)), "{what}");
        assert_eq!(dated.samples.last(), Some(&(0, 43, 1_800)), "{what}");
    }

    // The same anchor, synchronous and alone, is a reference: 200 cycles off.
    let dated = around_a_misdated_anchor(|stream| stream.stamp(400));
    assert_eq!((dated.anchors, dated.set_aside), (3, 0));
    assert_eq!(dated.gaps, [(1_000, 1_500), (1_500, 2_000)]);
}

#[test]
fn a_missing_anchor_is_a_gap() {
    // The event at 1500 starts at slot 0, but its anchor never came: the
    // stream lost it, or the comparator watched something else meanwhile.
    let stream = Stream::default()
        .anchor(0x100)
        .stamp(10)
        .value(1, 42, 2)
        .stamp(300)
        .anchor(0x200)
        .stamp(700);
    let dated = date(
        &stream.0,
        0,
        &[LEVEL],
        &expected(&[(1_000, 0x100), (1_500, 0x180), (2_000, 0x200)]),
    );
    assert_eq!(dated.anchors, 2);
    assert_eq!(dated.gaps, [(1_000, 2_000)]);
    assert_eq!(dated.samples, []);
}

#[test]
fn a_write_to_the_anchor_that_is_no_event_start_is_not_an_anchor() {
    // An event wrapping around the ring writes its tail over slot 0: the
    // anchor's comparator sends a word that no event of the trace carries.
    let stream = Stream::default()
        .anchor(0xdead)
        .stamp(5)
        .anchor(0x100)
        .stamp(10)
        .value(1, 42, 2)
        .stamp(300)
        .anchor(0xbeef)
        .stamp(100)
        .anchor(0x200)
        .stamp(600);
    let dated = date(&stream.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(dated.gaps, []);
    assert_eq!(dated.samples, [(0, 42, 1_300)]);
}

#[test]
fn samples_outside_the_anchors_are_undated() {
    let stream = Stream::default()
        .value(1, 1, 2)
        .stamp(10)
        .anchor(0x100)
        .stamp(10)
        .anchor(0x200)
        .stamp(1_000)
        .value(1, 2, 2)
        .stamp(10)
        .value(1, 3, 2);
    let dated = date(&stream.0, 0, &[LEVEL], &expected(&[(1_000, 0x100), (2_000, 0x200)]));
    assert_eq!(dated.undated, 3);
    assert_eq!(dated.samples, []);

    let none = date(&stream.0, 0, &[LEVEL], &[]);
    assert_eq!((none.undated, none.anchors, none.span), (3, 0, None));
}

/// The function of an ARMv8-M comparator sending a data value of 2^`log2`
/// bytes on each write.
fn v8(log2: u32) -> u32 {
    WRITE_VALUE | DATA_VALUE_PACKET << 4 | log2 << 10
}

fn dwt(devarch: u32, comparators: &[(u32, u32, u32)]) -> Dwt {
    Dwt {
        devarch,
        ctrl: (comparators.len() as u32) << 28,
        baud: 2_000_000,
        comparators: comparators
            .iter()
            .map(|&(comp, mask, function)| Comparator { comp, mask, function })
            .collect(),
    }
}

#[test]
fn watches_read_both_architectures() {
    let armv8m = dwt(
        0x4770_1a02,
        &[
            (0x2000_0014, 0, v8(2)),
            (0x2000_0100, 0, v8(1)),
            (0x2000_0200, 0, v8(0)),
            (0x2000_0300, 0, WRITE_VALUE | 0b01 << 4 | 2 << 10),
            (0x2000_0400, 0, v8(2)),
        ],
    );
    let watch = |comparator, address, size| Watch {
        comparator,
        address,
        size,
    };
    assert_eq!(
        watches(&armv8m),
        [
            watch(0, 0x2000_0014, 4),
            watch(1, 0x2000_0100, 2),
            watch(2, 0x2000_0200, 1)
        ]
    );

    let armv7m = dwt(
        0,
        &[
            (0x2000_0014, 2, WRITE_VALUE),
            (0x2000_0100, 1, WRITE_VALUE),
            (0x2000_0200, 3, WRITE_VALUE),
            (0x2000_0300, 0, 0b0111),
        ],
    );
    assert_eq!(watches(&armv7m), [watch(0, 0x2000_0014, 4), watch(1, 0x2000_0100, 2)]);

    let silent = Dwt {
        ctrl: armv8m.ctrl | NO_TRACE_PACKETS,
        ..armv8m
    };
    assert_eq!(watches(&silent), []);
}

const RING: u64 = 0x2000_1000;
const SLOTS_OFFSET: u64 = 0x10;

fn ring() -> Ring {
    Ring {
        name: "kymo::ring".to_owned(),
        address: RING,
        slots: 4,
        size: 0x50,
        write_index_offset: 0,
        lapped_offset: 4,
        slots_offset: SLOTS_OFFSET,
    }
}

fn catalogue() -> Catalogue {
    let tick = Event {
        id: 0,
        name: "app::tick".to_owned(),
        size: 0,
        fields: Vec::new(),
        text: None,
    };
    Catalogue {
        events: [(0, tick)].into_iter().collect(),
        rings: Vec::new(),
        symbols: Symbols::of_numbers(&[(0x2000_0100, 2, "app::level", Reading::Signed(2))]),
        interrupts: Interrupts::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }
}

fn events<'a>(catalogue: &'a Catalogue, events: &[(u32, u32, i64)]) -> Vec<Element<'a>> {
    events
        .iter()
        .map(|&(index, timestamp, cycles)| Element::Event {
            event: Decoded {
                index,
                timestamp,
                event: &catalogue.events[&0],
                payload: Vec::new(),
            },
            cycles,
            seconds: None,
        })
        .collect()
}

#[test]
fn watched_variables_are_named_and_read_from_the_elf() {
    let catalogue = catalogue();
    let anchor = (RING + SLOTS_OFFSET + 4) as u32;
    let dwt = dwt(
        0x4770_1a02,
        &[(0x2000_0100, 0, v8(1)), (anchor, 0, v8(2)), (0x2000_0200, 0, v8(0))],
    );
    let stream = Stream::default()
        .value(1, 0x100, 4)
        .stamp(10)
        .value(0, 0xfffe, 2)
        .stamp(300)
        .value(2, 7, 1)
        .stamp(100)
        .value(1, 0x200, 4)
        .stamp(600);
    // Index 1 does not start at slot 0: no anchor expected.
    let elements = events(&catalogue, &[(0, 0x100, 1_000), (1, 0x150, 1_350), (4, 0x200, 2_000)]);
    let watched = watched(&catalogue, &ring(), &dwt, &stream.0, &elements);
    let level = Curve {
        subject: "app::level".to_owned(),
        field: None,
    };
    let unknown = Curve {
        subject: "0x20000200".to_owned(),
        field: None,
    };
    assert_eq!(watched.refused, None);
    assert_eq!(watched.curves, [level.clone(), unknown.clone()]);
    let at = |element, cycles| Edge {
        element,
        cycles,
        lost: false,
    };
    assert_eq!(
        watched.samples,
        [
            Sampled {
                curve: level,
                value: Number::Integer(-2),
                at: at(0, 1_300),
            },
            Sampled {
                curve: unknown,
                value: Number::Integer(7),
                at: at(1, 1_400),
            },
        ]
    );
    assert_eq!((watched.anchors, watched.span), (2, Some((1_000, 2_000))));
}

#[test]
fn without_the_anchor_nothing_is_dated() {
    let catalogue = catalogue();
    let anchor = (RING + SLOTS_OFFSET + 4) as u32;
    for comparators in [
        &[(0x2000_0100, 0, v8(1))][..],
        &[(anchor, 0, v8(1))],
        &[(anchor + 4, 0, v8(2))],
    ] {
        let watched = watched(&catalogue, &ring(), &dwt(0x4770_1a02, comparators), &[], &[]);
        let refused = watched.refused.expect("refused");
        assert!(refused.contains("0x20001014"), "{refused}");
        assert_eq!(watched.curves, []);
    }
}

#[test]
fn the_summary_tells_what_was_lost() {
    let catalogue = catalogue();
    let anchor = (RING + SLOTS_OFFSET + 4) as u32;
    let dwt = dwt(0x4770_1a02, &[(0x2000_0100, 0, v8(1)), (anchor, 0, v8(2))]);
    let stream = Stream::default()
        .value(0, 1, 2)
        .stamp(10)
        .value(1, 0x100, 4)
        .stamp(10)
        .value(0, 2, 2)
        .stamp(10)
        .value(0, 3, 2)
        .overflow()
        .stamp(290)
        .value(1, 0x200, 4)
        .stamp(700);
    let elements = events(&catalogue, &[(0, 0x100, 1_000), (4, 0x200, 2_000)]);
    let watched = watched(&catalogue, &ring(), &dwt, &stream.0, &elements);
    assert_eq!(
        summary(&watched),
        "0 samples of 1 watched variables, 2 anchors, 1 gaps dropping 2 samples (1 overflows), 1 undated outside the anchors"
    );
    let refused = super::watched(&catalogue, &ring(), &self::dwt(0x4770_1a02, &[]), &[], &[]);
    assert!(summary(&refused).starts_with("warning: the DWT's samples are not dated: "));
}
