use std::collections::HashMap;

use pe_image::Image;

use crate::image_data::{ImportTable, thunk_target};

/// A class with the vtables and virtual methods known for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassInfo {
    pub name: String,
    pub bases: Vec<String>,
    pub vtables: Vec<u32>,
    /// (vtable slot, function RVA)
    pub methods: Vec<(u32, u32)>,
}

/// What is known about the executable's functions and data beyond the bytes themselves.
pub trait Symbols: Sync {
    /// The name shown for the function at `rva`: always something, `sub_<rva>` when nothing is known.
    fn function_name(&self, rva: u32) -> String;
    fn class_of(&self, rva: u32) -> Option<String>;
    /// The recovered source path the function came from.
    fn source_file(&self, rva: u32) -> Option<String>;
    fn notes(&self, rva: u32) -> Vec<String>;
    /// The name of the global variable at `rva`, if it has one.
    fn global_name(&self, rva: u32) -> Option<String>;
    /// Classes to give a header file each, even when none of their methods has a body.
    fn classes(&self) -> Vec<ClassInfo> {
        Vec::new()
    }
}

pub fn default_name(rva: u32) -> String {
    format!("sub_{rva:08x}")
}

/// Names from the image alone: import thunks, exports and `sub_<rva>`.
pub struct ImageSymbols<'a> {
    image: &'a Image<'a>,
    imports: ImportTable,
    exports: HashMap<u32, String>,
}

impl<'a> ImageSymbols<'a> {
    pub fn new(image: &'a Image<'a>) -> Self {
        let mut exports = HashMap::new();
        for export in image.exports() {
            if export.forwarder.is_some() {
                continue;
            }
            let name = export
                .name
                .unwrap_or_else(|| format!("ordinal_{}", export.ordinal));
            exports.entry(export.rva).or_insert(name);
        }
        Self {
            image,
            imports: ImportTable::new(image),
            exports,
        }
    }

    fn in_code(&self, rva: u32) -> bool {
        self.image.section_of(rva).is_some_and(|s| s.executable())
    }
}

impl Symbols for ImageSymbols<'_> {
    fn function_name(&self, rva: u32) -> String {
        if let Some(name) = self.exports.get(&rva).filter(|_| self.in_code(rva)) {
            return name.clone();
        }
        if let Some(index) =
            thunk_target(self.image, rva).and_then(|slot| self.imports.by_slot(slot))
        {
            return format!("j_{}", self.imports.get(index).name);
        }
        default_name(rva)
    }

    fn class_of(&self, _rva: u32) -> Option<String> {
        None
    }

    fn source_file(&self, _rva: u32) -> Option<String> {
        None
    }

    fn notes(&self, rva: u32) -> Vec<String> {
        let mut notes = Vec::new();
        if rva == self.image.entry {
            notes.push("image entry point".to_string());
        }
        if let Some(index) =
            thunk_target(self.image, rva).and_then(|slot| self.imports.by_slot(slot))
        {
            let import = self.imports.get(index);
            notes.push(format!("import thunk for {}!{}", import.dll, import.name));
        }
        if self.exports.contains_key(&rva) && self.in_code(rva) {
            notes.push("exported".to_string());
        }
        notes
    }

    fn global_name(&self, rva: u32) -> Option<String> {
        self.exports
            .get(&rva)
            .filter(|_| !self.in_code(rva))
            .cloned()
    }
}
