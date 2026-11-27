//! The whole-program database: functions, call graph, strings, globals, RTTI classes and
//! vtables, with names taken only from evidence in the executable.

mod build;
pub mod demangle;
mod model;
mod naming;
mod rtti;
mod scan;
#[cfg(test)]
mod testimage;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use pe_image::Image;
use serde::{Deserialize, Serialize};

pub use model::{ClassInfo, FunctionInfo, GlobalInfo, NameEvidence, SourceEvidence, VtableInfo};

const MAGIC: &[u8; 8] = b"LEODB\0\0\x01";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading or writing the database: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a program database of this version")]
    BadHeader,
    #[error("corrupt program database: {0}")]
    Corrupt(#[from] postcard::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildOptions {
    /// Worker threads used while scanning.
    pub threads: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions { threads: 4 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramDb {
    image_base: u64,
    functions: Vec<FunctionInfo>,
    globals: Vec<GlobalInfo>,
    classes: Vec<ClassInfo>,
    vtables: Vec<VtableInfo>,
    strings: Vec<(u32, String)>,
    #[serde(skip)]
    chunks: Vec<(u32, u32, u32)>,
}

/// Counts for a quick look at what the analysis found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub functions: usize,
    pub discovered: usize,
    pub call_edges: usize,
    pub functions_with_strings: usize,
    pub functions_with_imports: usize,
    pub named: usize,
    pub named_by_export: usize,
    pub named_by_import_thunk: usize,
    pub named_by_string: usize,
    pub named_by_vtable_slot: usize,
    pub with_class: usize,
    pub with_source_file: usize,
    pub source_files: usize,
    pub classes: usize,
    pub vtables: usize,
    pub globals: usize,
    pub named_globals: usize,
    pub strings: usize,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} functions ({} found from calls and vtables), {} call edges",
            self.functions, self.discovered, self.call_edges
        )?;
        writeln!(
            f,
            "  {} reference text, {} use imports, {} have a class",
            self.functions_with_strings, self.functions_with_imports, self.with_class
        )?;
        writeln!(f, "{} named functions:", self.named)?;
        writeln!(f, "  {:>8}  from exports", self.named_by_export)?;
        writeln!(f, "  {:>8}  from import thunks", self.named_by_import_thunk)?;
        writeln!(
            f,
            "  {:>8}  from assert and log strings",
            self.named_by_string
        )?;
        writeln!(
            f,
            "  {:>8}  from vtable slots (Class::vfN)",
            self.named_by_vtable_slot
        )?;
        writeln!(f, "{} classes, {} vtables", self.classes, self.vtables)?;
        writeln!(
            f,
            "{} source files, {} functions placed in one",
            self.source_files, self.with_source_file
        )?;
        write!(
            f,
            "{} globals ({} named), {} strings",
            self.globals, self.named_globals, self.strings
        )
    }
}

impl ProgramDb {
    /// Analyses the executable with the default options.
    pub fn build(image: &Image<'_>) -> ProgramDb {
        Self::build_with(image, &BuildOptions::default())
    }

    pub fn build_with(image: &Image<'_>, options: &BuildOptions) -> ProgramDb {
        build::run(image, options)
    }

    pub(crate) fn assemble(
        image_base: u64,
        functions: Vec<FunctionInfo>,
        globals: Vec<GlobalInfo>,
        classes: Vec<ClassInfo>,
        vtables: Vec<VtableInfo>,
        strings: Vec<(u32, String)>,
    ) -> ProgramDb {
        let mut db = ProgramDb {
            image_base,
            functions,
            globals,
            classes,
            vtables,
            strings,
            chunks: Vec::new(),
        };
        db.reindex();
        db
    }

    fn reindex(&mut self) {
        self.chunks = self
            .functions
            .iter()
            .enumerate()
            .flat_map(|(index, f)| f.chunks.iter().map(move |&(b, e)| (b, e, index as u32)))
            .collect();
        self.chunks.sort_unstable();
    }

    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(postcard::to_stdvec(self)?);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<ProgramDb, Error> {
        let bytes = std::fs::read(path)?;
        let body = bytes.strip_prefix(MAGIC).ok_or(Error::BadHeader)?;
        let mut db: ProgramDb = postcard::from_bytes(body)?;
        db.reindex();
        Ok(db)
    }

