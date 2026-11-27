//! File system calls the game makes, mapped onto the package, the virtual file system and Output/.
//!
//! Path rules are in `map`. The package is read-only: a write-open of a package file copies it into the overlay
//! first, a delete of one hides it for the rest of the run (nothing is written for that), and everything the game
//! creates lands under `Output/`. Handle-based calls (`ReadFile`, `WriteFile`, `SetFilePointerEx`, ...) are the
//! host's, since the shims hand out host handles.

use std::path::{Path, PathBuf};
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

/// Where a package file really is when a mod or a fix replaces it: given the path below the package folder, the
/// file that is read instead, or `None` to read the package's own.
pub type Redirect = Box<dyn Fn(&Path) -> Option<PathBuf> + Send + Sync>;

static REDIRECT: RwLock<Option<Redirect>> = RwLock::new(None);

/// Sets or clears the redirect for package files. Writes still land in the overlay, and a listing of a folder shows
/// only the package's own names and the overlay's, so a file a mod adds can be opened but not found by listing.
pub fn set_redirect(redirect: Option<Redirect>) {
    if let Ok(mut slot) = REDIRECT.write() {
        *slot = redirect;
    }
}

/// Maps a path the game names.
pub fn resolve(path: &str) -> Result<Target, Refusal> {
    let Ok(roots) = ROOTS.read() else {
        return Ok(Target::Passthrough);
    };
    let Some(roots) = roots.as_ref() else {
        return Ok(Target::Passthrough);
    };
    let target = roots.map(path)?;
    let Target::Layered { base, overlay } = target else {
        return Ok(target);
    };
    let replaced = REDIRECT.read().ok().and_then(|slot| {
        let relative = base.strip_prefix(roots.package()).ok()?;
        slot.as_ref().and_then(|redirect| redirect(relative))
    });
    Ok(Target::Layered {
        base: replaced.unwrap_or(base),
        overlay,
    })
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
