//! Game behaviour data to readable text and back, exactly.
//!
//! - [`rbf0`] and [`rbf0_xml`]: RBF0 binary XML to a tree and to XML that converts back to identical bytes.
//! - [`text`]: localised string tables to `Systems/Text/<language>.json`, patched back in place.
//! - [`catalog`]: which `Systems/` folder each kind of file goes to, and notes on what is not decoded.
//! - [`convert`]: the byte-level entry points the CLI and the archive exporter use.

pub mod catalog;
pub mod convert;
mod error;
pub mod rbf0;
pub mod rbf0_xml;
pub mod text;
pub mod xml;

pub use error::{Error, Result};
