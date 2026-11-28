//! File system calls the game makes, mapped onto the package, the virtual file system and Output/.
//!
//! Path rules are in `map`. The package is read-only: a write-open of a package file copies it into the overlay
//! first, a delete of one hides it for the rest of the run (nothing is written for that), and everything the game
//! creates lands under `Output/`. Handle-based calls (`ReadFile`, `WriteFile`, `SetFilePointerEx`, ...) are the
//! host's, since the shims hand out host handles.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::Shim;

mod blocked;
mod map;
mod policy;
pub use blocked::blocked_api;
pub use map::{Refusal, Roots, Target};

// integration
//
// The only place paths are resolved. `vfs::resolve` replaces `resolve` and `configure` once it exists; the shims
// below depend on nothing else.

/// Held by tests that read or change the folders, since they are process-wide.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

static ROOTS: RwLock<Option<Roots>> = RwLock::new(None);

/// Sets the package folder and the `Output/` folder. Both must be absolute paths with drive letters. Until this is
/// called, paths pass to Windows unchanged, and after it any path outside the two still reaches the host: use
/// [`configure_sandboxed`] to close them.
pub fn configure(package: PathBuf, output: PathBuf) -> bool {
    Roots::new(package, output).is_some_and(install)
}

/// Like [`configure`], and closes every path but the package (read only), `Output/` (read and write) and the
/// `reads` folders (read only, for the mods and fixes). Everything else is refused as the console would refuse a
/// path outside its sandbox, and no link inside the allowed folders may lead out of them.
pub fn configure_sandboxed(package: PathBuf, output: PathBuf, reads: Vec<PathBuf>) -> bool {
    Roots::new(package, output)
        .and_then(|roots| roots.sandboxed(reads))
        .is_some_and(install)
}

fn install(roots: Roots) -> bool {
    if let Ok(mut slot) = ROOTS.write() {
        *slot = Some(roots);
        true
    } else {
        false
    }
}

/// Forgets the folders, so paths pass to Windows unchanged again.
pub fn unconfigure() {
    if let Ok(mut slot) = ROOTS.write() {
        *slot = None;
    }
}

/// Whether the sandbox is on, which also decides what `LoadLibrary`, the registry and the pipes may do.
pub fn sandboxed() -> bool {
    ROOTS
        .read()
        .ok()
        .is_some_and(|roots| roots.as_ref().is_some_and(Roots::is_sandboxed))
}

/// Whether the game folder holds `path`, for the library loader to tell a console DLL's name from a stray path.
pub fn in_package(path: &str) -> bool {
    ROOTS.read().ok().is_some_and(|roots| {
        roots.as_ref().is_some_and(|roots| {
            matches!(roots.map(path), Ok(Target::Layered { base, .. }) if base.starts_with(roots.package()))
        })
    })
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
        roots.verify(&target)?;
        return Ok(target);
    };
    let replaced = REDIRECT.read().ok().and_then(|slot| {
        let relative = base.strip_prefix(roots.package()).ok()?;
        slot.as_ref().and_then(|redirect| redirect(relative))
    });
    let target = Target::Layered {
        base: replaced.unwrap_or(base),
        overlay,
    };
    roots.verify(&target)?;
    Ok(target)
}

// end of integration

#[cfg(windows)]
mod imp;
#[cfg(windows)]
pub use imp::read_location;

/// Opens a file the game named for reading, through the same mapping and sandbox as its own file calls.
pub fn open_read(text: &str) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        let path = imp::read_location(text)
            .map_err(|code| std::io::Error::from_raw_os_error(code as i32))?;
        std::fs::File::open(path)
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(text)
    }
}

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
