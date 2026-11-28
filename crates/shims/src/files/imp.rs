use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{FALSE, HANDLE, INVALID_HANDLE_VALUE, SetLastError, TRUE};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem as fs_api;
use windows_sys::Win32::Storage::FileSystem::{CREATEFILE2_EXTENDED_PARAMETERS, WIN32_FIND_DATAW};
use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

use super::{Refusal, Target, resolve};
use crate::Shim;
use crate::trace::{note, record_path};

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_ALREADY_EXISTS: u32 = 183;

const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const CREATE_NEW: u32 = 1;
const WRITE_ACCESS: u32 =
    0x4000_0000 | 0x1000_0000 | 0x2 | 0x4 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000;
const FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const MOVEFILE_REPLACE_EXISTING: u32 = 1;

static HIDDEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static DENIED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
const SEEN_LIMIT: usize = 1 << 16;

fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn hidden(path: &Path) -> bool {
    HIDDEN
        .lock()
        .ok()
        .and_then(|h| h.as_ref().map(|h| h.contains(&key(path))))
        .unwrap_or(false)
}

fn hide(path: &Path) {
    if let Ok(mut set) = HIDDEN.lock() {
        set.get_or_insert_with(HashSet::new).insert(key(path));
    }
}

fn unhide(path: &Path) {
    if let Ok(mut set) = HIDDEN.lock()
        && let Some(set) = set.as_mut()
    {
        set.remove(&key(path));
    }
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn set_error(code: u32) {
    // SAFETY: sets the calling thread's last error.
    unsafe { SetLastError(code) };
}

fn io_code(error: &io::Error) -> u32 {
    error
        .raw_os_error()
        .map_or(ERROR_ACCESS_DENIED, |c| c as u32)
}

fn wide_ptr_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the caller passes a terminated UTF-16 string, as every file API requires; reading stops at the
    // terminator or after 32767 characters.
    let length = (0..32767)
        .take_while(|i| unsafe { *ptr.add(*i) } != 0)
        .count();
    // SAFETY: `length` characters were just read.
    let slice = unsafe { std::slice::from_raw_parts(ptr, length) };
    std::ffi::OsString::from_wide(slice)
        .to_string_lossy()
        .into_owned()
}

fn narrow_ptr_to_string(ptr: *const u8) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: as above, for an 8-bit string.
    let length = (0..32767)
        .take_while(|i| unsafe { *ptr.add(*i) } != 0)
        .count();
    // SAFETY: `length` bytes were just read.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ptr, length) }).into_owned()
}

fn to_wide(path: &Path) -> Vec<u16> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let drive = wide.len() > 2 && wide[1] == u16::from(b':') && wide[0] < 128;
    if wide.len() > 259 && drive {
        let mut long: Vec<u16> = "\\\\?\\".encode_utf16().collect();
        long.extend(wide.iter().map(|c| {
            if *c == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                *c
            }
        }));
        wide = long;
    }
    wide.push(0);
    wide
}

/// Records a name the program used, once, for `--trace`.
fn trace_path(text: &str) {
    let fresh = SEEN
        .lock()
        .map(|mut seen| {
            let seen = seen.get_or_insert_with(HashSet::new);
            seen.len() < SEEN_LIMIT && seen.insert(text.to_string())
        })
        .unwrap_or(false);
    if fresh {
        let mut bytes: Vec<u16> = text.encode_utf16().collect();
        bytes.push(0);
        // SAFETY: `bytes` is a terminated UTF-16 string.
        unsafe { record_path(bytes.as_ptr().cast(), 1) };
    }
}

/// Notes a refused name once for the boot trace.
pub(super) fn note_refusal(refusal: Refusal, text: &str) {
    let fresh = DENIED
        .lock()
        .map(|mut seen| {
            let seen = seen.get_or_insert_with(HashSet::new);
            seen.len() < SEEN_LIMIT && seen.insert(text.to_lowercase())
        })
        .unwrap_or(false);
    if fresh {
        note(format!("file access refused ({refusal:?}): {text}"));
    }
}