    pub fn image_base(&self) -> u64 {
        self.image_base
    }

    /// Every function, sorted by RVA.
    pub fn functions(&self) -> &[FunctionInfo] {
        &self.functions
    }

    /// The function that starts at `rva`.
    pub fn function(&self, rva: u32) -> Option<&FunctionInfo> {
        let index = self.functions.binary_search_by_key(&rva, |f| f.rva).ok()?;
        Some(&self.functions[index])
    }

    /// The function with a piece of code at `rva`.
    pub fn function_containing(&self, rva: u32) -> Option<&FunctionInfo> {
        let at = self.chunks.partition_point(|c| c.0 <= rva);
        let &(_, end, index) = self.chunks[..at].last()?;
        (rva < end).then(|| &self.functions[index as usize])
    }

    /// The evidence-based name of the function or global at `rva`; `None` when it has none.
    pub fn name_of(&self, rva: u32) -> Option<&str> {
        self.function(rva)
            .and_then(|f| f.name.as_deref())
            .or_else(|| self.global(rva).and_then(|g| g.name.as_deref()))
    }

    /// The name to print for a function: its evidence-based name or `sub_<rva>`.
    pub fn display_name(&self, rva: u32) -> String {
        self.name_of(rva)
            .map_or_else(|| format!("sub_{rva:08x}"), str::to_string)
    }

    /// Globals in writable data that code references, sorted by RVA.
    pub fn globals(&self) -> &[GlobalInfo] {
        &self.globals
    }

    pub fn global(&self, rva: u32) -> Option<&GlobalInfo> {
        let index = self.globals.binary_search_by_key(&rva, |g| g.rva).ok()?;
        Some(&self.globals[index])
    }

    pub fn classes(&self) -> &[ClassInfo] {
        &self.classes
    }

    pub fn class(&self, name: &str) -> Option<&ClassInfo> {
        self.classes.iter().find(|c| c.name == name)
    }

    /// Every vtable found through its object locator, sorted by RVA.
    pub fn vtables(&self) -> &[VtableInfo] {
        &self.vtables
    }

    pub fn vtable(&self, rva: u32) -> Option<&VtableInfo> {
        let index = self.vtables.binary_search_by_key(&rva, |v| v.rva).ok()?;
        Some(&self.vtables[index])
    }

    /// The text of a string that code references.
    pub fn string(&self, rva: u32) -> Option<&str> {
        let index = self.strings.binary_search_by_key(&rva, |s| s.0).ok()?;
        Some(&self.strings[index].1)
    }

    /// Recovered source paths with the functions placed in each, sorted by path.
    pub fn source_files(&self) -> Vec<(String, Vec<u32>)> {
        let mut files: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
        for function in &self.functions {
            if let Some(file) = &function.source_file {
                files.entry(file).or_default().push(function.rva);
            }
        }
        files
            .into_iter()
            .map(|(file, functions)| (file.to_string(), functions))
            .collect()
    }

    pub fn summary(&self) -> Summary {
        let mut summary = Summary {
            functions: self.functions.len(),
            classes: self.classes.len(),
            vtables: self.vtables.len(),
            globals: self.globals.len(),
            named_globals: self.globals.iter().filter(|g| g.name.is_some()).count(),
            strings: self.strings.len(),
            ..Summary::default()
        };
        let mut files = BTreeSet::new();
        for function in &self.functions {
            summary.discovered += usize::from(function.discovered);
            summary.call_edges += function.callees.len();
            summary.functions_with_strings += usize::from(!function.strings.is_empty());
            summary.functions_with_imports += usize::from(!function.imports.is_empty());
            summary.with_class += usize::from(function.class.is_some());
            summary.named += usize::from(function.name.is_some());
            match function.name_evidence {
                Some(NameEvidence::Export { .. }) => summary.named_by_export += 1,
                Some(NameEvidence::ImportThunk { .. }) => summary.named_by_import_thunk += 1,
                Some(NameEvidence::String { .. }) => summary.named_by_string += 1,
                Some(NameEvidence::VirtualSlot { .. }) => summary.named_by_vtable_slot += 1,
                _ => {}
            }
            if let Some(file) = &function.source_file {
                summary.with_source_file += 1;
                files.insert(file.as_str());
            }
        }
        summary.source_files = files.len();
        summary
    }
}

#[cfg(test)]
mod tests;
