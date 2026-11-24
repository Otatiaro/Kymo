//! The load of a row over time: how many cycles its spans cover in each bin
//! of a view. A bin lasts 1, 2 or 5 × 10ⁿ cycles, the shortest that is a few
//! pixels wide, and bins are counted from the capture's origin, not from the
//! view's edge: panning changes neither their bounds nor their values. The
//! cycles a row covers before any point are a prefix sum and a binary search
//! away, so a bin's count is exact to the cycle at any zoom.

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

/// A bin from `from` to `to` cycles, and the cycles a row covers in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bin {
    pub from: i64,
    pub to: i64,
    pub covered: i64,
}

/// The bins of `width` cycles counted from `origin` that hold cycles of
/// `range` within `capture`, both from their first cycle to the one after
/// their last: each cut to the capture, with the cycles `covered` covers in
/// it.
pub fn bins(covered: &Covered, width: i64, origin: i64, range: (i64, i64), capture: (i64, i64)) -> Vec<Bin> {
    let (low, high) = (range.0.max(capture.0), range.1.min(capture.1));
    if low >= high {
        return Vec::new();
    }
    let first = (low - origin).div_euclid(width);
    let last = (high - 1 - origin).div_euclid(width);
    let mut at = (origin + first * width).max(capture.0);
    let mut before = covered.until(at);
    (first..=last)
        .map(|n| {
            let (from, to) = (at, (origin + (n + 1) * width).min(capture.1));
            let after = covered.until(to);
            let bin = Bin {
                from,
                to,
                covered: after - before,
            };
            (at, before) = (to, after);
            bin
        })
        .collect()
}

#[cfg(test)]
mod tests;
