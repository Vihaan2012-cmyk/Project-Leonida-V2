use std::collections::HashMap;

use pe_image::{Function, Image};

use crate::image_data::{ImportTable, text_at, thunk_target};

/// The image as the decompiler sees it.
pub struct ImageProgram<'a> {
    image: &'a Image<'a>,
    functions: Vec<Function>,
    primary: Vec<u32>,
    import_names: HashMap<u32, String>,
}

impl<'a> ImageProgram<'a> {
    pub fn new(image: &'a Image<'a>, functions: &[Function], imports: &ImportTable) -> Self {
        let import_names = imports
            .iter()
            .map(|(_, import)| (import.slot, import.name.clone()))
            .collect();
        Self {
            image,
            functions: functions.to_vec(),
            primary: functions.iter().map(|f| image.primary(*f).begin).collect(),
            import_names,
        }
    }

    fn rva(&self, va: u64) -> Option<u32> {
        u32::try_from(va.checked_sub(self.image.image_base)?).ok()
    }

    fn owner(&self, rva: u32) -> Option<u32> {
        let index = self.functions.partition_point(|f| f.begin <= rva);
        let index = index.checked_sub(1)?;
        (rva < self.functions[index].end).then(|| self.primary[index])
    }
}

impl decompile::Program for ImageProgram<'_> {
    fn image_base(&self) -> u64 {
        self.image.image_base
    }

    fn code(&self, va: u64) -> Option<&[u8]> {
        let rva = self.rva(va)?;
        self.image
            .section_of(rva)?
            .executable()
            .then(|| self.image.bytes_at(rva))
            .flatten()
    }

    fn bytes(&self, va: u64) -> Option<&[u8]> {
        self.image.bytes_at(self.rva(va)?)
    }

    fn symbol(&self, va: u64) -> decompile::Symbol {
        use decompile::Symbol;
        let Some(rva) = self.rva(va) else {
            return Symbol::Unknown;
        };
        if let Some(name) = self.import_names.get(&rva) {
            return Symbol::Import(name.clone());
        }
        match self.image.section_of(rva) {
            Some(section) if section.executable() => {
                match thunk_target(self.image, rva).and_then(|slot| self.import_names.get(&slot)) {
                    Some(name) => Symbol::Import(name.clone()),
                    None => Symbol::Function,
                }
            }
            Some(_) => text_at(self.image, rva).map_or(Symbol::Data, Symbol::Text),
            None => Symbol::Unknown,
        }
    }

    fn inside(&self, entry: u64, target: u64) -> bool {
        let (Some(entry), Some(target)) = (self.rva(entry), self.rva(target)) else {
            return false;
        };
        match (self.owner(entry), self.owner(target)) {
            (None, None) => target.abs_diff(entry) < 0x400,
            (a, b) => a == b,
        }
    }
}