/// Maps a name the program used. `writes` says whether the call changes anything, which decides the error a
/// path outside the sandbox is answered with.
pub(super) fn target(text: &str, writes: bool) -> Result<Target, u32> {
    trace_path(text);
    resolve(text).map_err(|refusal: Refusal| {
        note_refusal(refusal, text);
        refusal.code_for(writes)
    })
}

fn read_path(target: &Target, original: &str) -> PathBuf {
    match target {
        Target::Layered { base, overlay } => {
            if exists(overlay) || hidden(base) {
                overlay.clone()
            } else {
                base.clone()
            }
        }
        Target::Direct { path, .. } => path.clone(),
        Target::Passthrough => PathBuf::from(original),
    }
}

/// The host path a read of `text` would open.
pub fn read_location(text: &str) -> Result<PathBuf, u32> {
    let target = target(text, false)?;
    Ok(read_path(&target, text))
}

fn visible(target: &Target, original: &str) -> bool {
    match target {
        Target::Layered { base, overlay } => exists(overlay) || (exists(base) && !hidden(base)),
        _ => exists(&read_path(target, original)),
    }
}

/// The path a write goes to, with its folders made, copying a package file over first when `copy_up`.
fn write_path(target: &Target, original: &str, copy_up: bool) -> Result<PathBuf, u32> {
    match target {
        Target::Passthrough => Ok(PathBuf::from(original)),
        Target::Direct { path, make_parents } => {
            if let Some(parent) = path.parent() {
                if *make_parents {
                    std::fs::create_dir_all(parent).map_err(|e| io_code(&e))?;
                } else if !parent.is_dir() {
                    return Err(ERROR_PATH_NOT_FOUND);
                }
            }
            Ok(path.clone())
        }
        Target::Layered { base, overlay } => {
            let overlay_parent = overlay.parent().map(Path::to_path_buf).unwrap_or_default();
            let base_parent_visible = base.parent().is_some_and(|p| p.is_dir() && !hidden(p));
            if !overlay_parent.is_dir() && !base_parent_visible {
                return Err(ERROR_PATH_NOT_FOUND);
            }
            std::fs::create_dir_all(&overlay_parent).map_err(|e| io_code(&e))?;
            if copy_up && !exists(overlay) && base.is_file() && !hidden(base) {
                std::fs::copy(base, overlay).map_err(|e| io_code(&e))?;
            }
            unhide(base);
            Ok(overlay.clone())
        }
    }
}

fn fail_bool(code: u32) -> i32 {
    set_error(code);
    FALSE
}

fn create_file_common(
    name: &str,
    access: u32,
    disposition: u32,
    flags: u32,
) -> Result<Vec<u16>, u32> {
    let backup = flags & FLAG_BACKUP_SEMANTICS != 0;
    let writes = !backup
        && (access & WRITE_ACCESS != 0
            || disposition != OPEN_EXISTING
            || flags & FLAG_DELETE_ON_CLOSE != 0);
    let target = target(name, writes)?;
    if !writes {
        return Ok(to_wide(&read_path(&target, name)));
    }
    if disposition == CREATE_NEW && visible(&target, name) {
        return Err(ERROR_FILE_EXISTS);
    }
    let keeps = disposition == OPEN_EXISTING || disposition == OPEN_ALWAYS;
    let path = write_path(&target, name, keeps)?;
    Ok(to_wide(&path))
}

unsafe extern "win64" fn create_file_w(
    name: *const u16,
    access: u32,
    share: u32,
    security: *const SECURITY_ATTRIBUTES,
    disposition: u32,
    flags: u32,
    template: HANDLE,
) -> HANDLE {
    let text = wide_ptr_to_string(name);
    match create_file_common(&text, access, disposition, flags) {
        // SAFETY: a terminated path of our own making; the other arguments are the program's.
        Ok(path) => unsafe {
            fs_api::CreateFileW(
                path.as_ptr(),
                access,
                share,
                security,
                disposition,
                flags,
                template,
            )
        },
        Err(code) => {
            set_error(code);
            INVALID_HANDLE_VALUE
        }
    }
}

