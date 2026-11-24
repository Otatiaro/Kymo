use super::*;
use std::collections::BTreeMap;

/// A JSON value, its numbers kept as written.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub fn parse(text: &str) -> Json {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = parser.value();
        parser.space();
        assert_eq!(parser.at, text.len(), "trailing text after the JSON value");
        value
    }

    pub fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(members) => members.get(key).unwrap_or_else(|| panic!("no member {key}")),
            _ => panic!("{key}: not an object"),
        }
    }

    pub fn number(&self) -> f64 {
        match self {
            Json::Number(number) => number.parse().unwrap(),
            _ => panic!("not a number: {self:?}"),
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) {
        self.space();
        assert_eq!(
            self.bytes.get(self.at),
            Some(&byte),
            "expected {} at {}",
            byte as char,
            self.at
        );
        self.at += 1;
    }

    fn literal(&mut self, word: &str, value: Json) -> Json {
        assert!(
            self.bytes[self.at..].starts_with(word.as_bytes()),
            "bad literal at {}",
            self.at
        );
        self.at += word.len();
        value
    }

    fn value(&mut self) -> Json {
        self.space();
        match self.bytes[self.at] {
            b'n' => self.literal("null", Json::Null),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'"' => Json::String(self.string()),
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.bytes[self.at] == b']' {
                    self.at += 1;
                    return Json::Array(items);
                }
                loop {
                    items.push(self.value());
                    self.space();
                    self.at += 1;
                    match self.bytes[self.at - 1] {
                        b',' => continue,
                        b']' => return Json::Array(items),
                        other => panic!("unexpected {} in an array", other as char),
                    }
                }
            }
            b'{' => {
                self.at += 1;
                let mut members = BTreeMap::new();
                self.space();
                if self.bytes[self.at] == b'}' {
                    self.at += 1;
                    return Json::Object(members);
                }
                loop {
                    self.space();
                    let key = self.string();
                    self.eat(b':');
                    let value = self.value();
                    assert!(members.insert(key.clone(), value).is_none(), "duplicate member {key}");
                    self.space();
                    self.at += 1;
                    match self.bytes[self.at - 1] {
                        b',' => continue,
                        b'}' => return Json::Object(members),
                        other => panic!("unexpected {} in an object", other as char),
                    }
                }
            }
            _ => {
                let start = self.at;
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|byte| byte.is_ascii_digit() || b"+-.eE".contains(byte))
                {
                    self.at += 1;
                }
                let number = std::str::from_utf8(&self.bytes[start..self.at]).unwrap().to_owned();
                let grammar = number.strip_prefix('-').unwrap_or(&number);
                assert!(
                    grammar.starts_with(|c: char| c.is_ascii_digit())
                        && !(grammar.len() > 1 && grammar.starts_with('0') && grammar.as_bytes()[1].is_ascii_digit())
                        && number.parse::<f64>().is_ok_and(f64::is_finite),
                    "not a JSON number: {number}"
                );
                Json::Number(number)
            }
        }
    }

    fn string(&mut self) -> String {
        assert_eq!(self.bytes[self.at], b'"');
        self.at += 1;
        let mut out = String::new();
        loop {
            let rest = std::str::from_utf8(&self.bytes[self.at..]).unwrap();
            let character = rest.chars().next().expect("an unterminated string");
            self.at += character.len_utf8();
            match character {
                '"' => return out,
                '\\' => {
                    let escape = self.bytes[self.at];
                    self.at += 1;
                    out.push(match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'u' => {
                            let hex = std::str::from_utf8(&self.bytes[self.at..self.at + 4]).unwrap();
                            self.at += 4;
                            char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap()
                        }
                        other => panic!("bad escape \\{}", other as char),
                    });
                }
                character if (character as u32) < 0x20 => panic!("a raw control character in a string"),
                character => out.push(character),
            }
        }
    }
}

