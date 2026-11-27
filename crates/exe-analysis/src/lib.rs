//! The whole-program database.

pub mod demangle;
mod model;

pub use model::{ClassInfo, FunctionInfo, GlobalInfo, NameEvidence, SourceEvidence, VtableInfo};
