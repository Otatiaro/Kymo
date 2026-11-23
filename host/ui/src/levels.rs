//! A row's events at decreasing resolution, so that the columns of a view
//! are found in time bound by its width rather than by the events it spans.

use crate::timeline::View;

/// The cycles of a row's events, sorted, and coarser levels of them. Level
/// `k` cuts time into buckets of 2^k cycles and holds, for each bucket with
/// at least one event, its first and last event. A level is kept only when
/// it holds at most half the entries of the finer one kept before it, so the
/// levels together hold fewer entries than there are events.
pub struct Levels {
    cycles: Vec<i64>,
    /// The levels kept, finest first, each with its `k`.
    levels: Vec<(u32, Vec<(i64, i64)>)>,
}

impl Levels {
    pub fn new(cycles: Vec<i64>) -> Self {
        debug_assert!(cycles.is_sorted());
        // Two neighbours share their bucket of 2^k cycles once k reaches the
        // highest bit in which they differ: one pass over the neighbours
        // gives the size of every level, and only the levels kept are built.
        let mut apart = [0usize; i64::BITS as usize + 1];
        for pair in cycles.windows(2) {
            apart[(i64::BITS - (pair[0] ^ pair[1]).leading_zeros()) as usize] += 1;
        }
        let mut levels: Vec<(u32, Vec<(i64, i64)>)> = Vec::new();
        let mut kept = cycles.len();
        // Equal neighbours share every bucket.
        let mut size = cycles.len() - apart[0];
        for k in 1..i64::BITS {
            // The neighbours that share a bucket of 2^k but not of 2^(k-1).
            size -= apart[k as usize];
            if size * 2 > kept {
                continue;
            }
            let level = match levels.last() {
                Some((_, finer)) => group(finer.iter().copied(), k, size),
                None => group(cycles.iter().map(|&at| (at, at)), k, size),
            };
            levels.push((k, level));
            kept = size;
            if size <= 1 {
                break;
            }
        }
        Self { cycles, levels }
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

/// `entries`, sorted, merged by buckets of 2^`k` cycles: `size` of them.
fn group(entries: impl Iterator<Item = (i64, i64)>, k: u32, size: usize) -> Vec<(i64, i64)> {
    let mut buckets: Vec<(i64, i64)> = Vec::with_capacity(size);
    for (first, last) in entries {
        match buckets.last_mut() {
            Some(bucket) if bucket.0 >> k == first >> k => bucket.1 = last,
            _ => buckets.push((first, last)),
        }
    }
    buckets
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
            let levels = Levels::new(cycles.clone());
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

    /// The levels kept are those that halve the entries of the finer one
    /// kept before them; each holds the first and last event of each bucket,
    /// as grouping the events themselves gives them; together they hold
    /// fewer entries than there are events.
    #[test]
    fn levels_are_the_buckets_of_the_events() {
        let mut state = 0x2545_f491_4f6c_dd1d;
        let mut at = -50_000_000;
        let cycles: Vec<i64> = (0..100_000)
            .map(|_| {
                // Repeated cycles too, and a start below zero.
                at += (random(&mut state) % 10_000) as i64 * i64::from(!random(&mut state).is_multiple_of(4));
                at
            })
            .collect();
        let buckets = |k: u32| {
            let mut buckets: Vec<(i64, i64)> = Vec::new();
            for &at in &cycles {
                match buckets.last_mut() {
                    Some(bucket) if bucket.0 >> k == at >> k => bucket.1 = at,
                    _ => buckets.push((at, at)),
                }
            }
            buckets
        };
        let mut expected = Vec::new();
        let mut kept = cycles.len();
        for k in 1..i64::BITS {
            let level = buckets(k);
            if level.len() * 2 <= kept {
                kept = level.len();
                expected.push((k, level));
                if kept <= 1 {
                    break;
                }
            }
        }
        let levels = Levels::new(cycles.clone());
        assert_eq!(levels.levels.len(), expected.len());
        for ((k, level), (expected_k, expected)) in levels.levels.iter().zip(&expected) {
            assert_eq!(k, expected_k);
            assert_eq!(level, expected, "level {k}");
        }
        let entries: usize = levels.levels.iter().map(|(_, level)| level.len()).sum();
        assert!(entries < levels.cycles.len(), "{entries} entries");
    }
}