/// Where `value` breaks `schema`, the subset of JSON Schema the schemas of
/// host/schema use; a keyword out of it fails.
pub fn violations(schema: &Json, root: &Json, value: &Json, at: &str, out: &mut Vec<String>) {
    let Json::Object(keywords) = schema else {
        return out.push(format!("{at}: the schema is no object"));
    };
    for (keyword, argument) in keywords {
        match keyword.as_str() {
            "$schema" | "title" | "description" | "$defs" => {}
            "$ref" => {
                let Json::String(reference) = argument else {
                    return out.push(format!("{at}: $ref is no string"));
                };
                let target = reference
                    .strip_prefix("#/")
                    .expect("a local reference")
                    .split('/')
                    .fold(root, |schema, key| schema.get(key));
                violations(target, root, value, at, out);
            }
            "type" => {
                let types: Vec<&Json> = match argument {
                    Json::Array(types) => types.iter().collect(),
                    one => vec![one],
                };
                let fits = types.iter().any(|ty| match (ty, value) {
                    (Json::String(ty), value) => match (ty.as_str(), value) {
                        ("null", Json::Null)
                        | ("boolean", Json::Bool(_))
                        | ("string", Json::String(_))
                        | ("array", Json::Array(_))
                        | ("object", Json::Object(_))
                        | ("number", Json::Number(_)) => true,
                        ("integer", Json::Number(number)) => !number.contains(['.', 'e', 'E']),
                        _ => false,
                    },
                    _ => false,
                });
                if !fits {
                    out.push(format!("{at}: {value:?} is not of type {argument:?}"));
                }
            }
            "const" => {
                let equal = match (argument, value) {
                    (Json::Number(a), Json::Number(b)) => a.parse::<f64>().ok() == b.parse::<f64>().ok(),
                    (a, b) => a == b,
                };
                if !equal {
                    out.push(format!("{at}: {value:?} is not {argument:?}"));
                }
            }
            "enum" => {
                let Json::Array(choices) = argument else {
                    return out.push(format!("{at}: enum is no array"));
                };
                if !choices.contains(value) {
                    out.push(format!("{at}: {value:?} is none of {argument:?}"));
                }
            }
            "minimum" => {
                if let Json::Number(_) = value
                    && value.number() < argument.number()
                {
                    out.push(format!("{at}: {value:?} is below {argument:?}"));
                }
            }
            "anyOf" => {
                let Json::Array(choices) = argument else {
                    return out.push(format!("{at}: anyOf is no array"));
                };
                let fits = choices.iter().any(|choice| {
                    let mut broken = Vec::new();
                    violations(choice, root, value, at, &mut broken);
                    broken.is_empty()
                });
                if !fits {
                    out.push(format!("{at}: {value:?} fits none of anyOf"));
                }
            }
            "required" => {
                let (Json::Array(names), Json::Object(members)) = (argument, value) else {
                    continue;
                };
                for name in names {
                    if let Json::String(name) = name
                        && !members.contains_key(name)
                    {
                        out.push(format!("{at}: no member {name}"));
                    }
                }
            }
            "properties" => {
                let (Json::Object(properties), Json::Object(members)) = (argument, value) else {
                    continue;
                };
                for (name, member) in members {
                    if let Some(property) = properties.get(name) {
                        violations(property, root, member, &format!("{at}/{name}"), out);
                    }
                }
            }
            "additionalProperties" => {
                let Json::Object(members) = value else {
                    continue;
                };
                assert_eq!(argument, &Json::Bool(false), "only additionalProperties false");
                let known = match keywords.get("properties") {
                    Some(Json::Object(properties)) => properties.keys().collect(),
                    _ => Vec::new(),
                };
                for name in members.keys().filter(|name| !known.contains(name)) {
                    out.push(format!("{at}: member {name} is not in the schema"));
                }
            }
            "items" => {
                if let Json::Array(items) = value {
                    for (n, item) in items.iter().enumerate() {
                        violations(argument, root, item, &format!("{at}/{n}"), out);
                    }
                }
            }
            unknown => out.push(format!("{at}: keyword {unknown} is not checked")),
        }
    }
}

pub const SCHEMA: &str = include_str!("../../schema/stats.schema.json");

/// Fails unless `json` fits host/schema/stats.schema.json.
pub fn check_schema(json: &str) {
    fits_schema(SCHEMA, json);
}

/// Fails unless `json` fits `schema`.
pub fn fits_schema(schema: &str, json: &str) {
    let schema = Json::parse(schema);
    let mut broken = Vec::new();
    violations(&schema, &schema, &Json::parse(json), "", &mut broken);
    assert!(
        broken.is_empty(),
        "{json}\ndoes not fit the schema:\n{}",
        broken.join("\n")
    );
}

#[test]
fn percentiles_take_the_nearest_rank() {
    assert_eq!(
        [percentile(&[7], 50), percentile(&[7], 90), percentile(&[7], 99)],
        [7, 7, 7]
    );
    assert_eq!(
        [
            percentile(&[1, 2], 50),
            percentile(&[1, 2], 90),
            percentile(&[1, 2], 99)
        ],
        [1, 2, 2]
    );
    let hundred: Vec<i64> = (1..=100).collect();
    assert_eq!(
        [
            percentile(&hundred, 50),
            percentile(&hundred, 90),
            percentile(&hundred, 99)
        ],
        [50, 90, 99]
    );
    let hundred_and_one: Vec<i64> = (1..=101).collect();
    assert_eq!(
        [
            percentile(&hundred_and_one, 50),
            percentile(&hundred_and_one, 90),
            percentile(&hundred_and_one, 99)
        ],
        [51, 91, 100]
    );
}

