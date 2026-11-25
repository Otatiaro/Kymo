use super::*;

fn precise_bus_fault() -> Fault {
    Fault {
        cfsr: (1 << 9) | (1 << 15),
        hfsr: 1 << 30,
        mmfar: 0xe000_ed34,
        bfar: 0x4000_0000,
        pc: 0x0800_0124,
        lr: 0x0800_0201,
        xpsr: 0x0100_0000,
        exc_return: 0xffff_fffd,
        ..Fault::default()
    }
}

#[test]
fn names_the_bits_set_cfsr_then_hfsr() {
    assert_eq!(precise_bus_fault().bits(), ["PRECISERR", "BFARVALID", "FORCED"]);
    let unnamed = Fault {
        cfsr: (1 << 2) | (1 << 25),
        hfsr: 1 << 5,
        ..Fault::default()
    };
    assert_eq!(unnamed.bits(), ["CFSR[2]", "DIVBYZERO", "HFSR[5]"]);
}

#[test]
fn a_fault_address_only_when_its_valid_bit_is_set() {
    assert_eq!(precise_bus_fault().addresses(), [("bfar", 0x4000_0000)]);
    let imprecise = Fault {
        cfsr: 1 << 10,
        ..precise_bus_fault()
    };
    assert!(imprecise.addresses().is_empty());
    let memory = Fault {
        cfsr: (1 << 1) | (1 << 7),
        ..precise_bus_fault()
    };
    assert_eq!(memory.addresses(), [("mmfar", 0xe000_ed34)]);
}

#[test]
fn the_stack_and_the_mode_from_exc_return() {
    let thread_psp = precise_bus_fault();
    assert_eq!(thread_psp.stack(), "psp");
    assert_eq!(thread_psp.interrupted(), None);
    let thread_msp = Fault {
        exc_return: 0xffff_fff9,
        ..precise_bus_fault()
    };
    assert_eq!(thread_msp.stack(), "msp");
    assert_eq!(thread_msp.interrupted(), None);
    let handler = Fault {
        exc_return: 0xffff_fff1,
        xpsr: 0x0100_0000 | 45,
        ..precise_bus_fault()
    };
    assert_eq!(handler.stack(), "msp");
    assert_eq!(handler.interrupted(), Some(45));
}

#[test]
fn a_frame_stacking_faulted_on_is_untrusted() {
    assert!(precise_bus_fault().trusted());
    for bit in [4, 12] {
        let fault = Fault {
            cfsr: 1 << bit,
            ..precise_bus_fault()
        };
        assert!(!fault.trusted(), "bit {bit}");
    }
}

#[test]
fn describes_without_symbols() {
    let text = precise_bus_fault().describe(&Symbols::default(), &Lines::default(), &Code::default());
    assert_eq!(
        text,
        "bits=PRECISERR,BFARVALID,FORCED bfar=0x40000000 pc=0x08000124 lr=0x08000201 stack=psp sp=0x00000000"
    );
    let untrusted = Fault {
        cfsr: 1 << 12,
        exc_return: 0xffff_fff9,
        ..precise_bus_fault()
    };
    assert_eq!(
        untrusted.describe(&Symbols::default(), &Lines::default(), &Code::default()),
        "bits=STKERR,FORCED pc=0x08000124 lr=0x08000201 stack=msp sp=0x00000000 frame=untrusted"
    );
}

#[test]
fn a_frame_outside_the_stack_bounds_when_known() {
    let fault = |sp, stack_base, stack_top| Fault {
        sp,
        stack_base,
        stack_top,
        ..precise_bus_fault()
    };
    assert_eq!(
        fault(0x2000_0100, 0x2000_0100, 0x2000_0200).outside_stack(),
        Some(false)
    );
    assert_eq!(
        fault(0x2000_01fc, 0x2000_0100, 0x2000_0200).outside_stack(),
        Some(false)
    );
    assert_eq!(fault(0x2000_00f8, 0x2000_0100, 0x2000_0200).outside_stack(), Some(true));
    assert_eq!(fault(0x2000_0200, 0x2000_0100, 0x2000_0200).outside_stack(), Some(true));
    // MSP: the base unknown; FreeRTOS: both.
    assert_eq!(fault(0x2000_00f8, 0, 0x2000_0200).outside_stack(), None);
    assert_eq!(fault(0x2000_00f8, 0, 0).outside_stack(), None);
    let text =
        fault(0x2000_00f8, 0x2000_0100, 0x2000_0200).describe(&Symbols::default(), &Lines::default(), &Code::default());
    assert!(text.ends_with(" sp=0x200000f8 frame=outside-stack"), "{text}");
}

