//! Capture files: what `kymo record --out` keeps of a recording, with the
//! image's ELF or where it is, for `kymo decode` to read back.
//!
//! Little-endian throughout. A header, then records, each led by a tag byte:
//!
//! ```text
//! header  "KYMOCAP\0", u32 version (1), u32 first index recorded,
//!         u64 cycle counter frequency in hertz given to the recording (0: none),
//!         u16 length and UTF-8 bytes of the channel: the qualified name of
//!         the ring recorded, then the image's ELF:
//!         u8 1, u32 length and bytes of the ELF itself, or
//!         u8 2, u16 length and UTF-8 bytes of where it is: an absolute
//!         path or an http(s) URL
//! 1 pass  u64 host time (ns since the Unix epoch), u32 write index read
//! 2 event u32 index, u8 slot count, the event's slots as read (16 bytes each)
//! 3 lost  u32 first index lost, u32 index after the last one
//! 4 end   u32 index recording stopped at
//! 5 suspect u32 write index of the pass before, u32 of the pass that found
//!         them, u32 writers lapped meanwhile that no danger accounts for
//! 6 dwt   u32 DWT_DEVARCH, u32 DWT_CTRL, u32 SWO baud rate, u8 comparator
//!         count, then for each u32 DWT_COMPn, u32 DWT_MASKn, u32 DWT_FUNCTIONn
//! 7 swo   u32 length and the bytes read from SWO
//! ```
//!
//! A pass record comes before the events and losses that pass settled.
//! Events keep their raw slots: decoding is the reader's, with the catalogue.
//! A capture of variables watched by the DWT (swo.rs) holds one dwt record,
//! read at the start, and swo records whose bytes, put end to end, are the
//! stream as SWO carried it.
//!
//! A recording stopped short of its end record (killed, crashed, probe
//! unplugged) leaves a capture without one, perhaps ending inside a record:
//! it reads back up to its last whole record, and says where it was cut.

use crate::target::Result;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Duration;

const MAGIC: &[u8; 8] = b"KYMOCAP\0";
const VERSION: u32 = 1;
const SLOT_BYTES: usize = 16;

pub struct Header {
    pub first: u32,
    pub clock: Option<u64>,
    pub channel: String,
    pub elf: Elf,
}

/// The image's ELF, as a capture holds it.
#[derive(Debug, PartialEq, Eq)]
pub enum Elf {
    /// The bytes of the file.
    Embedded(Vec<u8>),
    /// An absolute path or an http(s) URL.
    Reference(String),
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
    Dwt(Dwt),
    Swo(&'a [u8]),
}

/// The DWT's configuration, as the debug port reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dwt {
    pub devarch: u32,
    pub ctrl: u32,
    /// The SWO baud rate the program set.
    pub baud: u32,
    pub comparators: Vec<Comparator>,
}

