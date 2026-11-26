use super::*;
use crate::decode::Decoded;
use crate::events::{Catalogue, Field};
use crate::interrupts::Interrupts;
use crate::stats::{self, Range};
use crate::symbols::Symbols;
use gimli::DwAte;

fn base(name: &str, size: u64, encoding: DwAte) -> Type {
    Type {
        name: name.to_owned(),
        size: Some(size),
        kind: Kind::Base(encoding),
    }
}

fn field(name: &str, offset: u64, ty: Type) -> Field {
    Field {
        name: name.to_owned(),
        offset,
        ty,
    }
}

/// `app::setpoint {int32_t x; uint16_t y; bool on; float f;}`, a duration's
/// begin with a numeric key, and `app::tick` with no field.
fn catalogue() -> Catalogue {
    let setpoint = vec![
        field("x", 0, base("int32_t", 4, gimli::DW_ATE_signed)),
        field("y", 4, base("uint16_t", 2, gimli::DW_ATE_unsigned)),
        field("on", 6, base("bool", 1, gimli::DW_ATE_boolean)),
        field("f", 8, base("float", 4, gimli::DW_ATE_float)),
    ];
    let key = vec![field("kymo_key", 0, base("uint32_t", 4, gimli::DW_ATE_unsigned))];
    let events = [
        ("app::setpoint", 12, setpoint),
        ("kymo::keyed_begin_of<app::batch>", 4, key),
        ("app::tick", 0, Vec::new()),
    ]
    .into_iter()
    .zip(0..)
    .map(|((name, size, fields), id)| {
        let event = Event {
            id,
            name: name.to_owned(),
            size,
            fields,
            text: None,
        };
        (id, event)
    })
    .collect();
    Catalogue {
        events,
        rings: Vec::new(),
        symbols: Symbols::default(),
        interrupts: Interrupts::default(),
        lines: Default::default(),
        code: Default::default(),
        warnings: Vec::new(),
    }
}

fn setpoint(x: i32, y: u16, f: f32) -> Vec<u8> {
    let mut payload = x.to_le_bytes().to_vec();
    payload.extend(y.to_le_bytes());
    payload.extend([1, 0]);
    payload.extend(f.to_le_bytes());
    payload
}

/// A trace of `steps` from index 0, 10 cycles apart: a setpoint's payload,
/// or `None` for a suspect over the next index.
fn trace<'a>(catalogue: &'a Catalogue, steps: &[Option<Vec<u8>>]) -> Trace<'a> {
    let event = &catalogue.events[&0];
    let mut elements = Vec::new();
    let mut index = 0;
    for (n, step) in steps.iter().enumerate() {
        match step {
            None => elements.push(Element::Suspect {
                from: index,
                to: index + 1,
                writers: 1,
            }),
            Some(payload) => {
                elements.push(Element::Event {
                    event: Decoded {
                        index,
                        timestamp: 0,
                        event,
                        payload: payload.clone(),
                    },
                    cycles: 10 * n as i64,
                    seconds: None,
                });
                index += 1;
            }
        }
    }
    Trace {
        first: 0,
        elements,
        end: Some(index),
        cut: None,
        frequency: None,
        measured: None,
        watched: None,
    }
}

#[test]
fn integers_and_floats_are_numeric_and_nothing_else() {
    let types = [
        ("int8_t", 1, gimli::DW_ATE_signed, Some(Reading::Signed(1))),
        ("signed char", 1, gimli::DW_ATE_signed_char, Some(Reading::Signed(1))),
        ("uint8_t", 1, gimli::DW_ATE_unsigned_char, Some(Reading::Unsigned(1))),
        ("int16_t", 2, gimli::DW_ATE_signed, Some(Reading::Signed(2))),
        ("uint32_t", 4, gimli::DW_ATE_unsigned, Some(Reading::Unsigned(4))),
        ("int64_t", 8, gimli::DW_ATE_signed, Some(Reading::Signed(8))),
        ("float", 4, gimli::DW_ATE_float, Some(Reading::Float(4))),
        ("double", 8, gimli::DW_ATE_float, Some(Reading::Float(8))),
        ("char", 1, gimli::DW_ATE_signed_char, None),
        ("char", 1, gimli::DW_ATE_unsigned_char, None),
        ("bool", 1, gimli::DW_ATE_boolean, None),
        ("long double", 16, gimli::DW_ATE_float, None),
        ("__int128", 16, gimli::DW_ATE_signed, None),
        ("_Float16", 2, gimli::DW_ATE_float, None),
    ];
    for (name, size, encoding, reading) in types {
        assert_eq!(Reading::of(&base(name, size, encoding)), reading, "{name}");
    }
    let enumeration = Type {
        name: "mode".to_owned(),
        size: Some(4),
        kind: Kind::Enum(vec![(0, "off".to_owned())]),
    };
    assert_eq!(Reading::of(&enumeration), None);
}

