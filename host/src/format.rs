//! The format specifications of C++'s `std::format` (C++20, [format.string.std]),
//! applied on the host to the values an event carries:
//!
//! ```text
//! [[fill]align][sign][#][0][width][.precision][type]
//! ```
//!
//! What this module produces is what `std::format` produces for the same
//! specification and value; format/reference.cpp is the reference program,
//! format/reference.txt its output. Left out: nested replacement fields for the width or the
//! precision, the locale option `L`, the debug type `?`, hexadecimal floating
//! point (`a`, `A`), and `c` on an integer that is not a `char`.

use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Sign {
    /// `-`, the default: a sign for negative numbers only.
    #[default]
    Negative,
    /// `+`: a sign for every number.
    Always,
    /// ` `: a space before positive numbers.
    Space,
}

/// A parsed format specification, the text after the `:` of a replacement field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub fill: char,
    pub align: Option<Align>,
    pub sign: Sign,
    pub alternate: bool,
    pub zero: bool,
    pub width: usize,
    pub precision: Option<usize>,
    pub ty: Option<char>,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            fill: ' ',
            align: None,
            sign: Sign::Negative,
            alternate: false,
            zero: false,
            width: 0,
            precision: None,
            ty: None,
        }
    }
}

/// What a value is to `std::format`, which decides the types and options its
/// specification admits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Integer,
    /// A `char`.
    Char,
    Bool,
    Float,
    /// A string: a `char` array, or an enumeration by the name of its value.
    Str,
}

/// A value to format.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Integer(i128),
    /// A `char`, by its code.
    Char(u8),
    Bool(bool),
    /// A `float`, whose shortest form is its own, not that of a `double`.
    F32(f32),
    F64(f64),
    Str(&'a str),
}

impl Value<'_> {
    pub fn class(&self) -> Class {
        match self {
            Value::Integer(_) => Class::Integer,
            Value::Char(_) => Class::Char,
            Value::Bool(_) => Class::Bool,
            Value::F32(_) | Value::F64(_) => Class::Float,
            Value::Str(_) => Class::Str,
        }
    }
}

fn align(c: char) -> Option<Align> {
    match c {
        '<' => Some(Align::Left),
        '>' => Some(Align::Right),
        '^' => Some(Align::Center),
        _ => None,
    }
}

/// The digits at the start of `rest`, as a number, and what follows them.
fn number(rest: &str) -> Result<(Option<usize>, &str), String> {
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    if end == 0 {
        return Ok((None, rest));
    }
    let value = rest[..end]
        .parse()
        .map_err(|_| format!("{} is too large", &rest[..end]))?;
    Ok((Some(value), &rest[end..]))
}

impl Spec {
    /// The specification `text`, as `std::format` reads it.
    pub fn parse(text: &str) -> Result<Spec, String> {
        let mut spec = Spec::default();
        let mut rest = text;
        let mut chars = rest.chars();
        let first = chars.next();
        let second = chars.next();
        if let (Some(fill), Some(Some(how))) = (first, second.map(align)) {
            if fill == '{' || fill == '}' {
                return Err(format!("{fill} cannot fill"));
            }
            spec.fill = fill;
            spec.align = Some(how);
            rest = &rest[fill.len_utf8() + 1..];
        } else if let Some(how) = first.and_then(align) {
            spec.align = Some(how);
            rest = &rest[1..];
        }
        if let Some(sign) = rest.chars().next().and_then(|c| match c {
            '+' => Some(Sign::Always),
            '-' => Some(Sign::Negative),
            ' ' => Some(Sign::Space),
            _ => None,
        }) {
            spec.sign = sign;
            rest = &rest[1..];
        }
        if let Some(after) = rest.strip_prefix('#') {
            spec.alternate = true;
            rest = after;
        }
        if let Some(after) = rest.strip_prefix('0') {
            spec.zero = true;
            rest = after;
        }
        if rest.starts_with('{') {
            return Err("a width from another field is not supported".to_owned());
        }
        let (width, after) = number(rest)?;
        spec.width = width.unwrap_or(0);
        rest = after;
        if let Some(after) = rest.strip_prefix('.') {
            if after.starts_with('{') {
                return Err("a precision from another field is not supported".to_owned());
            }
            let (precision, after) = number(after)?;
            spec.precision = Some(precision.ok_or("a precision without digits")?);
            rest = after;
        }
        if rest.starts_with('L') {
            return Err("the locale option L is not supported".to_owned());
        }
        let mut chars = rest.chars();
        spec.ty = chars.next();
        if let Some(extra) = chars.next() {
            return Err(format!("unexpected {extra}"));
        }
        if let Some(ty) = spec.ty
            && !"sbBcdoxXeEfFgG".contains(ty)
        {
            return Err(match ty {
                'a' | 'A' => "hexadecimal floating point (a, A) is not supported".to_owned(),
                '?' => "the debug type ? is not supported".to_owned(),
                _ => format!("unknown type {ty}"),
            });
        }
        Ok(spec)
    }

    /// Whether the type presents the value as an integer.
    fn integral(&self) -> bool {
        matches!(self.ty, Some('b' | 'B' | 'd' | 'o' | 'x' | 'X'))
    }

