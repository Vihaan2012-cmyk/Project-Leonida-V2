//! Reader for the game's `.rpf` archives, and the asset index built from them.
//!
//! The table of contents of an archive is encrypted and never read. Resources are found by walking the data
//! area, validating every stream by inflating it, and joining streams by the rules of the measured formats.

mod archive;
mod error;
mod group;
mod index;
mod inflate;
mod model;
mod names;
mod package;
mod walk;

#[cfg(test)]
mod tests;

pub use archive::{Archive, Header, ResourceData, ScanReport};
pub use error::Rpf9Error;
pub use index::{ArchiveInfo, Index, Query};
pub use model::{Codec, Kind, Layout, Resource, Segment, Space, hex};
pub use names::{find_names, joaat, name_hash, stem};
pub use package::{IndexOptions, Progress, index_archive, index_package, package_archives};
pub use walk::WalkStats;
