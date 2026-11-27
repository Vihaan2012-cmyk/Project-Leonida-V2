//! The virtual file system: base game, Leonida's fixes and mods merged into one case-insensitive tree that every
//! consumer reads through, with all writes sent to `Output/`.

mod filter;
mod order;
mod package;
mod path;
mod source;
mod tree;
mod write;

pub use filter::{Action, ContentFilter, FilterError, Match, Rule, Verdict};
pub use order::{ConfigError, ModEntry, ModSet, ModStatus, ModsConfig, Provided};
pub use package::{Dependency, MANIFEST_NAME, Manifest, Package, PackageError, PackageType, Version};
pub use path::{PathError, VPath, fold, glob_match, joaat};
pub use source::{LooseSource, MemorySource, ReadSeek, Source, SourceFile};
pub use tree::{BuildError, Conflict, DirEntry, FileInfo, LayerInfo, LayerKind, LoadReport, Vfs, VfsBuilder, VfsError};
pub use write::{WriteError, WritePolicy, encode_name};
