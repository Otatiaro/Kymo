//! A hardware fault, `kymo::fault` (target/include/kymo/fault.hpp), read as
//! the ARMv7-M and ARMv8-M architecture manuals name its registers: the bits
//! set in CFSR and HFSR, the fault address MMFAR or BFAR holds when its valid
//! bit says so, the stack the core stacked the frame on, and whether it could
//! stack it.

use crate::code::{Call, Code};
use crate::events::{Event, Kind, Pointee};
use crate::lines::Lines;
use crate::symbols::Symbols;
use crate::target::Result;

/// The words a `kymo::fault` holds before its return addresses, in its order.
pub(crate) const WORDS: [&str; 16] = [
    "cfsr",
    "hfsr",
    "mmfar",
    "bfar",
    "r0",
    "r1",
    "r2",
    "r3",
    "r12",
    "lr",
    "pc",
    "xpsr",
    "exc_return",
    "sp",
    "stack_base",
    "stack_top",
];

/// The words above the frame that may be return addresses, `returns`.
pub const RETURNS: usize = 8;

/// The qualified name of the event.
pub const EVENT: &str = "kymo::fault";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fault {
    pub cfsr: u32,
    pub hfsr: u32,
    pub mmfar: u32,
    pub bfar: u32,
    pub r0: u32,
    pub r1: u32,
    pub r2: u32,
    pub r3: u32,
    pub r12: u32,
    pub lr: u32,
    pub pc: u32,
    pub xpsr: u32,
    pub exc_return: u32,
    /// The frame's address.
    pub sp: u32,
    /// The lowest address of the stack the frame should be on, 0 when unknown.
    pub stack_base: u32,
    /// The address that stack starts from, 0 when unknown.
    pub stack_top: u32,
    /// Words above the frame that may be return addresses, 0 for none.
    pub returns: [u32; RETURNS],
}

/// Where each word of a `kymo::fault` is in its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    words: [usize; WORDS.len()],
    returns: usize,
}

impl Layout {
    /// The layout of `event`, a `kymo::fault`. A word missing, or not of 4
    /// bytes, is an error: host and target disagree.
    pub(crate) fn of(event: &Event) -> Result<Self> {
        let field = |wanted: &str| {
            event
                .fields
                .iter()
                .find(|field| field.name == wanted)
                .ok_or_else(|| format!("{}: no field {wanted}, which the host reads", event.name))
        };
        let mut words = [0; WORDS.len()];
        for (offset, name) in words.iter_mut().zip(WORDS) {
            let field = field(name)?;
            if field.ty.size != Some(4) {
                return Err(format!("{}: {name} is not a word", event.name).into());
            }
            *offset = field.offset as usize;
        }
        let returns = field("returns")?;
        match &returns.ty.kind {
            Kind::Array(element, lengths) if element.size == Some(4) && lengths[..] == [RETURNS as u64] => {}
            _ => return Err(format!("{}: returns is not {RETURNS} words", event.name).into()),
        }
        Ok(Self {
            words,
            returns: returns.offset as usize,
        })
    }

    /// The fault `payload` holds.
    pub(crate) fn read(&self, payload: &[u8]) -> Fault {
        let word = |offset: usize| {
            payload
                .get(offset..offset + 4)
                .map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        };
        let [
            cfsr,
            hfsr,
            mmfar,
            bfar,
            r0,
            r1,
            r2,
            r3,
            r12,
            lr,
            pc,
            xpsr,
            exc_return,
            sp,
            stack_base,
            stack_top,
        ] = self.words.map(word);
        Fault {
            cfsr,
            hfsr,
            mmfar,
            bfar,
            r0,
            r1,
            r2,
            r3,
            r12,
            lr,
            pc,
            xpsr,
            exc_return,
            sp,
            stack_base,
            stack_top,
            returns: std::array::from_fn(|n| word(self.returns + 4 * n)),
        }
    }
}

/// CFSR's bits: MMFSR, BFSR, then UFSR.
const CFSR: [(u32, &str); 20] = [
    (0, "IACCVIOL"),
    (1, "DACCVIOL"),
    (3, "MUNSTKERR"),
    (4, "MSTKERR"),
    (5, "MLSPERR"),
    (7, "MMARVALID"),
    (8, "IBUSERR"),
    (9, "PRECISERR"),
    (10, "IMPRECISERR"),
    (11, "UNSTKERR"),
    (12, "STKERR"),
    (13, "LSPERR"),
    (15, "BFARVALID"),
    (16, "UNDEFINSTR"),
    (17, "INVSTATE"),
    (18, "INVPC"),
    (19, "NOCP"),
    (20, "STKOF"),
    (24, "UNALIGNED"),
    (25, "DIVBYZERO"),
];
const HFSR: [(u32, &str); 3] = [(1, "VECTTBL"), (30, "FORCED"), (31, "DEBUGEVT")];

const MMARVALID: u32 = 1 << 7;
const MSTKERR: u32 = 1 << 4;
const STKERR: u32 = 1 << 12;
const BFARVALID: u32 = 1 << 15;
/// EXC_RETURN's SPSEL: the frame is on the process stack.
const SPSEL: u32 = 1 << 2;
/// EXC_RETURN's Mode: the fault came in thread mode.
const THREAD: u32 = 1 << 3;
/// xPSR's exception number, IPSR's.
const IPSR: u32 = 0x1ff;

