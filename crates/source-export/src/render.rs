use std::fmt::Write;

use pe_image::Image;

use crate::image_data::{ImportTable, text_at};
use crate::plan::Plan;
use crate::scan::Facts;
use crate::symbols::{ClassInfo, Symbols};

/// The first line of every generated file.
pub const MARKER: &str = concat!(
    "// leonida-decomp",
    ": generated from your game executable, do not commit"
);

const CALLERS_SHOWN: usize = 12;
const CALLEES_SHOWN: usize = 12;
const IMPORTS_SHOWN: usize = 10;
const STRINGS_SHOWN: usize = 6;
const STRING_WIDTH: usize = 60;
const GLOBALS_SHOWN: usize = 8;
const VTABLE_SLOTS_FILLED: u32 = 1024;

/// Everything the text of a function needs besides its code.
pub struct View<'a> {
    pub image: &'a Image<'a>,
    pub facts: &'a Facts,
    pub plan: &'a Plan,
    pub imports: &'a ImportTable,
    pub symbols: &'a dyn Symbols,
}

pub enum Outcome {
    Decompiled(String),
    Stub(String),
    Failed(String),
}

fn list(out: &mut String, label: &str, total: usize, shown: Vec<String>) {
    if total == 0 {
        return;
    }
    let more = total - shown.len();
    let tail = if more > 0 {
        format!(", ... (+{more} more)")
    } else {
        String::new()
    };
    let _ = writeln!(out, "// {label} ({total}): {}{tail}", shown.join(", "));
}

fn quoted(text: &str) -> String {
    let mut shown: String = text.chars().take(STRING_WIDTH).collect();
    if text.chars().count() > STRING_WIDTH {
        shown.push_str("...");
    }
    format!("{shown:?}")
}

impl View<'_> {
    pub fn name(&self, index: usize) -> &str {
        &self.plan.names[index]
    }

    pub fn path_of(&self, index: usize) -> &str {
        &self.plan.groups[self.plan.group_of[index] as usize].path
    }

    pub fn function_header(&self, index: usize) -> String {
        let entry = self.facts.entries[index];
        let mut out = String::new();
        let size = if entry.size == 0 {
            "size unknown (no unwind data)".to_string()
        } else {
            format!("size 0x{:x} ({} bytes)", entry.size, entry.size)
        };
        let _ = writeln!(
            out,
            "// ---- {}  rva 0x{:08x}  {size}",
            self.name(index),
            entry.rva
        );
        if let Some(class) = &self.plan.classes[index] {
            let _ = writeln!(out, "// class: {class}");
        }
        if let Some(source) = self.symbols.source_file(entry.rva) {
            let _ = writeln!(out, "// source: {source}");
        }
        let named = |indices: &[u32], shown: usize| -> Vec<String> {
            indices
                .iter()
                .take(shown)
                .map(|&i| self.name(i as usize).to_string())
                .collect()
        };
        let callers = &self.facts.callers[index];
        list(
            &mut out,
            "callers",
            callers.len(),
            named(callers, CALLERS_SHOWN),
        );
        let callees = &self.facts.callees[index];
        list(
            &mut out,
            "callees",
            callees.len(),
            named(callees, CALLEES_SHOWN),
        );
        let imports = &self.facts.imports[index];
        list(
            &mut out,
            "imports",
            imports.len(),
            imports
                .iter()
                .take(IMPORTS_SHOWN)
                .map(|&i| {
                    let import = self.imports.get(i as usize);
                    format!("{}!{}", import.dll, import.name)
                })
                .collect(),
        );
        let strings = &self.facts.strings[index];
        list(
            &mut out,
            "strings",
            strings.len(),
            strings
                .iter()
                .take(STRINGS_SHOWN)
                .filter_map(|&rva| text_at(self.image, rva).map(|t| quoted(&t)))
                .collect(),
        );
        let globals = &self.facts.globals[index];
        list(
            &mut out,
            "globals",
            globals.len(),
            globals
                .iter()
                .take(GLOBALS_SHOWN)
                .filter_map(|&rva| self.symbols.global_name(rva))
                .collect(),
        );
        for note in self.symbols.notes(entry.rva) {
            let _ = writeln!(out, "// note: {}", note.replace('\n', " "));
        }
        out
    }

    pub fn function_text(&self, index: usize, outcome: &Outcome) -> String {
        let mut out = self.function_header(index);
        match outcome {
            Outcome::Decompiled(code) => out.push_str(code),
            Outcome::Stub(reason) => {
                let _ = writeln!(out, "// stub: {}", reason.replace('\n', " "));
                let _ = writeln!(out, "void {}(void);", self.name(index));
            }
            Outcome::Failed(error) => {
                let _ = writeln!(out, "// decompile failed: {}", error.replace('\n', " "));
                let _ = writeln!(out, "void {}(void);", self.name(index));
            }
        }
        out.push('\n');
        out
    }
}

pub fn identifier(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

pub struct Method {
    pub name: String,
    pub rva: u32,
    pub path: String,
}

/// The header for one class: its vtable as a struct of function pointers, then every method with a body.
pub fn class_header(
    name: &str,
    info: Option<&ClassInfo>,
    methods: &[Method],
    resolve: &dyn Fn(u32) -> Option<(String, String)>,
) -> String {
    let id = identifier(name);
    let mut out = String::new();
    let _ = writeln!(out, "// class {name}");
    if let Some(info) = info {
        if !info.bases.is_empty() {
            let _ = writeln!(out, "// bases: {}", info.bases.join(", "));
        }
        if !info.vtables.is_empty() {
            let rvas: Vec<String> = info.vtables.iter().map(|v| format!("0x{v:08x}")).collect();
            let _ = writeln!(out, "// vtables at {}", rvas.join(", "));
        }
    }
    let _ = writeln!(out, "\nstruct {id};\n");
    let mut slots: Vec<(u32, u32)> = info.map(|i| i.methods.clone()).unwrap_or_default();
    slots.sort_by_key(|(slot, _)| *slot);
    slots.dedup_by_key(|(slot, _)| *slot);
    if !slots.is_empty() {
        let _ = writeln!(out, "struct {id}_vtbl {{");
        let mut next = 0;
        for (slot, rva) in slots {
            if slot >= VTABLE_SLOTS_FILLED {
                break;
            }
            for gap in next..slot {
                let _ = writeln!(out, "    void *slot_{gap}; // unknown");
            }
            next = slot + 1;
            match resolve(rva) {
                Some((function, path)) => {
                    let _ = writeln!(
                        out,
                        "    void (*slot_{slot}_{})(struct {id} *self); // 0x{rva:08x} in {path}",
                        identifier(&function)
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "    void (*slot_{slot})(struct {id} *self); // 0x{rva:08x}"
                    );
                }
            }
        }
        let _ = writeln!(out, "}};\n");
        let _ = writeln!(
            out,
            "struct {id} {{\n    const struct {id}_vtbl *vtbl;\n}};\n"
        );
    }
    if !methods.is_empty() {
        let _ = writeln!(out, "// methods with bodies ({}):", methods.len());
        for method in methods {
            let _ = writeln!(
                out,
                "//   0x{:08x}  {}  {}",
                method.rva, method.name, method.path
            );
        }
    }
    out
}
