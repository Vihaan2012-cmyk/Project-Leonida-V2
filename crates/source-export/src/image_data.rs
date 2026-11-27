use std::collections::HashMap;

use iced_x86::{Decoder, DecoderOptions, FlowControl};
use pe_image::Image;

pub struct ImportRef {
    pub dll: String,
    pub name: String,
    pub slot: u32,
}

/// Every imported function, indexed, with the IAT slot each one is read through.
pub struct ImportTable {
    imports: Vec<ImportRef>,
    slots: HashMap<u32, usize>,
}

impl ImportTable {
    pub fn new(image: &Image<'_>) -> Self {
        let mut imports = Vec::new();
        let mut slots = HashMap::new();
        for dll in image.imports() {
            for entry in dll.entries {
                let name = match (entry.name, entry.ordinal) {
                    (Some(name), _) => name,
                    (None, Some(ordinal)) => format!("#{ordinal}"),
                    _ => continue,
                };
                slots.insert(entry.iat_rva, imports.len());
                imports.push(ImportRef {
                    dll: dll.name.clone(),
                    name,
                    slot: entry.iat_rva,
                });
            }
        }
        Self { imports, slots }
    }

    pub fn by_slot(&self, slot: u32) -> Option<usize> {
        self.slots.get(&slot).copied()
    }

    pub fn get(&self, index: usize) -> &ImportRef {
        &self.imports[index]
    }

    pub fn len(&self) -> usize {
        self.imports.len()
    }

    pub fn is_empty(&self) -> bool {
        self.imports.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &ImportRef)> {
        self.imports.iter().enumerate()
    }
}

/// The IAT slot an import thunk (`jmp qword ptr [rip+x]`) at `rva` jumps through.
pub fn thunk_target(image: &Image<'_>, rva: u32) -> Option<u32> {
    let bytes = image.read(rva, 16)?;
    let mut decoder = Decoder::with_ip(
        64,
        bytes,
        image.image_base + u64::from(rva),
        DecoderOptions::NONE,
    );
    let instruction = decoder.decode();
    (instruction.flow_control() == FlowControl::IndirectBranch
        && instruction.is_ip_rel_memory_operand())
    .then(|| (instruction.ip_rel_memory_address() - image.image_base) as u32)
}

/// A NUL-terminated run of at least four printable bytes at `rva` in a data section.
pub fn text_at(image: &Image<'_>, rva: u32) -> Option<String> {
    if image.section_of(rva)?.executable() {
        return None;
    }
    let bytes = image.bytes_at(rva)?;
    let mut end = 0;
    for &byte in bytes.iter().take(512) {
        if byte == 0 {
            break;
        }
        if !((0x20..0x7F).contains(&byte) || matches!(byte, b'\t' | b'\n' | b'\r')) {
            return None;
        }
        end += 1;
    }
    (end >= 4 && end < bytes.len() && bytes[end] == 0)
        .then(|| String::from_utf8_lossy(&bytes[..end]).into_owned())
}
