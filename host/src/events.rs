//! The catalogue of event types, read from the target's ELF: each type id the
//! linker assigned, the type it stands for, and that type's layout from the
//! DWARF; and every trace ring, each static object of type `kymo::ring<N>`.
//! Nothing about an event is known to the host in advance.
//!
//! A type id is the offset of a symbol `kymo::anchor<Event>::name.kymo_id` in
//! `.kymo.ids` (target/include/kymo/type_id.hpp). The DWARF has no trace of
//! the anchor, so the type is found by name: the symbol is demangled, and the
//! structure of the same qualified name is looked up in the debug info.

use crate::interrupts::Interrupts;
use crate::symbols::Symbols;
use crate::target::Result;
use gimli::{AttributeValue, DebugInfoOffset, DwAte, EndianSlice, Operation, RunTimeEndian, UnitOffset};
use object::{Object, ObjectSection, ObjectSymbol};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const ID_SUFFIX: &str = ".kymo_id";
const ANCHOR_PREFIX: &str = "kymo::anchor<";
const ANCHOR_SUFFIX: &str = ">::name";
const RING_PREFIX: &str = "kymo::ring<";

/// A type as the DWARF describes it: how it reads in C++, its size when it
/// has one, and what its bytes hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Type {
    pub name: String,
    pub size: Option<u64>,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An integer, a character, a boolean or a floating-point number.
    Base(DwAte),
    /// An enumeration: its enumerators, by value.
    Enum(Vec<(i64, String)>),
    Pointer(Pointee),
    /// An array, by the length of each dimension.
    Array(Box<Type>, Vec<u64>),
    /// A `std::chrono::duration`, or a `time_point` by its duration since its
    /// clock's epoch: the count's type, and the seconds one count lasts as a
    /// ratio, numerator then denominator.
    Duration(Box<Type>, u64, u64),
    /// A structure, a class or a union: its data members.
    Record(Vec<Field>),
    /// No value to read: void, a function, or the structure behind a pointer.
    Opaque,
}

/// What a pointer points at, qualifiers and typedefs set aside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pointee {
    Function,
    /// A structure, a class or a union, by qualified name.
    Record(String),
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub offset: u64,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: u16,
    pub name: String,
    pub size: u64,
    pub fields: Vec<Field>,
}

/// A static object of type `kymo::ring<Slots>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ring {
    /// The object's qualified name.
    pub name: String,
    pub address: u64,
    pub slots: u32,
    pub size: u64,
    pub write_index_offset: u64,
    /// The writers lapped by the ring, the word right after the write index.
    pub lapped_offset: u64,
    pub slots_offset: u64,
}

pub struct Catalogue {
    /// Every event type of the image, by id.
    pub events: BTreeMap<u16, Event>,
    /// The trace rings of the image, by name.
    pub rings: Vec<Ring>,
    /// The functions and static objects pointer fields are named by.
    pub symbols: Symbols,
    /// The device interrupts the DWARF names (interrupts.rs).
    pub interrupts: Interrupts,
    /// What the image seems to lack, for the user to read: it decodes all the same.
    pub warnings: Vec<String>,
}

/// The enumeration of IRQ numbers, SvdGenerator's and CMSIS's (interrupts.rs).
const IRQN_TYPE: &str = "IRQn_Type";

/// The namespace of the OpSy adapter's events (target/opsy).
const OPSY_ADAPTER: &str = "kymo::opsy::";

/// The warning for an image that runs OpSy and traces none of its events.
const OPSY_UNHOOKED: &str = "the image runs OpSy without Kymo's hooks, so no kernel event is traced: \
                             declare `class hooks : public kymo::opsy::hooks<trace> {};` in namespace opsy, \
                             in an opsy_hooks.hpp on the include path (target/opsy/include/kymo/opsy/hooks.hpp)";

/// Whether the image holds code of OpSy's: a symbol in namespace `opsy`.
fn runs_opsy(file: &object::File) -> bool {
    file.symbols().any(|symbol| {
        symbol
            .name()
            .is_ok_and(|name| name.starts_with("_ZN4opsy") || name.starts_with("_ZNK4opsy"))
    })
}

/// The catalogue of the image at `path`, the interrupts its DWARF does not
/// name named by the SVD file `svd`.
pub fn read_with_svd(path: &Path, svd: Option<&Path>) -> Result<Catalogue> {
    let mut catalogue = read(path)?;
    if let Some(svd) = svd {
        catalogue.interrupts = catalogue.interrupts.or(&Interrupts::from_svd(svd)?);
    }
    Ok(catalogue)
}