/// A DWT comparator's registers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Comparator {
    pub comp: u32,
    pub mask: u32,
    pub function: u32,
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
        match &header.elf {
            Elf::Embedded(bytes) => {
                let length = u32::try_from(bytes.len()).map_err(|_| "ELF of more than 4 GiB")?;
                writer.file.write_all(&[1])?;
                writer.file.write_all(&length.to_le_bytes())?;
                writer.file.write_all(bytes)?;
            }
            Elf::Reference(place) => {
                let length = u16::try_from(place.len()).map_err(|_| "ELF reference of more than 65535 bytes")?;
                writer.file.write_all(&[2])?;
                writer.file.write_all(&length.to_le_bytes())?;
                writer.file.write_all(place.as_bytes())?;
            }
        }
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
            Record::Dwt(ref dwt) => {
                let count = u8::try_from(dwt.comparators.len()).map_err(|_| "more than 255 comparators")?;
                self.file.write_all(&[6])?;
                for word in [dwt.devarch, dwt.ctrl, dwt.baud] {
                    self.file.write_all(&word.to_le_bytes())?;
                }
                self.file.write_all(&[count])?;
                for comparator in &dwt.comparators {
                    for word in [comparator.comp, comparator.mask, comparator.function] {
                        self.file.write_all(&word.to_le_bytes())?;
                    }
                }
            }
            Record::Swo(bytes) => {
                let length = u32::try_from(bytes.len()).map_err(|_| "SWO bytes of more than 4 GiB")?;
                self.file.write_all(&[7])?;
                self.file.write_all(&length.to_le_bytes())?;
                self.file.write_all(bytes)?;
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

/// The image's ELF to read `bytes`, a capture or a copy of the ring called
/// `name`, with, and what errors call it: `given`, a file or an URL, when
/// there is one, else the ELF the capture holds or names.
pub fn elf(name: &str, bytes: &[u8], given: Option<&str>) -> Result<(Vec<u8>, String)> {
    if let Some(given) = given {
        return Ok((fetch(given)?, given.to_owned()));
    }
    if !is_capture(bytes) {
        return Err(format!("{name} is a copy of a ring: give the image's ELF with --elf").into());
    }
    match header(bytes)?.elf {
        Elf::Embedded(elf) => Ok((elf, format!("the ELF in {name}"))),
        Elf::Reference(place) => match fetch(&place) {
            Ok(elf) => Ok((elf, place)),
            Err(error) => Err(format!("{name}: its ELF: {error}; give it with --elf").into()),
        },
    }
}

/// The bytes at `place`: an http(s) URL, fetched whole, or a file.
pub fn fetch(place: &str) -> Result<Vec<u8>> {
    if !is_url(place) {
        return read_file(place);
    }
    let failed = |error: ureq::Error| format!("{place}: {error}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut response = agent.get(place).call().map_err(failed)?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(u64::MAX)
        .read_to_vec()
        .map_err(failed)?;
    Ok(bytes)
}

/// How a capture names the ELF given as `place`: an http(s) URL as it is, a
/// file by its absolute path.
pub fn reference(place: &str) -> Result<String> {
    if is_url(place) {
        return Ok(place.to_owned());
    }
    let path = std::path::absolute(place).map_err(|error| format!("{place}: {error}"))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Whether `place` is an http(s) URL rather than a file.
pub fn is_url(place: &str) -> bool {
    place.starts_with("http://") || place.starts_with("https://")
}

fn read_file(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| format!("{path}: {error}").into())
}

/// The header of the capture `bytes`.
pub fn header(bytes: &[u8]) -> Result<Header> {
    read_header(&mut Cursor { bytes })
}

fn read_header(cursor: &mut Cursor) -> Result<Header> {
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
    let elf = match cursor.take(1).ok_or(short)?[0] {
        1 => {
            let length = cursor.u32().ok_or(short)? as usize;
            Elf::Embedded(cursor.take(length).ok_or(short)?.to_vec())
        }
        2 => {
            let length = usize::from(cursor.u16().ok_or(short)?);
            let place = std::str::from_utf8(cursor.take(length).ok_or(short)?)
                .map_err(|_| "the capture's ELF reference is not UTF-8")?;
            Elf::Reference(place.to_owned())
        }
        form => return Err(format!("unknown form {form} of the capture's ELF").into()),
    };
    Ok(Header {
        first,
        clock: (clock != 0).then_some(clock),
        channel: channel.to_owned(),
        elf,
    })
}

/// The header of a capture and its whole records in order, up to its end
/// record or to where it was cut.
pub fn read(bytes: &[u8]) -> Result<Capture<'_>> {
    let mut cursor = Cursor { bytes };
    let header = read_header(&mut cursor)?;
    let mut records = Vec::new();
    let mut next = header.first;
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
        if !(1..=7).contains(&tag) {
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
            Record::Pass { .. } | Record::Suspect { .. } | Record::Dwt(_) | Record::Swo(_) => {}
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
            5 => Record::Suspect {
                from: self.u32()?,
                to: self.u32()?,
                writers: self.u32()?,
            },
            6 => {
                let (devarch, ctrl, baud) = (self.u32()?, self.u32()?, self.u32()?);
                let count = self.take(1)?[0];
                let comparators = (0..count)
                    .map(|_| {
                        Some(Comparator {
                            comp: self.u32()?,
                            mask: self.u32()?,
                            function: self.u32()?,
                        })
                    })
                    .collect::<Option<_>>()?;
                Record::Dwt(Dwt {
                    devarch,
                    ctrl,
                    baud,
                    comparators,
                })
            }
            _ => {
                let length = self.u32()? as usize;
                Record::Swo(self.take(length)?)
            }
        })
    }
}

#[cfg(test)]
mod tests;
