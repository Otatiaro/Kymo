//! A row's events at decreasing resolution, so that the columns of a view
//! are found in time bound by its width rather than by the events it spans.

use crate::timeline::View;

/// The cycles of a row's events, sorted, and coarser levels of them, which
/// only ever grow at their end. Level `k` cuts time into buckets of 2^k
/// cycles and holds, for each bucket with at least one event, its first and
/// last event. A level is built once it holds at most half the entries of
/// the finer one kept before it, and dropped once it holds more than two
/// thirds of them: events appended near that bound do not rebuild it over
/// and over, and the levels together hold at most twice as many entries as
/// there are events.
pub struct Levels {
    cycles: Vec<i64>,
    /// The neighbouring events whose cycles first differ at each bit, plus
    /// one; at 0, the equal ones. Two neighbours share their bucket of 2^k
    /// cycles once k reaches that count, so it gives the size of every
    /// level without building it.
    apart: [usize; i64::BITS as usize + 1],
    /// The levels kept, finest first, each with its `k`.
    levels: Vec<(u32, Vec<(i64, i64)>)>,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            cycles: Vec::new(),
            apart: [0; i64::BITS as usize + 1],
            levels: Vec::new(),
        }
    }
}

impl Levels {
    /// Appends `cycles`, sorted, none before the last event already held.
    pub fn extend(&mut self, cycles: &[i64]) {
        let Some(&next) = cycles.first() else {
            return;
        };
        assert!(
            cycles.is_sorted() && self.cycles.last().is_none_or(|&last| last <= next),
            "events appended out of order"
        );
        let old = self.cycles.len();
        self.cycles.extend_from_slice(cycles);
        for pair in self.cycles[old.saturating_sub(1)..].windows(2) {
            self.apart[(i64::BITS - (pair[0] ^ pair[1]).leading_zeros()) as usize] += 1;
        }
        // Each level takes what the finer one gained, its last bucket again
        // included: it may have grown.
        let mut gained = old;
        for n in 0..self.levels.len() {
            let (finer, coarser) = self.levels.split_at_mut(n);
            let (k, level) = &mut coarser[0];
            let before = level.len();
            match finer.last() {
                Some((_, finer)) => group(level, finer[gained..].iter().copied(), *k),
                None => group(level, self.cycles[gained..].iter().map(|&at| (at, at)), *k),
            }
            gained = before.saturating_sub(1);
        }
        self.settle();
    }

    /// Builds the levels that now halve the finer one kept before them, and
    /// drops those that no longer hold at most two thirds of it.
    fn settle(&mut self) {
        let mut built = std::mem::take(&mut self.levels).into_iter().peekable();
        let mut kept = self.cycles.len();
        // Equal neighbours share every bucket.
        let mut size = self.cycles.len() - self.apart[0];
        for k in 1..i64::BITS {
            // The neighbours that share a bucket of 2^k but not of 2^(k-1).
            size -= self.apart[k as usize];
            let existing = built.next_if(|(at, _)| *at == k);
            let keep = match existing {
                Some(_) => size * 3 <= kept * 2,
                None => size * 2 <= kept,
            };
            if !keep {
                continue;
            }
            let level = existing.map(|(_, level)| level).unwrap_or_else(|| {
                let mut level = Vec::with_capacity(size);
                match self.levels.last() {
                    Some((_, finer)) => group(&mut level, finer.iter().copied(), k),
                    None => group(&mut level, self.cycles.iter().map(|&at| (at, at)), k),
                }
                level
            });
            self.levels.push((k, level));
            kept = size;
            if size <= 1 {
                break;
            }
        }
    }

    pub fn cycles(&self) -> &[i64] {
        &self.cycles
    }

    /// The pixel columns of `view` holding at least one event, sorted,
    /// merged into runs. They are read from the coarsest level whose bucket
    /// is no wider than a pixel: a bucket's first and last event then fall
    /// in the same column or in two adjacent ones, and no column between
    /// them can be empty.
    pub fn columns(&self, view: &View) -> Vec<(f32, f32)> {
        let low = view.cycles(view.left);
        let high = view.origin + (view.start + view.span).ceil() as i64;
        let per_pixel = view.span / view.width;
        let coarsest = self
            .levels
            .iter()
            .rev()
            .find(|(k, _)| 2f64.powi(*k as i32) <= per_pixel);
        match coarsest {
            Some((_, level)) => {
                let from = level.partition_point(|bucket| bucket.1 < low);
                runs(view, level[from..].iter().copied(), high)
            }
            None => {
                let from = self.cycles.partition_point(|&at| at < low);
                runs(view, self.cycles[from..].iter().map(|&at| (at, at)), high)
            }
        }
    }
}

