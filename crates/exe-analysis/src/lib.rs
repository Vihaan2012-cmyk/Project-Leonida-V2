//! The whole-program database.

pub mod demangle;
mod model;
mod rtti;
mod scan;
#[cfg(test)]
mod testimage;

pub use model::{ClassInfo, FunctionInfo, GlobalInfo, NameEvidence, SourceEvidence, VtableInfo};
