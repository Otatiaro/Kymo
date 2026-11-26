//! What a pointer field points at, named from the image: the function or the
//! static object of the ELF's symbol table that holds the address.
//!
//! A function pointer carries the Thumb bit, cleared before the lookup. A
//! pointer to an object is named by the object alone when it points at its
//! start, or at a base class of the object's type that is the pointer's own
//! type (a task control block inside a task, for instance); anywhere else
//! inside the object it is named by the object and the offset. A null
//! pointer, and an address outside every symbol, have no name. A name that
//! symbols at other addresses share, statics of different translation units,
//! is followed by the address: `name@0x20000100`.

use crate::events::Pointee;
use crate::values::Reading;
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A function or an object of the image.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Symbol {
    address: u64,
    size: u64,
    name: String,
    /// The base class subobjects of an object's type, by offset in the
    /// object and qualified name, bases of bases included.
    bases: Vec<(u64, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Symbols {
    functions: Vec<Symbol>,
    objects: Vec<Symbol>,
    /// How each static variable of a numeric type reads, by address.
    numbers: BTreeMap<u64, Reading>,
}

impl Symbols {
    /// The functions and objects of `file`, demangled; `bases` gives the base
    /// class subobjects of the objects, by address, as the DWARF describes
    /// them, and `numbers` how its numeric variables read.
    pub fn read(
        file: &object::File,
        bases: &BTreeMap<u64, Vec<(u64, String)>>,
        numbers: BTreeMap<u64, Reading>,
    ) -> Self {
        let thumb = matches!(file.architecture(), object::Architecture::Arm);
        let mut symbols = Self {
            numbers,
            ..Self::default()
        };
        for symbol in file.symbols() {
            let (Ok(name), size) = (symbol.name(), symbol.size()) else {
                continue;
            };
            if size == 0 || name.is_empty() {
                continue;
            }
            let (list, address) = match symbol.kind() {
                SymbolKind::Text if thumb => (&mut symbols.functions, symbol.address() & !1),
                SymbolKind::Text => (&mut symbols.functions, symbol.address()),
                SymbolKind::Data => (&mut symbols.objects, symbol.address()),
                _ => continue,
            };
            list.push(Symbol {
                address,
                size,
                name: demangle(name),
                bases: bases.get(&address).cloned().unwrap_or_default(),
            });
        }
        for list in [&mut symbols.functions, &mut symbols.objects] {
            list.sort_by_key(|symbol| symbol.address);
            tell_apart(list);
        }
        symbols
    }

    /// Symbols of `functions` alone, each its address, its size and its name.
    #[cfg(test)]
    pub(crate) fn of_functions(functions: &[(u64, u64, &str)]) -> Self {
        Self {
            functions: functions
                .iter()
                .map(|&(address, size, name)| Symbol {
                    address,
                    size,
                    name: name.to_owned(),
                    bases: Vec::new(),
                })
                .collect(),
            ..Self::default()
        }
    }

    /// Symbols of `objects` alone, each its address, its size, its name and
    /// how it reads.
    #[cfg(test)]
    pub(crate) fn of_numbers(objects: &[(u64, u64, &str, Reading)]) -> Self {
        Self {
            objects: objects
                .iter()
                .map(|&(address, size, name, _)| Symbol {
                    address,
                    size,
                    name: name.to_owned(),
                    bases: Vec::new(),
                })
                .collect(),
            numbers: objects
                .iter()
                .map(|&(address, _, _, reading)| (address, reading))
                .collect(),
            ..Self::default()
        }
    }

    /// How the numeric variable at `address` reads; none for another object.
    pub fn number(&self, address: u64) -> Option<Reading> {
        self.numbers.get(&address).copied()
    }

    /// The address of the function that holds `address`, a Thumb address or
    /// not.
    pub fn function(&self, address: u64) -> Option<u64> {
        let address = address & !1;
        let index = self
            .functions
            .partition_point(|symbol| symbol.address <= address)
            .checked_sub(1)?;
        let symbol = &self.functions[index];
        (address - symbol.address < symbol.size).then_some(symbol.address)
    }

    /// The name of what a pointer to `pointee` holding `address` points at;
    /// none for a null pointer, even where an image has a symbol at 0 (a
    /// Cortex-M vector table).
    pub fn name(&self, pointee: &Pointee, address: u64) -> Option<String> {
        if address == 0 {
            return None;
        }
        let (list, address) = match pointee {
            Pointee::Function => (&self.functions, address & !1),
            _ => (&self.objects, address),
        };
        let index = list
            .partition_point(|symbol| symbol.address <= address)
            .checked_sub(1)?;
        let symbol = &list[index];
        let offset = address - symbol.address;
        if offset >= symbol.size {
            return None;
        }
        let base = match pointee {
            Pointee::Record(name) => symbol.bases.iter().any(|(at, base)| *at == offset && base == name),
            _ => false,
        };
        Some(if offset == 0 || base {
            symbol.name.clone()
        } else {
            format!("{}+{offset:#x}", symbol.name)
        })
    }
}

/// Names a symbol whose name others at other addresses share, statics of
/// different translation units, by its name and its address: `name@0x...`.
fn tell_apart(list: &mut [Symbol]) {
    let mut addresses: HashMap<String, BTreeSet<u64>> = HashMap::new();
    for symbol in list.iter() {
        addresses.entry(symbol.name.clone()).or_default().insert(symbol.address);
    }
    for symbol in list.iter_mut() {
        if addresses[&symbol.name].len() > 1 {
            symbol.name = format!("{}@{:#010x}", symbol.name, symbol.address);
        }
    }
}

/// A symbol's name as the demangler writes it, without a function's
/// parameters or return type. A name that is not mangled stays as it is.
fn demangle(name: &str) -> String {
    let options = cpp_demangle::DemangleOptions::new().no_params().no_return_type();
    cpp_demangle::Symbol::new(name)
        .ok()
        .and_then(|symbol| symbol.demangle_with_options(&options).ok())
        .unwrap_or_else(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(address: u64, size: u64, name: &str, bases: &[(u64, &str)]) -> Symbol {
        Symbol {
            address,
            size,
            name: name.to_owned(),
            bases: bases.iter().map(|(at, base)| (*at, (*base).to_owned())).collect(),
        }
    }

    fn symbols() -> Symbols {
        Symbols {
            functions: vec![symbol(0x100, 4, "first", &[]), symbol(0x104, 2, "second", &[])],
            objects: vec![
                symbol(0, 0x40, "vectors", &[]),
                symbol(0x2000, 0x110, "task", &[(0, "storage"), (0x100, "control_block")]),
                symbol(0x2110, 8, "lock", &[]),
                symbol(0x2200, 8, "after_gap", &[]),
            ],
            numbers: BTreeMap::new(),
        }
    }

    #[test]
    fn names_as_demangled() {
        assert_eq!(demangle("_ZN12_GLOBAL__N_16originE"), "(anonymous namespace)::origin");
        assert_eq!(
            demangle("_ZN2os4taskILj256EE5entryEv"),
            "os::task<(unsigned int)256>::entry"
        );
        assert_eq!(demangle("_Z1fIiJcEEvv"), "f<int, char>");
        assert_eq!(demangle("timer_handler"), "timer_handler");
    }

    #[test]
    fn a_name_shared_at_other_addresses_takes_the_address() {
        let mut list = vec![
            symbol(0x2000, 4, "(anonymous namespace)::task", &[]),
            symbol(0x2004, 4, "unique", &[]),
            symbol(0x2008, 4, "(anonymous namespace)::task", &[]),
            symbol(0x200c, 4, "alias", &[]),
            symbol(0x200c, 4, "alias", &[]),
        ];
        tell_apart(&mut list);
        let names: Vec<&str> = list.iter().map(|symbol| symbol.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "(anonymous namespace)::task@0x00002000",
                "unique",
                "(anonymous namespace)::task@0x00002008",
                "alias",
                "alias"
            ]
        );
        let symbols = Symbols {
            functions: Vec::new(),
            objects: list,
            numbers: BTreeMap::new(),
        };
        assert_eq!(
            symbols.name(&Pointee::Other, 0x200a).as_deref(),
            Some("(anonymous namespace)::task@0x00002008+0x2")
        );
    }

    #[test]
    fn functions_by_their_thumb_address() {
        let symbols = symbols();
        assert_eq!(symbols.name(&Pointee::Function, 0x105).as_deref(), Some("second"));
        assert_eq!(symbols.name(&Pointee::Function, 0x101).as_deref(), Some("first"));
        assert_eq!(symbols.name(&Pointee::Function, 0x103).as_deref(), Some("first+0x2"));
        assert_eq!(symbols.name(&Pointee::Function, 0x107), None);
    }

    #[test]
    fn objects_by_start_or_base_of_the_pointee() {
        let symbols = symbols();
        let block = Pointee::Record("control_block".to_owned());
        assert_eq!(symbols.name(&block, 0x2100).as_deref(), Some("task"));
        assert_eq!(symbols.name(&Pointee::Other, 0x2100).as_deref(), Some("task+0x100"));
        assert_eq!(
            symbols.name(&Pointee::Record("lock".to_owned()), 0x2100).as_deref(),
            Some("task+0x100")
        );
        let storage = Pointee::Record("storage".to_owned());
        assert_eq!(symbols.name(&storage, 0x2100).as_deref(), Some("task+0x100"));
        assert_eq!(symbols.name(&block, 0x2110).as_deref(), Some("lock"));
        assert_eq!(symbols.name(&Pointee::Other, 0x2117).as_deref(), Some("lock+0x7"));
        assert_eq!(symbols.name(&Pointee::Other, 0x2118), None);
        assert_eq!(symbols.name(&Pointee::Other, 0x1fff), None);
        assert_eq!(symbols.name(&Pointee::Other, 0x2200).as_deref(), Some("after_gap"));
        assert_eq!(symbols.name(&Pointee::Other, 0x4).as_deref(), Some("vectors+0x4"));
        assert_eq!(symbols.name(&Pointee::Other, 0), None);
    }
}