/// The catalogue of the image at `path`.
pub fn read(path: &Path) -> Result<Catalogue> {
    let data = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let file = object::File::parse(&*data)?;
    let ids = type_ids(&file)?;
    let mut warnings = Vec::new();
    if runs_opsy(&file) && !ids.values().any(|name| name.starts_with(OPSY_ADAPTER)) {
        warnings.push(OPSY_UNHOOKED.to_owned());
    }
    if ids.is_empty() {
        let hints = warnings
            .iter()
            .map(|warning| format!("; {warning}"))
            .collect::<String>();
        return Err(format!("{}: no event type ids (.kymo.ids){hints}", path.display()).into());
    }
    let debug = debug_info(&file, ids.values().map(String::as_str).collect())?;
    let events = ids
        .into_iter()
        .map(|(id, name)| {
            let (size, fields) = debug.layouts.get(&name).cloned().ok_or_else(|| {
                format!(
                    "{name} (id {id}): no structure of that name in the DWARF \
                     (an image built without -g, or a class template, whose arguments \
                     the DWARF spells its own way)"
                )
            })?;
            Ok((id, Event { id, name, size, fields }))
        })
        .collect::<Result<_>>()?;
    let interrupts = match debug.irq_enumerations.as_slice() {
        [enumerators] => Interrupts::from_enumerators(enumerators),
        [] => Interrupts::default(),
        _ => {
            warnings.push(format!(
                "the DWARF has {} different {IRQN_TYPE}: none names the interrupts",
                debug.irq_enumerations.len()
            ));
            Interrupts::default()
        }
    };
    Ok(Catalogue {
        events,
        rings: debug.rings,
        symbols: Symbols::read(&file, &debug.bases),
        interrupts,
        warnings,
    })
}

