//! File system calls the game makes, mapped onto the package, the virtual file system and Output/.
//!
//! Path rules are in [`map`]. The package is read-only: a write-open of a package file copies it into the overlay
//! first, a delete of one hides it for the rest of the run (nothing is written for that), and everything the game
//! creates lands under `Output/`. Handle-based calls (`ReadFile`, `WriteFile`, `SetFilePointerEx`, ...) are the
//! host's, since the shims hand out host handles.

use std::path::PathBuf;
use std::sync::RwLock;

use crate::Shim;

mod map;
pub use map::{Refusal, Roots, Target};

// integration
//
// The only place paths are resolved. `vfs::resolve` replaces `resolve` and `configure` once it exists; the shims
// below depend on nothing else.

static ROOTS: RwLock<Option<Roots>> = RwLock::new(None);

/// Sets the package folder and the `Output/` folder. Both must be absolute paths with drive letters. Until this is
/// called, paths pass to Windows unchanged.
pub fn configure(package: PathBuf, output: PathBuf) -> bool {
    let Some(roots) = Roots::new(package, output) else {
        return false;
    };
    if let Ok(mut slot) = ROOTS.write() {
        *slot = Some(roots);
        true
    } else {
        false
    }
}

/// Maps a path the game names.
pub fn resolve(path: &str) -> Result<Target, Refusal> {
    match ROOTS.read() {
        Ok(roots) => match roots.as_ref() {
            Some(roots) => roots.map(path),
            None => Ok(Target::Passthrough),
        },
        Err(_) => Ok(Target::Passthrough),
    }
}

// end of integration

#[cfg(windows)]
mod imp;
#[cfg(windows)]
pub use imp::read_location;

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        imp::shims()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
