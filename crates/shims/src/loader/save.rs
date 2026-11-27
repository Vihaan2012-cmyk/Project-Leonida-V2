//! `XGameSaveImpl`: containers and blobs as folders and files under `Output/saves/local-user`, and the
//! `files` folder of `XGameSaveFiles*` beside it.
//!
//! A container is a folder, a blob a file, both named with characters outside `[A-Za-z0-9_.-]` written as
//! `%XX` (a leading `.` too), the layout `saves.md` describes for importing saves. An update is applied by
//! writing each new blob beside its final name and renaming it over, so a failed write leaves the container as
//! it was. The container's display name is kept in `.display`. A save is never synced: there is no cloud.

use std::collections::HashSet;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::com::{ClassImpl, c_str, method};
use super::tasks::{self, AsyncBlock};
use super::{E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, HR_INSUFFICIENT_BUFFER, S_OK, saves_dir};

type This = *mut c_void;

const E_GS_INVALID_CONTAINER_NAME: i32 = 0x8083_0001_u32 as i32;
const E_GS_OUT_OF_LOCAL_STORAGE: i32 = 0x8083_0003_u32 as i32;
const E_GS_UPDATE_TOO_BIG: i32 = 0x8083_0005_u32 as i32;
const E_GS_QUOTA_EXCEEDED: i32 = 0x8083_0006_u32 as i32;
const E_GS_PROVIDED_BUFFER_TOO_SMALL: i32 = 0x8083_0007_u32 as i32;
const E_GS_BLOB_NOT_FOUND: i32 = 0x8083_0008_u32 as i32;
const E_GS_HANDLE_EXPIRED: i32 = 0x8083_000D_u32 as i32;

const MAX_UPDATE_BYTES: u64 = 16 * 1024 * 1024;
const QUOTA_BYTES: u64 = 1 << 30;
const MAX_NAME: usize = 256;
const BLOB_ENTRY: usize = 24;

static LIVE: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

fn track(handle: usize) {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(handle);
}

fn untrack(handle: usize) -> bool {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .remove(&handle)
}

fn is_live(handle: usize) -> bool {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .contains(&handle)
}

struct Provider {
    root: PathBuf,
}

struct Container {
    dir: PathBuf,
    name: String,
}

enum Op {
    Write(String, Vec<u8>),
    Delete(String),
}

struct Update {
    dir: PathBuf,
    display_name: String,
    ops: Vec<Op>,
}

fn encode(name: &str) -> String {
    let mut out = String::new();
    for byte in name.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || byte == b'_'
            || byte == b'-'
            || (byte == b'.' && !out.is_empty());
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode(encoded: &str) -> Option<String> {
    if encoded.is_empty() || encoded.starts_with('.') || encoded.ends_with('~') {
        return None;
    }
    let bytes = encoded.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `XGameSaveCreateContainer`'s rule: at most 256 characters, a path part of `[A-Za-z0-9_/]`, then a file part of
/// `[A-Za-z0-9_.-]` that is not empty, does not end in `.` and holds no `..`.
fn valid_container_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME {
        return false;
    }
    let (path, file) = match name.rfind('/') {
        Some(slash) => (&name[..=slash], &name[slash + 1..]),
        None => ("", name),
    };
    path.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'/')
        && !file.is_empty()
        && !file.ends_with('.')
        && !file.contains("..")
        && file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

fn valid_blob_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME
}

fn blobs(dir: &Path) -> Vec<(String, u64)> {
    let mut found: Vec<(String, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let name = decode(&entry.file_name().to_string_lossy())?;
            meta.is_file().then_some((name, meta.len()))
        })
        .collect();
    found.sort();
    found
}

fn folder_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn container_dirs(root: &Path, prefix: &str) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = decode(&entry.file_name().to_string_lossy())?;
            (entry.path().is_dir() && name.starts_with(prefix)).then(|| (name, entry.path()))
        })
        .collect();
    found.sort();
    found
}

#[repr(C)]
struct ContainerInfo {
    name: *const u8,
    display_name: *const u8,
    blob_count: u32,
    total_size: u64,
    last_modified: i64,
    needs_sync: u8,
}

#[repr(C)]
struct BlobInfo {
    name: *const u8,
    size: u32,
}

