//! The CPU's line in bars (ADR-099): at each cycle, what holds the core. A
//! handler, the innermost one when they nest; else a task other than idle;
//! else, where no switch has given the task on the core since a loss or the
//! start of the capture, the time unknown. Idle holds no piece: it is the
//! line's empty ground.
//!
//! Its pieces do not overlap. Each layer of the line, the time unknown, each
//! task, then the handlers, holds its pieces as a row's spans, and is painted
//! in that order: where pieces of several layers share a pixel column, the
//! last painted shows, and the pointer reads that one.

use crate::spans::Spans;
use crate::timeline::View;
use kymo::meaning::{Edge, Interrupt, Run, Span};

/// What a piece of the band shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Task(usize),
    /// The innermost handler open, and what it interrupts.
    Handler {
        handler: usize,
        under: Under,
    },
    Unknown,
}

/// What a handler interrupts: the handler it nests in, or the task on the
/// core (the idle task for none), or what a loss hides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Under {
    Handler(usize),
    Task(Option<usize>),
    Unknown,
}

/// A piece of the band, from `from` to `to` cycles: what holds the core, and
/// the whole span of it, a run, an interrupt or the time unknown, still open
/// at the end of the capture or not. The time unknown starts at the loss
/// before it, or at the capture's first event, not lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub from: i64,
    pub to: i64,
    pub held: Held,
    pub span: Span,
    pub still: bool,
}

/// A layer of the band, in the order they are painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Unknown,
    Task(usize),
    Handlers,
}

#[derive(Default)]
pub struct Band {
    pub pieces: Vec<Piece>,
    unknown: Spans,
    /// By task.
    tasks: Vec<Spans>,
    handlers: Spans,
}

impl Band {
    /// The band of `runs` and `interrupts`, each with whether it is still
    /// open, its span then ended at the last event; the capture's first
    /// event and its last at `bounds`.
    pub fn of(runs: &[(Run, bool)], interrupts: &[(Interrupt, bool)], bounds: Option<(Edge, Edge)>) -> Band {
        let mut runs = runs.to_vec();
        runs.sort_by_key(|(run, _)| run.span.from.cycles);
        let mut pieces = innermost(&runs, interrupts);
        let covered = merged(&pieces);
        let mut keep = |from: i64, to: i64, held: Held, span: Span, still: bool| {
            for (from, to) in outside(from, to, &covered) {
                pieces.push(Piece {
                    from,
                    to,
                    held,
                    span,
                    still,
                });
            }
        };
        let mut before = bounds.map(|(start, _)| start);
        for &(run, still) in &runs {
            if let Some(edge) = before
                && edge.cycles < run.span.from.cycles
            {
                let span = Span {
                    from: edge,
                    to: run.span.from,
                };
                keep(edge.cycles, run.span.from.cycles, Held::Unknown, span, false);
            }
            if let Some(task) = run.task {
                keep(
                    run.span.from.cycles,
                    run.span.to.cycles,
                    Held::Task(task),
                    run.span,
                    still,
                );
            }
            // Unknown again past a run a loss ends.
            before = run.span.to.lost.then_some(run.span.to);
        }
        // Unknown to the last event, still, without a switch since.
        if let (Some(from), Some((_, end))) = (before, bounds)
            && from.cycles < end.cycles
        {
            keep(from.cycles, end.cycles, Held::Unknown, Span { from, to: end }, true);
        }
        pieces.sort_by_key(|piece| piece.from);
        let mut band = Band::default();
        for (which, piece) in pieces.iter().enumerate() {
            let layer = match piece.held {
                Held::Unknown => &mut band.unknown,
                Held::Task(task) => grown(&mut band.tasks, task),
                Held::Handler { .. } => &mut band.handlers,
            };
            let edge = |cycles| Edge {
                element: 0,
                cycles,
                lost: false,
            };
            layer.push(
                Span {
                    from: edge(piece.from),
                    to: edge(piece.to),
                },
                which,
            );
        }
        band.pieces = pieces;
        band
    }

