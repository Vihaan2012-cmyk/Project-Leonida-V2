//! Loading Windows's own libraries: only from the Windows system folder, never by the default search order,
//! which would also look in the application folder, the current folder and `PATH`. The game folder holds copies
//! of the C++ runtime and other libraries whose signatures do not verify, and none of them may ever be loaded.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use thiserror::Error;
use windows_sys::Win32::Foundation::{FreeLibrary, GetLastError};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, LoadLibraryExW, SetDefaultDllDirectories,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

/// `LOAD_LIBRARY_SEARCH_SYSTEM32`: the system folder is the only place searched.
pub const SEARCH_SYSTEM32: u32 = 0x800;
/// `LOAD_LIBRARY_SEARCH_APPLICATION_DIR`: the folder of Leonida's own executable.
pub const SEARCH_APPLICATION_DIR: u32 = 0x200;

/// The `LoadLibraryEx` flags a program may keep: the ones that only say how the library is mapped. Every
/// `LOAD_LIBRARY_SEARCH_*` bit is dropped, so the program cannot widen the search.
const KEPT_FLAGS: u32 = 0x1 | 0x2 | 0x10 | 0x20 | 0x40 | 0x80;

const ERROR_MOD_NOT_FOUND: u32 = 126;
const REDISTRIBUTABLE: &[&str] = &[
    "msvcp140",
    "vcruntime140",
    "concrt140",
    "vccorlib140",
    "vcomp140",
];

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SystemLibraryError {
    #[error("{}", missing_text(.dll, .folder))]
    Missing { dll: String, folder: PathBuf },
    #[error("Windows found {dll} in the system folder but could not load it (error {code})")]
    Failed { dll: String, code: u32 },
    #[error(
        "{0} is not a plain library name; Leonida loads Windows libraries by name from the Windows system folder only"
    )]
    NotAName(String),
    #[error("{dll} came from {found}, not from the Windows system folder; Leonida unloaded it")]
    Outside { dll: String, found: String },
}

fn missing_text(dll: &str, folder: &Path) -> String {
    let lower = dll.to_ascii_lowercase();
    if REDISTRIBUTABLE.iter().any(|name| lower.starts_with(name)) {
        format!(
            "{dll} is missing from the Windows system folder ({}). It belongs to the Microsoft Visual C++ \
             Redistributable: install the latest x64 version from Microsoft, then start Leonida again. \
             Leonida never loads the copy in the game folder.",
            folder.display()
        )
    } else {
        format!(
            "{dll} is missing from the Windows system folder ({}). It is a Windows component: repair or \
             update Windows, then start Leonida again. Leonida never loads a copy from the game folder.",
            folder.display()
        )
    }
}

/// The Windows system folder, such as `C:\Windows\System32`.
pub fn system_directory() -> Option<PathBuf> {
    let mut buffer = [0u16; 520];
    // SAFETY: fills a buffer of the length given.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    (length > 0 && length < buffer.len())
        .then(|| PathBuf::from(OsString::from_wide(&buffer[..length])))
}