type ContainerCallback = unsafe extern "win64" fn(*const ContainerInfo, *mut c_void) -> u8;
type BlobCallback = unsafe extern "win64" fn(*const BlobInfo, *mut c_void) -> u8;

fn nul(text: &str) -> Vec<u8> {
    text.bytes().chain([0]).collect()
}

/// Reports one container to a callback; false when the callback asked to stop.
fn report_container(name: &str, dir: &Path, context: usize, callback: usize) -> bool {
    let list = blobs(dir);
    let display =
        std::fs::read_to_string(dir.join(".display")).unwrap_or_else(|_| name.to_string());
    let modified = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let (name, display) = (nul(name), nul(&display));
    let info = ContainerInfo {
        name: name.as_ptr(),
        display_name: display.as_ptr(),
        blob_count: list.len() as u32,
        total_size: list.iter().map(|(_, size)| size).sum(),
        last_modified: modified,
        needs_sync: 0,
    };
    // SAFETY: the callback the program gave, with the info valid for the call.
    let callback: ContainerCallback = unsafe { std::mem::transmute(callback) };
    // SAFETY: the program's own callback.
    unsafe { callback(&info, context as *mut c_void) != 0 }
}

fn provider_of<'a>(handle: usize) -> Option<&'a Provider> {
    // SAFETY: a live handle is a `Provider` made here.
    is_live(handle).then(|| unsafe { &*(handle as *const Provider) })
}

fn container_of<'a>(handle: usize) -> Option<&'a Container> {
    // SAFETY: a live handle is a `Container` made here.
    is_live(handle).then(|| unsafe { &*(handle as *const Container) })
}

fn update_of<'a>(handle: usize) -> Option<&'a mut Update> {
    // SAFETY: a live handle is an `Update` made here, used by one thread at a time as the GDK requires.
    is_live(handle).then(|| unsafe { &mut *(handle as *mut Update) })
}

fn user_root() -> PathBuf {
    saves_dir().join("local-user")
}

fn open_provider() -> usize {
    let root = user_root();
    let _ = std::fs::create_dir_all(&root);
    let handle = Box::into_raw(Box::new(Provider { root })) as usize;
    track(handle);
    handle
}

// ---- provider ----

unsafe extern "win64" fn initialize_provider(
    _: This,
    user: usize,
    config: *const u8,
    _sync: u8,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a string or null.
    if user == 0 || unsafe { c_str(config) }.is_none() {
        return E_INVALIDARG;
    }
    if std::fs::create_dir_all(user_root()).is_err() {
        return E_GS_OUT_OF_LOCAL_STORAGE;
    }
    // SAFETY: writable, checked.
    unsafe { *out = open_provider() };
    S_OK
}

unsafe extern "win64" fn initialize_provider_async(
    _: This,
    user: usize,
    config: *const u8,
    _sync: u8,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: a string or null.
    if user == 0 || unsafe { c_str(config) }.is_none() {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XGameSaveInitializeProviderAsync", || {
            if std::fs::create_dir_all(user_root()).is_err() {
                return (E_GS_OUT_OF_LOCAL_STORAGE, Vec::new());
            }
            (S_OK, open_provider().to_le_bytes().to_vec())
        })
    }
}

unsafe extern "win64" fn initialize_provider_result(
    _: This,
    block: *mut AsyncBlock,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    // SAFETY: the program's block and a result it can hold.
    unsafe {
        tasks::one_shot_result(
            block,
            "XGameSaveInitializeProviderAsync",
            std::mem::size_of::<usize>(),
            out as *mut c_void,
        )
    }
}

unsafe extern "win64" fn close_provider(_: This, provider: usize) {
    if untrack(provider) {
        // SAFETY: the box `open_provider` made, closed once.
        drop(unsafe { Box::from_raw(provider as *mut Provider) });
    }
}

fn remaining_quota(root: &Path) -> i64 {
    QUOTA_BYTES.saturating_sub(folder_bytes(root)) as i64
}

unsafe extern "win64" fn get_remaining_quota(_: This, provider: usize, remaining: *mut i64) -> i32 {
    if remaining.is_null() {
        return E_POINTER;
    }
    let Some(provider) = provider_of(provider) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: writable, checked.
    unsafe { *remaining = remaining_quota(&provider.root) };
    S_OK
}

