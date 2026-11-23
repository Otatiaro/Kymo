//! The model: a capture read back with the catalogue of the image that made
//! it, as elements in the order the recording settled them. Events are
//! decoded and dated: their timestamps extended to 64-bit cycles, and the
//! seconds since the first event at the frequency given, the capture's, or
//! else the one measured over the whole capture (time.rs).

use crate::capture::{self, Record};
use crate::decode::{self, Decoded};
use crate::events::{Catalogue, Ring};
use crate::target::Result;
use crate::time::Timeline;

/// A capture, read back.
pub struct Trace<'a> {
    /// The first index recorded.
    pub first: u32,
    pub elements: Vec<Element<'a>>,
    /// The index recording stopped at; none if the capture was cut short.
    pub end: Option<u32>,
    /// The frequency the seconds are at.
    pub frequency: Option<f64>,
    /// The frequency measured over the capture, and the seconds it spans.
    pub measured: Option<(f64, f64)>,
}

pub enum Element<'a> {
    Event {
        event: Decoded<'a>,
        cycles: i64,
        /// Since the first event; none without a frequency.
        seconds: Option<f64>,
    },
    /// The indexes `from..to`, overwritten before they could be read intact.
    Lost { from: u32, to: u32 },
    /// `writers` writers lapped while the write index went from `from` to
    /// `to`, that no danger accounts for: events recorded near it may hold
    /// their bytes.
    Suspect { from: u32, to: u32, writers: u32 },
}

impl<'a> Trace<'a> {
    /// The capture `bytes` of `ring`, its seconds at `clock` hertz, or the
    /// capture's, or else at the frequency measured over the whole capture.
    pub fn read(catalogue: &'a Catalogue, ring: &Ring, bytes: &[u8], clock: Option<u64>) -> Result<Self> {
        let (header, records) = capture::read(bytes)?;
        let mut timeline = Timeline::new(clock.or(header.clock));
        let mut elements = Vec::new();
        let mut end = None;
        for record in &records {
            match *record {
                Record::Pass { host_ns, write_index } => timeline.pass(host_ns, write_index),
                Record::Event { index, slots } => {
                    let event = decode::event(catalogue, ring, index, slots)?
                        .ok_or_else(|| format!("event {index}: its slots in the capture do not hold it whole"))?;
                    let cycles = timeline.event(index, event.timestamp);
                    elements.push(Element::Event {
                        event,
                        cycles,
                        seconds: None,
                    });
                }
                Record::Lost { from, to } => elements.push(Element::Lost { from, to }),
                Record::Suspect { from, to, writers } => elements.push(Element::Suspect { from, to, writers }),
                Record::End { next } => end = Some(next),
            }
        }
        // The frequency is known only once every event is read.
        let frequency = timeline.frequency();
        for element in &mut elements {
            if let Element::Event { cycles, seconds, .. } = element {
                *seconds = timeline.seconds(*cycles, frequency);
            }
        }
        Ok(Self {
            first: header.first,
            elements,
            end,
            frequency,
            measured: timeline.measured(),
        })
    }

    /// The writers reported suspect.
    pub fn suspects(&self) -> u64 {
        self.elements
            .iter()
            .map(|element| match *element {
                Element::Suspect { writers, .. } => u64::from(writers),
                _ => 0,
            })
            .sum()
    }

    /// The events.
    pub fn events(&self) -> usize {
        self.elements
            .iter()
            .filter(|element| matches!(element, Element::Event { .. }))
            .count()
    }
}
