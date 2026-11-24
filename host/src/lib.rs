//! Kymo host library: reads an image's event catalogue from its ELF, drains
//! the trace ring of a running target through a debug probe, keeps captures,
//! and decodes them. The `kymo` command line is built on it.

pub mod bench;
pub mod capture;
pub mod check;
pub mod decode;
pub mod elf;
pub mod events;
pub mod model;
pub mod opsy;
pub mod record;
mod ring;
pub mod stats;
mod swo;
pub mod symbols;
pub mod target;
pub mod time;