unsafe extern "win64" fn get_remaining_quota_async(
    _: This,
    provider: usize,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    let Some(provider) = provider_of(provider) else {
        return E_GS_HANDLE_EXPIRED;
    };
    let root = provider.root.clone();
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XGameSaveGetRemainingQuotaAsync", move || {
            (S_OK, remaining_quota(&root).to_le_bytes().to_vec())
        })
    }
}

unsafe extern "win64" fn get_remaining_quota_result(
    _: This,
    block: *mut AsyncBlock,
    remaining: *mut i64,
) -> i32 {
    if remaining.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block and a result it can hold.
    unsafe {
        tasks::one_shot_result(
            block,
            "XGameSaveGetRemainingQuotaAsync",
            8,
            remaining as *mut c_void,
        )
    }
}

fn delete_container(root: &Path, name: &str) -> i32 {
    if !valid_container_name(name) {
        return E_GS_INVALID_CONTAINER_NAME;
    }
    match std::fs::remove_dir_all(root.join(encode(name))) {
        Ok(()) => S_OK,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => S_OK,
        Err(_) => E_FAIL,
    }
}

unsafe extern "win64" fn delete_container_sync(_: This, provider: usize, name: *const u8) -> i32 {
    let Some(provider) = provider_of(provider) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: a string or null.
    match unsafe { c_str(name) } {
        Some(name) => delete_container(&provider.root, &name),
        None => E_INVALIDARG,
    }
}

unsafe extern "win64" fn delete_container_async(
    _: This,
    provider: usize,
    name: *const u8,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    let Some(provider) = provider_of(provider) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: a string or null.
    let Some(name) = (unsafe { c_str(name) }) else {
        return E_INVALIDARG;
    };
    let root = provider.root.clone();
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XGameSaveDeleteContainerAsync", move || {
            (delete_container(&root, &name), Vec::new())
        })
    }
}

unsafe extern "win64" fn delete_container_result(_: This, block: *mut AsyncBlock) -> i32 {
    // SAFETY: the program's block.
    unsafe {
        tasks::one_shot_result(
            block,
            "XGameSaveDeleteContainerAsync",
            0,
            std::ptr::null_mut(),
        )
    }
}

// ---- containers ----

unsafe extern "win64" fn get_container_info(
    _: This,
    provider: usize,
    name: *const u8,
    context: usize,
    callback: usize,
) -> i32 {
    if callback == 0 {
        return E_INVALIDARG;
    }
    let Some(provider) = provider_of(provider) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: a string or null.
    let Some(name) = (unsafe { c_str(name) }) else {
        return E_INVALIDARG;
    };
    if !valid_container_name(&name) {
        return E_GS_INVALID_CONTAINER_NAME;
    }
    let dir = provider.root.join(encode(&name));
    if dir.is_dir() {
        report_container(&name, &dir, context, callback);
    }
    S_OK
}

fn enumerate_containers(provider: usize, prefix: &str, context: usize, callback: usize) -> i32 {
    if callback == 0 {
        return E_INVALIDARG;
    }
    let Some(provider) = provider_of(provider) else {
        return E_GS_HANDLE_EXPIRED;
    };
    for (name, dir) in container_dirs(&provider.root, prefix) {
        if !report_container(&name, &dir, context, callback) {
            break;
        }
    }
    S_OK
}

unsafe extern "win64" fn enumerate_container_info(
    _: This,
    provider: usize,
    context: usize,
    callback: usize,
) -> i32 {
    enumerate_containers(provider, "", context, callback)
}

unsafe extern "win64" fn enumerate_container_info_by_name(
    _: This,
    provider: usize,
    prefix: *const u8,
    context: usize,
    callback: usize,
) -> i32 {
    // SAFETY: a string or null.
    let prefix = unsafe { c_str(prefix) }.unwrap_or_default();
    enumerate_containers(provider, &prefix, context, callback)
}

/// The folder appears when the first update is submitted.
unsafe extern "win64" fn create_container(
    _: This,
    provider: usize,
    name: *const u8,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    let Some(provider) = provider_of(provider) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: a string or null.
    let name = unsafe { c_str(name) }.unwrap_or_default();
    if !valid_container_name(&name) {
        return E_GS_INVALID_CONTAINER_NAME;
    }
    let container = Box::into_raw(Box::new(Container {
        dir: provider.root.join(encode(&name)),
        name,
    })) as usize;
    track(container);
    // SAFETY: writable, checked.
    unsafe { *out = container };
    S_OK
}

