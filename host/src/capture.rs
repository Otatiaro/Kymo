//! Capture files: what `kymo record --out` keeps of a recording, for
//! `kymo decode` to read back with the image's ELF.
//!
//! Little-endian throughout. A header, then records, each led by a tag byte:
//!
//! ```text
//! header  "KYMOCAP\0", u32 version (1), u32 first index recorded,
//!         u64 cycle counter frequency in hertz given to the recording (0: none),
//!         u16 length and UTF-8 bytes of the channel: the qualified name of
//!         the ring recorded
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
//!
//! A recording stopped short of its end record (killed, crashed, probe
//! unplugged) leaves a capture without one, perhaps ending inside a record:
//! it reads back up to its last whole record, and says where it was cut.

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
    pub channel: String,
}

/// Where a capture without an end record stops: the byte its whole records
/// end at, and the index after the last event or loss they hold.
#[derive(Debug, PartialEq, Eq)]
pub struct Cut {
    pub at: usize,
    pub next: u32,
}

/// A capture, read back.
pub struct Capture<'a> {
    pub header: Header,
    pub records: Vec<Record<'a>>,
    /// None when the capture holds its end record.
    pub cut: Option<Cut>,
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
        let length = u16::try_from(header.channel.len()).map_err(|_| "channel name of more than 65535 bytes")?;
        writer.file.write_all(&length.to_le_bytes())?;
        writer.file.write_all(header.channel.as_bytes())?;
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

    /// Puts what is written on disk: a recording stopped short loses no
    /// more than what came after.
    pub fn flush(&mut self) -> Result<()> {
        self.file.flush()?;
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

/// The header of a capture and its whole records in order, up to its end
/// record or to where it was cut.
pub fn read(bytes: &[u8]) -> Result<Capture<'_>> {
    let mut cursor = Cursor { bytes };
    let short = "the capture is cut short in its header";
    if cursor.take(8).ok_or(short)? != MAGIC {
        return Err("not a Kymo capture".into());
    }
    let version = cursor.u32().ok_or(short)?;
    if version != VERSION {
        return Err(format!("capture version {version}, this kymo reads {VERSION}").into());
    }
    let first = cursor.u32().ok_or(short)?;
    let clock = cursor.u64().ok_or(short)?;
    let length = usize::from(cursor.u16().ok_or(short)?);
    let channel = std::str::from_utf8(cursor.take(length).ok_or(short)?)
        .map_err(|_| "the capture's channel name is not UTF-8")?;
    let header = Header {
        first,
        clock: (clock != 0).then_some(clock),
        channel: channel.to_owned(),
    };
    let mut records = Vec::new();
    let mut next = first;
    loop {
        let at = bytes.len() - cursor.bytes.len();
        let Some(tag) = cursor.take(1) else {
            return Ok(Capture {
                header,
                records,
                cut: Some(Cut { at, next }),
            });
        };
        let tag = tag[0];
        if !(1..=5).contains(&tag) {
            return Err(format!("unknown record tag {tag} in the capture").into());
        }
        let Some(record) = cursor.record(tag) else {
            return Ok(Capture {
                header,
                records,
                cut: Some(Cut { at, next }),
            });
        };
        match record {
            Record::Event { index, slots } => {
                next = index.wrapping_add((slots.len() / SLOT_BYTES) as u32);
            }
            Record::Lost { to, .. } => next = to,
            Record::End { .. } => {
                records.push(record);
                return Ok(Capture {
                    header,
                    records,
                    cut: None,
                });
            }
            Record::Pass { .. } | Record::Suspect { .. } => {}
        }
        records.push(record);
    }
}

/// The bytes of a capture not read yet.
struct Cursor<'a> {
    bytes: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.bytes.len() < count {
            return None;
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Some(taken)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().expect("two bytes")))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    /// The record of a known `tag`, none if the capture ends inside it.
    fn record(&mut self, tag: u8) -> Option<Record<'a>> {
        Some(match tag {
            1 => Record::Pass {
                host_ns: self.u64()?,
                write_index: self.u32()?,
            },
            2 => {
                let index = self.u32()?;
                let count = usize::from(self.take(1)?[0]);
                Record::Event {
                    index,
                    slots: self.take(count * SLOT_BYTES)?,
                }
            }
            3 => Record::Lost {
                from: self.u32()?,
                to: self.u32()?,
            },
            4 => Record::End { next: self.u32()? },
            _ => Record::Suspect {
                from: self.u32()?,
                to: self.u32()?,
                writers: self.u32()?,
            },
        })
    }
}

#[cfg(test)]
mod tests;