unsafe extern "win64" fn create_file2(
    name: *const u16,
    access: u32,
    share: u32,
    disposition: u32,
    params: *const CREATEFILE2_EXTENDED_PARAMETERS,
) -> HANDLE {
    let text = wide_ptr_to_string(name);
    let flags = if params.is_null() {
        0
    } else {
        // SAFETY: a non-null parameter block is the program's, readable for its own `dwSize`.
        unsafe { (*params).dwFileFlags }
    };
    match create_file_common(&text, access, disposition, flags) {
        // SAFETY: as in `create_file_w`.
        Ok(path) => unsafe {
            fs_api::CreateFile2(path.as_ptr(), access, share, disposition, params)
        },
        Err(code) => {
            set_error(code);
            INVALID_HANDLE_VALUE
        }
    }
}

fn attributes_path(text: &str) -> Result<Vec<u16>, u32> {
    let target = target(text, false)?;
    Ok(to_wide(&read_path(&target, text)))
}

unsafe extern "win64" fn get_file_attributes_w(name: *const u16) -> u32 {
    match attributes_path(&wide_ptr_to_string(name)) {
        // SAFETY: a terminated path of our own making.
        Ok(path) => unsafe { fs_api::GetFileAttributesW(path.as_ptr()) },
        Err(code) => {
            set_error(code);
            u32::MAX
        }
    }
}

unsafe extern "win64" fn get_file_attributes_a(name: *const u8) -> u32 {
    match attributes_path(&narrow_ptr_to_string(name)) {
        // SAFETY: a terminated path of our own making.
        Ok(path) => unsafe { fs_api::GetFileAttributesW(path.as_ptr()) },
        Err(code) => {
            set_error(code);
            u32::MAX
        }
    }
}

unsafe extern "win64" fn get_file_attributes_ex_w(
    name: *const u16,
    level: i32,
    info: *mut c_void,
) -> i32 {
    match attributes_path(&wide_ptr_to_string(name)) {
        // SAFETY: a terminated path of our own making; `info` is the program's output buffer.
        Ok(path) => unsafe { fs_api::GetFileAttributesExW(path.as_ptr(), level, info) },
        Err(code) => fail_bool(code),
    }
}

unsafe extern "win64" fn get_file_attributes_ex_a(
    name: *const u8,
    level: i32,
    info: *mut c_void,
) -> i32 {
    match attributes_path(&narrow_ptr_to_string(name)) {
        // SAFETY: as above.
        Ok(path) => unsafe { fs_api::GetFileAttributesExW(path.as_ptr(), level, info) },
        Err(code) => fail_bool(code),
    }
}

struct Listing {
    entries: Vec<WIN32_FIND_DATAW>,
    next: usize,
}

static LISTINGS: Mutex<Option<HashMap<usize, Listing>>> = Mutex::new(None);
static NEXT_LISTING: AtomicUsize = AtomicUsize::new(0x4C45_0000_1000);

fn file_name(entry: &WIN32_FIND_DATAW) -> String {
    let end = entry
        .cFileName
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(entry.cFileName.len());
    String::from_utf16_lossy(&entry.cFileName[..end]).to_lowercase()
}

fn host_find_all(pattern: &Path) -> Vec<WIN32_FIND_DATAW> {
    let wide = to_wide(pattern);
    // SAFETY: an all-zero find record is valid.
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    // SAFETY: a terminated pattern and a record to fill.
    let handle = unsafe {
        fs_api::FindFirstFileExW(
            wide.as_ptr(),
            0,
            (&mut data as *mut WIN32_FIND_DATAW).cast(),
            0,
            std::ptr::null(),
            0,
        )
    };
    let mut entries = Vec::new();
    if handle == INVALID_HANDLE_VALUE {
        return entries;
    }
    loop {
        entries.push(data);
        // SAFETY: `handle` is open and `data` is a record.
        if unsafe { fs_api::FindNextFileW(handle, &mut data) } == 0 {
            break;
        }
    }
    // SAFETY: closes the handle opened above.
    unsafe { fs_api::FindClose(handle) };
    entries
}

