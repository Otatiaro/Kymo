//! The model: a capture read back with the catalogue of the image that made
//! it, as elements in the order the recording settled them. Events are
//! decoded and dated: their timestamps extended to 64-bit cycles, and the
//! seconds since the first event at the frequency given, the capture's, or
//! else the one measured over the whole capture (time.rs).

use crate::capture::{self, Record};
use crate::decode::{self, Decoded};
use crate::events::Catalogue;
use crate::swo::{self, Watched};
use crate::target::Result;
use crate::time::Timeline;

/// Where something starts or ends: an element of the trace, and its cycles.
/// At a loss, the edge is somewhere in what it hides; its cycles are those
/// of the event before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub element: usize,
    pub cycles: i64,
    pub lost: bool,
}

impl Edge {
    /// Where the edge falls in the trace, to compare edges by: its element,
    /// then its cycles. An edge the SWO dates between two events falls at the
    /// last event at or before it, and after that event.
    pub fn at(&self) -> (usize, i64) {
        (self.element, self.cycles)
    }
}

/// A capture, read back.
pub struct Trace<'a> {
    /// The first index recorded.
    pub first: u32,
    pub elements: Vec<Element<'a>>,
    /// The index recording stopped at; none if the capture was cut short.
    pub end: Option<u32>,
    /// Where the capture was cut short.
    pub cut: Option<capture::Cut>,
    /// The frequency the seconds are at.
    pub frequency: Option<f64>,
    /// The frequency measured over the capture, and the seconds it spans.
    pub measured: Option<(f64, f64)>,
    /// The variables the DWT watched and their samples; none without the
    /// DWT's configuration in the capture.
    pub watched: Option<Watched>,
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
    /// The capture `bytes`, read with the ring its header names, its seconds
    /// at `clock` hertz, or the capture's, or else at the frequency measured
    /// over the whole capture.
    pub fn read(catalogue: &'a Catalogue, bytes: &[u8], clock: Option<u64>) -> Result<Self> {
        let capture::Capture { header, records, cut } = capture::read(bytes)?;
        let ring = catalogue.ring(Some(&header.channel))?;
        let mut timeline = Timeline::new(clock.or(header.clock));
        let mut elements = Vec::new();
        let mut end = None;
        let mut dwt = None;
        let mut swo = Vec::new();
        for record in &records {
            match *record {
                Record::Dwt(ref config) => dwt = Some(config),
                Record::Swo(bytes) => swo.extend_from_slice(bytes),
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
        let watched = dwt.map(|dwt| swo::watched(catalogue, ring, dwt, &swo, &elements));
        Ok(Self {
            first: header.first,
            elements,
            end,
            cut,
            frequency,
            measured: timeline.measured(),
            watched,
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

    /// The slots the events take, and the indexes lost.
    pub fn slots(&self) -> (u64, u64) {
        self.elements
            .iter()
            .fold((0, 0), |(taken, lost), element| match element {
                Element::Event { event, .. } => (taken + decode::slots_for(event.event.size), lost),
                Element::Lost { from, to } => (taken, lost + u64::from(to.wrapping_sub(*from))),
                Element::Suspect { .. } => (taken, lost),
            })
    }
}

#[cfg(test)]
mod tests;
