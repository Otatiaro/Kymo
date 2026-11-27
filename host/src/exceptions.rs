//! The exceptions the DWT traces, nested into the spans of their handlers.
//!
//! Each exception packet the SWO dates (swo.rs) enters an exception, exits
//! it, or returns to the exception it interrupted, 0 for thread mode. An
//! exit followed by an entry, without a return between them, is a tail
//! chain. Packets are dated within the stream's coverage alone, from its
//! first anchor to its last less its gaps: at the start of each stretch of
//! it, which exceptions are open is unknown, as after a loss of the ring,
//! until a return to thread mode shows none is. A handler open at the end of
//! a stretch ends somewhere in what follows it.

use crate::itm::Function;
use crate::swo::{Exception, Watched};

/// Where an exception's span starts or ends: the cycles of its packet, or a
/// bound of the coverage, `lost`, where the packet is not in the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub cycles: i64,
    pub lost: bool,
}

/// An exception's handler running, `depth` exceptions below it: `None`
/// where the start of a stretch hides which ones were open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub exception: u16,
    pub depth: Option<usize>,
    pub from: At,
    pub to: At,
}

/// The exceptions' spans, in the order they end, and the cycles of the
/// packets that contradict the nesting.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Nested {
    pub lines: Vec<Line>,
    pub contradictions: Vec<i64>,
}

/// The stretches of `watched`'s stream whose packets are dated, in cycles:
/// from its first anchor to its last, less its gaps.
pub fn coverage(watched: &Watched) -> Vec<(i64, i64)> {
    let Some((first, last)) = watched.span else {
        return Vec::new();
    };
    let mut stretches = Vec::new();
    let mut start = first;
    for &(from, to) in &watched.gaps {
        if from > start {
            stretches.push((start, from));
        }
        start = start.max(to);
    }
    if last > start {
        stretches.push((start, last));
    }
    stretches
}

/// The handlers' spans `exceptions`, in the order of the stream, give
/// within `coverage`.
pub fn nest(exceptions: &[Exception], coverage: &[(i64, i64)]) -> Nested {
    let mut nesting = Nesting::default();
    let mut stretches = coverage.iter().copied();
    let mut stretch = stretches.next();
    if let Some((start, _)) = stretch {
        nesting.start(start);
    }
    for packet in exceptions {
        while let Some((_, end)) = stretch
            && packet.cycles > end
        {
            nesting.end(end);
            stretch = stretches.next();
            if let Some((start, _)) = stretch {
                nesting.start(start);
            }
        }
        match stretch {
            Some((start, _)) if packet.cycles >= start => nesting.packet(packet),
            Some(_) => {}
            None => break,
        }
    }
    if let Some((_, end)) = stretch {
        nesting.end(end);
    }
    nesting.nested
}

/// The exceptions open, innermost last, each with its entry and depth.
#[derive(Default)]
struct Nesting {
    open: Vec<(u16, At, Option<usize>)>,
    /// Whether every exception open is in `open`: false from the start of a
    /// stretch to the next return to thread mode.
    known: bool,
    /// The start of the stretch.
    start: Option<At>,
    nested: Nested,
}

impl Nesting {
    fn start(&mut self, cycles: i64) {
        self.open.clear();
        self.known = false;
        self.start = Some(At { cycles, lost: true });
    }

    /// Ends the stretch at `cycles`: the exceptions open end in what follows.
    fn end(&mut self, cycles: i64) {
        let to = At { cycles, lost: true };
        while let Some((exception, from, depth)) = self.open.pop() {
            self.nested.lines.push(Line {
                exception,
                depth,
                from,
                to,
            });
        }
    }

    fn packet(&mut self, packet: &Exception) {
        let at = At {
            cycles: packet.cycles,
            lost: false,
        };
        let number = packet.number;
        match packet.function {
            Function::Entered => {
                let depth = self.known.then_some(self.open.len());
                self.open.push((number, at, depth));
            }
            Function::Exited => {
                let (from, depth) = match (self.open.last(), self.start) {
                    (Some(&(open, from, depth)), _) if open == number => {
                        self.open.pop();
                        (from, depth)
                    }
                    (None, Some(start)) if !self.known => (start, None),
                    _ => return self.nested.contradictions.push(at.cycles),
                };
                self.nested.lines.push(Line {
                    exception: number,
                    depth,
                    from,
                    to: at,
                });
            }
            Function::Returned if number == 0 => {
                if !self.open.is_empty() {
                    self.open.clear();
                    self.nested.contradictions.push(at.cycles);
                }
                self.known = true;
            }
            Function::Returned => {
                let resumes = match self.open.last() {
                    Some(&(open, ..)) => open == number,
                    None => !self.known,
                };
                if !resumes {
                    self.nested.contradictions.push(at.cycles);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
