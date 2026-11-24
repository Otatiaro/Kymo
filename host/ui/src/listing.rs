//! The event list's lines: every element of the trace by time, those of a
//! range found by two binary searches. A loss or a suspect line stands at the
//! cycles of the event recorded before it, after that event; before the first
//! event, at its cycles, before it.

use kymo::model::Element;

/// An element of the trace and the cycles it is listed at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub cycles: i64,
    pub element: usize,
}

#[derive(Default)]
pub struct Events {
    /// By cycles, equals in the trace's order.
    entries: Vec<Entry>,
    /// The losses and suspect lines before the first event.
    pending: Vec<usize>,
    /// The cycles of the last event, in the trace's order.
    before: Option<i64>,
}

impl Events {
    /// Takes in `elements` from `from` on.
    pub fn add(&mut self, elements: &[Element], from: usize) {
        for (n, element) in elements.iter().enumerate().skip(from) {
            match (element, self.before) {
                (Element::Event { cycles, .. }, _) => {
                    let cycles = *cycles;
                    let pending = self.pending.drain(..).map(|element| Entry { cycles, element });
                    self.entries.extend(pending);
                    self.entries.push(Entry { cycles, element: n });
                    self.before = Some(cycles);
                }
                (_, Some(cycles)) => self.entries.push(Entry { cycles, element: n }),
                (_, None) => self.pending.push(n),
            }
        }
        // Elements are unique: no two entries are equal.
        self.entries.sort_unstable_by_key(|entry| (entry.cycles, entry.element));
    }

    /// The entries from `from` to `to` cycles, both included.
    pub fn within(&self, from: i64, to: i64) -> &[Entry] {
        let low = self.entries.partition_point(|entry| entry.cycles < from);
        let high = self.entries.partition_point(|entry| entry.cycles <= to);
        &self.entries[low..high.max(low)]
    }
}

#[cfg(test)]
mod tests;
