//! The text filter (ADR-076 §2, ADR-095 §5), case aside: one of the plain
//! words at least, every word after `+`, none after `-`. A plain word is
//! looked for in an event's type and in its fields as `kymo decode` writes
//! them, pointers by what they point at; `field=value` holds for an event
//! whose field of that name holds that integer (decimal, or hexadecimal
//! after `0x`), and `field=` for one that has a field of that name.

use crate::decode::{self, Decoded};
use crate::symbols::Symbols;

/// One word of the filter.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Word {
    Text(String),
    Field { name: String, value: Option<i128> },
}

/// The words of a filter, sorted by their sign.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Words {
    some: Vec<Word>,
    wanted: Vec<Word>,
    unwanted: Vec<Word>,
}

impl Words {
    /// The words of `text`; an error names a `field=value` whose value is
    /// not an integer.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut words = Self::default();
        for word in text.split_whitespace() {
            let (list, word) = match (word.strip_prefix('+'), word.strip_prefix('-')) {
                (Some(word), _) => (&mut words.wanted, word),
                (_, Some(word)) => (&mut words.unwanted, word),
                _ => (&mut words.some, word),
            };
            if word.is_empty() {
                continue;
            }
            list.push(match word.split_once('=') {
                Some((name, value)) if !name.is_empty() => Word::Field {
                    name: name.to_owned(),
                    value: (!value.is_empty())
                        .then(|| integer(value).ok_or_else(|| format!("{word}: {value} is not an integer")))
                        .transpose()?,
                },
                _ => Word::Text(word.to_lowercase()),
            });
        }
        Ok(words)
    }

    /// Whether a word leaves anything out.
    pub fn active(&self) -> bool {
        !self.some.is_empty() || !self.wanted.is_empty() || !self.unwanted.is_empty()
    }

    /// Whether the event `decoded` stays, its pointer fields named from
    /// `symbols`.
    pub fn keeps(&self, symbols: &Symbols, decoded: &Decoded) -> bool {
        if !self.active() {
            return true;
        }
        let mut text = decoded.event.name.clone();
        text.push(' ');
        decode::fields(&mut text, symbols, decoded);
        let text = text.to_lowercase();
        let has = |word: &Word| match word {
            Word::Text(word) => text.contains(word.as_str()),
            Word::Field { name, value } => decoded.event.fields.iter().any(|field| {
                field.name == *name
                    && value.is_none_or(|value| {
                        decode::integer(&field.ty, &decoded.payload[field.offset as usize..]) == Some(value)
                    })
            }),
        };
        (self.some.is_empty() || self.some.iter().any(has))
            && self.wanted.iter().all(has)
            && !self.unwanted.iter().any(has)
    }
}

/// `text` as an integer: decimal, signed or not, or hexadecimal after `0x`.
fn integer(text: &str) -> Option<i128> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, text),
    };
    let value = match digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        Some(hex) => i128::from(u64::from_str_radix(hex, 16).ok()?),
        None => i128::from(digits.parse::<u64>().ok()?),
    };
    Some(if negative { -value } else { value })
}

#[cfg(test)]
mod tests;
