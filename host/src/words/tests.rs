use super::*;
use crate::events::{Event, Field, Kind, Type};

fn base(name: &str, size: u64, encoding: gimli::DwAte) -> Type {
    Type {
        name: name.to_owned(),
        size: Some(size),
        kind: Kind::Base(encoding),
    }
}

/// `app::al_status` with `code` (uint16_t) at 0 and `delta` (int8_t) at 2.
fn event() -> Event {
    Event {
        id: 1,
        name: "app::al_status".to_owned(),
        size: 3,
        fields: vec![
            Field {
                name: "code".to_owned(),
                offset: 0,
                ty: base("uint16_t", 2, gimli::DW_ATE_unsigned),
            },
            Field {
                name: "delta".to_owned(),
                offset: 2,
                ty: base("int8_t", 1, gimli::DW_ATE_signed),
            },
        ],
        text: None,
    }
}

fn decoded(event: &Event, code: u16, delta: i8) -> Decoded<'_> {
    let mut payload = code.to_le_bytes().to_vec();
    payload.push(delta as u8);
    Decoded {
        index: 0,
        timestamp: 0,
        event,
        payload,
    }
}

fn keeps(text: &str, decoded: &Decoded) -> bool {
    Words::parse(text).unwrap().keeps(&Symbols::default(), decoded)
}

#[test]
fn a_plain_word_is_looked_for_in_the_type_and_the_fields() {
    let event = event();
    let decoded = decoded(&event, 26, -3);
    assert!(keeps("AL_STATUS", &decoded));
    assert!(keeps("code=26", &decoded));
    assert!(keeps("delta=-3", &decoded));
    assert!(!keeps("mutex", &decoded));
    assert!(keeps("mutex status", &decoded));
    assert!(!keeps("+mutex status", &decoded));
    assert!(!keeps("-status", &decoded));
}

#[test]
fn a_field_holds_its_value_in_decimal_or_hexadecimal() {
    let event = event();
    let decoded = decoded(&event, 0x1a, -3);
    assert!(keeps("code=0x1a", &decoded));
    assert!(keeps("code=0x1A", &decoded));
    assert!(keeps("code=26", &decoded));
    assert!(!keeps("code=25", &decoded));
    assert!(!keeps("code=0x1a0", &decoded));
    assert!(keeps("delta=-3", &decoded));
    // A signed field's bits are not its value.
    assert!(!keeps("delta=253", &decoded));
    assert!(!keeps("delta=0xfd", &decoded));
    // A field of another name does not hold the value.
    assert!(!keeps("other=26", &decoded));
    assert!(keeps("+code=26 -delta=0", &decoded));
    assert!(!keeps("+code=26 -delta=-3", &decoded));
}

#[test]
fn a_field_without_a_value_holds_for_an_event_that_has_it() {
    let event = event();
    let decoded = decoded(&event, 1, 1);
    assert!(keeps("code=", &decoded));
    assert!(!keeps("status=", &decoded));
    assert!(!keeps("-code=", &decoded));
}

#[test]
fn a_value_that_is_not_an_integer_is_an_error() {
    for text in ["code=x", "code=0x", "code=1.5", "+code=-", "code=0xffffffffffffffffff"] {
        assert!(Words::parse(text).is_err(), "{text}");
    }
    assert!(Words::parse("=5").is_ok_and(|words| words.active()));
}

#[test]
fn a_sign_alone_or_no_word_leaves_out_nothing() {
    let event = event();
    for text in ["", "  ", "-", "+", "- +"] {
        let words = Words::parse(text).unwrap();
        assert!(!words.active(), "{text:?}");
        assert!(keeps(text, &decoded(&event, 0, 0)), "{text:?}");
    }
}
