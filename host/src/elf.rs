//! What the host needs from the target's ELF: the addresses and sizes of the
//! symbols it reads, so nothing about the target is configured by hand.

use crate::target::Result;
use object::{Object, ObjectSymbol};
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct Symbol {
    pub address: u32,
    pub size: u32,
}

pub struct Elf {
    symbols: Vec<(String, Symbol)>,
}

impl Elf {
    pub fn read(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let file = object::File::parse(&*data)?;
        let symbols = file
            .symbols()
            .filter_map(|symbol| {
                let name = symbol.name().ok()?.to_owned();
                let address = u32::try_from(symbol.address()).ok()?;
                let size = u32::try_from(symbol.size()).ok()?;
                Some((name, Symbol { address, size }))
            })
            .collect();
        Ok(Self { symbols })
    }

    pub fn symbol(&self, name: &str) -> Result<Symbol> {
        self.symbols
            .iter()
            .find(|(symbol, _)| symbol == name)
            .map(|(_, symbol)| *symbol)
            .ok_or_else(|| format!("symbol {name} not in the ELF").into())
    }
}
