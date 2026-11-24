//! The names of a Cortex-M's exceptions, by number, as IPSR gives it.
//!
//! The architecture names exceptions 1 to 15. A device interrupt, exception
//! 16 + n, is named by the enumeration of IRQ numbers its headers declare,
//! `IRQn_Type`: SvdGenerator's (OpSy's) and the vendors' CMSIS headers name
//! interrupt n `<NAME>_IRQn = n`, and the DWARF of an image built with -g
//! lists every enumerator of an enumeration it uses. An SVD file names the
//! interrupts the DWARF does not. Any other is `IRQ<n>`.
//!
//! The DWARF's `IRQn_Type` is an enumeration of that name, in any namespace,
//! or a typedef of that name to an enumeration (C's `typedef enum { … }`).
//! An image holding two different ones names no interrupt from either.

use std::collections::BTreeMap;
use std::path::Path;

use crate::target::Result;

/// The first exception number of the device interrupts.
const FIRST_IRQ: u16 = 16;

/// The exceptions the architecture names, ARMv6-M to ARMv8-M.
const ARCHITECTURE: [(u16, &str); 11] = [
    (1, "Reset"),
    (2, "NMI"),
    (3, "HardFault"),
    (4, "MemManage"),
    (5, "BusFault"),
    (6, "UsageFault"),
    (7, "SecureFault"),
    (11, "SVCall"),
    (12, "DebugMonitor"),
    (14, "PendSV"),
    (15, "SysTick"),
];

/// The suffix of an IRQ number's enumerator.
const IRQN: &str = "_IRQn";

/// The names of the device interrupts, by exception number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interrupts {
    names: BTreeMap<u16, String>,
}

impl Interrupts {
    /// The interrupts `enumerators` name, those of an `IRQn_Type`: `<NAME>_IRQn`
    /// for interrupt n is `<NAME>`. A negative enumerator, CMSIS's for an
    /// exception the architecture names, is left out; of several enumerators
    /// of one value, the first names the interrupt.
    pub fn from_enumerators(enumerators: &[(i64, String)]) -> Self {
        let mut names = BTreeMap::new();
        for (value, label) in enumerators {
            let Some(exception) = u16::try_from(*value).ok().and_then(|irq| irq.checked_add(FIRST_IRQ)) else {
                continue;
            };
            let name = label.strip_suffix(IRQN).unwrap_or(label);
            names.entry(exception).or_insert_with(|| name.to_owned());
        }
        Self { names }
    }

    /// The interrupts of the SVD file at `path`: each `<interrupt>`'s
    /// `<name>` for its `<value>`.
    pub fn from_svd(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse_svd(&text).map_err(|error| format!("{}: {error}", path.display()).into())
    }

    fn parse_svd(text: &str) -> Result<Self> {
        let document = roxmltree::Document::parse(text)?;
        let mut enumerators = Vec::new();
        for interrupt in document.descendants().filter(|node| node.has_tag_name("interrupt")) {
            let child = |tag: &str| {
                interrupt
                    .children()
                    .find(|node| node.has_tag_name(tag))
                    .and_then(|node| node.text())
                    .map(str::trim)
            };
            let (Some(name), Some(value)) = (child("name"), child("value")) else {
                return Err("an <interrupt> without a <name> and a <value>".into());
            };
            let value =
                parse_number(value).ok_or_else(|| format!("interrupt {name}: value {value} is not a number"))?;
            enumerators.push((value, name.to_owned()));
        }
        Ok(Self::from_enumerators(&enumerators))
    }

    /// Adds the interrupts `other` names and `self` does not.
    pub fn or(mut self, other: &Interrupts) -> Self {
        for (exception, name) in &other.names {
            self.names.entry(*exception).or_insert_with(|| name.clone());
        }
        self
    }

    /// The device interrupts named, by exception number.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &str)> {
        self.names.iter().map(|(exception, name)| (*exception, name.as_str()))
    }

    /// The name of exception `exception`.
    pub fn name(&self, exception: u16) -> String {
        if let Some(&(_, name)) = ARCHITECTURE.iter().find(|(number, _)| *number == exception) {
            return name.to_owned();
        }
        if exception < FIRST_IRQ {
            return format!("exception{exception}");
        }
        self.names
            .get(&exception)
            .cloned()
            .unwrap_or_else(|| format!("IRQ{}", exception - FIRST_IRQ))
    }
}

/// An SVD number: decimal, or hexadecimal after `0x`.
fn parse_number(text: &str) -> Option<i64> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => i64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enumerators(list: &[(i64, &str)]) -> Vec<(i64, String)> {
        list.iter()
            .map(|(value, label)| (*value, (*label).to_owned()))
            .collect()
    }

    #[test]
    fn the_architecture_names_its_exceptions() {
        let interrupts = Interrupts::default();
        assert_eq!(interrupts.name(2), "NMI");
        assert_eq!(interrupts.name(3), "HardFault");
        assert_eq!(interrupts.name(11), "SVCall");
        assert_eq!(interrupts.name(14), "PendSV");
        assert_eq!(interrupts.name(15), "SysTick");
        assert_eq!(interrupts.name(8), "exception8");
    }

    #[test]
    fn an_unnamed_interrupt_is_named_by_its_number() {
        assert_eq!(Interrupts::default().name(16 + 45), "IRQ45");
    }

    #[test]
    fn svdgenerator_names_interrupts_from_zero() {
        let interrupts = Interrupts::from_enumerators(&enumerators(&[(0, "WWDG_IRQn"), (45, "TIM2_IRQn")]));
        assert_eq!(interrupts.name(16), "WWDG");
        assert_eq!(interrupts.name(16 + 45), "TIM2");
        assert_eq!(interrupts.name(16 + 46), "IRQ46");
    }

    #[test]
    fn cmsis_exceptions_below_zero_are_the_architecture_s() {
        let interrupts = Interrupts::from_enumerators(&enumerators(&[
            (-14, "NonMaskableInt_IRQn"),
            (-1, "SysTick_IRQn"),
            (28, "TIM2_IRQn"),
            (28, "TIM2_alias_IRQn"),
        ]));
        assert_eq!(interrupts.name(2), "NMI");
        assert_eq!(interrupts.name(15), "SysTick");
        assert_eq!(interrupts.name(16 + 28), "TIM2");
    }

    #[test]
    fn an_svd_names_its_interrupts() {
        let svd = "<device><peripherals><peripheral><name>TIM2</name>\
                   <interrupt><name>TIM2</name><description>TIM2 global interrupt</description><value>45</value></interrupt>\
                   </peripheral><peripheral><name>TIM3</name>\
                   <interrupt><name>TIM3</name><value>0x2E</value></interrupt>\
                   </peripheral></peripherals></device>";
        let interrupts = Interrupts::parse_svd(svd).unwrap();
        assert_eq!(interrupts.name(16 + 45), "TIM2");
        assert_eq!(interrupts.name(16 + 46), "TIM3");
    }

    #[test]
    fn an_svd_interrupt_without_a_value_is_an_error() {
        assert!(Interrupts::parse_svd("<device><interrupt><name>TIM2</name></interrupt></device>").is_err());
    }

    #[test]
    fn the_dwarf_comes_before_the_svd() {
        let dwarf = Interrupts::from_enumerators(&enumerators(&[(45, "TIM2_IRQn")]));
        let svd = Interrupts::from_enumerators(&enumerators(&[(45, "TIMER2"), (46, "TIM3")]));
        let interrupts = dwarf.or(&svd);
        assert_eq!(interrupts.name(16 + 45), "TIM2");
        assert_eq!(interrupts.name(16 + 46), "TIM3");
    }
}
