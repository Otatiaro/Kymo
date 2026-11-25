//! The text filter of the event list (ADR-076 §2): words a type's qualified
//! name must contain, and words after `-` it must not, case aside.

#[derive(Default)]
pub(super) struct Words {
    /// The text typed.
    pub text: String,
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
        self.wanted.clear();
        self.unwanted.clear();
        for word in self.text.split_whitespace() {
            match word.strip_prefix('-') {
                Some("") => {}
                Some(word) => self.unwanted.push(word.to_lowercase()),
                None => self.wanted.push(word.to_lowercase()),
            }
        }
    }

    /// Whether a word leaves a type out.
    pub fn active(&self) -> bool {
        !self.wanted.is_empty() || !self.unwanted.is_empty()
    }

    /// Whether the type named `name` in full stays.
    pub fn keeps(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.wanted.iter().all(|word| name.contains(word.as_str()))
            && !self.unwanted.iter().any(|word| name.contains(word.as_str()))
    }
}

#[cfg(test)]
mod tests;