fn split_pattern(text: &str) -> (&str, &str) {
    match text.rfind(['\\', '/']) {
        Some(at) => (&text[..=at], &text[at + 1..]),
        None => ("", text),
    }
}

unsafe fn find_first(
    pattern: *const u16,
    level: i32,
    data: *mut c_void,
    operation: i32,
    filter: *const c_void,
    flags: u32,
) -> HANDLE {
    let text = wide_ptr_to_string(pattern);
    let (folder, mask) = split_pattern(&text);
    let mapped = match target(folder, false) {
        Ok(mapped) => mapped,
        Err(code) => {
            set_error(code);
            return INVALID_HANDLE_VALUE;
        }
    };
    let (base, overlay) = match &mapped {
        Target::Layered { base, overlay } => (Some(base.clone()), Some(overlay.clone())),
        Target::Direct { path, .. } => (None, Some(path.clone())),
        Target::Passthrough => {
            let wide = to_wide(Path::new(&text));
            // SAFETY: a terminated pattern; the other arguments are the program's.
            return unsafe {
                fs_api::FindFirstFileExW(wide.as_ptr(), level, data, operation, filter, flags)
            };
        }
    };
    let base_here = base.as_ref().filter(|b| b.is_dir() && !hidden(b));
    let overlay_here = overlay.as_ref().filter(|o| o.is_dir());
    if base_here.is_none() && overlay_here.is_none() {
        set_error(ERROR_PATH_NOT_FOUND);
        return INVALID_HANDLE_VALUE;
    }
    let mut entries: Vec<WIN32_FIND_DATAW> = Vec::new();
    let mut names: HashSet<String> = HashSet::new();
    for (folder, from_base) in [(overlay_here, false), (base_here, true)] {
        let Some(folder) = folder else { continue };
        for entry in host_find_all(&folder.join(mask)) {
            let name = file_name(&entry);
            if from_base && hidden(&folder.join(&name)) {
                continue;
            }
            if names.insert(name) {
                entries.push(entry);
            }
        }
    }
    if entries.is_empty() {
        set_error(ERROR_FILE_NOT_FOUND);
        return INVALID_HANDLE_VALUE;
    }
    // SAFETY: `data` is the program's `WIN32_FIND_DATAW`.
    unsafe { *data.cast::<WIN32_FIND_DATAW>() = entries[0] };
    let handle = NEXT_LISTING.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut listings) = LISTINGS.lock() {
        listings
            .get_or_insert_with(HashMap::new)
            .insert(handle, Listing { entries, next: 1 });
    }
    handle as HANDLE
}

unsafe extern "win64" fn find_first_file_w(pattern: *const u16, data: *mut c_void) -> HANDLE {
    // SAFETY: forwarded as given.
    unsafe { find_first(pattern, 0, data, 0, std::ptr::null(), 0) }
}

unsafe extern "win64" fn find_first_file_ex_w(
    pattern: *const u16,
    level: i32,
    data: *mut c_void,
    operation: i32,
    filter: *const c_void,
    flags: u32,
) -> HANDLE {
    // SAFETY: forwarded as given.
    unsafe { find_first(pattern, level, data, operation, filter, flags) }
}

unsafe extern "win64" fn find_next_file_w(handle: HANDLE, data: *mut c_void) -> i32 {
    let ours = LISTINGS.lock().ok().and_then(|mut listings| {
        let listing = listings.as_mut()?.get_mut(&(handle as usize))?;
        Some(match listing.entries.get(listing.next) {
            Some(entry) => {
                listing.next += 1;
                Some(*entry)
            }
            None => None,
        })
    });
    match ours {
        Some(Some(entry)) => {
            // SAFETY: `data` is the program's `WIN32_FIND_DATAW`.
            unsafe { *data.cast::<WIN32_FIND_DATAW>() = entry };
            TRUE
        }
        Some(None) => fail_bool(ERROR_NO_MORE_FILES),
        // SAFETY: a host handle from `FindFirstFileExW`.
        None => unsafe { fs_api::FindNextFileW(handle, data.cast()) },
    }
}

