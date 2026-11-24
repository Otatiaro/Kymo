//! The load of a row over time: how many cycles its spans cover in each bin
//! of a view. A bin lasts 1, 2 or 5 × 10ⁿ cycles, the shortest that is a few
//! pixels wide, and bins are counted from the capture's origin, not from the
//! view's edge: panning changes neither their bounds nor their values. The
//! cycles a row covers before any point are a prefix sum and a binary search
//! away, so a bin's count is exact to the cycle at any zoom.
//!
//! A row's share of a bin is over the time its state is known: a loss hides
//! it, and only the next event that gives the state again makes it known.
//! A capture records one ring, so a loss hides every row it shows.

/// The narrowest a bin is drawn, in pixels.
pub const PIXELS: f64 = 4.0;

/// The spans of a row, which do not overlap, in order, and the one still
/// open at the end of the capture, if any.
#[derive(Default)]
pub struct Covered {
    from: Vec<i64>,
    to: Vec<i64>,
    /// The cycles each span and those before it cover.
    through: Vec<i64>,
    open: Option<i64>,
}

impl Covered {
    /// Appends the span from `from` to `to`, which starts no earlier than
    /// those held end.
    pub fn push(&mut self, from: i64, to: i64) {
        let to = to.max(from);
        let before = self.through.last().copied().unwrap_or_default();
        self.from.push(from);
        self.to.push(to);
        self.through.push(before + to - from);
    }

    /// The span still open, from `from` to the end of the capture.
    pub fn open(&mut self, from: Option<i64>) {
        self.open = from;
    }

    /// The cycles covered before `at`.
    pub fn until(&self, at: i64) -> i64 {
        let n = self.to.partition_point(|&to| to <= at);
        let mut covered = n.checked_sub(1).map_or(0, |last| self.through[last]);
        if n < self.from.len() && self.from[n] < at {
            covered += at - self.from[n];
        }
        if let Some(open) = self.open
            && open < at
        {
            covered += at - open;
        }
        covered
    }

    /// The cycles any of `spans` covers, which may overlap and come in any
    /// order, and from the earliest `open` to the end of the capture.
    pub fn union(spans: impl IntoIterator<Item = (i64, i64)>, open: Option<i64>) -> Covered {
        let mut spans: Vec<_> = spans.into_iter().filter(|&(from, to)| from < to).collect();
        spans.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (from, to) in spans {
            match merged.last_mut() {
                Some(last) if from <= last.1 => last.1 = last.1.max(to),
                _ => merged.push((from, to)),
            }
        }
        let open = open.map(|open| {
            merged
                .iter()
                .filter(|&&(_, to)| to >= open)
                .map(|&(from, _)| from)
                .fold(open, i64::min)
        });
        let mut union = Covered::default();
        for (from, to) in merged {
            if open.is_none_or(|open| to < open) {
                union.push(from, to);
            }
        }
        union.open(open);
        union
    }

    /// The spans, the open one last, ended at `i64::MAX`.
    fn spans(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.from
            .iter()
            .zip(&self.to)
            .map(|(&from, &to)| (from, to))
            .chain(self.open.map(|open| (open, i64::MAX)))
    }

    /// The cycles both `self` and `known` cover: a row's spans, cut to the
    /// time its state is known.
    pub fn within(&self, known: &Covered) -> Covered {
        let mut within = Covered::default();
        let (mut row, mut known) = (self.spans().peekable(), known.spans().peekable());
        while let (Some(&(a, b)), Some(&(c, d))) = (row.peek(), known.peek()) {
            let (from, to) = (a.max(c), b.min(d));
            if to == i64::MAX {
                within.open = Some(from);
            } else if from < to {
                within.push(from, to);
            }
            if b <= d {
                row.next();
            } else {
                known.next();
            }
        }
        within
    }
}

/// The cycles a bin lasts in a view of `per_pixel` cycles per pixel: the
/// shortest 1, 2 or 5 × 10ⁿ cycles at least `PIXELS` wide.
pub fn width(per_pixel: f64) -> i64 {
    let wanted = PIXELS * per_pixel;
    let mut decade: i64 = 1;
    loop {
        for width in [decade, 2 * decade, 5 * decade] {
            if width as f64 >= wanted {
                return width;
            }
        }
        decade *= 10;
    }
}

/// A bin from `from` to `to` cycles, the cycles a row covers in it and
/// those in which its state is known. Its share is `covered / known`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bin {
    pub from: i64,
    pub to: i64,
    pub covered: i64,
    pub known: i64,
}

/// The bins of `width` cycles counted from `origin` that hold cycles of
/// `range` within `capture`, both from their first cycle to the one after
/// their last: each cut to the capture, with the cycles `covered` and
/// `known` cover in it. `covered` lies within `known` (`Covered::within`).
pub fn bins(
    covered: &Covered,
    known: &Covered,
    width: i64,
    origin: i64,
    range: (i64, i64),
    capture: (i64, i64),
) -> Vec<Bin> {
    let (low, high) = (range.0.max(capture.0), range.1.min(capture.1));
    if low >= high {
        return Vec::new();
    }
    let first = (low - origin).div_euclid(width);
    let last = (high - 1 - origin).div_euclid(width);
    let mut at = (origin + first * width).max(capture.0);
    let mut before = (covered.until(at), known.until(at));
    (first..=last)
        .map(|n| {
            let (from, to) = (at, (origin + (n + 1) * width).min(capture.1));
            let after = (covered.until(to), known.until(to));
            let bin = Bin {
                from,
                to,
                covered: after.0 - before.0,
                known: after.1 - before.1,
            };
            (at, before) = (to, after);
            bin
        })
        .collect()
}

/// The CPU row: the time known and, within it, the time handlers run, at
/// any depth, and the time busy, handlers or a task other than idle. Idle
/// interrupted by a handler is busy.
#[derive(Default)]
pub struct Cpu {
    pub known: Covered,
    pub handlers: Covered,
    pub busy: Covered,
}

/// A bin of the CPU row, from `from` to `to` cycles. Its stack, from the
/// bottom: `handlers`, then `tasks` other than idle, then idle up to
/// `known`; the rest of the bin is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuBin {
    pub from: i64,
    pub to: i64,
    pub known: i64,
    pub handlers: i64,
    pub tasks: i64,
}

/// The CPU row's bins, as `bins` gives a row's.
pub fn cpu_bins(cpu: &Cpu, width: i64, origin: i64, range: (i64, i64), capture: (i64, i64)) -> Vec<CpuBin> {
    let handlers = bins(&cpu.handlers, &cpu.known, width, origin, range, capture);
    let busy = bins(&cpu.busy, &cpu.known, width, origin, range, capture);
    handlers
        .into_iter()
        .zip(busy)
        .map(|(handlers, busy)| CpuBin {
            from: handlers.from,
            to: handlers.to,
            known: handlers.known,
            handlers: handlers.covered,
            tasks: busy.covered - handlers.covered,
        })
        .collect()
}

#[cfg(test)]
mod tests;