    /// Whether this specification suits a value of `class`, as `std::format`
    /// decides; why not, otherwise.
    pub fn check(&self, class: Class) -> Result<(), String> {
        let ty = self.ty;
        let numeric = match class {
            Class::Integer => match ty {
                None => true,
                Some('c') => return Err("c on an integer that is not a char is not supported".to_owned()),
                Some(_) if self.integral() => true,
                Some(ty) => return Err(format!("type {ty} does not suit an integer")),
            },
            Class::Char | Class::Bool => {
                let own = if class == Class::Char { 'c' } else { 's' };
                match ty {
                    None => false,
                    Some(ty) if ty == own => false,
                    Some('c') if class == Class::Bool => false,
                    Some(_) if self.integral() => true,
                    Some(ty) => return Err(format!("type {ty} does not suit a {}", name(class))),
                }
            }
            Class::Float => match ty {
                None | Some('e' | 'E' | 'f' | 'F' | 'g' | 'G') => true,
                Some(ty) => return Err(format!("type {ty} does not suit a floating-point number")),
            },
            Class::Str => match ty {
                None | Some('s') => false,
                Some(ty) => return Err(format!("type {ty} does not suit a string")),
            },
        };
        if !numeric && (self.sign != Sign::Negative || self.alternate || self.zero) {
            return Err(format!("sign, # and 0 do not suit a {} shown as such", name(class)));
        }
        if self.precision.is_some() && !matches!(class, Class::Float | Class::Str) {
            return Err(format!("a precision does not suit a {}", name(class)));
        }
        Ok(())
    }

    /// Writes `value` as this specification presents it; `check` has
    /// accepted its class.
    pub fn write(&self, out: &mut String, value: Value) {
        match value {
            Value::Integer(n) => self.integer(out, n),
            Value::Char(code) if self.integral() => self.integer(out, i128::from(code)),
            Value::Char(code) => self.pad(out, &char::from(code).to_string(), Align::Left),
            Value::Bool(b) if self.integral() => self.integer(out, i128::from(b)),
            Value::Bool(b) if self.ty == Some('c') => self.pad(out, &char::from(u8::from(b)).to_string(), Align::Left),
            Value::Bool(b) => self.pad(out, if b { "true" } else { "false" }, Align::Left),
            Value::F32(x) => self.float(out, f64::from(x), true),
            Value::F64(x) => self.float(out, x, false),
            Value::Str(text) => {
                let text = match self.precision {
                    Some(limit) => text.chars().take(limit).collect(),
                    None => text.to_owned(),
                };
                self.pad(out, &text, Align::Left);
            }
        }
    }

    fn sign_of(&self, negative: bool) -> &'static str {
        match (negative, self.sign) {
            (true, _) => "-",
            (false, Sign::Always) => "+",
            (false, Sign::Space) => " ",
            (false, Sign::Negative) => "",
        }
    }

    fn integer(&self, out: &mut String, n: i128) {
        let magnitude = n.unsigned_abs();
        let (prefix, digits) = match self.ty {
            Some('b') => ("0b", format!("{magnitude:b}")),
            Some('B') => ("0B", format!("{magnitude:b}")),
            Some('o') => (if magnitude == 0 { "" } else { "0" }, format!("{magnitude:o}")),
            Some('x') => ("0x", format!("{magnitude:x}")),
            Some('X') => ("0X", format!("{magnitude:X}")),
            _ => ("", magnitude.to_string()),
        };
        let prefix = if self.alternate { prefix } else { "" };
        let text = format!("{}{prefix}{digits}", self.sign_of(n < 0));
        let lead = self.sign_of(n < 0).len() + prefix.len();
        self.number(out, &text, lead, true);
    }

    fn float(&self, out: &mut String, x: f64, single: bool) {
        let upper = matches!(self.ty, Some('E' | 'F' | 'G'));
        let sign = self.sign_of(x.is_sign_negative());
        let magnitude = x.abs();
        let body = if magnitude.is_nan() {
            "nan".to_owned()
        } else if magnitude.is_infinite() {
            "inf".to_owned()
        } else {
            match (self.ty, self.precision) {
                (None, None) => shortest(magnitude, single, self.alternate),
                (None, Some(p)) => general(magnitude, p, self.alternate),
                (Some('e' | 'E'), p) => scientific(magnitude, p.unwrap_or(6), self.alternate),
                (Some('f' | 'F'), p) => fixed(magnitude, p.unwrap_or(6), self.alternate),
                (_, p) => general(magnitude, p.unwrap_or(6), self.alternate),
            }
        };
        let body = if upper { body.to_uppercase() } else { body };
        let text = format!("{sign}{body}");
        self.number(out, &text, sign.len(), x.is_finite());
    }

    /// Writes a number, `lead` bytes of sign and prefix then its digits:
    /// padded with zeros after them when the option `0` applies, else with
    /// the fill, aligned right unless told otherwise.
    fn number(&self, out: &mut String, text: &str, lead: usize, zeros: bool) {
        if self.zero && self.align.is_none() {
            let length = text.chars().count();
            out.push_str(&text[..lead]);
            if zeros {
                for _ in length..self.width {
                    out.push('0');
                }
                out.push_str(&text[lead..]);
            } else {
                // inf and nan take no zeros: the fill, right-aligned.
                let spaces = self.width.saturating_sub(length);
                out.truncate(out.len() - lead);
                for _ in 0..spaces {
                    out.push(' ');
                }
                out.push_str(text);
            }
        } else {
            self.pad(out, text, Align::Right);
        }
    }

    /// Writes `text` padded with the fill to the width, aligned as asked or
    /// else as `default`.
    fn pad(&self, out: &mut String, text: &str, default: Align) {
        let length = text.chars().count();
        let missing = self.width.saturating_sub(length);
        let (before, after) = match self.align.unwrap_or(default) {
            Align::Left => (0, missing),
            Align::Right => (missing, 0),
            Align::Center => (missing / 2, missing - missing / 2),
        };
        let fill = |out: &mut String, count: usize| {
            for _ in 0..count {
                out.push(self.fill);
            }
        };
        fill(out, before);
        out.push_str(text);
        fill(out, after);
    }
}

