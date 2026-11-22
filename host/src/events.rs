//! The catalogue of event types, read from the target's ELF: each type id the
//! linker assigned, the type it stands for, and that type's layout from the
//! DWARF; and the geometry of the trace ring. Nothing about an event is known
//! to the host in advance.
//!
//! A type id is the offset of a symbol `kymo::anchor<Event>::name.kymo_id` in
//! `.kymo.ids` (target/include/kymo/type_id.hpp). The DWARF has no trace of
//! the anchor, so the type is found by name: the symbol is demangled, and the
//! structure of the same qualified name is looked up in the debug info.

use crate::target::Result;
use gimli::{AttributeValue, DwAte, EndianSlice, RunTimeEndian, UnitOffset};
use object::{Object, ObjectSection, ObjectSymbol};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

const ID_SUFFIX: &str = ".kymo_id";
const ANCHOR_PREFIX: &str = "kymo::anchor<";
const ANCHOR_SUFFIX: &str = ">::name";
const RING: &str = "kymo::trace_ring";

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
    Pointer,
    /// An array, by the length of each dimension.
    Array(Box<Type>, Vec<u64>),
    /// A structure, a class or a union: its data members.
    Record(Vec<Field>),
    /// No value to read: void, a function, or the structure behind a pointer.
    Opaque,
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

/// `kymo::ring<Slots, MaxEventSlots>`, as the image instantiates it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ring {
    pub slots: u32,
    pub max_event_slots: u32,
    pub size: u64,
    pub write_index_offset: u64,
    pub slots_offset: u64,
}

pub struct Catalogue {
    /// Every event type of the image, by id.
    pub events: BTreeMap<u16, Event>,
    /// The trace ring, when the image has one.
    pub ring: Option<Ring>,
}

/// The catalogue of the image at `path`.
pub fn read(path: &Path) -> Result<Catalogue> {
    let data = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let file = object::File::parse(&*data)?;
    let ids = type_ids(&file)?;
    if ids.is_empty() {
        return Err(format!("{}: no event type ids (.kymo.ids)", path.display()).into());
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
    Ok(Catalogue {
        events,
        ring: debug.ring,
    })
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

struct DebugInfo {
    layouts: BTreeMap<String, Layout>,
    ring: Option<Ring>,
}

/// The layout of each structure named in `wanted`, the same in every unit
/// that defines it, and the ring.
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

    let mut layouts: BTreeMap<String, Layout> = BTreeMap::new();
    let mut ring = None;
    let mut headers = dwarf.units();
    while let Some(header) = headers.next()? {
        let unit = dwarf.unit(header)?;
        let mut scope: Vec<(isize, String)> = Vec::new();
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            let depth = entry.depth();
            while scope.last().is_some_and(|(level, _)| *level >= depth) {
                scope.pop();
            }
            let tag = entry.tag();
            let scoping = matches!(
                tag,
                gimli::DW_TAG_namespace | gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type
            );
            if !scoping && tag != gimli::DW_TAG_variable {
                continue;
            }
            let name = match entry.attr_value(gimli::DW_AT_name) {
                Some(value) => dwarf.attr_string(&unit, value)?.to_string_lossy().into_owned(),
                None if tag == gimli::DW_TAG_namespace => "(anonymous namespace)".to_owned(),
                None => continue,
            };
            let qualified = scope
                .iter()
                .map(|(_, part)| part.as_str())
                .chain([name.as_str()])
                .collect::<Vec<_>>()
                .join("::");
            if tag == gimli::DW_TAG_variable {
                if ring.is_none() && qualified == RING {
                    let ty = type_of(entry)?.ok_or("kymo::trace_ring has no type")?;
                    ring = Some(ring_layout(&dwarf, &unit, ty)?);
                }
                continue;
            }
            scope.push((depth, name));
            if tag == gimli::DW_TAG_namespace
                || entry.attr(gimli::DW_AT_declaration).is_some()
                || !wanted.contains(&qualified.as_str())
            {
                continue;
            }
            let layout = record(&dwarf, &unit, entry.offset())?;
            match layouts.get(&qualified) {
                Some(previous) if *previous != layout => {
                    return Err(format!("{qualified} has two layouts in the DWARF").into());
                }
                Some(_) => {}
                None => {
                    layouts.insert(qualified, layout);
                }
            }
        }
    }
    Ok(DebugInfo { layouts, ring })
}

/// The geometry of `kymo::ring<Slots, MaxEventSlots>` at `offset`.
fn ring_layout(dwarf: &gimli::Dwarf<Slice>, unit: &gimli::Unit<Slice>, offset: UnitOffset) -> Result<Ring> {
    let entry = unit.entry(offset)?;
    let size = entry
        .attr_value(gimli::DW_AT_byte_size)
        .and_then(|value| value.udata_value())
        .ok_or("kymo::trace_ring has no size")?;
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
    let get = |map: &BTreeMap<String, u64>, name: &str| {
        map.get(name)
            .copied()
            .ok_or_else(|| format!("kymo::trace_ring: no {name} in the DWARF"))
    };
    let ring = Ring {
        slots: u32::try_from(get(&parameters, "Slots")?)?,
        max_event_slots: u32::try_from(get(&parameters, "MaxEventSlots")?)?,
        size,
        write_index_offset: get(&members, "write_index")?,
        slots_offset: get(&members, "slots")?,
    };
    if !ring.slots.is_power_of_two() || ring.max_event_slots == 0 || ring.max_event_slots > ring.slots {
        return Err(format!("kymo::trace_ring: unexpected geometry {ring:?}").into());
    }
    Ok(ring)
}

/// The size and the data members of the structure at `offset`.
fn record(dwarf: &gimli::Dwarf<Slice>, unit: &gimli::Unit<Slice>, offset: UnitOffset) -> Result<Layout> {
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
        let ty = describe(dwarf, unit, type_offset, false)?;
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
fn describe(dwarf: &gimli::Dwarf<Slice>, unit: &gimli::Unit<Slice>, offset: UnitOffset, shallow: bool) -> Result<Type> {
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
                Some(inner) => describe(dwarf, unit, inner, shallow)?,
                None => void(),
            };
            Type {
                name: format!("{} {qualifier}", inner.name),
                ..inner
            }
        }
        gimli::DW_TAG_typedef => {
            let inner = describe(dwarf, unit, inner.ok_or("a typedef of nothing")?, shallow)?;
            Type {
                name: name.unwrap_or_default(),
                ..inner
            }
        }
        gimli::DW_TAG_pointer_type | gimli::DW_TAG_reference_type | gimli::DW_TAG_rvalue_reference_type => {
            let pointee = match inner {
                Some(inner) => describe(dwarf, unit, inner, true)?.name,
                None => "void".to_owned(),
            };
            let sigil = match entry.tag() {
                gimli::DW_TAG_pointer_type => "*",
                gimli::DW_TAG_reference_type => "&",
                _ => "&&",
            };
            Type {
                name: format!("{pointee}{sigil}"),
                size: Some(own_size.unwrap_or(address_size)),
                kind: Kind::Pointer,
            }
        }
        gimli::DW_TAG_subroutine_type => Type {
            name: "function".to_owned(),
            size: None,
            kind: Kind::Opaque,
        },
        gimli::DW_TAG_array_type => {
            let element = describe(dwarf, unit, inner.ok_or("an array of nothing")?, shallow)?;
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
                let (size, fields) = record(dwarf, unit, offset)?;
                Type {
                    name: unnamed(),
                    size: Some(size),
                    kind: Kind::Record(fields),
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
}
