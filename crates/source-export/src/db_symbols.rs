use exe_analysis::{NameEvidence, ProgramDb};

use crate::symbols::{ClassInfo, Symbols, default_name};

const SLOTS_SHOWN: usize = 4;

/// Names, classes, source files and notes from the whole-program database.
pub struct DbSymbols<'a> {
    db: &'a ProgramDb,
}

impl<'a> DbSymbols<'a> {
    pub fn new(db: &'a ProgramDb) -> Self {
        Self { db }
    }

    fn evidence(&self, evidence: &NameEvidence) -> String {
        match evidence {
            NameEvidence::Export { ordinal } => format!("named from export ordinal {ordinal}"),
            NameEvidence::ImportThunk { import } => format!("named after the import {import}"),
            NameEvidence::String { string } => match self.db.string(*string) {
                Some(text) => format!("named from the string {text:?} at 0x{string:08x}"),
                None => format!("named from the string at 0x{string:08x}"),
            },
            NameEvidence::VirtualSlot { vtable, slot } => {
                format!("named from slot {slot} of the vtable at 0x{vtable:08x}")
            }
            NameEvidence::GlobalWriter { function } => {
                format!("named from the global written by 0x{function:08x}")
            }
        }
    }
}

impl Symbols for DbSymbols<'_> {
    fn function_name(&self, rva: u32) -> String {
        self.db
            .name_of(rva)
            .map_or_else(|| default_name(rva), str::to_string)
    }

    fn class_of(&self, rva: u32) -> Option<String> {
        self.db.function(rva)?.class.clone()
    }

    fn source_file(&self, rva: u32) -> Option<String> {
        self.db.function(rva)?.source_file.clone()
    }

    fn notes(&self, rva: u32) -> Vec<String> {
        let Some(function) = self.db.function(rva) else {
            return Vec::new();
        };
        let mut notes = Vec::new();
        if let Some(evidence) = &function.name_evidence {
            notes.push(self.evidence(evidence));
        }
        if let Some(source) = &function.source_evidence {
            let how = if source.sole_reference {
                "only this function references"
            } else {
                "this function references"
            };
            notes.push(format!(
                "source file taken from the path string at 0x{:08x}, which {how}",
                source.string
            ));
        }
        if !function.vtable_slots.is_empty() {
            let shown: Vec<String> = function
                .vtable_slots
                .iter()
                .take(SLOTS_SHOWN)
                .map(|(vtable, slot)| {
                    let class = self.db.vtable(*vtable).map(|v| v.class.as_str());
                    format!("{}[{slot}] at 0x{vtable:08x}", class.unwrap_or("vtable"))
                })
                .collect();
            let more = function.vtable_slots.len().saturating_sub(SLOTS_SHOWN);
            let tail = if more > 0 {
                format!(", ... (+{more} more)")
            } else {
                String::new()
            };
            notes.push(format!("in vtables: {}{tail}", shown.join(", ")));
        }
        if function.discovered {
            notes.push("found from a call or a vtable, not the exception table".to_string());
        }
        notes
    }

    fn global_name(&self, rva: u32) -> Option<String> {
        self.db.global(rva)?.name.clone()
    }

    fn classes(&self) -> Vec<ClassInfo> {
        self.db
            .classes()
            .iter()
            .map(|c| ClassInfo {
                name: c.name.clone(),
                bases: c.bases.clone(),
                vtables: c.vtables.clone(),
                methods: c.methods.clone(),
            })
            .collect()
    }

    fn extra_functions(&self) -> Vec<(u32, u32)> {
        self.db
            .functions()
            .iter()
            .filter(|f| f.discovered)
            .map(|f| (f.rva, f.size))
            .collect()
    }
}