impl Catalogue {
    /// The ring named `channel`, or else the image's only ring.
    pub fn ring(&self, channel: Option<&str>) -> Result<&Ring> {
        let names = || {
            self.rings
                .iter()
                .map(|ring| ring.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match (channel, self.rings.as_slice()) {
            (_, []) => Err("the image has no kymo::ring".into()),
            (None, [ring]) => Ok(ring),
            (None, _) => Err(format!("the image has several rings, choose one with --channel: {}", names()).into()),
            (Some(channel), rings) => rings
                .iter()
                .find(|ring| ring.name == channel)
                .ok_or_else(|| format!("the image has no ring {channel}; its rings: {}", names()).into()),
        }
    }
}

/// The qualified name of the type behind each id.
fn type_ids(file: &object::File) -> Result<BTreeMap<u16, String>> {
    let Some(section) = file.section_by_name(".kymo.ids") else {
        return Ok(BTreeMap::new());
    };
    let index = section.index();
    let mut ids = BTreeMap::new();
    for symbol in file.symbols() {
        if symbol.section_index() != Some(index) {
            continue;
        }
        let Some(mangled) = symbol.name()?.strip_suffix(ID_SUFFIX) else {
            continue;
        };
        let demangled = cpp_demangle::Symbol::new(mangled)
            .map_err(|error| format!("{mangled}: {error}"))?
            .demangle()
            .map_err(|error| format!("{mangled}: {error}"))?;
        let name = demangled
            .strip_prefix(ANCHOR_PREFIX)
            .and_then(|rest| rest.strip_suffix(ANCHOR_SUFFIX))
            .ok_or_else(|| format!("{demangled}: not a kymo::anchor"))?;
        let id = u16::try_from(symbol.address() - section.address())?;
        if let Some(other) = ids.insert(id, name.to_owned()) {
            return Err(format!("id {id} names both {other} and {name}").into());
        }
    }
    Ok(ids)
}

type Slice<'a> = EndianSlice<'a, RunTimeEndian>;
type Layout = (u64, Vec<Field>);

type Unit<'a> = gimli::Unit<Slice<'a>>;
type Names = HashMap<DebugInfoOffset, String>;

struct DebugInfo {
    layouts: BTreeMap<String, Layout>,
    rings: Vec<Ring>,
    /// The base class subobjects of each static object, by address.
    bases: BTreeMap<u64, Vec<(u64, String)>>,
    /// The enumerators of each different `IRQn_Type`.
    irq_enumerations: Vec<Vec<(i64, String)>>,
}

/// The layout of each structure named in `wanted`, the same in every unit
/// that defines it, the rings, and the base classes of static objects.
fn debug_info(file: &object::File, wanted: Vec<&str>) -> Result<DebugInfo> {
    let endian = if file.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };
    let sections = gimli::DwarfSections::load(|id| -> Result<Cow<[u8]>> {
        Ok(match file.section_by_name(id.name()) {
            Some(section) => section.uncompressed_data()?,
            None => Cow::Borrowed(&[]),
        })
    })?;
    let dwarf = sections.borrow(|section| EndianSlice::new(section, endian));

    let mut units = Vec::new();
    let mut headers = dwarf.units();
    while let Some(header) = headers.next()? {
        units.push(dwarf.unit(header)?);
    }

    // Every structure, class and union by qualified name, the units that
    // define each, every static variable by address, and every variable by
    // qualified name.
    let mut names = Names::new();
    let mut definitions: BTreeMap<String, Vec<DebugInfoOffset>> = BTreeMap::new();
    let mut variables = Vec::new();
    let mut variable_names = Names::new();
    let mut irq_enumerations: Vec<Vec<(i64, String)>> = Vec::new();
    for unit in &units {
        let mut scope: Vec<(isize, String)> = Vec::new();
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            let depth = entry.depth();
            while scope.last().is_some_and(|(level, _)| *level >= depth) {
                scope.pop();
            }
            let tag = entry.tag();
            if matches!(tag, gimli::DW_TAG_enumeration_type | gimli::DW_TAG_typedef)
                && entry.attr(gimli::DW_AT_declaration).is_none()
                && let Some(value) = entry.attr_value(gimli::DW_AT_name)
                && dwarf.attr_string(unit, value)?.slice() == IRQN_TYPE.as_bytes()
                && let Kind::Enum(enumerators) = describe(&dwarf, unit, &Names::new(), entry.offset(), true)?.kind
                && !irq_enumerations.contains(&enumerators)
            {
                irq_enumerations.push(enumerators);
            }
            let scoping = matches!(
                tag,
                gimli::DW_TAG_namespace
                    | gimli::DW_TAG_structure_type
                    | gimli::DW_TAG_class_type
                    | gimli::DW_TAG_union_type
            );
            if !scoping && tag != gimli::DW_TAG_variable {
                continue;
            }
            let global = global(unit, entry.offset())?;
            let name = match entry.attr_value(gimli::DW_AT_name) {
                Some(value) => Some(dwarf.attr_string(unit, value)?.to_string_lossy().into_owned()),
                None if tag == gimli::DW_TAG_namespace => Some("(anonymous namespace)".to_owned()),
                None => None,
            };
            let qualified = name.as_ref().map(|name| {
                scope
                    .iter()
                    .map(|(_, part)| part.as_str())
                    .chain([name.as_str()])
                    .collect::<Vec<_>>()
                    .join("::")
            });
            if tag == gimli::DW_TAG_variable {
                if let Some(address) = static_address(&dwarf, unit, entry)? {
                    variables.push((address, global));
                }
                if let Some(qualified) = qualified {
                    variable_names.insert(global, qualified);
                }
                continue;
            }
            let (Some(name), Some(qualified)) = (name, qualified) else {
                continue;
            };
            scope.push((depth, name));
            if tag == gimli::DW_TAG_namespace {
                continue;
            }
            if entry.attr(gimli::DW_AT_declaration).is_none() {
                definitions.entry(qualified.clone()).or_default().push(global);
            }
            names.insert(global, qualified);
        }
    }

    let mut layouts: BTreeMap<String, Layout> = BTreeMap::new();
    for name in wanted {
        for &die in definitions.get(name).into_iter().flatten() {
            let (unit, offset) = locate(&units, die)?;
            if !matches!(
                unit.entry(offset)?.tag(),
                gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type
            ) {
                continue;
            }
            let layout = record(&dwarf, unit, &names, offset)?;
            match layouts.get(name) {
                Some(previous) if *previous != layout => {
                    return Err(format!("{name} has two layouts in the DWARF").into());
                }
                Some(_) => {}
                None => {
                    layouts.insert(name.to_owned(), layout);
                }
            }
        }
    }

    let mut bases = BTreeMap::new();
    let mut rings: Vec<Ring> = Vec::new();
    for (address, variable) in variables {
        let Some(ty) = variable_type(&units, variable)? else {
            continue;
        };
        let ty = strip(&units, ty)?;
        let Some(name) = names.get(&ty) else {
            continue;
        };
        if name.starts_with(RING_PREFIX) && !rings.iter().any(|ring| ring.address == address) {
            let object = variable_name(&units, &variable_names, variable)?
                .ok_or_else(|| format!("a {name} at {address:#x} without a name"))?;
            let (unit, offset) = locate(&units, ty)?;
            rings.push(ring_layout(&dwarf, unit, offset, object, address)?);
        }
        let mut found = Vec::new();
        base_classes(&units, &names, &definitions, name, 0, &mut found)?;
        if !found.is_empty() {
            bases.insert(address, found);
        }
    }
    rings.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(DebugInfo {
        layouts,
        rings,
        bases,
        irq_enumerations,
    })
}