unsafe extern "win64" fn close_container(_: This, container: usize) {
    if untrack(container) {
        // SAFETY: the box `create_container` made, closed once.
        drop(unsafe { Box::from_raw(container as *mut Container) });
    }
}

fn enumerate_blobs(container: usize, prefix: &str, context: usize, callback: usize) -> i32 {
    if callback == 0 {
        return E_INVALIDARG;
    }
    let Some(container) = container_of(container) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: the callback the program gave, with its documented shape.
    let callback: BlobCallback = unsafe { std::mem::transmute(callback) };
    for (name, size) in blobs(&container.dir) {
        if !name.starts_with(prefix) {
            continue;
        }
        let text = nul(&name);
        let info = BlobInfo {
            name: text.as_ptr(),
            size: size as u32,
        };
        // SAFETY: the program's own callback.
        if unsafe { callback(&info, context as *mut c_void) } == 0 {
            break;
        }
    }
    S_OK
}

unsafe extern "win64" fn enumerate_blob_info(
    _: This,
    container: usize,
    context: usize,
    callback: usize,
) -> i32 {
    enumerate_blobs(container, "", context, callback)
}

unsafe extern "win64" fn enumerate_blob_info_by_name(
    _: This,
    container: usize,
    prefix: *const u8,
    context: usize,
    callback: usize,
) -> i32 {
    // SAFETY: a string or null.
    let prefix = unsafe { c_str(prefix) }.unwrap_or_default();
    enumerate_blobs(container, &prefix, context, callback)
}

// ---- reading ----

/// The blobs as `XGameSaveReadBlobData` hands them back: an array of `{name, size, data}` entries, then each
/// blob's bytes and name. The entries hold offsets from the start; [`relocate`] makes them addresses.
fn pack(container: &Container, names: Option<Vec<String>>) -> Result<Vec<u8>, i32> {
    let list: Vec<(String, Vec<u8>)> = match names {
        Some(names) => names
            .into_iter()
            .map(|name| {
                std::fs::read(container.dir.join(encode(&name)))
                    .map(|bytes| (name, bytes))
                    .map_err(|_| E_GS_BLOB_NOT_FOUND)
            })
            .collect::<Result<_, _>>()?,
        None => blobs(&container.dir)
            .into_iter()
            .filter_map(|(name, _)| {
                let bytes = std::fs::read(container.dir.join(encode(&name))).ok()?;
                Some((name, bytes))
            })
            .collect(),
    };
    let mut out = vec![0u8; list.len() * BLOB_ENTRY];
    for (i, (name, bytes)) in list.iter().enumerate() {
        let name_at = out.len() as u64;
        out.extend(nul(name));
        let data_at = out.len() as u64;
        out.extend_from_slice(bytes);
        let entry = &mut out[i * BLOB_ENTRY..(i + 1) * BLOB_ENTRY];
        entry[0..8].copy_from_slice(&name_at.to_le_bytes());
        entry[8..12].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        entry[16..24].copy_from_slice(&data_at.to_le_bytes());
    }
    Ok(out)
}

fn blob_count(packed: &[u8]) -> usize {
    packed.get(0..8).map_or(0, |first| {
        u64::from_le_bytes(first.try_into().unwrap_or([0; 8])) as usize / BLOB_ENTRY
    })
}

/// Copies a packed list into the program's buffer and turns its offsets into addresses.
///
/// # Safety
/// `out` is writable for `packed.len()` bytes.
unsafe fn deliver(packed: &[u8], out: *mut u8) {
    // SAFETY: as the caller promises.
    unsafe { std::ptr::copy_nonoverlapping(packed.as_ptr(), out, packed.len()) };
    for i in 0..blob_count(packed) {
        // SAFETY: the entries lie in the bytes just written.
        unsafe {
            let entry = out.add(i * BLOB_ENTRY);
            let name = (entry as *const u64).read_unaligned();
            let data = (entry.add(16) as *const u64).read_unaligned();
            (entry as *mut u64).write_unaligned(out as u64 + name);
            (entry.add(16) as *mut u64).write_unaligned(out as u64 + data);
        }
    }
}

