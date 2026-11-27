//! MSVC x64 run-time type information: type descriptors, complete object locators, class
//! hierarchy descriptors and the vtables that point at them.

use std::collections::{BTreeMap, HashMap};

use memchr::memmem;

use crate::demangle;
use crate::model::{ClassInfo, VtableInfo};
use crate::scan::Layout;

const MAX_NAME: usize = 2048;
const MAX_SLOTS: usize = 8192;
const MAX_BASES: u32 = 512;

pub(crate) struct Rtti {
    pub classes: Vec<ClassInfo>,
    pub vtables: Vec<VtableInfo>,
}

struct Locator {
    rva: u32,
    offset: u32,
    type_descriptor: u32,
    hierarchy: u32,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// Data sections with bytes in the file, as `(rva, bytes)`.
fn data_sections<'a>(layout: &Layout<'a, '_>) -> Vec<(u32, &'a [u8])> {
    layout
        .image
        .sections
        .iter()
        .filter(|s| !s.executable())
        .filter_map(|s| {
            let bytes = layout.image.read(s.virtual_address, s.raw_size as usize)?;
            Some((s.virtual_address, bytes))
        })
        .collect()
}

fn type_descriptors(layout: &Layout<'_, '_>, sections: &[(u32, &[u8])]) -> HashMap<u32, String> {
    let high = layout.base + u64::from(layout.image.image_size);
    let mut found = HashMap::new();
    for &(va, bytes) in sections {
        for at in memmem::find_iter(bytes, b".?A") {
            if at < 16 || !matches!(bytes.get(at + 3), Some(b'V' | b'U' | b'T' | b'W')) {
                continue;
            }
            let Some(pointer) = u64_at(bytes, at - 16) else {
                continue;
            };
            if pointer < layout.base || pointer >= high {
                continue;
            }
            let rest = &bytes[at..bytes.len().min(at + MAX_NAME)];
            let Some(end) = rest.iter().position(|b| *b == 0) else {
                continue;
            };
            let name = &rest[..end];
            if !name.ends_with(b"@@") || !name.iter().all(|b| (0x21..0x7F).contains(b)) {
                continue;
            }
            found.insert(
                va + at as u32 - 16,
                String::from_utf8_lossy(name).into_owned(),
            );
        }
    }
    found
}

fn locators(sections: &[(u32, &[u8])], types: &HashMap<u32, String>) -> Vec<Locator> {
    let mut found = Vec::new();
    for &(va, bytes) in sections {
        let mut at = 0;
        while at + 24 <= bytes.len() {
            let rva = va + at as u32;
            if u32_at(bytes, at + 20) == Some(rva) && u32_at(bytes, at) == Some(1) {
                let type_descriptor = u32_at(bytes, at + 12).unwrap_or(0);
                if types.contains_key(&type_descriptor) {
                    found.push(Locator {
                        rva,
                        offset: u32_at(bytes, at + 4).unwrap_or(0),
                        type_descriptor,
                        hierarchy: u32_at(bytes, at + 16).unwrap_or(0),
                    });
                }
            }
            at += 4;
        }
    }
    found
}

struct Hierarchy {
    direct: Vec<u32>,
    all: Vec<u32>,
}

/// Type descriptors of the bases a class hierarchy descriptor lists. The array is a depth-first
/// walk: the class itself, then each direct base followed by its own bases.
fn hierarchy(layout: &Layout<'_, '_>, rva: u32, types: &HashMap<u32, String>) -> Option<Hierarchy> {
    let image = layout.image;
    let count = u32_at(image.read(rva, 16)?, 8)?;
    if count == 0 || count > MAX_BASES {
        return None;
    }
    let array = u32_at(image.read(rva, 16)?, 12)?;
    let table = image.read(array, 4 * count as usize)?;
    let mut entries = Vec::new();
    for index in 0..count as usize {
        let descriptor = image.read(u32_at(table, 4 * index)?, 28)?;
        let td = u32_at(descriptor, 0)?;
        types.contains_key(&td).then_some(())?;
        entries.push((td, u32_at(descriptor, 4)?));
    }
    let mut direct = Vec::new();
    let mut at = 1;
    while at < entries.len() {
        direct.push(entries[at].0);
        at += 1 + entries[at].1 as usize;
    }
    Some(Hierarchy {
        direct,
        all: entries[1..].iter().map(|e| e.0).collect(),
    })
}

fn vtables(
    layout: &Layout<'_, '_>,
    sections: &[(u32, &[u8])],
    locators: &[Locator],
) -> Vec<(u32, usize, Vec<u32>)> {
    let by_address: HashMap<u64, usize> = locators
        .iter()
        .enumerate()
        .map(|(index, l)| (layout.base + u64::from(l.rva), index))
        .collect();
    let mut found = Vec::new();
    for &(va, bytes) in sections {
        let mut at = 0;
        while at + 8 <= bytes.len() {
            let Some(&index) = u64_at(bytes, at).and_then(|v| by_address.get(&v)) else {
                at += 8;
                continue;
            };
            let mut slots = Vec::new();
            let mut slot = at + 8;
            while slots.len() < MAX_SLOTS {
                let Some(target) = u64_at(bytes, slot).and_then(|v| layout.rva(v)) else {
                    break;
                };
                if !layout.is_code(target) {
                    break;
                }
                slots.push(target);
                slot += 8;
            }
            found.push((va + at as u32 + 8, index, slots));
            at += 8;
        }
    }
    found
}