#[test]
fn a_field_reads_as_its_type_says() {
    let bytes = [0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x80];
    assert_eq!(Reading::Signed(1).read(&bytes), Some(Number::Integer(-2)));
    assert_eq!(Reading::Unsigned(1).read(&bytes), Some(Number::Integer(0xFE)));
    assert_eq!(Reading::Signed(2).read(&bytes), Some(Number::Integer(-2)));
    assert_eq!(Reading::Unsigned(4).read(&bytes), Some(Number::Integer(0xFFFF_FFFE)));
    assert_eq!(
        Reading::Signed(8).read(&bytes),
        Some(Number::Integer(i128::from(i64::from_le_bytes(bytes))))
    );
    assert_eq!(
        Reading::Unsigned(8).read(&bytes),
        Some(Number::Integer(i128::from(u64::from_le_bytes(bytes))))
    );
    assert_eq!(
        Reading::Float(4).read(&(-2.5f32).to_le_bytes()),
        Some(Number::Float(-2.5))
    );
    assert_eq!(Reading::Float(8).read(&0.1f64.to_le_bytes()), Some(Number::Float(0.1)));
    assert_eq!(Reading::Unsigned(4).read(&bytes[..3]), None);
}

#[test]
fn the_curves_are_the_numeric_fields_of_the_application_events() {
    let catalogue = catalogue();
    let names: Vec<String> = curves(&catalogue).iter().map(Curve::to_string).collect();
    assert_eq!(names, ["app::setpoint.f", "app::setpoint.x", "app::setpoint.y"]);
}

#[test]
fn each_emission_samples_each_field_at_its_offset() {
    let catalogue = catalogue();
    let trace = trace(
        &catalogue,
        &[Some(setpoint(-7, 65535, 0.5)), Some(setpoint(3, 1, -1.25))],
    );
    let values = sample(&catalogue, &trace);
    let read: Vec<(String, Number, usize)> = values
        .samples
        .iter()
        .map(|sample| (sample.curve.to_string(), sample.value, sample.at.element))
        .collect();
    assert_eq!(
        read,
        [
            ("app::setpoint.x".to_owned(), Number::Integer(-7), 0),
            ("app::setpoint.y".to_owned(), Number::Integer(65535), 0),
            ("app::setpoint.f".to_owned(), Number::Float(0.5), 0),
            ("app::setpoint.x".to_owned(), Number::Integer(3), 1),
            ("app::setpoint.y".to_owned(), Number::Integer(1), 1),
            ("app::setpoint.f".to_owned(), Number::Float(-1.25), 1),
        ]
    );
}

#[test]
fn stats_count_the_samples_nan_apart_and_exclude_a_suspect() {
    let catalogue = catalogue();
    let steps = [
        Some(setpoint(5, 0, f32::NAN)),
        Some(setpoint(-3, 0, 2.0)),
        None,
        Some(setpoint(1000, 0, 1.0)),
        Some(setpoint(-3, 0, 4.0)),
        Some(setpoint(9, 0, 1.0)),
    ];
    let trace = trace(&catalogue, &steps);
    let values = sample(&catalogue, &trace);
    let report = stats::sampled(&trace, &values, Range::default());
    let x = report.iter().find(|sampled| sampled.curve.field.as_deref() == Some("x")).expect("x");
    assert_eq!((x.count, x.excluded_suspect, x.nan), (4, 1, 0));
    let spread = x.spread.as_ref().expect("a spread");
    assert_eq!(spread.min.value, Number::Integer(-3));
    assert_eq!(spread.min.index, 1, "the earliest of equal minima");
    assert_eq!(spread.max.value, Number::Integer(9));
    assert_eq!((spread.p50, spread.p90), (Number::Integer(-3), Number::Integer(9)));
    assert_eq!(spread.mean, (5.0 - 3.0 - 3.0 + 9.0) / 4.0);

    let f = report.iter().find(|sampled| sampled.curve.field.as_deref() == Some("f")).expect("f");
    assert_eq!((f.count, f.nan), (3, 1));
    let spread = f.spread.as_ref().expect("a spread");
    assert_eq!(
        (spread.min.value, spread.max.value),
        (Number::Float(1.0), Number::Float(4.0))
    );
    assert_eq!(spread.min.index, 4, "the earliest of equal minima");
    assert!((spread.mean - 7.0 / 3.0).abs() < 1e-12);
}

#[test]
fn stats_keep_the_samples_in_the_range() {
    let catalogue = catalogue();
    let steps: Vec<Option<Vec<u8>>> = (0..5).map(|n| Some(setpoint(n, 0, 0.0))).collect();
    let trace = trace(&catalogue, &steps);
    let values = sample(&catalogue, &trace);
    let range = Range {
        from: Some(10),
        to: Some(40),
    };
    let report = stats::sampled(&trace, &values, range);
    let x = report.iter().find(|sampled| sampled.curve.field.as_deref() == Some("x")).expect("x");
    let spread = x.spread.as_ref().expect("a spread");
    assert_eq!(x.count, 3);
    assert_eq!(
        (spread.min.value, spread.max.value),
        (Number::Integer(1), Number::Integer(3))
    );
}
