//! The load of a row over time: how many cycles its spans cover in each bin
//! of a view. A bin lasts 1, 2 or 5 × 10ⁿ cycles, the shortest that is as
//! wide as the view asks, and bins are counted from the capture's origin, not
//! from the view's edge: panning changes neither their bounds nor their values. The
//! cycles a row covers before any point are a prefix sum and a binary search
//! away, so a bin's count is exact to the cycle at any zoom.
//!
//! A row's share of a bin is over the time its state is known: a loss hides
//! it, and only the next event that gives the state again makes it known.
//! A capture records one ring, so a loss hides every row it shows.

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
        self.before(at, self.to.partition_point(|&to| to <= at))
    }

    /// The cycles covered between each two successive `edges`, which come
    /// in order.
    pub fn counts(&self, edges: &[i64]) -> Vec<i64> {
        let mut ended = 0;
        let until: Vec<i64> = edges.iter().map(|&at| self.until_from(at, &mut ended)).collect();
        until.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }

    /// The cycles covered before `at`, as `until` counts them, when `at` is
    /// no earlier than the point `ended` was last moved for: the spans ended
    /// by `at` are searched from `ended` forward, in steps that double, and
    /// `ended` moves past them. Points close in time cost a few steps each.
    fn until_from(&self, at: i64, ended: &mut usize) -> i64 {
        let to = &self.to[*ended..];
        let mut step = 1;
        while step < to.len() && to[step - 1] <= at {
            step *= 2;
        }
        let low = step / 2;
        *ended += low + to[low..step.min(to.len())].partition_point(|&to| to <= at);
        self.before(at, *ended)
    }

    /// The cycles covered before `at`, `n` the number of spans it ends.
    fn before(&self, at: i64, n: usize) -> i64 {
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

    /// The cycles `self` covers and `other` does not: a task's runs less the
    /// time handlers interrupt it.
    pub fn without(&self, other: &Covered) -> Covered {
        let mut without = Covered::default();
        let mut keep = |from: i64, to: i64| {
            if to == i64::MAX {
                without.open = Some(from);
            } else if from < to {
                without.push(from, to);
            }
        };
        let mut cuts = other.spans().peekable();
        for (mut from, to) in self.spans() {
            while let Some(&(cut_from, cut_to)) = cuts.peek() {
                if cut_to <= from {
                    cuts.next();
                    continue;
                }
                if cut_from >= to {
                    break;
                }
                if cut_from > from {
                    keep(from, cut_from);
                }
                if cut_to >= to {
                    from = to;
                    break;
                }
                from = cut_to;
                cuts.next();
            }
            if from < to {
                keep(from, to);
            }
        }
        without
    }
}

/// The cycles a bin lasts when it should last `wanted` cycles: the
/// shortest 1, 2 or 5 × 10ⁿ cycles that is no shorter, a cycle at least.
pub fn width(wanted: f64) -> i64 {
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

/// The edges of the bins of `width` cycles counted from `origin` that hold
/// cycles of `range` within `capture`, both from their first cycle to the
/// one after their last: the first bin's start, then each bin's end, each
/// cut to the capture. Empty when no bin holds any.
pub fn edges(width: i64, origin: i64, range: (i64, i64), capture: (i64, i64)) -> Vec<i64> {
    let (low, high) = (range.0.max(capture.0), range.1.min(capture.1));
    if low >= high {
        return Vec::new();
    }
    let first = (low - origin).div_euclid(width);
    let last = (high - 1 - origin).div_euclid(width);
    std::iter::once((origin + first * width).max(capture.0))
        .chain((first..=last).map(|n| (origin + (n + 1) * width).min(capture.1)))
        .collect()
}

/// The bins `edges` gives, as `edges` takes its arguments, with the cycles
/// `covered` and `known` cover in each. `covered` lies within `known`
/// (`Covered::within`).
pub fn bins(
    covered: &Covered,
    known: &Covered,
    width: i64,
    origin: i64,
    range: (i64, i64),
    capture: (i64, i64),
) -> Vec<Bin> {
    let edges = edges(width, origin, range, capture);
    edges
        .windows(2)
        .zip(covered.counts(&edges))
        .zip(known.counts(&edges))
        .map(|((pair, covered), known)| Bin {
            from: pair[0],
            to: pair[1],
            covered,
            known,
        })
        .collect()
}

/// The CPU row: the time known and, within it, the time handlers run, at
/// any depth, and the time busy, handlers or a task other than idle. Idle
/// interrupted by a handler is busy. By task other than idle, the time it
/// runs and no handler does: their sum is busy less handlers.
#[derive(Default)]
pub struct Cpu {
    pub known: Covered,
    pub handlers: Covered,
    pub busy: Covered,
    pub tasks: Vec<Covered>,
}

/// A bin of the CPU row, from `from` to `to` cycles. Its stack, from the
/// bottom: `handlers`, then `tasks` other than idle, `each` of them by
/// task, then idle up to `known`; the rest of the bin is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuBin {
    pub from: i64,
    pub to: i64,
    pub known: i64,
    pub handlers: i64,
    pub tasks: i64,
    pub each: Vec<i64>,
}

/// The CPU row's bins, as `bins` gives a row's.
pub fn cpu_bins(cpu: &Cpu, width: i64, origin: i64, range: (i64, i64), capture: (i64, i64)) -> Vec<CpuBin> {
    let bins = |covered| bins(covered, &cpu.known, width, origin, range, capture);
    let handlers = bins(&cpu.handlers);
    let busy = bins(&cpu.busy);
    let each: Vec<Vec<Bin>> = cpu.tasks.iter().map(bins).collect();
    handlers
        .into_iter()
        .zip(busy)
        .enumerate()
        .map(|(n, (handlers, busy))| CpuBin {
            from: handlers.from,
            to: handlers.to,
            known: handlers.known,
            handlers: handlers.covered,
            tasks: busy.covered - handlers.covered,
            each: each.iter().map(|task| task[n].covered).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