fn name(class: Class) -> &'static str {
    match class {
        Class::Integer => "integer",
        Class::Char => "char",
        Class::Bool => "bool",
        Class::Float => "floating-point number",
        Class::Str => "string",
    }
}

/// The digits of `x` in scientific notation and its exponent, from Rust's
/// rendering `d.ddde-5` or `de7`.
fn split_exponent(text: &str) -> (&str, i32) {
    let (mantissa, exponent) = text.split_once('e').expect("an exponent");
    (mantissa, exponent.parse().expect("a decimal exponent"))
}

/// The exponent as C writes it: a sign and at least two digits.
fn c_exponent(out: &mut String, exponent: i32) {
    let _ = write!(
        out,
        "e{}{:02}",
        if exponent < 0 { '-' } else { '+' },
        exponent.unsigned_abs()
    );
}

/// Adds a decimal point to a mantissa that has none, as `#` asks.
fn with_point(mantissa: &str, alternate: bool) -> String {
    if alternate && !mantissa.contains('.') {
        format!("{mantissa}.")
    } else {
        mantissa.to_owned()
    }
}

/// `x` in scientific notation with `precision` digits after the point,
/// rounded correctly: `std::to_chars` with `chars_format::scientific`.
fn scientific(x: f64, precision: usize, alternate: bool) -> String {
    let text = format!("{x:.precision$e}");
    let (mantissa, exponent) = split_exponent(&text);
    let mut out = with_point(mantissa, alternate);
    c_exponent(&mut out, exponent);
    out
}

/// `x` with `precision` digits after the point: `chars_format::fixed`.
fn fixed(x: f64, precision: usize, alternate: bool) -> String {
    with_point(&format!("{x:.precision$}"), alternate)
}

/// `x` with `precision` significant digits, in fixed or scientific notation
/// as its exponent decides, trailing zeros removed unless `alternate`:
/// `chars_format::general`, which is C's `%g`.
fn general(x: f64, precision: usize, alternate: bool) -> String {
    let precision = precision.max(1);
    let (_, exponent) = split_exponent(&format!("{x:.*e}", precision - 1));
    let text = if exponent < precision as i32 && exponent >= -4 {
        fixed(x, (precision as i32 - 1 - exponent) as usize, alternate)
    } else {
        scientific(x, precision - 1, alternate)
    };
    if alternate {
        return text;
    }
    let (mantissa, exponent) = match text.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (text.as_str(), None),
    };
    let mantissa = if mantissa.contains('.') {
        mantissa.trim_end_matches('0').trim_end_matches('.')
    } else {
        mantissa
    };
    match exponent {
        Some(exponent) => format!("{mantissa}e{exponent}"),
        None => mantissa.to_owned(),
    }
}

/// The shortest text that reads back as `x` (as a `float` when `single`), in
/// fixed notation unless scientific is shorter: `std::to_chars` without a
/// format, what `std::format` writes with neither type nor precision.
fn shortest(x: f64, single: bool, alternate: bool) -> String {
    let text = if single {
        format!("{:e}", x as f32)
    } else {
        format!("{x:e}")
    };
    let (mantissa, exponent) = split_exponent(&text);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let mut scientific = mantissa.to_owned();
    c_exponent(&mut scientific, exponent);
    let fixed = if exponent < 0 {
        format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize))
    } else {
        let whole = exponent as usize + 1;
        if digits.len() > whole {
            format!("{}.{}", &digits[..whole], &digits[whole..])
        } else {
            // An integer: its exact value, which may not end in zeros.
            format!("{x:.0}")
        }
    };
    // The shorter notation is chosen first; `#` then adds its point.
    if fixed.len() <= scientific.len() {
        with_point(&fixed, alternate)
    } else {
        let (mantissa, _) = split_exponent(&text);
        let mut out = with_point(mantissa, alternate);
        c_exponent(&mut out, exponent);
        out
    }
}

#[cfg(test)]
mod tests;
