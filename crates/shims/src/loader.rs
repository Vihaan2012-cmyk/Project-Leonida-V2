//! Run-time library loading (LoadLibrary*, GetModuleHandle*, GetProcAddress, FreeLibrary) for console DLLs, and the GDK runtime behind xgameruntime.dll.
//!
//! The program reaches the GDK runtime without importing it: `xgameruntime.lib`'s thunks `LoadLibraryW`
//! `xgameruntime.dll` and `GetProcAddress` its entry points. [`modules`] answers those calls from the shim
//! registry, [`runtime`] is what the entry points hand out: the runtime's COM classes, all offline.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::Shim;

#[cfg(windows)]
mod com;
#[cfg(windows)]
mod modules;

pub(crate) const S_OK: i32 = 0;
pub(crate) const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
pub(crate) const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
pub(crate) const E_POINTER: i32 = 0x8000_4003_u32 as i32;
pub(crate) const E_ABORT: i32 = 0x8000_4004_u32 as i32;
pub(crate) const E_FAIL: i32 = 0x8000_4005_u32 as i32;
pub(crate) const E_UNEXPECTED: i32 = 0x8000_FFFF_u32 as i32;
pub(crate) const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
pub(crate) const E_PENDING: i32 = 0x8000_000A_u32 as i32;
pub(crate) const HR_INSUFFICIENT_BUFFER: i32 = 0x8007_007A_u32 as i32;
pub(crate) const HR_NOT_FOUND: i32 = 0x8007_0490_u32 as i32;
pub(crate) const HR_NO_TASK_QUEUE: i32 = 0x8007_01AB_u32 as i32;
pub(crate) const HR_NETWORK_UNREACHABLE: i32 = 0x8007_04CF_u32 as i32;
pub(crate) const HR_NOT_SUPPORTED: i32 = 0x8007_0032_u32 as i32;
pub(crate) const HR_PATH_NOT_FOUND: i32 = 0x8007_0003_u32 as i32;
pub(crate) const HR_DISK_FULL: i32 = 0x8007_0070_u32 as i32;

/// Where the game package and the folder for everything Leonida writes lie.
#[derive(Debug, Clone, Default)]
struct Paths {
    game_dir: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    main_module: usize,
}

static PATHS: Mutex<Paths> = Mutex::new(Paths {
    game_dir: None,
    output_dir: None,
    main_module: 0,
});

/// Tells the runtime where the game package (for `MicrosoftGame.Config`) and the `Output` folder (saves and
/// persistent storage go below it) are. Without this, `LEONIDA_GAME_DIR` and `LEONIDA_OUTPUT_DIR` are used,
/// and `Output` in the working folder.
pub fn configure(game_dir: Option<PathBuf>, output_dir: PathBuf) {
    if let Ok(mut paths) = PATHS.lock() {
        paths.game_dir = game_dir;
        paths.output_dir = Some(output_dir);
    }
}

/// The image the program runs from, so `GetModuleHandle(NULL)` names it and not Leonida's own executable.
pub fn set_main_module(base: usize) {
    if let Ok(mut paths) = PATHS.lock() {
        paths.main_module = base;
    }
}

pub(crate) fn main_module() -> usize {
    PATHS.lock().map(|p| p.main_module).unwrap_or(0)
}

pub(crate) fn game_dir() -> Option<PathBuf> {
    let configured = PATHS.lock().ok().and_then(|p| p.game_dir.clone());
    configured.or_else(|| std::env::var_os("LEONIDA_GAME_DIR").map(PathBuf::from))
}

pub(crate) fn output_dir() -> PathBuf {
    let configured = PATHS.lock().ok().and_then(|p| p.output_dir.clone());
    configured
        .or_else(|| std::env::var_os("LEONIDA_OUTPUT_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("Output"))
}

pub(crate) fn saves_dir() -> PathBuf {
    output_dir().join("saves")
}

pub(crate) fn persistent_dir() -> PathBuf {
    output_dir()
        .join("runtime")
        .join("persistent-local-storage")
}

fn game_config() -> Option<String> {
    std::fs::read_to_string(game_dir()?.join("MicrosoftGame.Config")).ok()
}

/// The text of the first `<tag>` element.
fn element_text(xml: &str, tag: &str) -> Option<String> {
    let open = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let close = xml[open..].find(&format!("</{tag}>"))? + open;
    Some(xml[open..close].trim().to_string())
}

/// The value of `attribute` on the first `<tag ...>` element.
fn attribute(xml: &str, tag: &str, attribute: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag} "))?;
    let end = xml[start..].find('>')? + start;
    let element = &xml[start..end];
    let from = element.find(&format!("{attribute}=\""))? + attribute.len() + 2;
    let to = element[from..].find('"')? + from;
    Some(element[from..to].to_string())
}

/// `<TitleId>` of `MicrosoftGame.Config`, eight hexadecimal digits.
pub(crate) fn title_id() -> Option<u32> {
    let text = element_text(&game_config()?, "TitleId")?;
    u32::from_str_radix(&text, 16).ok()
}

/// The size `<PersistentLocalStorage><SizeMB>` of `MicrosoftGame.Config` allows, in bytes.
pub(crate) fn persistent_quota() -> u64 {
    game_config()
        .and_then(|xml| element_text(&xml, "SizeMB"))
        .and_then(|mb| mb.parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
        * 1024
}

/// The package identifier the program asks for: the identity name of `MicrosoftGame.Config`, which is short
/// enough for the 33 bytes the GDK allows.
pub(crate) fn package_identifier() -> String {
    game_config()
        .and_then(|xml| attribute(&xml, "Identity", "Name"))
        .filter(|name| !name.is_empty() && name.len() < 33)
        .unwrap_or_else(|| "LeonidaPackage".to_string())
}

/// Fixed text from the environment, or `default`.
pub(crate) fn setting(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Shims for the library functions and the GDK runtime's entry points.
pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        modules::shims()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Serves `QueryApiImpl`, for tests and for callers that hold no module handle.
#[cfg(windows)]
pub fn query_api(class: &[u8; 16], interface: &[u8; 16], out: &mut usize) -> i32 {
    com::query(
        &com::Guid(*class),
        &com::Guid(*interface),
        out as *mut usize as *mut *mut std::ffi::c_void,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_config_elements() {
        let xml = "<Game><TitleId>7952987A</TitleId><Identity Name=\"A.B\" Version=\"1\" />\
                   <PersistentLocalStorage><SizeMB>2048</SizeMB></PersistentLocalStorage></Game>";
        assert_eq!(element_text(xml, "TitleId").as_deref(), Some("7952987A"));
        assert_eq!(attribute(xml, "Identity", "Name").as_deref(), Some("A.B"));
        assert_eq!(attribute(xml, "Identity", "Version").as_deref(), Some("1"));
        assert_eq!(element_text(xml, "SizeMB").as_deref(), Some("2048"));
        assert_eq!(element_text(xml, "Missing"), None);
    }
}
