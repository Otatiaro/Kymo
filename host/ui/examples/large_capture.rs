//! Builds a large capture from a small one, to measure kymo-ui on it: the
//! events of the small capture repeated with their indexes and laps shifted,
//! a loss and a suspect line in each copy, and synthetic times.
//!
//! usage: large_capture <capture> <out> <events>
//!
//! The large capture holds or names the ELF the small one does.
//!
//! For instance, from a capture of the OpSy QEMU suite made by
//! `kymo record --out`:
//!
//! ```text
//! cargo run --release -p kymo-ui --example large_capture -- suite.kcap large.kcap 1000000
//! kymo-ui large.kcap --measure
//! ```

use kymo::capture::{self, Header, Record, Writer};
use kymo::decode;
use kymo::events;
use kymo::target::Result;
use std::path::PathBuf;

/// The bits of a slot's header that carry its lap.
const LAP_SHIFT: u32 = 17;
/// Where the times are in an event's first slot.
const CYCLES: std::ops::Range<usize> = 4..8;
const SLOT_BYTES: usize = 16;
/// The events of each copy left out as lost, and the one a suspect line starts at.
const LOST: std::ops::Range<usize> = 100..110;
const SUSPECT: usize = 200;

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, wanted] = arguments.as_slice() else {
        return Err("usage: large_capture <capture> <out> <events>".into());
    };
    let bytes = std::fs::read(input)?;
    let (image, image_name) = capture::elf(input, &bytes, None)?;
    let catalogue = events::parse(&image, &image_name)?;
    let wanted: usize = wanted.parse()?;
    let capture::Capture { header, records, .. } = capture::read(&bytes)?;
    let ring = catalogue.ring(header.channel.as_deref())?;
    let events: Vec<(u32, &[u8])> = records
        .iter()
        .filter_map(|record| match *record {
            Record::Event { index, slots } => Some((index, slots)),
            _ => None,
        })
        .collect();
    let end = records
        .iter()
        .find_map(|record| match *record {
            Record::End { next } => Some(next),
            _ => None,
        })
        .ok_or("the capture has no end")?;
    if events.len() <= SUSPECT {
        return Err(format!(
            "the capture holds {} events, {} at least needed",
            events.len(),
            SUSPECT + 1
        )
        .into());
    }
    // Each copy starts on a whole lap, so its events keep their positions.
    let period = (end - header.first).div_ceil(ring.slots) * ring.slots;
    let copies = wanted.div_ceil(events.len()) as u32;
    let mut out = Writer::create(
        &PathBuf::from(output),
        &Header {
            first: header.first,
            clock: Some(25_000_000),
            channel: header.channel.clone(),
            elf: header.elf,
        },
    )?;
    let mut written = 0usize;
    let mut cycles = 0u32;
    for copy in 0..copies {
        let shift = copy * period;
        let mut n = 0;
        while n < events.len() {
            let index = events[n].0 + shift;
            if n == LOST.start {
                let to = events[LOST.end].0 + shift;
                out.write(&Record::Lost { from: index, to })?;
                n = LOST.end;
                continue;
            }
            if n == SUSPECT {
                out.write(&Record::Suspect {
                    from: index,
                    to: index + 40,
                    writers: 1,
                })?;
            }
            let mut slots = events[n].1.to_vec();
            // An event's slots follow each other: one past the ring's end is
            // on the next lap.
            for (k, slot) in (0u32..).zip(slots.chunks_mut(SLOT_BYTES)) {
                let word = u32::from_le_bytes(slot[..4].try_into().expect("a header word"));
                let word = word & ((1 << LAP_SHIFT) - 1) | decode::lap(index + k, ring) << LAP_SHIFT;
                slot[..4].copy_from_slice(&word.to_le_bytes());
            }
            // Bursts of 500 events 100 to 200 cycles apart, then 500 events
            // 5,000 to 10,000 apart: QEMU's cycle counter stays at zero.
            let spread = (written as u32).wrapping_mul(2_654_435_761) >> 8;
            let gap = if (written / 500).is_multiple_of(2) {
                100 + spread % 100
            } else {
                5000 + spread % 5000
            };
            cycles = cycles.wrapping_add(gap);
            slots[CYCLES].copy_from_slice(&cycles.to_le_bytes());
            out.write(&Record::Event { index, slots: &slots })?;
            written += 1;
            n += 1;
        }
    }
    out.write(&Record::End {
        next: end + (copies - 1) * period,
    })?;
    out.finish()?;
    println!("{written} events, {copies} copies");
    Ok(())
}
