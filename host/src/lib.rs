//! Kymo host library: reads an image's event catalogue from its ELF, drains
//! the trace ring of a running target through a debug probe, keeps captures,
//! and decodes them. The `kymo` command line is built on it.

pub mod capture;
pub mod check;
pub mod code;
pub mod decode;
pub mod durations;
pub mod elf;
pub mod events;
pub mod exceptions;
pub mod fault;
pub mod format;
mod freertos;
pub mod interrupts;
pub mod intervals;
pub mod itm;
pub mod lines;
pub mod load;
pub mod mcp;
pub mod meaning;
pub mod model;
mod opsy;
pub mod power;
pub mod record;
#[cfg(any(test, feature = "served"))]
pub mod served;
pub mod stats;
pub mod swo;
pub mod symbols;
pub mod target;
pub mod text;
pub mod time;
pub mod values;
pub mod words;
mod zephyr;