unsafe extern "win64" fn find_close(handle: HANDLE) -> i32 {
    let removed = LISTINGS
        .lock()
        .ok()
        .and_then(|mut listings| listings.as_mut()?.remove(&(handle as usize)))
        .is_some();
    if removed {
        TRUE
    } else {
        // SAFETY: a host handle from `FindFirstFileExW`.
        unsafe { fs_api::FindClose(handle) }
    }
}

fn create_directory(text: &str) -> Result<(), u32> {
    let target = target(text, true)?;
    if visible(&target, text) {
        return Err(ERROR_ALREADY_EXISTS);
    }
    let path = write_path(&target, text, false)?;
    std::fs::create_dir(&path).map_err(|e| io_code(&e))
}

unsafe extern "win64" fn create_directory_w(name: *const u16, _security: *const c_void) -> i32 {
    match create_directory(&wide_ptr_to_string(name)) {
        Ok(()) => TRUE,
        Err(code) => fail_bool(code),
    }
}

fn delete_file(text: &str) -> Result<(), u32> {
    let target = target(text, true)?;
    match &target {
        Target::Layered { base, overlay } => {
            let in_package = base.is_file() && !hidden(base);
            if overlay.is_dir() || base.is_dir() && !exists(overlay) {
                return Err(ERROR_ACCESS_DENIED);
            }
            if exists(overlay) {
                std::fs::remove_file(overlay).map_err(|e| io_code(&e))?;
            } else if !in_package {
                return Err(ERROR_FILE_NOT_FOUND);
            }
            if in_package {
                hide(base);
            }
            Ok(())
        }
        other => std::fs::remove_file(read_path(other, text)).map_err(|e| io_code(&e)),
    }
}

unsafe extern "win64" fn delete_file_w(name: *const u16) -> i32 {
    match delete_file(&wide_ptr_to_string(name)) {
        Ok(()) => TRUE,
        Err(code) => fail_bool(code),
    }
}

fn remove_directory(text: &str) -> Result<(), u32> {
    let target = target(text, true)?;
    match &target {
        Target::Layered { base, overlay } => {
            if base.is_dir() && !hidden(base) {
                Err(ERROR_ACCESS_DENIED)
            } else if overlay.is_dir() {
                std::fs::remove_dir(overlay).map_err(|e| io_code(&e))
            } else {
                Err(ERROR_PATH_NOT_FOUND)
            }
        }
        other => std::fs::remove_dir(read_path(other, text)).map_err(|e| io_code(&e)),
    }
}

unsafe extern "win64" fn remove_directory_w(name: *const u16) -> i32 {
    match remove_directory(&wide_ptr_to_string(name)) {
        Ok(()) => TRUE,
        Err(code) => fail_bool(code),
    }
}

unsafe extern "win64" fn set_file_attributes_w(name: *const u16, attributes: u32) -> i32 {
    let text = wide_ptr_to_string(name);
    let result = target(&text, true).and_then(|target| {
        if let Target::Layered { base, overlay } = &target
            && !exists(overlay)
        {
            if base.is_dir() {
                write_path(&target, &text, false)?;
                std::fs::create_dir(overlay).map_err(|e| io_code(&e))?;
            } else {
                write_path(&target, &text, true)?;
            }
            if !exists(overlay) {
                return Err(ERROR_FILE_NOT_FOUND);
            }
        }
        Ok(to_wide(&read_path(&target, &text)))
    });
    match result {
        // SAFETY: a terminated path of our own making.
        Ok(path) => unsafe { fs_api::SetFileAttributesW(path.as_ptr(), attributes) },
        Err(code) => fail_bool(code),
    }
}