/// # Safety
/// `names` is null or `count` string pointers.
unsafe fn requested(names: *const *const u8, count: u32) -> Option<Option<Vec<String>>> {
    if names.is_null() {
        return Some(None);
    }
    let mut out = Vec::new();
    for i in 0..count as usize {
        // SAFETY: as the caller promises.
        let name = unsafe { c_str(*names.add(i)) }?;
        if !valid_blob_name(&name) {
            return None;
        }
        out.push(name);
    }
    Some(Some(out))
}

unsafe extern "win64" fn read_blob_data(
    _: This,
    container: usize,
    names: *const *const u8,
    count: *mut u32,
    size: usize,
    out: *mut u8,
) -> i32 {
    if count.is_null() || out.is_null() {
        return E_POINTER;
    }
    let Some(container) = container_of(container) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: the program's name list.
    let Some(names) = (unsafe { requested(names, *count) }) else {
        return E_INVALIDARG;
    };
    let packed = match pack(container, names) {
        Ok(packed) => packed,
        Err(error) => return error,
    };
    if size < packed.len() {
        return E_GS_PROVIDED_BUFFER_TOO_SMALL;
    }
    // SAFETY: `size` bytes are writable and the packed list fits.
    unsafe {
        deliver(&packed, out);
        *count = blob_count(&packed) as u32;
    }
    S_OK
}

unsafe extern "win64" fn read_blob_data_async(
    _: This,
    container: usize,
    names: *const *const u8,
    count: u32,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    let Some(container) = container_of(container) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: the program's name list, copied now since it may be freed at once.
    let Some(names) = (unsafe { requested(names, count) }) else {
        return E_INVALIDARG;
    };
    let copy = Container {
        dir: container.dir.clone(),
        name: container.name.clone(),
    };
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XGameSaveReadBlobDataAsync", move || {
            match pack(&copy, names) {
                Ok(packed) => (S_OK, packed),
                Err(error) => (error, Vec::new()),
            }
        })
    }
}

unsafe extern "win64" fn read_blob_data_result(
    _: This,
    block: *mut AsyncBlock,
    size: usize,
    out: *mut u8,
    count: *mut u32,
) -> i32 {
    if out.is_null() || count.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *count = 0 };
    let mut needed = 0usize;
    // SAFETY: the program's block.
    let status = unsafe { tasks::get_result_size(block, &mut needed) };
    if status < 0 {
        // SAFETY: the program's block.
        return unsafe { tasks::finish_call(block) };
    }
    if size < needed {
        return E_GS_PROVIDED_BUFFER_TOO_SMALL;
    }
    let mut packed = vec![0u8; needed];
    // SAFETY: the program's block and a buffer of the size it asked for.
    let result = unsafe {
        tasks::one_shot_result(
            block,
            "XGameSaveReadBlobDataAsync",
            needed,
            packed.as_mut_ptr() as *mut c_void,
        )
    };
    if result >= 0 {
        // SAFETY: `size >= needed` bytes are writable.
        unsafe {
            deliver(&packed, out);
            *count = blob_count(&packed) as u32;
        }
    }
    result
}

// ---- updates ----

unsafe extern "win64" fn create_update(
    _: This,
    container: usize,
    display_name: *const u8,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    let Some(container) = container_of(container) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: a string or null.
    let display_name = unsafe { c_str(display_name) }.unwrap_or_else(|| container.name.clone());
    let update = Box::into_raw(Box::new(Update {
        dir: container.dir.clone(),
        display_name,
        ops: Vec::new(),
    })) as usize;
    track(update);
    // SAFETY: writable, checked.
    unsafe { *out = update };
    S_OK
}

unsafe extern "win64" fn close_update(_: This, update: usize) {
    if untrack(update) {
        // SAFETY: the box `create_update` made, closed once.
        drop(unsafe { Box::from_raw(update as *mut Update) });
    }
}

/// One write or delete per blob per update; the last one stands.
fn queue_op(update: &mut Update, op: Op) {
    let name = match &op {
        Op::Write(name, _) | Op::Delete(name) => name.clone(),
    };
    update.ops.retain(|o| match o {
        Op::Write(n, _) | Op::Delete(n) => *n != name,
    });
    update.ops.push(op);
}

