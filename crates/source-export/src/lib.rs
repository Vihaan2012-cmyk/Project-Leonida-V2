//! Writes the whole executable as a browsable tree of source-like files.
//!
//! Every generated file starts with a marker line and is derived from game code: it is never committed.

mod db_symbols;
mod export;
mod image_data;
mod index;
mod plan;
mod program;
mod render;
mod scan;
mod symbols;

pub use db_symbols::DbSymbols;
pub use export::{Error, Options, Summary, export};
pub use plan::{Kind, Selector, class_path, source_path};
pub use render::MARKER;
pub use symbols::{ClassInfo, ImageSymbols, Symbols, default_name};
