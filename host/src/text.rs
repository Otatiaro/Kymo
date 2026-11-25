//! The text of an application event (ADR-088): the string its type declares
//! as `static constexpr char kymo_text[]`, read from the DWARF, with each
//! replacement field `{name}` or `{name:spec}` replaced by the value of the
//! field `name`, presented as `std::format` presents it (format.rs). `{{`
//! and `}}` write a brace.
//!
//! A field is an integer, a `bool`, a `char`, a floating-point number, an
//! enumeration, written by the name of its value, or a `char` array, written
//! as a string up to its first NUL. The text is checked against the fields
//! when the catalogue is read: a name the event does not have, or a
//! specification that does not suit its field, is an error.

use crate::events::{Field, Kind, Type};
use crate::format::{Class, Spec, Value};
use std::fmt::Write;

/// The name of the static member that holds an event's text.
pub const MEMBER: &str = "kymo_text";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Literal(String),
    /// A replacement field: the event's field, by position, and how to write it.
    Field(usize, Spec),
}

/// An event's text, checked against its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    /// The text as the type declares it.
    pub template: String,
    pieces: Vec<Piece>,
}

/// What a field's bytes read as, when they can be written in a text.
fn class(ty: &Type) -> Option<Class> {
    let size = ty.size?;
    match &ty.kind {
        Kind::Base(gimli::DW_ATE_boolean) if size <= 8 => Some(Class::Bool),
        Kind::Base(gimli::DW_ATE_float) if size == 4 || size == 8 => Some(Class::Float),
        Kind::Base(gimli::DW_ATE_float) => None,
        Kind::Base(_) if is_char(ty) => Some(Class::Char),
        Kind::Base(_) if size <= 8 => Some(Class::Integer),
        Kind::Enum(_) if size <= 8 => Some(Class::Str),
        Kind::Array(element, lengths) if lengths.len() == 1 && is_char(element) => Some(Class::Str),
        _ => None,
    }
}

/// Whether `ty` is `char`, which `std::format` writes as a character; `signed
/// char` and `unsigned char`, `std::int8_t` and `std::uint8_t`, are integers.
pub fn is_char(ty: &Type) -> bool {
    matches!(
        ty.kind,
        Kind::Base(gimli::DW_ATE_signed_char | gimli::DW_ATE_unsigned_char)
    ) && ty.size == Some(1)
        && ty.name == "char"
}

impl Text {
    /// The text `template` of an event with `fields`, or why it does not suit them.
    pub fn parse(template: &str, fields: &[Field]) -> Result<Text, String> {
        let mut pieces = Vec::new();
        let mut literal = String::new();
        let mut rest = template;
        while let Some(at) = rest.find(['{', '}']) {
            literal.push_str(&rest[..at]);
            let brace = &rest[at..];
            if let Some(after) = brace.strip_prefix("{{") {
                literal.push('{');
                rest = after;
                continue;
            }
            if let Some(after) = brace.strip_prefix("}}") {
                literal.push('}');
                rest = after;
                continue;
            }
            if brace.starts_with('}') {
                return Err(format!(
                    "a lone }} at {}: write }}}} for a brace",
                    template.len() - brace.len()
                ));
            }
            let end = brace
                .find('}')
                .ok_or_else(|| format!("a {{ at {} is not closed", template.len() - brace.len()))?;
            let inside = &brace[1..end];
            let (name, spec) = inside.split_once(':').unwrap_or((inside, ""));
            if name.is_empty() {
                return Err(format!("{{{inside}}}: a field is named, not numbered"));
            }
            let index = fields
                .iter()
                .position(|field| field.name == name)
                .ok_or_else(|| format!("{{{inside}}}: the event has no field {name}"))?;
            let field = &fields[index];
            let class = class(&field.ty)
                .ok_or_else(|| format!("{{{inside}}}: {} cannot be written in a text", field.ty.name))?;
            let spec = Spec::parse(spec).map_err(|error| format!("{{{inside}}}: {error}"))?;
            spec.check(class).map_err(|error| format!("{{{inside}}}: {error}"))?;
            if !literal.is_empty() {
                pieces.push(Piece::Literal(std::mem::take(&mut literal)));
            }
            pieces.push(Piece::Field(index, spec));
            rest = &brace[end + 1..];
        }
        literal.push_str(rest);
        if !literal.is_empty() {
            pieces.push(Piece::Literal(literal));
        }
        Ok(Text {
            template: template.to_owned(),
            pieces,
        })
    }

    /// Writes the text of the event of `fields` whose bytes are `payload`.
    pub fn write(&self, out: &mut String, fields: &[Field], payload: &[u8]) {
        for piece in &self.pieces {
            match piece {
                Piece::Literal(text) => out.push_str(text),
                Piece::Field(index, spec) => {
                    let field = &fields[*index];
                    let bytes = &payload[field.offset as usize..];
                    let text;
                    let value = match &field.ty.kind {
                        Kind::Enum(enumerators) => {
                            let (unsigned, signed) = integers(&bytes[..field.ty.size.unwrap_or_default() as usize]);
                            text = match enumerators
                                .iter()
                                .find(|(value, _)| *value == signed || *value as u64 == unsigned)
                            {
                                Some((_, name)) => name.clone(),
                                None => unsigned.to_string(),
                            };
                            Value::Str(&text)
                        }
                        Kind::Array(..) => {
                            text = string(&bytes[..field.ty.size.unwrap_or_default() as usize]);
                            Value::Str(&text)
                        }
                        _ => scalar(&field.ty, bytes),
                    };
                    spec.write(out, value);
                }
            }
        }
    }
}

/// The value at the start of `bytes`, unsigned then signed.
fn integers(bytes: &[u8]) -> (u64, i64) {
    let unsigned = bytes
        .iter()
        .rev()
        .fold(0u64, |value, byte| value << 8 | u64::from(*byte));
    let shift = 64 - 8 * bytes.len() as u32;
    (unsigned, ((unsigned << shift) as i64) >> shift)
}

/// The value of a base type at the start of `bytes`.
fn scalar<'a>(ty: &Type, bytes: &[u8]) -> Value<'a> {
    let size = ty.size.unwrap_or_default() as usize;
    let (unsigned, signed) = integers(&bytes[..size]);
    match ty.kind {
        Kind::Base(gimli::DW_ATE_boolean) => Value::Bool(unsigned != 0),
        Kind::Base(gimli::DW_ATE_float) if size == 4 => Value::F32(f32::from_bits(unsigned as u32)),
        Kind::Base(gimli::DW_ATE_float) => Value::F64(f64::from_bits(unsigned)),
        _ if is_char(ty) => Value::Char(unsigned as u8),
        Kind::Base(gimli::DW_ATE_signed | gimli::DW_ATE_signed_char) => Value::Integer(i128::from(signed)),
        _ => Value::Integer(i128::from(unsigned)),
    }
}

/// A `char` array up to its first NUL: printable ASCII as is, any other byte
/// as `\xNN`.
fn string(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &byte in bytes.iter().take_while(|&&byte| byte != 0) {
        match byte {
            b' '..=b'~' => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "\\x{byte:02x}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
