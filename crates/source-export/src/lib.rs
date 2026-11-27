//! Writes the whole executable as a browsable tree of source-like files.

mod image_data;
mod program;
mod symbols;

pub use symbols::{ClassInfo, ImageSymbols, Symbols, default_name};