/// Merges `entries`, sorted, none before the last of `buckets`, into
/// `buckets` of 2^`k` cycles.
fn group(buckets: &mut Vec<(i64, i64)>, entries: impl Iterator<Item = (i64, i64)>, k: u32) {
    for (first, last) in entries {
        match buckets.last_mut() {
            Some(bucket) if bucket.0 >> k == first >> k => bucket.1 = last,
            _ => buckets.push((first, last)),
        }
    }
}

/// The columns of `entries`, each spanning a pixel column or two adjacent
/// ones, up to the first that starts past `high`.
fn runs(view: &View, entries: impl Iterator<Item = (i64, i64)>, high: i64) -> Vec<(f32, f32)> {
    let mut runs: Vec<(f32, f32)> = Vec::new();
    for (first, last) in entries.take_while(|entry| entry.0 <= high) {
        let from = view.x((first - view.origin) as f64).floor();
        let to = view.x((last - view.origin) as f64).floor() + 1.0;
        match runs.last_mut() {
            Some(run) if from <= run.1 => run.1 = run.1.max(to),
            _ => runs.push((from, to)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small xorshift, so the cases repeat.
    fn random(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// The runs drawn are the pixel columns of every event in view, each
    /// event placed on its own, at zooms from a few cycles to millions per
    /// pixel.
    #[test]
    fn runs_cover_the_columns_of_the_events_in_view() {
        let mut state = 0x9e37_79b9_7f4a_7c15;
        let mut coarse = 0;
        for _ in 0..5000 {
            let mut at = 1000 + (random(&mut state) % 1000) as i64;
            let cycles: Vec<i64> = (0..random(&mut state) % 400)
                .map(|_| {
                    // Bursts and gaps, so columns hold many events or none.
                    at += if random(&mut state).is_multiple_of(8) {
                        (random(&mut state) % 100_000) as i64
                    } else {
                        (random(&mut state) % 50) as i64
                    };
                    at
                })
                .collect();
            let span = 10f64.powi((random(&mut state) % 7) as i32) * (100 + random(&mut state) % 900) as f64;
            let view = View {
                origin: 1000,
                start: (random(&mut state) % 200_000) as f64 - 1000.0,
                span,
                left: 260.0,
                width: 100.0 + (random(&mut state) % 1500) as f64,
            };
            let mut levels = Levels::default();
            levels.extend(&cycles);
            if levels
                .levels
                .iter()
                .any(|(k, _)| 2f64.powi(*k as i32) <= view.span / view.width)
            {
                coarse += 1;
            }
            let right = view.left + view.width as f32;
            let mut expected: Vec<f32> = cycles
                .iter()
                .map(|&at| view.x((at - view.origin) as f64).floor())
                .filter(|&x| x >= view.left && x < right)
                .collect();
            expected.dedup();
            let drawn: Vec<f32> = levels
                .columns(&view)
                .into_iter()
                .flat_map(|(from, to)| (from as i64..to as i64).map(|x| x as f32))
                .filter(|&x| x >= view.left && x < right)
                .collect();
            assert_eq!(drawn, expected, "{cycles:?}");
        }
        // Most cases read a coarse level, not the events themselves.
        assert!(coarse > 2500, "{coarse} cases on a coarse level");
    }

    /// The buckets of 2^`k` cycles of `cycles`, grouped one event at a time.
    fn buckets(cycles: &[i64], k: u32) -> Vec<(i64, i64)> {
        let mut buckets: Vec<(i64, i64)> = Vec::new();
        for &at in cycles {
            match buckets.last_mut() {
                Some(bucket) if bucket.0 >> k == at >> k => bucket.1 = at,
                _ => buckets.push((at, at)),
            }
        }
        buckets
    }

    /// Each level holds the buckets of the events at its `k`. Against the
    /// finer one kept before it, a level held `before` is kept while it
    /// holds at most two thirds of it, another one once it holds at most
    /// half. Together they hold at most twice as many entries as there are
    /// events.
    fn check(levels: &Levels, cycles: &[i64], before: &[u32]) {
        assert_eq!(levels.cycles, cycles);
        let mut kept = cycles.len();
        let mut held = levels.levels.iter().peekable();
        for k in 1..i64::BITS {
            let expected = buckets(cycles, k);
            match held.next_if(|(at, _)| *at == k) {
                Some((_, level)) => {
                    assert_eq!(level, &expected, "level {k}");
                    let bound = if before.contains(&k) { kept * 2 / 3 } else { kept / 2 };
                    assert!(level.len() <= bound, "level {k}: {} of {kept}", level.len());
                    kept = level.len();
                    if kept <= 1 {
                        break;
                    }
                }
                None => {
                    let bound = if before.contains(&k) { kept * 2 / 3 } else { kept / 2 };
                    assert!(
                        expected.len() > bound,
                        "level {k} skipped: {} of {kept}",
                        expected.len()
                    );
                }
            }
        }
        assert!(held.next().is_none());
        let entries: usize = levels.levels.iter().map(|(_, level)| level.len()).sum();
        assert!(entries <= 2 * cycles.len(), "{entries} entries");
    }

    /// Cycles with repeats, a start below zero, and a density that changes
    /// along the way, so that levels are built and dropped as they grow.
    fn events(state: &mut u64, count: usize) -> Vec<i64> {
        let mut at = -50_000_000;
        (0..count)
            .map(|n| {
                let spread = [10_000, 50, 1_000_000, 300][n / (count / 8).max(1) % 4];
                at += (random(state) % spread) as i64 * i64::from(!random(state).is_multiple_of(4));
                at
            })
            .collect()
    }

    /// Built at once, the levels kept are exactly those that halve the
    /// finer one kept before them, and hold fewer entries than the events.
    #[test]
    fn levels_built_at_once_halve_each_other() {
        let mut state = 0x2545_f491_4f6c_dd1d;
        let cycles = events(&mut state, 100_000);
        let mut levels = Levels::default();
        levels.extend(&cycles);
        check(&levels, &cycles, &[]);
        let mut kept = cycles.len();
        for (k, level) in &levels.levels {
            assert!(level.len() * 2 <= kept, "level {k}");
            kept = level.len();
        }
        let entries: usize = levels.levels.iter().map(|(_, level)| level.len()).sum();
        assert!(entries < cycles.len(), "{entries} entries");
    }

    /// Appended in pieces of any size, the levels hold the buckets of the
    /// events after each piece.
    #[test]
    fn levels_appended_in_pieces_are_the_buckets_of_the_events() {
        let mut state = 0x6a09_e667_f3bc_c908;
        for _ in 0..20 {
            let cycles = events(&mut state, 20_000);
            let mut levels = Levels::default();
            let mut from = 0;
            let mut held = 0;
            while from < cycles.len() {
                let before: Vec<u32> = levels.levels.iter().map(|(k, _)| *k).collect();
                let to = (from + 1 + (random(&mut state) % 2000) as usize).min(cycles.len());
                levels.extend(&cycles[from..to]);
                check(&levels, &cycles[..to], &before);
                // A level held across a piece that a fresh build would drop.
                held += usize::from(levels.levels.iter().any(|(k, level)| {
                    let finer = levels.levels.iter().take_while(|(at, _)| at < k).last();
                    level.len() * 2 > finer.map_or(to, |(_, finer)| finer.len())
                }));
                from = to;
            }
            assert!(held > 0, "no level kept past half of the finer one");
        }
    }

    #[test]
    #[should_panic(expected = "out of order")]
    fn events_before_the_last_held_are_refused() {
        let mut levels = Levels::default();
        levels.extend(&[10, 20]);
        levels.extend(&[15]);
    }

    #[test]
    #[should_panic(expected = "out of order")]
    fn events_out_of_order_in_a_piece_are_refused() {
        Levels::default().extend(&[10, 30, 20]);
    }
}