pub(crate) fn scan(layout: &Layout<'_, '_>) -> Rtti {
    let sections = data_sections(layout);
    let types = type_descriptors(layout, &sections);
    let locators = locators(&sections, &types);
    let vtables = vtables(layout, &sections, &locators);
    let name_of = |td: u32| {
        let decorated = &types[&td];
        demangle::type_name(decorated).unwrap_or_else(|| decorated.clone())
    };
    let mut hierarchies: HashMap<u32, Option<Hierarchy>> = HashMap::new();
    let mut grouped: BTreeMap<u32, Vec<&Locator>> = BTreeMap::new();
    for locator in &locators {
        grouped
            .entry(locator.type_descriptor)
            .or_default()
            .push(locator);
    }
    let mut infos = Vec::new();
    for (td, group) in &grouped {
        let first = group[0];
        let parsed = hierarchies
            .entry(first.hierarchy)
            .or_insert_with(|| hierarchy(layout, first.hierarchy, &types));
        let (bases, ancestors) = parsed.as_ref().map_or((Vec::new(), Vec::new()), |h| {
            (
                h.direct.iter().map(|b| name_of(*b)).collect(),
                h.all.iter().map(|b| name_of(*b)).collect(),
            )
        });
        let mut own: Vec<&(u32, usize, Vec<u32>)> = vtables
            .iter()
            .filter(|(_, index, _)| locators[*index].type_descriptor == *td)
            .collect();
        own.sort_by_key(|(rva, index, _)| (locators[*index].offset, *rva));
        infos.push(ClassInfo {
            name: name_of(*td),
            type_descriptor: *td,
            vtables: own.iter().map(|(rva, _, _)| *rva).collect(),
            bases,
            ancestors,
            methods: own
                .first()
                .map(|(_, _, slots)| {
                    slots
                        .iter()
                        .enumerate()
                        .map(|(slot, rva)| (slot as u32, *rva))
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    let mut table: Vec<VtableInfo> = vtables
        .iter()
        .map(|(rva, index, slots)| {
            let locator = &locators[*index];
            VtableInfo {
                rva: *rva,
                class: name_of(locator.type_descriptor),
                type_descriptor: locator.type_descriptor,
                locator: locator.rva,
                offset: locator.offset,
                slots: slots.clone(),
            }
        })
        .collect();
    table.sort_by_key(|v| v.rva);
    table.dedup_by_key(|v| v.rva);
    Rtti {
        classes: infos,
        vtables: table,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testimage::{Builder, Code};
    use pe_image::Image;

    /// Hand-built RTTI for `Animal` and `Dog : Animal` (in namespace `Zoo`), with a secondary
    /// vtable for Dog. The vtables point at functions at 0x1400 to 0x1500.
    pub(crate) fn write_animals(b: &mut Builder) {
        let r = Builder::FREE_RDATA;
        let type_info_vtable = Builder::va(Builder::TEXT);
        let (animal_td, dog_td) = (r, r + 0x40);
        for (td, name) in [(animal_td, ".?AVAnimal@@"), (dog_td, ".?AVDog@Zoo@@")] {
            b.put64(td, type_info_vtable);
            b.put(td + 16, name.as_bytes());
        }
        let (animal_bcd, dog_bcd, animal_in_dog_bcd) = (r + 0x100, r + 0x120, r + 0x140);
        let (animal_chd, dog_chd) = (r + 0x160, r + 0x180);
        let (animal_array, dog_array) = (r + 0x1A0, r + 0x1C0);
        b.put32(animal_bcd, animal_td);
        b.put32(animal_bcd + 28 - 4, animal_chd);
        b.put32(dog_bcd, dog_td);
        b.put32(dog_bcd + 4, 1);
        b.put32(dog_bcd + 24, dog_chd);
        b.put32(animal_in_dog_bcd, animal_td);
        b.put32(animal_in_dog_bcd + 24, animal_chd);
        b.put32(animal_chd + 8, 1);
        b.put32(animal_chd + 12, animal_array);
        b.put32(animal_array, animal_bcd);
        b.put32(dog_chd + 8, 2);
        b.put32(dog_chd + 12, dog_array);
        b.put32(dog_array, dog_bcd);
        b.put32(dog_array + 4, animal_in_dog_bcd);
        let (animal_col, dog_col, dog_col_secondary) = (r + 0x200, r + 0x220, r + 0x240);
        for (col, offset, td, chd) in [
            (animal_col, 0, animal_td, animal_chd),
            (dog_col, 0, dog_td, dog_chd),
            (dog_col_secondary, 16, dog_td, dog_chd),
        ] {
            b.put32(col, 1);
            b.put32(col + 4, offset);
            b.put32(col + 12, td);
            b.put32(col + 16, chd);
            b.put32(col + 20, col);
        }
        let (animal_vtable, dog_vtable, dog_secondary) = (r + 0x308, r + 0x330, r + 0x360);
        for (vtable, col, slots) in [
            (animal_vtable, animal_col, vec![0x1400u32, 0x1440]),
            (dog_vtable, dog_col, vec![0x1480, 0x1440, 0x14C0]),
            (dog_secondary, dog_col_secondary, vec![0x1500]),
        ] {
            b.put64(vtable - 8, Builder::va(col));
            for (index, slot) in slots.iter().enumerate() {
                b.put64(vtable + 8 * index as u32, Builder::va(*slot));
            }
        }
    }

    fn rtti_image() -> Vec<u8> {
        let mut b = Builder::new();
        write_animals(&mut b);
        let mut code = Code::at(Builder::TEXT);
        code.ret();
        b.code(code);
        for slot in [0x1400, 0x1440, 0x1480, 0x14C0, 0x1500] {
            let mut code = Code::at(slot);
            code.ret();
            b.code(code);
        }
        b.finish()
    }

    #[test]
    fn reads_classes_bases_and_vtables() {
        let bytes = rtti_image();
        let image = Image::parse(&bytes).unwrap();
        let layout = Layout::new(&image);
        let rtti = scan(&layout);
        let r = Builder::FREE_RDATA;
        let names: Vec<&str> = rtti.classes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Animal", "Zoo::Dog"]);
        let animal = &rtti.classes[0];
        let dog = &rtti.classes[1];
        assert!(animal.bases.is_empty());
        assert_eq!(dog.bases, ["Animal"]);
        assert_eq!(dog.ancestors, ["Animal"]);
        assert_eq!(animal.vtables, [r + 0x308]);
        assert_eq!(dog.vtables, [r + 0x330, r + 0x360]);
        assert_eq!(animal.methods, [(0, 0x1400), (1, 0x1440)]);
        assert_eq!(dog.methods, [(0, 0x1480), (1, 0x1440), (2, 0x14C0)]);
        let secondary = rtti.vtables.iter().find(|v| v.rva == r + 0x360).unwrap();
        assert_eq!(
            (secondary.offset, secondary.slots.as_slice()),
            (16, &[0x1500][..])
        );
        assert_eq!(secondary.class, "Zoo::Dog");
        assert_eq!(rtti.vtables.len(), 3);
    }

    #[test]
    fn ignores_text_that_only_looks_like_a_type_name() {
        let mut b = Builder::new();
        b.put(Builder::FREE_RDATA + 0x40, b".?AVLoose@@\0");
        let bytes = b.finish();
        let image = Image::parse(&bytes).unwrap();
        let rtti = scan(&Layout::new(&image));
        assert!(rtti.classes.is_empty() && rtti.vtables.is_empty());
    }

    #[test]
    fn multiple_inheritance_lists_direct_bases_only() {
        let mut b = Builder::new();
        let r = Builder::FREE_RDATA;
        let type_info_vtable = Builder::va(Builder::TEXT);
        let tds: Vec<(u32, &str)> = [".?AVA@@", ".?AVB@@", ".?AVC@@", ".?AVD@@"]
            .iter()
            .enumerate()
            .map(|(i, n)| (r + 0x40 * i as u32, *n))
            .collect();
        for (td, name) in &tds {
            b.put64(*td, type_info_vtable);
            b.put(*td + 16, name.as_bytes());
        }
        // D : B, C and B : A. The array walk is D, B, A, C.
        let array = r + 0x200;
        let bcds = [r + 0x300, r + 0x320, r + 0x340, r + 0x360];
        let order = [(3usize, 3u32), (1, 1), (0, 0), (2, 0)];
        for (slot, (class, contained)) in order.iter().enumerate() {
            b.put32(bcds[slot], tds[*class].0);
            b.put32(bcds[slot] + 4, *contained);
            b.put32(array + 4 * slot as u32, bcds[slot]);
        }
        let (chd, col) = (r + 0x380, r + 0x3A0);
        b.put32(chd + 8, 4);
        b.put32(chd + 12, array);
        b.put32(col, 1);
        b.put32(col + 12, tds[3].0);
        b.put32(col + 16, chd);
        b.put32(col + 20, col);
        let bytes = b.finish();
        let image = Image::parse(&bytes).unwrap();
        let rtti = scan(&Layout::new(&image));
        let d = rtti.classes.iter().find(|c| c.name == "D").unwrap();
        assert_eq!(d.bases, ["B", "C"]);
        assert_eq!(d.ancestors, ["B", "A", "C"]);
    }
}
