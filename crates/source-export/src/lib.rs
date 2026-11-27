//! Writes the whole executable as a browsable tree of source-like files.

mod image_data;
mod plan;
mod program;
mod scan;
mod symbols;

pub use plan::{Kind, Selector, class_path, source_path};
pub use symbols::{ClassInfo, ImageSymbols, Symbols, default_name};