unsafe extern "win64" fn submit_blob_write(
    _: This,
    update: usize,
    name: *const u8,
    data: *const u8,
    size: usize,
) -> i32 {
    let Some(update) = update_of(update) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: a string or null.
    let Some(name) = (unsafe { c_str(name) }).filter(|n| valid_blob_name(n)) else {
        return E_INVALIDARG;
    };
    if data.is_null() && size != 0 {
        return E_POINTER;
    }
    // SAFETY: `size` readable bytes, checked non-null.
    let bytes = if size == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, size) }.to_vec()
    };
    queue_op(update, Op::Write(name, bytes));
    S_OK
}

unsafe extern "win64" fn submit_blob_delete(_: This, update: usize, name: *const u8) -> i32 {
    let Some(update) = update_of(update) else {
        return E_GS_HANDLE_EXPIRED;
    };
    // SAFETY: a string or null.
    let Some(name) = (unsafe { c_str(name) }).filter(|n| valid_blob_name(n)) else {
        return E_INVALIDARG;
    };
    queue_op(update, Op::Delete(name));
    S_OK
}

fn commit(dir: &Path, display_name: &str, ops: &[Op]) -> i32 {
    let new_bytes: u64 = ops
        .iter()
        .map(|op| match op {
            Op::Write(_, bytes) => bytes.len() as u64,
            Op::Delete(_) => 0,
        })
        .sum();
    if new_bytes > MAX_UPDATE_BYTES {
        return E_GS_UPDATE_TOO_BIG;
    }
    let root = dir.parent().unwrap_or(dir);
    if folder_bytes(root) + new_bytes > QUOTA_BYTES {
        return E_GS_QUOTA_EXCEEDED;
    }
    if std::fs::create_dir_all(dir).is_err() {
        return E_GS_OUT_OF_LOCAL_STORAGE;
    }
    let mut staged = Vec::new();
    for op in ops {
        if let Op::Write(name, bytes) = op {
            let temp = dir.join(format!("{}~", encode(name)));
            if std::fs::write(&temp, bytes).is_err() {
                for (temp, _) in &staged {
                    let _ = std::fs::remove_file(temp);
                }
                let _ = std::fs::remove_file(&temp);
                return E_GS_OUT_OF_LOCAL_STORAGE;
            }
            staged.push((temp, dir.join(encode(name))));
        }
    }
    for (temp, target) in &staged {
        if std::fs::rename(temp, target).is_err() {
            return E_FAIL;
        }
    }
    for op in ops {
        if let Op::Delete(name) = op {
            let _ = std::fs::remove_file(dir.join(encode(name)));
        }
    }
    let _ = std::fs::write(dir.join(".display"), display_name);
    S_OK
}

unsafe extern "win64" fn submit_update(_: This, update: usize) -> i32 {
    let Some(update) = update_of(update) else {
        return E_GS_HANDLE_EXPIRED;
    };
    let result = commit(&update.dir, &update.display_name, &update.ops);
    if result >= 0 {
        update.ops.clear();
    }
    result
}

unsafe extern "win64" fn submit_update_async(
    _: This,
    update: usize,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    let Some(update) = update_of(update) else {
        return E_GS_HANDLE_EXPIRED;
    };
    let (dir, display_name) = (update.dir.clone(), update.display_name.clone());
    let ops = std::mem::take(&mut update.ops);
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XGameSaveSubmitUpdateAsync", move || {
            (commit(&dir, &display_name, &ops), Vec::new())
        })
    }
}

unsafe extern "win64" fn submit_update_result(_: This, block: *mut AsyncBlock) -> i32 {
    // SAFETY: the program's block.
    unsafe { tasks::one_shot_result(block, "XGameSaveSubmitUpdateAsync", 0, std::ptr::null_mut()) }
}

// ---- XGameSaveFiles ----

fn files_folder(config: &str) -> PathBuf {
    saves_dir().join("files").join(encode(config))
}

