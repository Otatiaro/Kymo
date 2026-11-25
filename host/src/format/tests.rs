use super::*;

/// What format.rs writes for the case of a line of reference.txt, or `error`.
fn written(ty: &str, spec: &str, value: &str) -> String {
    let integer = || value.parse::<i128>().expect("an integer");
    let bits = || u64::from_str_radix(value.trim_start_matches("0x"), 16).expect("hexadecimal bits");
    let value = match ty {
        "f32" => Value::F32(f32::from_bits(bits() as u32)),
        "f64" => Value::F64(f64::from_bits(bits())),
        "bool" => Value::Bool(integer() != 0),
        "char" => Value::Char(integer() as u8),
        "str" => Value::Str(value),
        _ => Value::Integer(integer()),
    };
    let spec = match Spec::parse(spec) {
        Ok(spec) => spec,
        Err(_) => return "error".to_owned(),
    };
    if spec.check(value.class()).is_err() {
        return "error".to_owned();
    }
    let mut out = String::new();
    spec.write(&mut out, value);
    out
}

#[test]
fn every_case_reads_as_std_format_writes_it() {
    let reference = include_str!("reference.txt");
    let mut wrong = Vec::new();
    let mut count = 0;
    for line in reference.lines() {
        let line = line.trim_end_matches('\r');
        let mut columns = line.splitn(4, '\t');
        let (Some(ty), Some(spec), Some(value), Some(expected)) =
            (columns.next(), columns.next(), columns.next(), columns.next())
        else {
            panic!("a malformed line: {line:?}");
        };
        count += 1;
        let got = written(ty, spec, value);
        if got != expected {
            wrong.push(format!(
                "{ty} {{:{spec}}} {value}: std::format {expected:?}, Kymo {got:?}"
            ));
        }
    }
    assert!(count > 1000, "{count} cases");
    assert!(
        wrong.is_empty(),
        "{} of {count} cases differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn what_kymo_leaves_out_is_rejected() {
    for spec in ["L", "a", "A", "?", "{}", ".{}", "{0}", "x}", "10x5"] {
        assert!(Spec::parse(spec).is_err(), "{spec}");
    }
    assert!(Spec::parse("c").unwrap().check(Class::Integer).is_err());
    assert!(Spec::parse("c").unwrap().check(Class::Char).is_ok());
}
