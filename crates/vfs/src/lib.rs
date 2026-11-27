//! The virtual file system: base game, Leonida's fixes and mods merged into one case-insensitive tree that every
//! consumer reads through, with all writes sent to `Output/`.

mod path;

pub use path::{PathError, VPath, fold, glob_match, joaat};