unsafe extern "win64" fn files_folder_async(
    _: This,
    user: usize,
    config: *const u8,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: a string or null.
    let Some(config) = (unsafe { c_str(config) }) else {
        return E_INVALIDARG;
    };
    if user == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(block, "XGameSaveFilesGetFolderWithUiAsync", move || {
            let folder = files_folder(&config);
            if std::fs::create_dir_all(&folder).is_err() {
                return (E_GS_OUT_OF_LOCAL_STORAGE, Vec::new());
            }
            (S_OK, nul(&folder.to_string_lossy()))
        })
    }
}

unsafe extern "win64" fn files_folder_result(
    _: This,
    block: *mut AsyncBlock,
    size: usize,
    folder: *mut u8,
) -> i32 {
    if folder.is_null() {
        return E_POINTER;
    }
    let mut needed = 0usize;
    // SAFETY: the program's block.
    let status = unsafe { tasks::get_result_size(block, &mut needed) };
    if status >= 0 && size < needed {
        return E_GS_PROVIDED_BUFFER_TOO_SMALL;
    }
    // SAFETY: the program's block and buffer.
    let result = unsafe {
        tasks::one_shot_result(
            block,
            "XGameSaveFilesGetFolderWithUiAsync",
            size,
            folder as *mut c_void,
        )
    };
    if result == HR_INSUFFICIENT_BUFFER {
        E_GS_PROVIDED_BUFFER_TOO_SMALL
    } else {
        result
    }
}

unsafe extern "win64" fn files_remaining_quota(
    _: This,
    user: usize,
    config: *const u8,
    remaining: *mut i64,
) -> i32 {
    if remaining.is_null() {
        return E_POINTER;
    }
    // SAFETY: a string or null.
    let Some(config) = (unsafe { c_str(config) }) else {
        return E_INVALIDARG;
    };
    if user == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: writable, checked.
    unsafe { *remaining = QUOTA_BYTES.saturating_sub(folder_bytes(&files_folder(&config))) as i64 };
    S_OK
}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![ClassImpl {
        class: "XGameSaveImpl",
        root: "IXGameSaveImpl",
        methods: vec![
            method!("XGameSaveInitializeProvider", initialize_provider),
            method!(
                "XGameSaveInitializeProviderAsync",
                initialize_provider_async
            ),
            method!(
                "XGameSaveInitializeProviderResult",
                initialize_provider_result
            ),
            method!("XGameSaveCloseProvider", close_provider),
            method!("XGameSaveGetRemainingQuota", get_remaining_quota),
            method!("XGameSaveGetRemainingQuotaAsync", get_remaining_quota_async),
            method!(
                "XGameSaveGetRemainingQuotaResult",
                get_remaining_quota_result
            ),
            method!("XGameSaveDeleteContainer", delete_container_sync),
            method!("XGameSaveDeleteContainerAsync", delete_container_async),
            method!("XGameSaveDeleteContainerResult", delete_container_result),
            method!("XGameSaveGetContainerInfo", get_container_info),
            method!("XGameSaveEnumerateContainerInfo", enumerate_container_info),
            method!(
                "XGameSaveEnumerateContainerInfoByName",
                enumerate_container_info_by_name
            ),
            method!("XGameSaveCreateContainer", create_container),
            method!("XGameSaveCloseContainer", close_container),
            method!("XGameSaveEnumerateBlobInfo", enumerate_blob_info),
            method!(
                "XGameSaveEnumerateBlobInfoByName",
                enumerate_blob_info_by_name
            ),
            method!("XGameSaveReadBlobData", read_blob_data),
            method!("XGameSaveReadBlobDataAsync", read_blob_data_async),
            method!("XGameSaveReadBlobDataResult", read_blob_data_result),
            method!("XGameSaveCreateUpdate", create_update),
            method!("XGameSaveCloseUpdate", close_update),
            method!("XGameSaveSubmitBlobWrite", submit_blob_write),
            method!("XGameSaveSubmitBlobDelete", submit_blob_delete),
            method!("XGameSaveSubmitUpdate", submit_update),
            method!("XGameSaveSubmitUpdateAsync", submit_update_async),
            method!("XGameSaveSubmitUpdateResult", submit_update_result),
            method!("XGameSaveFilesGetFolderWithUiAsync", files_folder_async),
            method!("XGameSaveFilesGetFolderWithUiResult", files_folder_result),
            method!("XGameSaveFilesGetRemainingQuota", files_remaining_quota),
        ],
        failure: E_NOTIMPL,
    }]
}
