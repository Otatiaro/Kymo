//! The source line of a code address, from the DWARF's line programs.
//!
//! Each sequence of a line program covers a range of addresses; a row gives
//! the file and line from its address to the next row's. A function the linker
//! discarded keeps its sequence, moved to address 0: where sequences overlap,
//! the one that starts last holds the address, which is the one the code at
//! that address was linked from.

use crate::target::Result;
use gimli::{EndianSlice, RunTimeEndian};

type Slice<'a> = EndianSlice<'a, RunTimeEndian>;

/// A file and line of source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Line {
    pub file: String,
    pub line: u64,
}

impl std::fmt::Display for Line {
    fn fmt(&self, out: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(out, "{}:{}", self.file, self.line)
    }
}

/// A sequence: its rows, by address, each with its line in `Lines::lines`
/// (none for a row without one), and the address past its end.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sequence {
    start: u64,
    end: u64,
    rows: Vec<(u64, Option<usize>)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    sequences: Vec<Sequence>,
    lines: Vec<Line>,
}

impl Lines {
    /// The line programs of `units`.
    pub(crate) fn read(dwarf: &gimli::Dwarf<Slice>, units: &[gimli::Unit<Slice>]) -> Result<Self> {
        let mut lines = Self::default();
        let mut known = std::collections::HashMap::new();
        for unit in units {
            let Some(program) = unit.line_program.clone() else {
                continue;
            };
            let mut rows = program.rows();
            let mut sequence: Vec<(u64, Option<usize>)> = Vec::new();
            while let Some((header, row)) = rows.next_row()? {
                if row.end_sequence() {
                    if let Some(&(start, _)) = sequence.first() {
                        lines.sequences.push(Sequence {
                            start,
                            end: row.address(),
                            rows: std::mem::take(&mut sequence),
                        });
                    }
                    continue;
                }
                let line = match (row.file(header), row.line()) {
                    (Some(file), Some(number)) => {
                        let name = dwarf
                            .attr_string(unit, file.path_name())?
                            .to_string_lossy()
                            .into_owned();
                        let path = match file.directory(header) {
                            Some(directory) if !is_absolute(&name) => {
                                let directory = dwarf.attr_string(unit, directory)?.to_string_lossy().into_owned();
                                if directory.is_empty() {
                                    name
                                } else {
                                    format!("{directory}/{name}")
                                }
                            }
                            _ => name,
                        };
                        let line = Line {
                            file: path,
                            line: number.get(),
                        };
                        Some(*known.entry(line.clone()).or_insert_with(|| {
                            lines.lines.push(line);
                            lines.lines.len() - 1
                        }))
                    }
                    _ => None,
                };
                sequence.push((row.address(), line));
            }
        }
        for sequence in &mut lines.sequences {
            sequence.rows.sort_by_key(|&(address, _)| address);
        }
        lines.sequences.sort_by_key(|sequence| sequence.start);
        Ok(lines)
    }

    /// The line of the code at `address`, a Thumb address or not.
    pub fn at(&self, address: u64) -> Option<&Line> {
        let address = address & !1;
        let sequence = self.sequences[..self.sequences.partition_point(|sequence| sequence.start <= address)]
            .iter()
            .rev()
            .find(|sequence| address < sequence.end)?;
        let row = sequence.rows.partition_point(|&(at, _)| at <= address).checked_sub(1)?;
        sequence.rows[row].1.map(|line| &self.lines[line])
    }
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Lines {
        let line = |file: &str, line| Line {
            file: file.to_owned(),
            line,
        };
        Lines {
            sequences: vec![
                Sequence {
                    start: 0,
                    end: 0x40,
                    rows: vec![(0, Some(2))],
                },
                Sequence {
                    start: 0x20,
                    end: 0x30,
                    rows: vec![(0x20, Some(0)), (0x24, Some(1)), (0x2c, None)],
                },
            ],
            lines: vec![line("a.cpp", 10), line("a.cpp", 12), line("gone.cpp", 1)],
        }
    }

    #[test]
    fn the_row_at_or_before_the_address() {
        let lines = lines();
        assert_eq!(lines.at(0x21).map(ToString::to_string).as_deref(), Some("a.cpp:10"));
        assert_eq!(lines.at(0x24).map(ToString::to_string).as_deref(), Some("a.cpp:12"));
        assert_eq!(lines.at(0x2b).map(ToString::to_string).as_deref(), Some("a.cpp:12"));
        assert_eq!(lines.at(0x2c), None);
    }

    #[test]
    fn the_sequence_that_starts_last_holds_the_address() {
        let lines = lines();
        assert_eq!(lines.at(0x10).map(ToString::to_string).as_deref(), Some("gone.cpp:1"));
        assert_eq!(lines.at(0x30).map(ToString::to_string).as_deref(), Some("gone.cpp:1"));
        assert_eq!(lines.at(0x40), None);
    }
}
