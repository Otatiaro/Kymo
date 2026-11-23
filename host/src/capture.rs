//! Capture files: what `kymo record --out` keeps of a recording, for
//! `kymo decode` to read back with the image's ELF.
//!
//! Little-endian throughout. A header, then records, each led by a tag byte:
//!
//! ```text
//! header  "KYMOCAP\0", u32 version (1), u32 first index recorded,
//!         u64 cycle counter frequency in hertz given to the recording (0: none)
//! 1 pass  u64 host time (ns since the Unix epoch), u32 write index read
//! 2 event u32 index, u8 slot count, the event's slots as read (16 bytes each)
//! 3 lost  u32 first index lost, u32 index after the last one
//! 4 end   u32 index recording stopped at
//! 5 suspect u32 write index of the pass before, u32 of the pass that found
//!         them, u32 writers lapped meanwhile that no danger accounts for
//! ```
//!
//! A pass record comes before the events and losses that pass settled.
//! Events keep their raw slots: decoding is the reader's, with the catalogue.

use crate::target::Result;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"KYMOCAP\0";
const VERSION: u32 = 1;
const SLOT_BYTES: usize = 16;

pub struct Header {
    pub first: u32,
    pub clock: Option<u64>,
}

pub enum Record<'a> {
    Pass { host_ns: u64, write_index: u32 },
    Event { index: u32, slots: &'a [u8] },
    Lost { from: u32, to: u32 },
    Suspect { from: u32, to: u32, writers: u32 },
    End { next: u32 },
}

pub struct Writer {
    file: BufWriter<File>,
}

impl Writer {
    pub fn create(path: &Path, header: &Header) -> Result<Self> {
        let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut writer = Self {
            file: BufWriter::new(file),
        };
        writer.file.write_all(MAGIC)?;
        writer.file.write_all(&VERSION.to_le_bytes())?;
        writer.file.write_all(&header.first.to_le_bytes())?;
        writer.file.write_all(&header.clock.unwrap_or(0).to_le_bytes())?;
        Ok(writer)
    }

    pub fn write(&mut self, record: &Record) -> Result<()> {
        match *record {
            Record::Pass { host_ns, write_index } => {
                self.file.write_all(&[1])?;
                self.file.write_all(&host_ns.to_le_bytes())?;
                self.file.write_all(&write_index.to_le_bytes())?;
            }
            Record::Event { index, slots } => {
                let count = u8::try_from(slots.len() / SLOT_BYTES).map_err(|_| "event of more than 255 slots")?;
                self.file.write_all(&[2])?;
                self.file.write_all(&index.to_le_bytes())?;
                self.file.write_all(&[count])?;
                self.file.write_all(slots)?;
            }
            Record::Lost { from, to } => {
                self.file.write_all(&[3])?;
                self.file.write_all(&from.to_le_bytes())?;
                self.file.write_all(&to.to_le_bytes())?;
            }
            Record::Suspect { from, to, writers } => {
                self.file.write_all(&[5])?;
                self.file.write_all(&from.to_le_bytes())?;
                self.file.write_all(&to.to_le_bytes())?;
                self.file.write_all(&writers.to_le_bytes())?;
            }
            Record::End { next } => {
                self.file.write_all(&[4])?;
                self.file.write_all(&next.to_le_bytes())?;
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.file.flush()?;
        Ok(())
    }
}

/// Whether `bytes` are a capture rather than a copy of the ring.
pub fn is_capture(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// The header of a capture, and its records in order.
pub fn read(bytes: &[u8]) -> Result<(Header, Vec<Record<'_>>)> {
    let mut cursor = Cursor { bytes };
    if cursor.take(8)? != MAGIC {
        return Err("not a Kymo capture".into());
    }
    let version = cursor.u32()?;
    if version != VERSION {
        return Err(format!("capture version {version}, this kymo reads {VERSION}").into());
    }
    let first = cursor.u32()?;
    let clock = cursor.u64()?;
    let header = Header {
        first,
        clock: (clock != 0).then_some(clock),
    };
    let mut records = Vec::new();
    while !cursor.bytes.is_empty() {
        let record = match cursor.take(1)?[0] {
            1 => Record::Pass {
                host_ns: cursor.u64()?,
                write_index: cursor.u32()?,
            },
            2 => {
                let index = cursor.u32()?;
                let count = usize::from(cursor.take(1)?[0]);
                Record::Event {
                    index,
                    slots: cursor.take(count * SLOT_BYTES)?,
                }
            }
            3 => Record::Lost {
                from: cursor.u32()?,
                to: cursor.u32()?,
            },
            4 => Record::End { next: cursor.u32()? },
            5 => Record::Suspect {
                from: cursor.u32()?,
                to: cursor.u32()?,
                writers: cursor.u32()?,
            },
            tag => return Err(format!("unknown record tag {tag} in the capture").into()),
        };
        records.push(record);
    }
    Ok((header, records))
}

/// The bytes of a capture not read yet.
struct Cursor<'a> {
    bytes: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if self.bytes.len() < count {
            return Err("the capture is cut short".into());
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }
}
