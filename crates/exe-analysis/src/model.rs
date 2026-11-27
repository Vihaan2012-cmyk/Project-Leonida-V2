use serde::{Deserialize, Serialize};

/// What a function's name was taken from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NameEvidence {
    /// The executable exports it under this name.
    Export { ordinal: u32 },
    /// It only jumps through this import (the thunk is named after the import).
    ImportThunk { import: String },
    /// A string that contains the name is referenced from this function and no other.
    String { string: u32 },
    /// It is slot `slot` of the vtable at `vtable`, so it is `Class::vf<slot>`.
    VirtualSlot { vtable: u32, slot: u32 },
    /// A global's name: the one named function that writes it.
    GlobalWriter { function: u32 },
}

impl NameEvidence {
    pub fn kind(&self) -> &'static str {
        match self {
            NameEvidence::Export { .. } => "export",
            NameEvidence::ImportThunk { .. } => "import thunk",
            NameEvidence::String { .. } => "assert/log string",
            NameEvidence::VirtualSlot { .. } => "vtable slot",
            NameEvidence::GlobalWriter { .. } => "single writer",
        }
    }
}

/// Where a function's source file came from: a path string such as `x:\src\foo.cpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEvidence {
    pub string: u32,
    /// The path string is referenced from this function and no other.
    pub sole_reference: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionInfo {
    pub rva: u32,
    /// Bytes of code across all its pieces.
    pub size: u32,
    /// `(begin, end)` of each piece; the entry at `rva` comes first. More than one when the
    /// compiler split the function and chained its unwind data.
    pub chunks: Vec<(u32, u32)>,
    /// Found by following a call or a vtable rather than from the exception table.
    pub discovered: bool,
    pub name: Option<String>,
    pub name_evidence: Option<NameEvidence>,
    pub class: Option<String>,
    pub source_file: Option<String>,
    pub source_evidence: Option<SourceEvidence>,
    /// Functions that call or tail-jump to this one (primary RVAs).
    pub callers: Vec<u32>,
    /// Functions this one calls or tail-jumps to (primary RVAs).
    pub callees: Vec<u32>,
    /// RVAs of text referenced by RIP-relative `lea` and `mov`.
    pub strings: Vec<u32>,
    /// Imported function names used directly or through a thunk it calls; ordinals read
    /// `dll!#n`.
    pub imports: Vec<String>,
    /// `(vtable RVA, slot)` for each vtable slot that holds this function.
    pub vtable_slots: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalInfo {
    pub rva: u32,
    pub name: Option<String>,
    pub name_evidence: Option<NameEvidence>,
    /// Functions that read it, write it, or take its address (primary RVAs).
    pub readers: Vec<u32>,
    pub writers: Vec<u32>,
    pub address_taken: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassInfo {
    /// `A::B` form; the decorated name when it cannot be demangled.
    pub name: String,
    pub type_descriptor: u32,
    pub vtables: Vec<u32>,
    /// Direct base classes.
    pub bases: Vec<String>,
    /// Every base class, as the class hierarchy descriptor lists them.
    pub ancestors: Vec<String>,
    /// `(slot, function RVA)` of the primary vtable (the one at offset 0 when there is one).
    pub methods: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VtableInfo {
    /// The address the object's vptr holds: the first slot.
    pub rva: u32,
    pub class: String,
    pub type_descriptor: u32,
    pub locator: u32,
    /// Offset of this vtable's subobject in the class.
    pub offset: u32,
    /// Function RVA of each slot, up to the first slot that holds no code address.
    pub slots: Vec<u32>,
}