/// Four functions, each calling the one below: `outer` at 0x100 calls
/// `middle` at 0x200 directly, which calls `inner` at 0x300 through r3,
/// which calls `leaf` at 0x400 directly; `leaf` faults at 0x404.
fn chain() -> (Symbols, Code) {
    let symbols = Symbols::of_functions(&[
        (0x100, 0x10, "outer"),
        (0x200, 0x10, "middle"),
        (0x300, 0x10, "inner"),
        (0x400, 0x10, "leaf"),
    ]);
    let code = Code::from_sections(vec![
        // 0x100: bl 0x200, then the return at 0x104.
        (
            0x100,
            [0xf000u16, 0xf87e]
                .iter()
                .flat_map(|halfword| halfword.to_le_bytes())
                .collect(),
        ),
        // 0x200: movs r0, r0; blx r3, then the return at 0x204.
        (
            0x200,
            [0x0000u16, 0x4798]
                .iter()
                .flat_map(|halfword| halfword.to_le_bytes())
                .collect(),
        ),
        // 0x300: bl 0x400, then the return at 0x304.
        (
            0x300,
            [0xf000u16, 0xf87e]
                .iter()
                .flat_map(|halfword| halfword.to_le_bytes())
                .collect(),
        ),
        (0x400, vec![0; 8]),
    ]);
    (symbols, code)
}

#[test]
fn levels_follow_calls_to_the_level_below() {
    let (symbols, code) = chain();
    let fault = Fault {
        pc: 0x404,
        lr: 0x305,
        // inner's lr pushed again, an odd word that follows no call, middle's,
        // outer's, then one more level.
        returns: [0x305, 0x403, 0x205, 0x105, 0x305, 0, 0, 0],
        ..Fault::default()
    };
    assert_eq!(fault.levels(&symbols, &code), [0x305, 0x205, 0x105]);
    let text = fault.describe(&symbols, &Lines::default(), &code);
    assert!(
        text.ends_with(" probable-callers=[inner+0x4, middle+0x4, outer+0x4]"),
        "{text}"
    );
}

#[test]
fn a_direct_call_to_another_function_is_not_a_level() {
    let (symbols, code) = chain();
    // lr follows inner's call to leaf, but pc is in middle.
    let fault = Fault {
        pc: 0x208,
        lr: 0x305,
        ..Fault::default()
    };
    assert!(fault.levels(&symbols, &code).is_empty());
    // An indirect call leads anywhere.
    let indirect = Fault {
        pc: 0x408,
        lr: 0x205,
        ..Fault::default()
    };
    assert_eq!(indirect.levels(&symbols, &code), [0x205]);
}

#[test]
fn the_layout_reads_each_word_where_the_event_puts_it() {
    use crate::events::{Field, Type};
    let word = Type {
        name: "uint32_t".to_owned(),
        size: Some(4),
        kind: Kind::Base(gimli::DW_ATE_unsigned),
    };
    let mut fields: Vec<Field> = WORDS
        .iter()
        .zip(0..)
        .map(|(name, n)| Field {
            name: (*name).to_owned(),
            offset: 4 * n,
            ty: word.clone(),
        })
        .collect();
    let returns = |length| Field {
        name: "returns".to_owned(),
        offset: 64,
        ty: Type {
            name: "std::array".to_owned(),
            size: Some(4 * length),
            kind: Kind::Array(Box::new(word.clone()), vec![length]),
        },
    };
    fields.push(returns(8));
    let event = Event {
        id: 0,
        name: EVENT.to_owned(),
        size: 96,
        fields: fields.clone(),
        text: None,
    };
    let payload: Vec<u8> = (1..=24u32).flat_map(u32::to_le_bytes).collect();
    let fault = Layout::of(&event).unwrap().read(&payload);
    assert_eq!((fault.cfsr, fault.pc, fault.exc_return), (1, 11, 13));
    assert_eq!((fault.sp, fault.stack_base, fault.stack_top), (14, 15, 16));
    assert_eq!(fault.returns, [17, 18, 19, 20, 21, 22, 23, 24]);
    // A word missing, a word of other size, returns of other length.
    let mut missing = event.clone();
    missing.fields.retain(|field| field.name != "stack_top");
    assert!(Layout::of(&missing).is_err());
    let mut narrow = event.clone();
    narrow.fields[0].ty.size = Some(2);
    assert!(Layout::of(&narrow).is_err());
    let mut short = event;
    *short.fields.last_mut().unwrap() = returns(4);
    assert!(Layout::of(&short).is_err());
}