fn global(unit: &Unit, offset: UnitOffset) -> Result<DebugInfoOffset> {
    offset
        .to_debug_info_offset(&unit.header)
        .ok_or_else(|| "a DWARF entry outside .debug_info".into())
}

/// The unit that holds the entry at `offset`, and its offset in that unit.
fn locate<'a, 'b>(units: &'b [Unit<'a>], offset: DebugInfoOffset) -> Result<(&'b Unit<'a>, UnitOffset)> {
    units
        .iter()
        .find_map(|unit| offset.to_unit_offset(&unit.header).map(|local| (unit, local)))
        .ok_or_else(|| format!("no DWARF unit holds the entry at {:#x}", offset.0).into())
}

/// The entry an attribute refers to, in its own unit or in another.
fn reference(unit: &Unit, value: AttributeValue<Slice>) -> Result<Option<DebugInfoOffset>> {
    Ok(match value {
        AttributeValue::UnitRef(offset) => Some(global(unit, offset)?),
        AttributeValue::DebugInfoRef(offset) => Some(offset),
        _ => None,
    })
}

/// The address of a variable that has one fixed address: a static object.
fn static_address(
    dwarf: &gimli::Dwarf<Slice>,
    unit: &Unit,
    entry: &gimli::DebuggingInformationEntry<Slice>,
) -> Result<Option<u64>> {
    let Some(AttributeValue::Exprloc(expression)) = entry.attr_value(gimli::DW_AT_location) else {
        return Ok(None);
    };
    let mut operations = expression.operations(unit.encoding());
    Ok(match (operations.next()?, operations.next()?) {
        (Some(Operation::Address { address }), None) => Some(address),
        (Some(Operation::AddressIndex { index }), None) => Some(dwarf.address(unit, index)?),
        _ => None,
    })
}

