//! The text filter of the event list (ADR-076 §2), on a type's qualified
//! name, case aside: one of the plain words at least, every word after `+`,
//! none after `-`.

#[derive(Default)]
pub(super) struct Words {
    /// The text typed.
    pub text: String,
    some: Vec<String>,
    wanted: Vec<String>,
    unwanted: Vec<String>,
}

impl Words {
    /// Reads the words of `text`, which `self.text` becomes.
    pub fn set(&mut self, text: &str) {
        text.clone_into(&mut self.text);
        self.read();
    }

    /// Reads the words of `self.text`, after it was typed in.
    pub fn read(&mut self) {
        self.some.clear();
        self.wanted.clear();
        self.unwanted.clear();
        for word in self.text.split_whitespace() {
            let (list, word) = match (word.strip_prefix('+'), word.strip_prefix('-')) {
                (Some(word), _) => (&mut self.wanted, word),
                (_, Some(word)) => (&mut self.unwanted, word),
                _ => (&mut self.some, word),
            };
            if !word.is_empty() {
                list.push(word.to_lowercase());
            }
        }
    }

    /// Whether a word leaves a type out.
    pub fn active(&self) -> bool {
        !self.some.is_empty() || !self.wanted.is_empty() || !self.unwanted.is_empty()
    }

    /// Whether the type named `name` in full stays.
    pub fn keeps(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        let has = |word: &String| name.contains(word.as_str());
        (self.some.is_empty() || self.some.iter().any(has))
            && self.wanted.iter().all(has)
            && !self.unwanted.iter().any(has)
    }
}

#[cfg(test)]
mod tests;