fn move_file(from: &str, to: &str, flags: u32) -> Result<(), u32> {
    let source = target(from, true)?;
    let destination = target(to, true)?;
    if visible(&destination, to) && flags & MOVEFILE_REPLACE_EXISTING == 0 {
        return Err(ERROR_ALREADY_EXISTS);
    }
    let dst = write_path(&destination, to, false)?;
    if let Target::Layered { base, overlay } = &source {
        if exists(overlay) {
            std::fs::rename(overlay, &dst).map_err(|e| io_code(&e))?;
        } else if base.is_dir() || hidden(base) || !exists(base) {
            return Err(if exists(base) && !hidden(base) {
                ERROR_ACCESS_DENIED
            } else {
                ERROR_FILE_NOT_FOUND
            });
        } else {
            std::fs::copy(base, &dst).map_err(|e| io_code(&e))?;
        }
        if base.is_file() {
            hide(base);
        }
        Ok(())
    } else {
        std::fs::rename(read_path(&source, from), &dst).map_err(|e| io_code(&e))
    }
}

unsafe extern "win64" fn move_file_ex_w(from: *const u16, to: *const u16, flags: u32) -> i32 {
    match move_file(&wide_ptr_to_string(from), &wide_ptr_to_string(to), flags) {
        Ok(()) => TRUE,
        Err(code) => fail_bool(code),
    }
}

unsafe extern "win64" fn get_disk_free_space_ex_w(
    name: *const u16,
    available: *mut u64,
    total: *mut u64,
    free: *mut u64,
) -> i32 {
    let mut path: Option<PathBuf> = None;
    if !name.is_null() {
        let text = wide_ptr_to_string(name);
        match target(&text, false) {
            Ok(target) => {
                let mut here = read_path(&target, &text);
                while !here.exists() && here.pop() {}
                path = Some(here);
            }
            Err(code) => return fail_bool(code),
        }
    }
    let wide = path.as_deref().map(to_wide);
    let pointer = wide.as_ref().map_or(std::ptr::null(), |w| w.as_ptr());
    // SAFETY: a terminated path or null; the outputs are the program's.
    unsafe { fs_api::GetDiskFreeSpaceExW(pointer, available, total, free) }
}

fn ucrt(name: &str) -> usize {
    let symbol: Vec<u8> = name.bytes().chain([0]).collect();
    let Ok(module) = crate::hostlib::load("ucrtbase.dll", 0) else {
        return 0;
    };
    // SAFETY: asks the system's C runtime, loaded from the system folder, for one export.
    unsafe { GetProcAddress(module as *mut c_void, symbol.as_ptr()).map_or(0, |f| f as usize) }
}

type WFopen = unsafe extern "C" fn(*const u16, *const u16) -> *mut c_void;
type WStat = unsafe extern "C" fn(*const u16, *mut c_void) -> i32;

fn crt_function(slot: &'static OnceLock<usize>, name: &str) -> usize {
    *slot.get_or_init(|| ucrt(name))
}

static WFOPEN: OnceLock<usize> = OnceLock::new();
static WSTAT: OnceLock<usize> = OnceLock::new();

fn open_stream(text: &str, mode: &str) -> *mut c_void {
    let function = crt_function(&WFOPEN, "_wfopen");
    if function == 0 {
        return std::ptr::null_mut();
    }
    let writes = mode.contains(['w', 'a', '+']);
    let path = target(text, writes).and_then(|target| {
        if !writes {
            return Ok(read_path(&target, text));
        }
        write_path(
            &target,
            text,
            mode.contains(['a', '+']) && !mode.contains('w'),
        )
    });
    let Ok(path) = path else {
        return std::ptr::null_mut();
    };
    let wide = to_wide(&path);
    let mode: Vec<u16> = mode.encode_utf16().chain([0]).collect();
    // SAFETY: `function` is `_wfopen`; both strings are terminated.
    unsafe { std::mem::transmute::<usize, WFopen>(function)(wide.as_ptr(), mode.as_ptr()) }
}

unsafe extern "win64" fn wfopen(name: *const u16, mode: *const u16) -> *mut c_void {
    open_stream(&wide_ptr_to_string(name), &wide_ptr_to_string(mode))
}