/// The type of the variable at `offset`. A variable defined apart from its
/// declaration, or emitted again by link-time optimisation, carries no type
/// but refers to the entry that has it.
fn variable_type(units: &[Unit], mut offset: DebugInfoOffset) -> Result<Option<DebugInfoOffset>> {
    for _ in 0..8 {
        let (unit, local) = locate(units, offset)?;
        let entry = unit.entry(local)?;
        if let Some(value) = entry.attr_value(gimli::DW_AT_type) {
            return reference(unit, value);
        }
        let origin = entry
            .attr_value(gimli::DW_AT_abstract_origin)
            .or_else(|| entry.attr_value(gimli::DW_AT_specification));
        match origin {
            Some(value) => match reference(unit, value)? {
                Some(next) => offset = next,
                None => return Ok(None),
            },
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// The qualified name of the variable at `offset`, given by the entry that
/// declares it when the one that defines it has none.
fn variable_name(units: &[Unit], variable_names: &Names, mut offset: DebugInfoOffset) -> Result<Option<String>> {
    for _ in 0..8 {
        if let Some(name) = variable_names.get(&offset) {
            return Ok(Some(name.clone()));
        }
        let (unit, local) = locate(units, offset)?;
        let entry = unit.entry(local)?;
        let origin = entry
            .attr_value(gimli::DW_AT_abstract_origin)
            .or_else(|| entry.attr_value(gimli::DW_AT_specification));
        match origin.map(|value| reference(unit, value)).transpose()?.flatten() {
            Some(next) => offset = next,
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// The type at `offset` with its qualifiers and typedefs removed.
fn strip(units: &[Unit], mut offset: DebugInfoOffset) -> Result<DebugInfoOffset> {
    loop {
        let (unit, local) = locate(units, offset)?;
        let entry = unit.entry(local)?;
        if !matches!(
            entry.tag(),
            gimli::DW_TAG_typedef | gimli::DW_TAG_const_type | gimli::DW_TAG_volatile_type
        ) {
            return Ok(offset);
        }
        match entry.attr_value(gimli::DW_AT_type) {
            Some(value) => match reference(unit, value)? {
                Some(next) => offset = next,
                None => return Ok(offset),
            },
            None => return Ok(offset),
        }
    }
}

/// The base classes of the class `name`, by offset from `at` and qualified
/// name, bases of bases included, as one definition of the class gives them.
fn base_classes(
    units: &[Unit],
    names: &Names,
    definitions: &BTreeMap<String, Vec<DebugInfoOffset>>,
    name: &str,
    at: u64,
    found: &mut Vec<(u64, String)>,
) -> Result<()> {
    let Some(&definition) = definitions.get(name).and_then(|list| list.first()) else {
        return Ok(());
    };
    let (unit, local) = locate(units, definition)?;
    let mut tree = unit.entries_tree(Some(local))?;
    let mut children = tree.root()?.children();
    let mut direct = Vec::new();
    while let Some(child) = children.next()? {
        let entry = child.entry();
        if entry.tag() != gimli::DW_TAG_inheritance {
            continue;
        }
        // A virtual base has no fixed offset.
        let Some(offset) = entry
            .attr_value(gimli::DW_AT_data_member_location)
            .map_or(Some(0), |value| value.udata_value())
        else {
            continue;
        };
        let Some(base) = entry.attr_value(gimli::DW_AT_type) else {
            continue;
        };
        if let Some(base) = reference(unit, base)? {
            direct.push((at + offset, base));
        }
    }
    for (offset, base) in direct {
        let Some(base) = names.get(&strip(units, base)?) else {
            continue;
        };
        found.push((offset, base.clone()));
        base_classes(units, names, definitions, base, offset, found)?;
    }
    Ok(())
}

/// The ring `name` at `address`, of the type `kymo::ring<Slots>` at `offset`.
fn ring_layout(
    dwarf: &gimli::Dwarf<Slice>,
    unit: &gimli::Unit<Slice>,
    offset: UnitOffset,
    name: String,
    address: u64,
) -> Result<Ring> {
    let entry = unit.entry(offset)?;
    let size = entry
        .attr_value(gimli::DW_AT_byte_size)
        .and_then(|value| value.udata_value())
        .ok_or_else(|| format!("{name}: its type has no size"))?;
    let mut parameters = BTreeMap::new();
    let mut members = BTreeMap::new();
    let mut tree = unit.entries_tree(Some(offset))?;
    let mut children = tree.root()?.children();
    while let Some(child) = children.next()? {
        let entry = child.entry();
        let Some(name) = entry.attr_value(gimli::DW_AT_name) else {
            continue;
        };
        let name = dwarf.attr_string(unit, name)?.to_string_lossy().into_owned();
        match entry.tag() {
            gimli::DW_TAG_template_value_parameter => {
                if let Some(value) = entry
                    .attr_value(gimli::DW_AT_const_value)
                    .and_then(|value| value.udata_value())
                {
                    parameters.insert(name, value);
                }
            }
            gimli::DW_TAG_member => {
                if let Some(value) = entry
                    .attr_value(gimli::DW_AT_data_member_location)
                    .and_then(|value| value.udata_value())
                {
                    members.insert(name, value);
                }
            }
            _ => {}
        }
    }
    let get = |map: &BTreeMap<String, u64>, wanted: &str| {
        map.get(wanted)
            .copied()
            .ok_or_else(|| format!("{name}: no {wanted} in the DWARF"))
    };
    let ring = Ring {
        slots: u32::try_from(get(&parameters, "Slots")?)?,
        size,
        lapped_offset: get(&members, "lapped")?,
        write_index_offset: get(&members, "write_index")?,
        slots_offset: get(&members, "slots")?,
        name,
        address,
    };
    if ring.slots < 2
        || !ring.slots.is_power_of_two()
        || ring.lapped_offset != ring.write_index_offset + 4
        || ring.size < ring.slots_offset + u64::from(ring.slots) * 16
    {
        return Err(format!("{}: unexpected geometry {ring:?}", ring.name).into());
    }
    Ok(ring)
}

/// The size and the data members of the structure at `offset`.
fn record(dwarf: &gimli::Dwarf<Slice>, unit: &Unit, names: &Names, offset: UnitOffset) -> Result<Layout> {
    let entry = unit.entry(offset)?;
    let size = entry
        .attr_value(gimli::DW_AT_byte_size)
        .and_then(|value| value.udata_value())
        .ok_or("a structure without a size")?;
    let mut fields = Vec::new();
    let mut tree = unit.entries_tree(Some(offset))?;
    let mut children = tree.root()?.children();
    while let Some(child) = children.next()? {
        let member = child.entry();
        if member.tag() != gimli::DW_TAG_member || member.attr(gimli::DW_AT_declaration).is_some() {
            continue; // static members: DW_TAG_variable in DWARF 5, declared members before
        }
        let name = match member.attr_value(gimli::DW_AT_name) {
            Some(value) => dwarf.attr_string(unit, value)?.to_string_lossy().into_owned(),
            None => String::new(),
        };
        if member.attr(gimli::DW_AT_bit_size).is_some() {
            return Err(format!("{name}: bit-fields are not supported").into());
        }
        let offset = match member.attr_value(gimli::DW_AT_data_member_location) {
            Some(value) => value
                .udata_value()
                .ok_or_else(|| format!("{name}: offset is not a constant"))?,
            None => 0,
        };
        let type_offset = type_of(member)?.ok_or_else(|| format!("{name}: no type"))?;
        let ty = describe(dwarf, unit, names, type_offset, false)?;
        if ty.size.is_none() {
            return Err(format!("{name}: {} has no size", ty.name).into());
        }
        fields.push(Field { name, offset, ty });
    }
    Ok((size, fields))
}

fn type_of(entry: &gimli::DebuggingInformationEntry<Slice>) -> Result<Option<UnitOffset>> {
    match entry.attr_value(gimli::DW_AT_type) {
        Some(AttributeValue::UnitRef(offset)) => Ok(Some(offset)),
        Some(_) => Err("a type outside its unit".into()),
        None => Ok(None),
    }
}

fn void() -> Type {
    Type {
        name: "void".to_owned(),
        size: None,
        kind: Kind::Opaque,
    }
}

/// The type at `offset`. Behind a pointer (`shallow`), a structure is named
/// but not opened: it may point back to itself.
fn describe(
    dwarf: &gimli::Dwarf<Slice>,
    unit: &Unit,
    names: &Names,
    offset: UnitOffset,
    shallow: bool,
) -> Result<Type> {
    let entry = unit.entry(offset)?;
    let name = match entry.attr_value(gimli::DW_AT_name) {
        Some(value) => Some(dwarf.attr_string(unit, value)?.to_string_lossy().into_owned()),
        None => None,
    };
    let own_size = entry
        .attr_value(gimli::DW_AT_byte_size)
        .and_then(|value| value.udata_value());
    let inner = type_of(&entry)?;
    let address_size = u64::from(unit.encoding().address_size);
    let unnamed = || name.clone().unwrap_or_else(|| "<unnamed>".to_owned());
    Ok(match entry.tag() {
        gimli::DW_TAG_const_type | gimli::DW_TAG_volatile_type => {
            let qualifier = if entry.tag() == gimli::DW_TAG_const_type {
                "const"
            } else {
                "volatile"
            };
            let inner = match inner {
                Some(inner) => describe(dwarf, unit, names, inner, shallow)?,
                None => void(),
            };
            Type {
                name: format!("{} {qualifier}", inner.name),
                ..inner
            }
        }
        gimli::DW_TAG_typedef => {
            let inner = describe(dwarf, unit, names, inner.ok_or("a typedef of nothing")?, shallow)?;
            Type {
                name: name.unwrap_or_default(),
                ..inner
            }
        }
        gimli::DW_TAG_pointer_type | gimli::DW_TAG_reference_type | gimli::DW_TAG_rvalue_reference_type => {
            let (pointee, target) = match inner {
                Some(inner) => (
                    describe(dwarf, unit, names, inner, true)?.name,
                    pointee(unit, names, inner)?,
                ),
                None => ("void".to_owned(), Pointee::Other),
            };
            let sigil = match entry.tag() {
                gimli::DW_TAG_pointer_type => "*",
                gimli::DW_TAG_reference_type => "&",
                _ => "&&",
            };
            Type {
                name: format!("{pointee}{sigil}"),
                size: Some(own_size.unwrap_or(address_size)),
                kind: Kind::Pointer(target),
            }
        }
        gimli::DW_TAG_subroutine_type => Type {
            name: "function".to_owned(),
            size: None,
            kind: Kind::Opaque,
        },
        gimli::DW_TAG_array_type => {
            let element = describe(dwarf, unit, names, inner.ok_or("an array of nothing")?, shallow)?;
            let mut lengths = Vec::new();
            let mut known = true;
            let mut tree = unit.entries_tree(Some(offset))?;
            let mut children = tree.root()?.children();
            while let Some(child) = children.next()? {
                let range = child.entry();
                match (
                    range
                        .attr_value(gimli::DW_AT_count)
                        .and_then(|value| value.udata_value()),
                    range
                        .attr_value(gimli::DW_AT_upper_bound)
                        .and_then(|value| value.udata_value()),
                ) {
                    (Some(count), _) => lengths.push(count),
                    (None, Some(upper)) => lengths.push(upper + 1),
                    (None, None) => {
                        known = false;
                        lengths.push(0);
                    }
                }
            }
            let dimensions: String = lengths
                .iter()
                .map(|length| if known { format!("[{length}]") } else { "[]".to_owned() })
                .collect();
            let size = element
                .size
                .filter(|_| known)
                .map(|size| size * lengths.iter().product::<u64>());
            let kind = if known {
                Kind::Array(Box::new(element.clone()), lengths)
            } else {
                Kind::Opaque
            };
            Type {
                name: format!("{}{dimensions}", element.name),
                size,
                kind,
            }
        }
        gimli::DW_TAG_base_type => {
            let encoding = match entry.attr_value(gimli::DW_AT_encoding) {
                Some(AttributeValue::Encoding(encoding)) => encoding,
                _ => return Err(format!("{}: a base type without an encoding", unnamed()).into()),
            };
            Type {
                name: unnamed(),
                size: own_size,
                kind: Kind::Base(encoding),
            }
        }
        gimli::DW_TAG_enumeration_type => {
            let mut enumerators = Vec::new();
            let mut tree = unit.entries_tree(Some(offset))?;
            let mut children = tree.root()?.children();
            while let Some(child) = children.next()? {
                let enumerator = child.entry();
                if enumerator.tag() != gimli::DW_TAG_enumerator {
                    continue;
                }
                let value = enumerator.attr_value(gimli::DW_AT_const_value);
                let value = value.as_ref().and_then(AttributeValue::sdata_value).or_else(|| {
                    value
                        .as_ref()
                        .and_then(AttributeValue::udata_value)
                        .map(|value| value as i64)
                });
                let label = enumerator.attr_value(gimli::DW_AT_name);
                if let (Some(value), Some(label)) = (value, label) {
                    enumerators.push((value, dwarf.attr_string(unit, label)?.to_string_lossy().into_owned()));
                }
            }
            Type {
                name: unnamed(),
                size: own_size,
                kind: Kind::Enum(enumerators),
            }
        }
        gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type | gimli::DW_TAG_union_type => {
            if shallow || entry.attr(gimli::DW_AT_declaration).is_some() {
                Type {
                    name: unnamed(),
                    size: own_size,
                    kind: Kind::Opaque,
                }
            } else {
                let (size, fields) = record(dwarf, unit, names, offset)?;
                let qualified = names.get(&global(unit, offset)?);
                match standard_array(qualified, size, &fields).or_else(|| standard_chrono(qualified, size, &fields)) {
                    Some(kind) => Type {
                        name: qualified.cloned().unwrap_or_else(unnamed),
                        size: Some(size),
                        kind,
                    },
                    None => Type {
                        name: unnamed(),
                        size: Some(size),
                        kind: Kind::Record(fields),
                    },
                }
            }
        }
        _ => Type {
            name: unnamed(),
            size: own_size,
            kind: Kind::Opaque,
        },
    })
}

/// The array a `std::array` of `size` bytes holds, when `fields` are those of
/// one: a single member, `_M_elems` (libstdc++) or `__elems_` (libc++), a C
/// array that fills it. An empty `std::array` holds no C array and stays a
/// structure.
fn standard_array(qualified: Option<&String>, size: u64, fields: &[Field]) -> Option<Kind> {
    if !standard(qualified?)?.starts_with("array<") {
        return None;
    }
    match only_member(size, fields, &["_M_elems", "__elems_"])? {
        kind @ Kind::Array(..) => Some(kind.clone()),
        _ => None,
    }
}

/// The duration a `std::chrono::duration` or `time_point` of `size` bytes
/// reads as, when `fields` are those of one: a single member that fills it,
/// the count, `__r` (libstdc++) or `__rep_` (libc++), an integer; the
/// duration of a time point, `__d` or `__d_`. The period is read from the
/// duration's name, `ratio<1, 1000>` and the like.
fn standard_chrono(qualified: Option<&String>, size: u64, fields: &[Field]) -> Option<Kind> {
    let name = standard(qualified?)?.strip_prefix("chrono::")?;
    if name.starts_with("duration<") {
        let rep = match only_member(size, fields, &["__r", "__rep_"])? {
            Kind::Base(encoding) if *encoding != gimli::DW_ATE_float => &fields[0].ty,
            _ => return None,
        };
        let (num, den) = ratio(name)?;
        Some(Kind::Duration(Box::new(rep.clone()), num, den))
    } else if name.starts_with("time_point<") {
        match only_member(size, fields, &["__d", "__d_"])? {
            kind @ Kind::Duration(..) => Some(kind.clone()),
            _ => None,
        }
    } else {
        None
    }
}

/// The period of a duration named `name`: the first `ratio<num, den>` in it,
/// the literal suffixes a compiler may print (`1000L`) left out.
fn ratio(name: &str) -> Option<(u64, u64)> {
    let (_, args) = name.split_once("ratio<")?;
    let args = &args[..args.find('>')?];
    let number = |text: &str| text.trim().trim_end_matches(['u', 'U', 'l', 'L']).parse::<u64>().ok();
    let (num, den) = match args.split_once(',') {
        Some((num, den)) => (number(num)?, number(den)?),
        None => (number(args)?, 1),
    };
    (num > 0 && den > 0).then_some((num, den))
}

/// A name in namespace `std`, that namespace and libc++'s inline one
/// (`std::__1::array`) left out.
fn standard(qualified: &str) -> Option<&str> {
    let name = qualified.strip_prefix("std::")?;
    Some(match name.split_once("::") {
        Some((inline, rest)) if inline.starts_with("__") => rest,
        _ => name,
    })
}

/// The kind of the single member of a structure of `size` bytes, when it
/// fills it and has one of the `names`.
fn only_member<'a>(size: u64, fields: &'a [Field], names: &[&str]) -> Option<&'a Kind> {
    let [field] = fields else {
        return None;
    };
    let filled = field.offset == 0 && field.ty.size == Some(size);
    (filled && names.contains(&field.name.as_str())).then_some(&field.ty.kind)
}

/// What the type at `offset`, the target of a pointer, is.
fn pointee(unit: &Unit, names: &Names, mut offset: UnitOffset) -> Result<Pointee> {
    loop {
        let entry = unit.entry(offset)?;
        match entry.tag() {
            gimli::DW_TAG_typedef | gimli::DW_TAG_const_type | gimli::DW_TAG_volatile_type => match type_of(&entry)? {
                Some(inner) => offset = inner,
                None => return Ok(Pointee::Other),
            },
            gimli::DW_TAG_subroutine_type => return Ok(Pointee::Function),
            gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type | gimli::DW_TAG_union_type => {
                return Ok(names
                    .get(&global(unit, offset)?)
                    .map_or(Pointee::Other, |name| Pointee::Record(name.clone())));
            }
            _ => return Ok(Pointee::Other),
        }
    }
}

/// The catalogue as `kymo events` prints it: one line per event, then one
/// indented line per field.
pub fn print(catalogue: &Catalogue) {
    for event in catalogue.events.values() {
        println!("event {} {} {}", event.id, event.name, event.size);
        for field in &event.fields {
            let size = field.ty.size.unwrap_or_default();
            println!("  field {} {} {} {}", field.name, field.offset, size, field.ty.name);
        }
    }
    for (exception, name) in catalogue.interrupts.iter() {
        println!("interrupt {exception} {name}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, ty: Type) -> Field {
        Field {
            name: name.to_owned(),
            offset: 0,
            ty,
        }
    }

    fn base(encoding: gimli::DwAte, size: u64) -> Type {
        Type {
            name: "long long int".to_owned(),
            size: Some(size),
            kind: Kind::Base(encoding),
        }
    }

    #[test]
    fn a_period_reads_from_the_ratio_in_the_name() {
        let periods = [
            "duration<long long int, std::ratio<1, 1000> >",
            "duration<long long, std::__1::ratio<1L, 1000000000L> >",
            "duration<int, std::ratio<60> >",
            "duration<int, std::ratio<0, 1> >",
            "duration<int, long>",
        ]
        .map(ratio);
        assert_eq!(
            periods,
            [Some((1, 1_000)), Some((1, 1_000_000_000)), Some((60, 1)), None, None]
        );
    }

    #[test]
    fn a_duration_and_a_time_point_read_as_a_count() {
        let count = base(gimli::DW_ATE_signed, 8);
        let name = |name: &str| Some(name.to_owned());
        let duration = standard_chrono(
            name("std::chrono::duration<long long int, std::ratio<1, 1000> >").as_ref(),
            8,
            &[field("__r", count.clone())],
        );
        assert_eq!(duration, Some(Kind::Duration(Box::new(count.clone()), 1, 1_000)));
        let ty = Type {
            name: "duration".to_owned(),
            size: Some(8),
            kind: duration.unwrap(),
        };
        let point = standard_chrono(
            name("std::__1::chrono::time_point<clock, duration>").as_ref(),
            8,
            &[field("__d_", ty.clone())],
        );
        assert_eq!(point, Some(ty.kind));
        // Not a duration's layout: read as a structure.
        let float = base(gimli::DW_ATE_float, 8);
        let other = [
            (
                "std::chrono::duration<double, std::ratio<1, 1> >",
                8,
                field("__r", float),
            ),
            (
                "std::chrono::duration<long long int, std::ratio<1, 1> >",
                16,
                field("__r", count.clone()),
            ),
            (
                "std::chrono::duration<long long int, std::ratio<1, 1> >",
                8,
                field("ticks", count.clone()),
            ),
            (
                "app::duration<long long int, std::ratio<1, 1> >",
                8,
                field("__r", count),
            ),
        ];
        for (qualified, size, member) in other {
            assert_eq!(
                standard_chrono(name(qualified).as_ref(), size, &[member]),
                None,
                "{qualified}"
            );
        }
    }
}