/// Libraries named by an API set are resolved by the loader, not found as files.
fn api_set(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("api-ms-win-") || lower.starts_with("ext-ms-win-")
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

fn module_path(module: usize) -> Option<String> {
    let mut buffer = [0u16; 1024];
    // SAFETY: fills a buffer of the length given.
    let length = unsafe {
        GetModuleFileNameW(
            module as *mut std::ffi::c_void,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    (length > 0 && length < buffer.len()).then(|| String::from_utf16_lossy(&buffer[..length]))
}

fn same_file(a: &str, b: &Path) -> bool {
    let plain = |text: &str| {
        text.trim_start_matches("\\\\?\\")
            .replace('/', "\\")
            .to_lowercase()
    };
    plain(a) == plain(&b.to_string_lossy())
}

/// Loads the Windows library `name` from the system folder. `flags` are the program's `LoadLibraryEx` flags, of
/// which only the ones about how the library is mapped are kept.
pub fn load_in(folder: &Path, name: &str, flags: u32) -> Result<usize, SystemLibraryError> {
    if name.is_empty() || name.contains(['\\', '/', ':']) || name.contains("..") {
        return Err(SystemLibraryError::NotAName(name.to_string()));
    }
    let file = if name.contains('.') {
        name.to_string()
    } else {
        format!("{name}.dll")
    };
    let missing = || SystemLibraryError::Missing {
        dll: file.clone(),
        folder: folder.to_path_buf(),
    };
    let flags = (flags & KEPT_FLAGS) | SEARCH_SYSTEM32;
    let by_name = api_set(&file);
    let full = folder.join(&file);
    if !by_name && !full.is_file() {
        return Err(missing());
    }
    let wanted = if by_name {
        wide(std::ffi::OsStr::new(&file))
    } else {
        wide(full.as_os_str())
    };
    // SAFETY: loads a library from the Windows system folder only: by full path there, or by an API set name,
    // which the loader resolves itself; the flag keeps its dependencies there too.
    let module = unsafe { LoadLibraryExW(wanted.as_ptr(), std::ptr::null_mut(), flags) } as usize;
    if module == 0 {
        // SAFETY: reads the calling thread's last error.
        let code = unsafe { GetLastError() };
        return Err(if code == ERROR_MOD_NOT_FOUND && by_name {
            missing()
        } else {
            SystemLibraryError::Failed { dll: file, code }
        });
    }
    if !by_name && flags & 0x3 == 0 {
        let found = module_path(module).unwrap_or_default();
        if !same_file(&found, &full) {
            // SAFETY: releases the module just loaded.
            unsafe { FreeLibrary(module as *mut std::ffi::c_void) };
            return Err(SystemLibraryError::Outside { dll: file, found });
        }
    }
    Ok(module)
}

/// Loads the Windows library `name` from the system folder.
pub fn load(name: &str, flags: u32) -> Result<usize, SystemLibraryError> {
    let folder = system_directory().ok_or_else(|| SystemLibraryError::Missing {
        dll: name.to_string(),
        folder: PathBuf::from("the Windows system folder"),
    })?;
    load_in(&folder, name, flags)
}

/// Stops at the first of `names` that Windows cannot provide from its system folder.
pub fn require<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<(), SystemLibraryError> {
    for name in names {
        load(name, 0)?;
    }
    Ok(())
}

/// Restricts what a library load without its own flags searches to the system folder and Leonida's own
/// folder, for the rest of the process: the game folder and the current folder are never searched.
pub fn confine_search_path() -> Result<(), u32> {
    // SAFETY: sets the process's default library search folders.
    if unsafe { SetDefaultDllDirectories(SEARCH_SYSTEM32 | SEARCH_APPLICATION_DIR) } == 0 {
        // SAFETY: reads the calling thread's last error.
        return Err(unsafe { GetLastError() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_library_loads_from_the_system_folder() {
        let folder = system_directory().expect("system folder");
        assert!(folder.is_dir());
        for name in [
            "kernel32.dll",
            "KERNEL32",
            "ucrtbase.dll",
            "api-ms-win-core-file-l1-1-0.dll",
        ] {
            assert!(load(name, 0).is_ok(), "{name}");
        }
        let module = load("kernel32.dll", 0).expect("kernel32");
        assert!(same_file(
            &module_path(module).expect("path"),
            &folder.join("kernel32.dll")
        ));
    }

    #[test]
    fn names_with_folders_are_not_names() {
        for name in [
            "C:\\Windows\\System32\\kernel32.dll",
            "..\\kernel32.dll",
            "game/msvcp140.dll",
            "kernel32.dll:stream",
            "",
        ] {
            assert_eq!(
                load(name, 0),
                Err(SystemLibraryError::NotAName(name.to_string())),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_copy_beside_the_program_or_in_the_current_folder_is_never_loaded() {
        let system = system_directory().expect("system folder");
        let decoy = "leonida-decoy-system-library.dll";
        let beside = std::env::current_exe().expect("exe").with_file_name(decoy);
        std::fs::copy(system.join("version.dll"), &beside).expect("copy");
        let here = Path::new(decoy);
        let _ = std::fs::copy(system.join("version.dll"), here);
        let outcome = load(decoy, 0);
        let _ = std::fs::remove_file(&beside);
        let _ = std::fs::remove_file(here);
        assert_eq!(
            outcome,
            Err(SystemLibraryError::Missing {
                dll: decoy.to_string(),
                folder: system
            })
        );
    }

    #[test]
    fn a_decoy_in_a_folder_of_its_own_is_not_found_by_name_either() {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        std::fs::create_dir_all(&scratch).expect("scratch");
        let dir = tempfile::Builder::new()
            .prefix("hostlib-")
            .tempdir_in(&scratch)
            .expect("temp dir");
        let system = system_directory().expect("system folder");
        let decoy = "msvcp140_leonida_decoy.dll";
        std::fs::copy(system.join("version.dll"), dir.path().join(decoy)).expect("copy");
        assert!(matches!(
            load_in(dir.path(), "version.dll", 0),
            Err(SystemLibraryError::Missing { .. })
        ));
        assert!(matches!(
            load(decoy, 0),
            Err(SystemLibraryError::Missing { .. })
        ));
    }

    #[test]
    fn a_missing_c_runtime_asks_for_the_redistributable() {
        let error = require(["msvcp140_leonida_absent.dll"]).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("msvcp140_leonida_absent.dll"), "{text}");
        assert!(text.contains("Visual C++ Redistributable"), "{text}");
        assert!(
            text.contains("never loads the copy in the game folder"),
            "{text}"
        );
        let other = require(["leonida-absent-component.dll"])
            .unwrap_err()
            .to_string();
        assert!(other.contains("Windows component"), "{other}");
        assert!(!other.contains("Redistributable"), "{other}");
    }

    #[test]
    fn the_programs_search_flags_never_widen_the_search() {
        for given in [0x100, 0x200, 0x400, 0x1000, 0x8, 0xFFFF_FFFF] {
            let kept = (given & KEPT_FLAGS) | SEARCH_SYSTEM32;
            assert_eq!(
                kept & (0x100 | 0x200 | 0x400 | 0x1000 | 0x8),
                0,
                "{given:#x}"
            );
            assert_ne!(kept & SEARCH_SYSTEM32, 0);
        }
    }

    #[test]
    fn a_runtime_that_is_there_passes_the_check() {
        assert_eq!(require(["kernel32.dll", "ucrtbase.dll"]), Ok(()));
    }
}