impl Fault {
    /// The bits set in CFSR, then in HFSR, by name; a bit the architecture
    /// does not name as `CFSR[n]` or `HFSR[n]`.
    pub fn bits(&self) -> Vec<String> {
        let mut bits = Vec::new();
        for (register, value, named) in [("CFSR", self.cfsr, &CFSR[..]), ("HFSR", self.hfsr, &HFSR[..])] {
            for bit in (0..32).filter(|bit| value & (1 << bit) != 0) {
                bits.push(
                    named
                        .iter()
                        .find(|(at, _)| *at == bit)
                        .map_or_else(|| format!("{register}[{bit}]"), |(_, name)| (*name).to_owned()),
                );
            }
        }
        bits
    }

    /// The fault addresses their valid bits validate: MMFAR, then BFAR.
    pub fn addresses(&self) -> Vec<(&'static str, u32)> {
        let mut addresses = Vec::new();
        if self.cfsr & MMARVALID != 0 {
            addresses.push(("mmfar", self.mmfar));
        }
        if self.cfsr & BFARVALID != 0 {
            addresses.push(("bfar", self.bfar));
        }
        addresses
    }

    /// The stack the core stacked the frame on.
    pub fn stack(&self) -> &'static str {
        if self.exc_return & SPSEL != 0 { "psp" } else { "msp" }
    }

    /// Whether the core stacked the frame: not when stacking itself faulted
    /// (MSTKERR, STKERR), and the frame then holds what was found there.
    pub fn trusted(&self) -> bool {
        self.cfsr & (MSTKERR | STKERR) == 0
    }

    /// The exception the fault interrupted, xPSR's: none in thread mode.
    pub fn interrupted(&self) -> Option<u16> {
        (self.exc_return & THREAD == 0).then_some((self.xpsr & IPSR) as u16)
    }

    /// Whether the frame lies outside the stack bounds the event holds: an
    /// overflowed stack or a corrupt stack pointer. None when they are unknown.
    pub fn outside_stack(&self) -> Option<bool> {
        (self.stack_base != 0 && self.stack_top != 0).then_some(self.sp < self.stack_base || self.sp >= self.stack_top)
    }

    /// The first call levels above the faulting function, innermost first,
    /// each a return address: of the frame's lr, then of the words above the
    /// frame, those that follow a call in `code` and, when the call is
    /// direct, whose call leads to the function of the level below. An old
    /// return address left in the stack can pass: the levels are probable.
    pub fn levels(&self, symbols: &Symbols, code: &Code) -> Vec<u32> {
        let mut levels = Vec::new();
        let mut below = symbols.function(u64::from(self.pc));
        for candidate in std::iter::once(self.lr).chain(self.returns) {
            if levels.len() == LEVELS {
                break;
            }
            let kept = match code.call_before(u64::from(candidate)) {
                Some(Call::Direct(target)) => below == Some(target),
                Some(Call::Indirect) => true,
                None => false,
            };
            if kept {
                levels.push(candidate);
                below = symbols.function(u64::from(candidate));
            }
        }
        levels
    }

    /// The fault by name and value: its bits, the fault addresses valid, pc
    /// and lr each with its function and source line, the stack and the
    /// frame's address, `frame` `untrusted` for a frame the core could not
    /// stack and `outside-stack` for a frame outside the stack's bounds, and
    /// the probable call levels.
    pub fn facts(&self, symbols: &Symbols, lines: &Lines, code: &Code) -> Vec<(&'static str, String)> {
        let mut facts = vec![("bits", self.bits().join(","))];
        for (register, address) in self.addresses() {
            facts.push((register, format!("0x{address:08x}")));
        }
        let place = |address: u32| {
            let address = u64::from(address);
            symbols
                .name(&Pointee::Function, address)
                .map(|function| match lines.at(address) {
                    Some(line) => format!("{function} at {line}"),
                    None => function,
                })
        };
        for (register, address) in [("pc", self.pc), ("lr", self.lr)] {
            facts.push((
                register,
                match place(address) {
                    Some(place) => format!("0x{address:08x} ({place})"),
                    None => format!("0x{address:08x}"),
                },
            ));
        }
        facts.push(("stack", self.stack().to_owned()));
        facts.push(("sp", format!("0x{:08x}", self.sp)));
        if !self.trusted() {
            facts.push(("frame", "untrusted".to_owned()));
        }
        if self.outside_stack() == Some(true) {
            facts.push(("frame", "outside-stack".to_owned()));
        }
        let levels: Vec<String> = self
            .levels(symbols, code)
            .into_iter()
            .map(|address| place(address).unwrap_or_else(|| format!("0x{address:08x}")))
            .collect();
        if !levels.is_empty() {
            facts.push(("probable-callers", format!("[{}]", levels.join(", "))));
        }
        facts
    }

    /// The fault as `kymo decode --meaning` writes it after its context: its
    /// facts, each `name=value`.
    pub fn describe(&self, symbols: &Symbols, lines: &Lines, code: &Code) -> String {
        written(&self.facts(symbols, lines, code))
    }
}

/// `facts`, each `name=value`, separated by spaces.
pub fn written(facts: &[(&'static str, String)]) -> String {
    facts
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The call levels `levels` gives at most.
pub const LEVELS: usize = 3;

#[cfg(test)]
mod tests;
