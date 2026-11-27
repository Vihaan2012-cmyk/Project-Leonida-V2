//! Reader for the game's `.rpf` archives, and the asset index built from them.
//!
//! The table of contents of an archive is encrypted and never read. Resources are found by walking the data
//! area, validating every stream by inflating it, and joining streams by the rules of the measured formats.

mod error;

pub use error::Rpf9Error;
