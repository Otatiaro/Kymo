use super::*;
use crate::events::Pointee;

fn base(name: &str, encoding: gimli::DwAte, size: u64) -> Type {
    Type {
        name: name.to_owned(),
        size: Some(size),
        kind: Kind::Base(encoding),
    }
}

/// An event's fields, one after the other, and a payload holding `bytes` for each.
fn event(fields: &[(&str, Type, &[u8])]) -> (Vec<Field>, Vec<u8>) {
    let mut payload = Vec::new();
    let fields = fields
        .iter()
        .map(|(name, ty, bytes)| {
            let field = Field {
                name: (*name).to_owned(),
                offset: payload.len() as u64,
                ty: ty.clone(),
            };
            payload.extend_from_slice(bytes);
            field
        })
        .collect();
    (fields, payload)
}

fn state() -> Type {
    Type {
        name: "app::state".to_owned(),
        size: Some(1),
        kind: Kind::Enum(vec![(0, "IDLE".to_owned()), (1, "RUN".to_owned())]),
    }
}

fn written(template: &str, fields: &[Field], payload: &[u8]) -> String {
    let text = Text::parse(template, fields).unwrap();
    let mut out = String::new();
    text.write(&mut out, fields, payload);
    out
}

#[test]
fn writes_each_kind_of_field() {
    let name = Type {
        name: "char[6]".to_owned(),
        size: Some(6),
        kind: Kind::Array(Box::new(base("char", gimli::DW_ATE_signed_char, 1)), vec![6]),
    };
    let (fields, payload) = event(&[
        ("delta", base("int", gimli::DW_ATE_signed, 4), &(-42i32).to_le_bytes()),
        (
            "count",
            base("unsigned int", gimli::DW_ATE_unsigned, 4),
            &u32::MAX.to_le_bytes(),
        ),
        ("small", base("signed char", gimli::DW_ATE_signed_char, 1), &[0xff]),
        ("letter", base("char", gimli::DW_ATE_signed_char, 1), b"K"),
        ("ok", base("bool", gimli::DW_ATE_boolean, 1), &[1]),
        ("ratio", base("float", gimli::DW_ATE_float, 4), &0.1f32.to_le_bytes()),
        ("volts", base("double", gimli::DW_ATE_float, 8), &3.3f64.to_le_bytes()),
        ("from", state(), &[1]),
        ("to", state(), &[7]),
        ("name", name, b"ab\x01\0zz"),
    ]);
    assert_eq!(
        written(
            "{delta} {count:#x} {small} {letter} {ok} {ratio} {volts:.2f} {from}->{to} [{name:>6}]",
            &fields,
            &payload
        ),
        "-42 0xffffffff -1 K true 0.1 3.30 RUN->7 [ab\\x01]"
    );
}

#[test]
fn braces_double_and_literals_stay() {
    let (fields, payload) = event(&[("x", base("int", gimli::DW_ATE_signed, 4), &7i32.to_le_bytes())]);
    assert_eq!(written("{{x}} = {x}}}", &fields, &payload), "{x} = 7}");
    assert_eq!(written("no field", &fields, &payload), "no field");
    assert_eq!(written("", &fields, &payload), "");
}

#[test]
fn rejects_what_does_not_suit_the_fields() {
    let pointer = Type {
        name: "void*".to_owned(),
        size: Some(4),
        kind: Kind::Pointer(Pointee::Other),
    };
    let (fields, _) = event(&[
        ("x", base("int", gimli::DW_ATE_signed, 4), &[0; 4]),
        ("v", base("float", gimli::DW_ATE_float, 4), &[0; 4]),
        ("s", state(), &[0]),
        ("p", pointer, &[0; 4]),
    ]);
    for (template, error) in [
        ("{y}", "no field y"),
        ("{}", "named, not numbered"),
        ("{0}", "no field 0"),
        ("{x", "not closed"),
        ("x}", "lone }"),
        ("{x:f}", ""),
        ("{x:c}", ""),
        ("{v:x}", ""),
        ("{s:d}", ""),
        ("{x:L}", ""),
        ("{p}", "cannot be written"),
    ] {
        let result = Text::parse(template, &fields);
        assert!(
            result.as_ref().is_err_and(|got| got.contains(error)),
            "{template}: {result:?}"
        );
    }
}
