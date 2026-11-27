//! The virtual file system: base game, Leonida's fixes and mods merged into one case-insensitive tree that every
//! consumer reads through, with all writes sent to `Output/`.

mod package;
mod path;
mod source;

pub use package::{Dependency, MANIFEST_NAME, Manifest, Package, PackageError, PackageType, Version};
pub use path::{PathError, VPath, fold, glob_match, joaat};
pub use source::{LooseSource, MemorySource, ReadSeek, Source, SourceFile};
