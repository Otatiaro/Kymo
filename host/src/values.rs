//! The application's values (ADR-096): each numeric field of an application
//! event is a curve, `type.field`, and each emission of the event a sample of
//! it. An application event is one outside the `kymo::` namespace: the
//! library's own events, the adapters' and the edges of durations are not.
//!
//! A numeric field is an integer of 1, 2, 4 or 8 bytes, signed or not, or a
//! floating-point number of 4 or 8 bytes, read as its DWARF type says. A
//! `bool`, a `char`, an enumeration, an array or a nested structure is not.
//!
//! A sample holds until the next one: the curve is in steps. A loss cuts the
//! step: its value is unknown until the next sample. stats.rs counts the
//! samples, not the time they hold; a NaN is a gap, counted apart.

use crate::events::{Catalogue, Event, Kind, Type};
use crate::model::{Edge, Element, Trace};
use std::cmp::Ordering;
use std::fmt;

/// The kind and the metric of a curve's group in `kymo stats` and `kymo
/// check`; the subject is its event, the detail its field.
pub const KIND: &str = "value";

/// A sample's value, as its field holds it: an integer of up to 64 bits,
/// signed or not, or a floating-point number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Integer(i128),
    Float(f64),
}

impl Number {
    pub fn as_f64(self) -> f64 {
        match self {
            Number::Integer(value) => value as f64,
            Number::Float(value) => value,
        }
    }

    pub fn is_nan(self) -> bool {
        matches!(self, Number::Float(value) if value.is_nan())
    }

    /// The order of values: integers exactly, floating-point numbers by
    /// value. A curve holds only one of the two.
    pub fn total_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Number::Integer(a), Number::Integer(b)) => a.cmp(b),
            (a, b) => a.as_f64().total_cmp(&b.as_f64()),
        }
    }
}

impl fmt::Display for Number {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Number::Integer(value) => write!(formatter, "{value}"),
            Number::Float(value) => write!(formatter, "{value}"),
        }
    }
}

/// How a numeric field's bytes read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    Signed(usize),
    Unsigned(usize),
    Float(usize),
}

impl Reading {
    /// The reading of a field of type `ty`, none when it is not numeric.
    pub fn of(ty: &Type) -> Option<Self> {
        let size = usize::try_from(ty.size?).ok()?;
        let integer = matches!(size, 1 | 2 | 4 | 8) && !crate::text::is_char(ty);
        match ty.kind {
            Kind::Base(gimli::DW_ATE_float) if size == 4 || size == 8 => Some(Reading::Float(size)),
            Kind::Base(gimli::DW_ATE_signed | gimli::DW_ATE_signed_char) if integer => Some(Reading::Signed(size)),
            Kind::Base(gimli::DW_ATE_unsigned | gimli::DW_ATE_unsigned_char) if integer => {
                Some(Reading::Unsigned(size))
            }
            _ => None,
        }
    }

    fn size(self) -> usize {
        match self {
            Reading::Signed(size) | Reading::Unsigned(size) | Reading::Float(size) => size,
        }
    }

    /// The value at the start of `bytes`, little-endian; none when they are
    /// too short.
    pub fn read(self, bytes: &[u8]) -> Option<Number> {
        let bytes = bytes.get(..self.size())?;
        let unsigned = bytes
            .iter()
            .rev()
            .fold(0u64, |value, byte| value << 8 | u64::from(*byte));
        let shift = 64 - 8 * self.size() as u32;
        Some(match self {
            Reading::Signed(_) => Number::Integer(i128::from(((unsigned << shift) as i64) >> shift)),
            Reading::Unsigned(_) => Number::Integer(i128::from(unsigned)),
            Reading::Float(4) => Number::Float(f64::from(f32::from_bits(unsigned as u32))),
            Reading::Float(_) => Number::Float(f64::from_bits(unsigned)),
        })
    }
}

/// A curve: a numeric field of an application event.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Curve {
    /// The event's qualified name.
    pub subject: String,
    pub field: String,
}

impl fmt::Display for Curve {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "{}.{}", self.subject, self.field)
    }
}

/// Whether `event` is the application's: outside the `kymo::` namespace.
pub fn is_application(event: &Event) -> bool {
    !event.name.starts_with("kymo::")
}

/// The curves of `event`, with where each field is and how it reads.
fn fields(event: &Event) -> impl Iterator<Item = (Curve, usize, Reading)> + '_ {
    event
        .fields
        .iter()
        .filter(|_| is_application(event))
        .filter_map(move |field| {
            let reading = Reading::of(&field.ty)?;
            let offset = usize::try_from(field.offset).ok()?;
            let curve = Curve {
                subject: event.name.clone(),
                field: field.name.clone(),
            };
            Some((curve, offset, reading))
        })
}

/// The curves the image declares, in order.
pub fn curves(catalogue: &Catalogue) -> Vec<Curve> {
    let mut curves: Vec<Curve> = catalogue
        .events
        .values()
        .flat_map(|event| fields(event).map(|(curve, ..)| curve))
        .collect();
    curves.sort();
    curves
}

/// A sample of a curve: its value and the event that carried it.
#[derive(Clone, Debug, PartialEq)]
pub struct Sampled {
    pub curve: Curve,
    pub value: Number,
    pub at: Edge,
}

/// The curves an image declares, and their samples in a trace.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Values {
    pub curves: Vec<Curve>,
    /// In the order of the trace, an event's fields in their order.
    pub samples: Vec<Sampled>,
}

/// The samples of every curve in `trace`.
pub fn sample(catalogue: &Catalogue, trace: &Trace) -> Values {
    let mut samples = Vec::new();
    for (element, item) in trace.elements.iter().enumerate() {
        let Element::Event { event, cycles, .. } = item else {
            continue;
        };
        let at = Edge {
            element,
            cycles: *cycles,
            lost: false,
        };
        for (curve, offset, reading) in fields(event.event) {
            if let Some(value) = event.payload.get(offset..).and_then(|bytes| reading.read(bytes)) {
                samples.push(Sampled { curve, value, at });
            }
        }
    }
    Values {
        curves: curves(catalogue),
        samples,
    }
}

#[cfg(test)]
mod tests;
