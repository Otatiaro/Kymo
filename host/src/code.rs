//! The image's code, read for the call that precedes a return address.
//!
//! A Thumb return address follows the call that left it: a BL, 32 bits, whose
//! target is direct, or a BLX through a register, 16 bits. A BLX to an
//! immediate address would enter the ARM state, which M-profile cores do not
//! have: it is not a call there.

use object::{Object, ObjectSection, SectionKind};

/// A call instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// A BL, to the function at this address.
    Direct(u64),
    /// A BLX through a register.
    Indirect,
}

/// The executable sections of an image, by address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Code {
    sections: Vec<(u64, Vec<u8>)>,
}

impl Code {
    /// The executable sections of `file`.
    pub fn read(file: &object::File) -> Self {
        let mut sections: Vec<_> = file
            .sections()
            .filter(|section| section.kind() == SectionKind::Text)
            .filter_map(|section| Some((section.address(), section.data().ok()?.to_vec())))
            .filter(|(_, data)| !data.is_empty())
            .collect();
        sections.sort_by_key(|&(address, _)| address);
        Self { sections }
    }

    /// Code made of `sections`, each its address and its bytes.
    pub fn from_sections(mut sections: Vec<(u64, Vec<u8>)>) -> Self {
        sections.sort_by_key(|&(address, _)| address);
        Self { sections }
    }

    /// The halfword of code at `address`.
    fn halfword(&self, address: u64) -> Option<u16> {
        let index = self
            .sections
            .partition_point(|(start, _)| *start <= address)
            .checked_sub(1)?;
        let (start, data) = &self.sections[index];
        let at = usize::try_from(address - start).ok()?;
        Some(u16::from_le_bytes(data.get(at..at.checked_add(2)?)?.try_into().ok()?))
    }

    /// The call `address`, a return address with its Thumb bit, follows.
    pub fn call_before(&self, address: u64) -> Option<Call> {
        if address & 1 == 0 {
            return None;
        }
        let next = address - 1;
        let halfword = |back: u64| next.checked_sub(back).and_then(|at| self.halfword(at));
        if let (Some(first), Some(second)) = (halfword(4), halfword(2)) {
            // BL: 11110 S imm10, then 11 J1 1 J2 imm11.
            if first & 0xf800 == 0xf000 && second & 0xd000 == 0xd000 {
                let first = u64::from(first);
                let second = u64::from(second);
                let s = (first >> 10) & 1;
                let i1 = !((second >> 13) ^ s) & 1;
                let i2 = !((second >> 11) ^ s) & 1;
                let offset = (s << 24) | (i1 << 23) | (i2 << 22) | ((first & 0x3ff) << 12) | ((second & 0x7ff) << 1);
                // Sign-extended from 25 bits, from the instruction's PC: its address plus 4.
                let offset = (offset ^ (1 << 24)).wrapping_sub(1 << 24);
                return Some(Call::Direct(next.wrapping_add(offset) & 0xffff_ffff));
            }
        }
        // BLX Rm: 010001111 Rm 000.
        let last = halfword(2)?;
        (last & 0xff87 == 0x4780).then_some(Call::Indirect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(at: u64, halfwords: &[u16]) -> Code {
        Code::from_sections(vec![(
            at,
            halfwords.iter().flat_map(|halfword| halfword.to_le_bytes()).collect(),
        )])
    }

    #[test]
    fn a_bl_forward_and_backward() {
        // At 0x100: bl 0x200 (offset +0xfc from 0x104).
        let forward = code(0x100, &[0xf000, 0xf87e]);
        assert_eq!(forward.call_before(0x105), Some(Call::Direct(0x200)));
        // At 0x200: bl 0x100 (offset -0x104 from 0x204).
        let backward = code(0x200, &[0xf7ff, 0xff7e]);
        assert_eq!(backward.call_before(0x205), Some(Call::Direct(0x100)));
    }

    #[test]
    fn a_blx_through_a_register() {
        // At 0x100: movs r0, r0; blx r3.
        let indirect = code(0x100, &[0x0000, 0x4798]);
        assert_eq!(indirect.call_before(0x105), Some(Call::Indirect));
    }

    #[test]
    fn no_call_before() {
        // At 0x100: a BLX to an immediate address, then two movs.
        let other = code(0x100, &[0xf000, 0xe87c, 0x0000, 0x0000]);
        assert_eq!(other.call_before(0x105), None);
        assert_eq!(other.call_before(0x109), None);
        // An even address, an address outside the code, one at its start.
        let forward = code(0x100, &[0xf000, 0xf87e]);
        assert_eq!(forward.call_before(0x104), None);
        assert_eq!(forward.call_before(0x305), None);
        assert_eq!(forward.call_before(0x101), None);
    }
}