    /// The layers, in the order they are painted.
    pub fn layers(&self) -> impl DoubleEndedIterator<Item = (Layer, &Spans)> {
        std::iter::once((Layer::Unknown, &self.unknown))
            .chain(
                self.tasks
                    .iter()
                    .enumerate()
                    .map(|(task, spans)| (Layer::Task(task), spans)),
            )
            .chain(std::iter::once((Layer::Handlers, &self.handlers)))
    }

    /// The piece shown at pixel column `x` of `view`: of the last layer
    /// painted there, the piece whose columns hold `x`.
    pub fn at(&self, view: &View, x: f32) -> Option<&Piece> {
        self.layers()
            .rev()
            .find_map(|(_, spans)| spans.at_column(view, x))
            .map(|which| &self.pieces[which])
    }

    /// The most pieces a layer shows in `view`.
    pub fn busiest(&self, view: &View) -> usize {
        self.layers()
            .map(|(_, spans)| spans.count(view))
            .max()
            .unwrap_or_default()
    }
}

/// The pieces the handlers hold: at each cycle, the innermost interrupt
/// open, the one entered last; what it interrupts, the one it nests in, or
/// else the run on the core when it was entered, a switch during it aside.
fn innermost(runs: &[(Run, bool)], interrupts: &[(Interrupt, bool)]) -> Vec<Piece> {
    let mut order: Vec<usize> = (0..interrupts.len()).collect();
    let bounds = |n: usize| (interrupts[n].0.span.from.cycles, interrupts[n].0.span.to.cycles);
    // Outer before inner where two start at the same cycle.
    order.sort_by_key(|&n| (bounds(n).0, std::cmp::Reverse(bounds(n).1)));
    let under = |stack: &[usize], at: i64| match stack.len().checked_sub(2) {
        Some(outer) => Under::Handler(interrupts[stack[outer]].0.handler),
        None => {
            let n = runs.partition_point(|(run, _)| run.span.from.cycles <= at);
            match n.checked_sub(1).map(|n| runs[n].0) {
                Some(run) if run.span.to.cycles > at => Under::Task(run.task),
                _ => Under::Unknown,
            }
        }
    };
    let mut pieces = Vec::new();
    let mut emit = |stack: &[usize], from: i64, to: i64| {
        if from < to {
            let (interrupt, still) = interrupts[stack[stack.len() - 1]];
            pieces.push(Piece {
                from,
                to,
                held: Held::Handler {
                    handler: interrupt.handler,
                    under: under(stack, bounds(stack[0]).0),
                },
                span: interrupt.span,
                still,
            });
        }
    };
    // The interrupts open, the innermost last; the cycles pieces reach.
    let mut stack: Vec<usize> = Vec::new();
    let mut at = i64::MIN;
    for next in order.into_iter().map(Some).chain([None]) {
        let limit = next.map_or(i64::MAX, |n| bounds(n).0);
        while let Some(&top) = stack.last() {
            let end = bounds(top).1;
            if end > limit {
                break;
            }
            emit(&stack, at, end);
            at = at.max(end);
            stack.pop();
        }
        let Some(next) = next else { break };
        if !stack.is_empty() {
            emit(&stack, at, limit);
        }
        at = at.max(limit);
        stack.push(next);
    }
    pieces
}

/// The cycles `pieces`, in order and apart, cover, as spans merged where
/// they touch.
fn merged(pieces: &[Piece]) -> Vec<(i64, i64)> {
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for piece in pieces {
        match merged.last_mut() {
            Some(last) if piece.from <= last.1 => last.1 = last.1.max(piece.to),
            _ => merged.push((piece.from, piece.to)),
        }
    }
    merged
}

/// The parts of `from..to` no span of `covered`, in order and apart, holds.
fn outside(from: i64, to: i64, covered: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut parts = Vec::new();
    let mut at = from;
    let first = covered.partition_point(|&(_, end)| end <= from);
    for &(start, end) in &covered[first..] {
        if start >= to {
            break;
        }
        if start > at {
            parts.push((at, start));
        }
        at = at.max(end);
    }
    if at < to {
        parts.push((at, to));
    }
    parts
}

/// `list[n]`, the list grown to hold it.
fn grown<T: Default>(list: &mut Vec<T>, n: usize) -> &mut T {
    if list.len() <= n {
        list.resize_with(n + 1, T::default);
    }
    &mut list[n]
}

#[cfg(test)]
mod tests;