unsafe extern "win64" fn fopen(name: *const u8, mode: *const u8) -> *mut c_void {
    open_stream(&narrow_ptr_to_string(name), &narrow_ptr_to_string(mode))
}

unsafe extern "win64" fn wstat64(name: *const u16, buffer: *mut c_void) -> i32 {
    let function = crt_function(&WSTAT, "_wstat64");
    let text = wide_ptr_to_string(name);
    match attributes_path(&text) {
        // SAFETY: `function` is `_wstat64`; the path is terminated and `buffer` is the program's.
        Ok(path) if function != 0 => unsafe {
            std::mem::transmute::<usize, WStat>(function)(path.as_ptr(), buffer)
        },
        _ => -1,
    }
}

unsafe extern "win64" fn remove(name: *const u8) -> i32 {
    match delete_file(&narrow_ptr_to_string(name)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

const FILE_SETS: &[&str] = &[
    "kernel32.dll",
    "kernelbase.dll",
    "api-ms-win-core-file-l1-1-0.dll",
    "api-ms-win-core-file-l1-2-0.dll",
    "api-ms-win-core-file-l1-2-1.dll",
    "api-ms-win-core-file-l1-2-2.dll",
    "api-ms-win-core-file-l1-2-4.dll",
    "api-ms-win-core-file-l2-1-0.dll",
    "api-ms-win-core-file-l2-1-1.dll",
    "api-ms-win-core-file-l2-1-2.dll",
    "api-ms-win-core-file-l2-1-3.dll",
];

pub fn shims() -> Vec<Shim> {
    let functions: [(&str, usize); 16] = [
        ("CreateFileW", create_file_w as *const () as usize),
        ("CreateFile2", create_file2 as *const () as usize),
        (
            "GetFileAttributesW",
            get_file_attributes_w as *const () as usize,
        ),
        (
            "GetFileAttributesA",
            get_file_attributes_a as *const () as usize,
        ),
        (
            "GetFileAttributesExW",
            get_file_attributes_ex_w as *const () as usize,
        ),
        (
            "GetFileAttributesExA",
            get_file_attributes_ex_a as *const () as usize,
        ),
        ("FindFirstFileW", find_first_file_w as *const () as usize),
        (
            "FindFirstFileExW",
            find_first_file_ex_w as *const () as usize,
        ),
        ("FindNextFileW", find_next_file_w as *const () as usize),
        ("FindClose", find_close as *const () as usize),
        ("CreateDirectoryW", create_directory_w as *const () as usize),
        ("DeleteFileW", delete_file_w as *const () as usize),
        ("RemoveDirectoryW", remove_directory_w as *const () as usize),
        (
            "SetFileAttributesW",
            set_file_attributes_w as *const () as usize,
        ),
        ("MoveFileExW", move_file_ex_w as *const () as usize),
        (
            "GetDiskFreeSpaceExW",
            get_disk_free_space_ex_w as *const () as usize,
        ),
    ];
    let mut shims = Vec::new();
    for (name, address) in functions.iter() {
        for dll in FILE_SETS {
            shims.push(Shim {
                dll,
                name,
                address: *address,
            });
        }
    }
    for (dll, name, address) in [
        (
            "api-ms-win-crt-stdio-l1-1-0.dll",
            "_wfopen",
            wfopen as *const () as usize,
        ),
        (
            "api-ms-win-crt-stdio-l1-1-0.dll",
            "fopen",
            fopen as *const () as usize,
        ),
        (
            "api-ms-win-crt-filesystem-l1-1-0.dll",
            "_wstat64",
            wstat64 as *const () as usize,
        ),
        (
            "api-ms-win-crt-filesystem-l1-1-0.dll",
            "remove",
            remove as *const () as usize,
        ),
        ("ucrtbase.dll", "_wfopen", wfopen as *const () as usize),
        ("ucrtbase.dll", "fopen", fopen as *const () as usize),
        ("ucrtbase.dll", "_wstat64", wstat64 as *const () as usize),
        ("ucrtbase.dll", "remove", remove as *const () as usize),
    ] {
        shims.push(Shim { dll, name, address });
    }
    shims
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
