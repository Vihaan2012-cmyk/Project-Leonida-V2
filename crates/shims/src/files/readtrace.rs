//! `LEONIDA_TRACE_READS=<part of a path>`: notes where the program reads files whose path contains that text,
//! through `ReadFile` and `SetFilePointerEx` (served by Windows otherwise) and DirectStorage. Off, these calls go
//! straight to Windows.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{FILE_CURRENT, ReadFile, SetFilePointerEx};
use windows_sys::Win32::System::IO::OVERLAPPED;

use crate::trace::note;

/// The most read notes one boot writes.
const LIMIT: usize = 4000;

static WRITTEN: AtomicUsize = AtomicUsize::new(0);
static HANDLES: Mutex<Option<HashMap<usize, String>>> = Mutex::new(None);

/// The text paths are matched against, when tracing is on.
pub fn wanted() -> Option<&'static str> {
    static WANTED: OnceLock<Option<String>> = OnceLock::new();
    WANTED
        .get_or_init(|| {
            std::env::var("LEONIDA_TRACE_READS")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| v.to_lowercase())
        })
        .as_deref()
}

/// Whether a path is one whose reads are noted.
pub fn matches(path: &str) -> bool {
    wanted().is_some_and(|w| path.to_lowercase().contains(w))
}

/// Remembers a handle the program opened on a traced path.
pub fn opened(handle: HANDLE, path: &str) {
    if handle as isize == -1 || handle.is_null() || !matches(path) {
        return;
    }
    let mut handles = HANDLES.lock().unwrap_or_else(|e| e.into_inner());
    handles
        .get_or_insert_with(HashMap::new)
        .insert(handle as usize, path.to_string());
}

fn path_of(handle: HANDLE) -> Option<String> {
    HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(&(handle as usize))
        .cloned()
}

/// Notes one line, up to `LIMIT` in a boot.
pub fn write(text: String) {
    if WRITTEN.fetch_add(1, Ordering::Relaxed) < LIMIT {
        note(text);
    }
}

/// Notes a DirectStorage request on a traced file.
pub fn request(key: HANDLE, offset: u64, size: u32, options: u64) {
    if let Some(path) = path_of(key) {
        write(format!(
            "DirectStorage read {path} at {offset}, {size} bytes, options {options:#x}"
        ));
    }
}

pub(super) unsafe extern "win64" fn read_file(
    handle: HANDLE,
    buffer: *mut u8,
    bytes: u32,
    read: *mut u32,
    overlapped: *mut OVERLAPPED,
) -> i32 {
    let traced = if wanted().is_some() {
        path_of(handle)
    } else {
        None
    };
    if let Some(path) = traced {
        let at = if overlapped.is_null() {
            let mut position = 0i64;
            // SAFETY: asks Windows for the handle's current position without moving it.
            if unsafe { SetFilePointerEx(handle, 0, &mut position, FILE_CURRENT) } != 0 {
                format!("{position}")
            } else {
                "?".into()
            }
        } else {
            // SAFETY: the program's OVERLAPPED, valid for the call.
            let o = unsafe { &*overlapped };
            // SAFETY: the offset words of the union, which a file read uses.
            let offset = unsafe {
                u64::from(o.Anonymous.Anonymous.Offset)
                    | (u64::from(o.Anonymous.Anonymous.OffsetHigh) << 32)
            };
            format!("{offset} (overlapped)")
        };
        write(format!("read {path} at {at}, {bytes} bytes"));
    }
    // SAFETY: the program's arguments, passed on unchanged.
    unsafe { ReadFile(handle, buffer, bytes, read, overlapped) }
}

pub(super) unsafe extern "win64" fn set_file_pointer_ex(
    handle: HANDLE,
    distance: i64,
    new_position: *mut i64,
    method: u32,
) -> i32 {
    // SAFETY: the program's arguments, passed on unchanged.
    let ok = unsafe { SetFilePointerEx(handle, distance, new_position, method) };
    if wanted().is_some()
        && let Some(path) = path_of(handle)
    {
        write(format!("seek {path} by {distance} (method {method})"));
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_traced_without_the_setting() {
        if std::env::var_os("LEONIDA_TRACE_READS").is_none() {
            assert!(!matches("G:\\shaders.rpf"));
        }
    }
}
