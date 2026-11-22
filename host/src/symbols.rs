//! What a pointer field points at, named from the image: the function or the
//! static object of the ELF's symbol table that holds the address.
//!
//! A function pointer carries the Thumb bit, cleared before the lookup. A
//! pointer to an object is named by the object alone when it points at its
//! start, or at a base class of the object's type that is the pointer's own
//! type (a task control block inside a task, for instance); anywhere else
//! inside the object it is named by the object and the offset. A null
//! pointer, and an address outside every symbol, have no name.

use crate::events::Pointee;
use object::{Object, ObjectSymbol, SymbolKind};
use std::collections::BTreeMap;

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
}

impl Symbols {
    /// The functions and objects of `file`, demangled; `bases` gives the base
    /// class subobjects of the objects, by address, as the DWARF describes them.
    pub fn read(file: &object::File, bases: &BTreeMap<u64, Vec<(u64, String)>>) -> Self {
        let thumb = matches!(file.architecture(), object::Architecture::Arm);
        let mut symbols = Self::default();
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
        symbols.functions.sort_by_key(|symbol| symbol.address);
        symbols.objects.sort_by_key(|symbol| symbol.address);
        symbols
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

/// A symbol's name as C++ writes it, without a function's parameters, and
/// without spaces, which separate an event's fields: dropped after a comma,
/// `_` elsewhere (`(anonymous_namespace)::origin`). A name that is not
/// mangled stays as it is.
fn demangle(name: &str) -> String {
    let options = cpp_demangle::DemangleOptions::new().no_params().no_return_type();
    let demangled = cpp_demangle::Symbol::new(name)
        .ok()
        .and_then(|symbol| symbol.demangle_with_options(&options).ok())
        .unwrap_or_else(|| name.to_owned());
    demangled.replace(", ", ",").replace(' ', "_")
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
        }
    }

    #[test]
    fn names_without_spaces() {
        assert_eq!(demangle("_ZN12_GLOBAL__N_16originE"), "(anonymous_namespace)::origin");
        assert_eq!(
            demangle("_ZN2os4taskILj256EE5entryEv"),
            "os::task<(unsigned_int)256>::entry"
        );
        assert_eq!(demangle("_Z1fIiJcEEvv"), "f<int,char>");
        assert_eq!(demangle("timer_handler"), "timer_handler");
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