#[test]
fn a_histogram_has_at_most_32_bins_on_multiples_of_its_width() {
    let fits = |values: &[i64], width, first, counts: &[u64]| {
        let histogram = histogram(values);
        assert_eq!(
            (histogram.width, histogram.first, histogram.counts.as_slice()),
            (width, first, counts),
            "{values:?}"
        );
    };
    fits(&[100, 100], 1, 100, &[2]);
    let thirty_two: Vec<i64> = (0..32).collect();
    fits(&thirty_two, 1, 0, &[1; 32]);
    let thirty_three: Vec<i64> = (0..33).collect();
    let mut pairs = vec![2; 16];
    pairs.push(1);
    fits(&thirty_three, 2, 0, &pairs);
    fits(&[3, 7, 149], 5, 0, &{
        let mut counts = vec![0; 30];
        counts[0] = 1;
        counts[1] = 1;
        counts[29] = 1;
        counts
    });
    fits(&[-3, 4], 1, -3, &[1, 0, 0, 0, 0, 0, 0, 1]);
    fits(&[1_000, 99_000], 5_000, 0, &{
        let mut counts = vec![0; 20];
        counts[0] = 1;
        counts[19] = 1;
        counts
    });
}

#[test]
fn the_parser_and_the_validator_reject_what_they_should() {
    let schema = Json::parse(SCHEMA);
    let point = r#"{"value":1,"index":0,"cycles":5,"seconds":null}"#;
    let report = format!(
        r#"{{"schema":"kymo-stats","version":1,"frequency":null,"from":0,"to":10,"origin":0,"known":10,"joined":null,
        "losses":0,"suspects":0,"contradictions":0,"groups":[{{"kind":"task","subject":"a","metric":"net","detail":null,"count":1,
        "excluded_loss":0,"excluded_suspect":0,"cut":0,"lower_bound":false,"jitter":null,"share":0.1,"distribution":
        {{"min":{point},"p50":1,"p90":1,"p99":1,"max":{point},"mean":1.0,"total":1,"histogram":{{"width":1,"first":1,
        "counts":[1]}},"worst":[{point}]}}}}]}}"#
    );
    check_schema(&report);
    let broken = |json: &str| {
        let mut out = Vec::new();
        violations(&schema, &schema, &Json::parse(json), "", &mut out);
        out
    };
    for (from, to) in [
        (r#""version":1"#, r#""version":2"#),
        (r#""known":10"#, r#""known":-1"#),
        (r#""joined":null"#, r#""joined":0"#),
        (r#""joined":null"#, r#""joined":"1""#),
        (r#""from":0"#, r#""from":0.5"#),
        (r#""detail":null"#, r#""detail":3"#),
        (r#""count":1,"#, ""),
        (r#""seconds":null}"#, r#""seconds":null,"extra":1}"#),
        (r#""distribution":"#, r#""distribution":7,"unused":"#),
    ] {
        assert!(report.contains(from), "{from}");
        assert!(
            !broken(&report.replacen(from, to, 1)).is_empty(),
            "{from} -> {to} passes"
        );
    }
    // A schema keyword the validator does not check is a failure, not a pass.
    let mut out = Vec::new();
    violations(&Json::parse(r#"{"pattern":"x"}"#), &schema, &Json::Null, "", &mut out);
    assert!(!out.is_empty());
    for bad in [r#"{"a":01}"#, r#"{"a":1,}"#, r#"{"a":1}x"#, "[\"\u{1}\"]"] {
        assert!(std::panic::catch_unwind(|| Json::parse(bad)).is_err(), "{bad} parses");
    }
}

#[test]
fn every_member_the_output_writes_is_in_the_schema() {
    // Remove each required member from the schema in turn: the output then
    // has a member the schema does not know, and fails.
    let schema = Json::parse(SCHEMA);
    let Json::Object(top) = &schema else { unreachable!() };
    let Json::Array(required) = &top["required"] else {
        unreachable!()
    };
    let report = r#"{"schema":"kymo-stats","version":1,"frequency":null,"from":0,"to":10,"origin":0,"known":10,
        "joined":12,"losses":0,"suspects":0,"contradictions":0,"groups":[]}"#;
    for name in required {
        let Json::String(name) = name else { unreachable!() };
        let mut cut = top.clone();
        let Some(Json::Object(properties)) = cut.get_mut("properties") else {
            unreachable!()
        };
        properties.remove(name);
        let cut = Json::Object(cut);
        let mut out = Vec::new();
        violations(&cut, &cut, &Json::parse(report), "", &mut out);
        assert!(
            !out.is_empty(),
            "{name} removed from the schema, and the output still fits"
        );
    }
}
