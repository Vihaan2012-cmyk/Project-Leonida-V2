//! `XPackageImpl`: the game's own package as the one installed package, its chunks the ones of the game
//! folder, and no content package. The answers are the ones of a console with the whole game installed and
//! no network.
//!
//! Measured on the executable (all calls go through the `XPackageImpl` wrappers at RVAs 0x55dffc8-0x55e0928,
//! interface version 4): the game asks for its identifier into a 33-byte buffer, creates an installation
//! monitor for it and registers a progress callback that only raises a flag, asks the availability of one
//! chunk selector at a time (the selector array is 16-byte records: kind 0 language, 1 tag, 2 chunk id, 3
//! feature, value at +8) and treats 0 as installed and 3 as unavailable, and reads its progress record
//! (32 bytes, "completed" at +0x19). The game installs chunks only for availability 1 or 2, never answered
//! here. It mounts content packages with `XPackageMountWithUiAsync` and waits on the block; the wrapper of
//! `XPackageEnumeratePackages` (0x55e0684) has no caller.
//!
//! Not measured: the error codes of an uninstalled package (`HR_NOT_FOUND` is used) and the layout of
//! `XPackageDetails`.

mod layout;

use std::collections::HashMap;
use std::ffi::{CString, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use layout::Layout;

use super::com::{ClassImpl, c_str, method, write_c_str};
use super::store::note_once;
use super::tasks::{self, AsyncBlock};
use super::{
    E_INVALIDARG, E_NOTIMPL, E_POINTER, HR_INSUFFICIENT_BUFFER, HR_NETWORK_UNREACHABLE,
    HR_NOT_FOUND, S_OK, attribute, element_text, game_config, game_dir, package_identifier,
};

type This = *mut c_void;

const E_ACCESS_DENIED: i32 = 0x8007_0005_u32 as i32;

/// `XPackageGetCurrentProcessPackageIdentifier(bufferSize, buffer)`.
unsafe extern "win64" fn current_identifier(_: This, size: usize, buffer: *mut u8) -> i32 {
    if buffer.is_null() || size == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: `size` bytes are writable.
    if unsafe { write_c_str(buffer, size, &package_identifier()) } {
        S_OK
    } else {
        HR_INSUFFICIENT_BUFFER
    }
}

/// The program runs from the package folder, so it is a packaged process.
unsafe extern "win64" fn is_packaged_process(_: This) -> u8 {
    1
}

/// `XPackageGetUserLocale(localeSize, locale)`: the `language` setting (`LEONIDA_LOCALE` overrides), `en-US` otherwise.
unsafe extern "win64" fn user_locale(_: This, size: usize, buffer: *mut u8) -> i32 {
    if buffer.is_null() || size == 0 {
        return E_INVALIDARG;
    }
    let locale = super::locale::locale_tag();
    // SAFETY: `size` bytes are writable.
    if unsafe { write_c_str(buffer, size, &locale) } {
        S_OK
    } else {
        HR_INSUFFICIENT_BUFFER
    }
}

/// `XPackageMount(identifier, &mount)`: the game itself cannot be mounted, and no content package is installed.
unsafe extern "win64" fn mount(_: This, identifier: *const u8, handle: *mut usize) -> i32 {
    if handle.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *handle = 0 };
    // SAFETY: a string or null.
    let Some(identifier) = (unsafe { c_str(identifier) }) else {
        return E_INVALIDARG;
    };
    mount_status(&identifier)
}

fn mount_status(identifier: &str) -> i32 {
    if identifier == package_identifier() {
        return E_ACCESS_DENIED;
    }
    note_once(
        "XPackageMount",
        &format!("XPackageMount({identifier}): no such content package is installed"),
    );
    HR_NOT_FOUND
}

/// No mount is ever made, so every mount handle is one the program invented.
unsafe extern "win64" fn mount_path_size(_: This, _handle: usize, size: *mut usize) -> i32 {
    if !size.is_null() {
        // SAFETY: writable, checked.
        unsafe { *size = 0 };
    }
    E_INVALIDARG
}

unsafe extern "win64" fn mount_path(
    _: This,
    _handle: usize,
    _size: usize,
    _buffer: *mut u8,
) -> i32 {
    E_INVALIDARG
}

unsafe extern "win64" fn close_mount(_: This, _handle: usize) {}

const MOUNT_LABEL: &str = "XPackageMountWithUi";

/// `XPackageMountWithUiAsync(identifier, block)`: the call begins and completes with the mount's failure.
unsafe extern "win64" fn mount_with_ui_async(
    _: This,
    identifier: *const u8,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: a string or null.
    let Some(identifier) = (unsafe { c_str(identifier) }) else {
        return E_INVALIDARG;
    };
    let status = mount_status(&identifier);
    // SAFETY: the program's block.
    unsafe { tasks::begin_one_shot(block, MOUNT_LABEL, move || (status, Vec::new())) }
}

unsafe extern "win64" fn mount_with_ui_result(
    _: This,
    block: *mut AsyncBlock,
    handle: *mut usize,
) -> i32 {
    if handle.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *handle = 0 };
    // SAFETY: the program's block and handle.
    unsafe { tasks::one_shot_result(block, MOUNT_LABEL, 8, handle as *mut c_void) }
}

// ---- chunks ----

const AVAILABLE: u32 = 0;
const UNAVAILABLE: u32 = 3;

#[repr(C)]
struct RawSelector {
    kind: u32,
    _padding: u32,
    value: usize,
}

enum Selector {
    Language(String),
    Tag,
    Chunk(u32),
    Feature,
}

/// # Safety
/// `selectors` is null or holds `count` selector records, any text in them a string or null.
unsafe fn read_selectors(selectors: *const RawSelector, count: u32) -> Option<Vec<Selector>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if selectors.is_null() {
        return None;
    }
    (0..count as usize)
        .map(|i| {
            // SAFETY: as the caller promises.
            let raw = unsafe { &*selectors.add(i) };
            // SAFETY: as the caller promises.
            let text = || unsafe { c_str(raw.value as *const u8) }.unwrap_or_default();
            match raw.kind {
                0 => Some(Selector::Language(text())),
                1 => Some(Selector::Tag),
                2 => Some(Selector::Chunk(raw.value as u32)),
                3 => Some(Selector::Feature),
                _ => None,
            }
        })
        .collect()
}

fn layout() -> Layout {
    game_dir().map(|dir| Layout::read(&dir)).unwrap_or_default()
}

fn availability(layout: &Layout, selector: &Selector) -> u32 {
    let installed = match selector {
        Selector::Chunk(id) => layout.chunk_installed(*id),
        Selector::Language(tag) => layout.language_installed(tag),
        Selector::Tag | Selector::Feature => false,
    };
    if installed { AVAILABLE } else { UNAVAILABLE }
}

fn all_available(layout: &Layout, selectors: &[Selector]) -> bool {
    selectors
        .iter()
        .all(|selector| availability(layout, selector) == AVAILABLE)
}

/// Only the game's own package is installed.
///
/// # Safety
/// `identifier` is a string or null.
unsafe fn own_package(identifier: *const u8) -> Result<(), i32> {
    // SAFETY: as the caller promises.
    let Some(identifier) = (unsafe { c_str(identifier) }) else {
        return Err(E_INVALIDARG);
    };
    if identifier == package_identifier() {
        Ok(())
    } else {
        note_once(
            "XPackage:unknown",
            &format!("XPackage call for {identifier}: no such package is installed"),
        );
        Err(HR_NOT_FOUND)
    }
}

/// The selectors of a chunk call on the game's own package.
///
/// # Safety
/// As for [`own_package`] and [`read_selectors`].
unsafe fn chunk_call(
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
) -> Result<Vec<Selector>, i32> {
    // SAFETY: as the caller promises.
    unsafe { own_package(identifier) }?;
    // SAFETY: as the caller promises.
    unsafe { read_selectors(selectors, count) }.ok_or(E_INVALIDARG)
}

/// `XPackageFindChunkAvailability(identifier, count, selectors, &availability)`: installed when every selector
/// is, unavailable otherwise; nothing is ever pending or installable.
unsafe extern "win64" fn find_chunk_availability(
    _: This,
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
    out: *mut u32,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = UNAVAILABLE };
    // SAFETY: the program's arguments.
    let selectors = match unsafe { chunk_call(identifier, count, selectors) } {
        Ok(selectors) => selectors,
        Err(status) => return status,
    };
    let all = all_available(&layout(), &selectors);
    // SAFETY: writable, checked.
    unsafe { *out = if all { AVAILABLE } else { UNAVAILABLE } };
    S_OK
}

type ChunkCallback = unsafe extern "win64" fn(*mut c_void, *const RawSelector, u32) -> u8;

/// `XPackageEnumerateChunkAvailability(identifier, kind, context, callback)`: the languages or chunks the game
/// folder knows, each with its state.
unsafe extern "win64" fn enumerate_chunk_availability(
    _: This,
    identifier: *const u8,
    kind: u32,
    context: *mut c_void,
    callback: usize,
) -> i32 {
    if callback == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: a string or null.
    if let Err(status) = unsafe { own_package(identifier) } {
        return status;
    }
    let layout = layout();
    let visit: ChunkCallback =
        // SAFETY: the program's callback, of the documented shape.
        unsafe { std::mem::transmute(callback) };
    let call = |raw: RawSelector, availability: u32| {
        // SAFETY: the program's callback, handed a record that lives for the call.
        unsafe { visit(context, &raw, availability) != 0 }
    };
    match kind {
        0 => {
            let languages: Vec<CString> = layout
                .languages
                .iter()
                .filter_map(|l| CString::new(l.as_str()).ok())
                .collect();
            for language in &languages {
                let raw = RawSelector {
                    kind: 0,
                    _padding: 0,
                    value: language.as_ptr() as usize,
                };
                if !call(raw, AVAILABLE) {
                    break;
                }
            }
        }
        2 => {
            for chunk in &layout.chunks {
                let raw = RawSelector {
                    kind: 2,
                    _padding: 0,
                    value: chunk.id as usize,
                };
                let state = availability(&layout, &Selector::Chunk(chunk.id));
                if !call(raw, state) {
                    break;
                }
            }
        }
        _ => {}
    }
    S_OK
}

/// The order of an install that is finished cannot change.
unsafe extern "win64" fn change_chunk_install_order(
    _: This,
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
) -> i32 {
    // SAFETY: the program's arguments.
    match unsafe { chunk_call(identifier, count, selectors) } {
        Ok(_) => S_OK,
        Err(status) => status,
    }
}

/// `XPackageEstimateDownloadSize(identifier, count, selectors, &bytes, &confirm)`: installed chunks cost nothing.
unsafe extern "win64" fn estimate_download_size(
    _: This,
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
    bytes: *mut u64,
    confirm: *mut u8,
) -> i32 {
    if !bytes.is_null() {
        // SAFETY: writable, checked.
        unsafe { *bytes = 0 };
    }
    if !confirm.is_null() {
        // SAFETY: writable, checked.
        unsafe { *confirm = 0 };
    }
    // SAFETY: the program's arguments.
    let selectors = match unsafe { chunk_call(identifier, count, selectors) } {
        Ok(selectors) => selectors,
        Err(status) => return status,
    };
    if all_available(&layout(), &selectors) {
        S_OK
    } else {
        HR_NOT_FOUND
    }
}

/// Installed files are never removed.
unsafe extern "win64" fn uninstall_chunks(
    _: This,
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
) -> i32 {
    // SAFETY: the program's arguments.
    match unsafe { chunk_call(identifier, count, selectors) } {
        Ok(_) => {
            note_once(
                "XPackageUninstallChunks",
                "XPackageUninstallChunks: refused, the game folder is never changed",
            );
            E_ACCESS_DENIED
        }
        Err(status) => status,
    }
}

unsafe extern "win64" fn uninstall_package(_: This, identifier: *const u8) -> u8 {
    note_once(
        "XPackageUninstallPackage",
        &format!(
            "XPackageUninstallPackage({}): refused, nothing is uninstalled",
            // SAFETY: a string or null.
            unsafe { c_str(identifier) }.unwrap_or_default()
        ),
    );
    0
}

// ---- installation monitors ----

struct Monitor {
    queue: usize,
    notify: bool,
    bytes: u64,
}

static MONITORS: Mutex<Option<HashMap<usize, Monitor>>> = Mutex::new(None);
static TOKENS: AtomicU64 = AtomicU64::new(1 << 40);

fn monitors() -> MutexGuard<'static, Option<HashMap<usize, Monitor>>> {
    MONITORS.lock().unwrap_or_else(|e| e.into_inner())
}

fn new_monitor(queue: usize, notify: bool, bytes: u64) -> usize {
    let handle = Box::into_raw(Box::new(0u64)) as usize;
    monitors().get_or_insert_with(HashMap::new).insert(
        handle,
        Monitor {
            queue,
            notify,
            bytes,
        },
    );
    handle
}

fn selected_bytes(layout: &Layout, selectors: &[Selector]) -> u64 {
    if selectors.is_empty() {
        return layout.installed_bytes(None);
    }
    selectors
        .iter()
        .map(|selector| match selector {
            Selector::Chunk(id) => layout.installed_bytes(Some(*id)),
            _ => 0,
        })
        .sum()
}

/// `XPackageCreateInstallationMonitor(identifier, count, selectors, intervalMs, queue, &monitor)`.
unsafe extern "win64" fn create_installation_monitor(
    _: This,
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
    _interval: u32,
    queue: usize,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    // SAFETY: the program's arguments.
    let selectors = match unsafe { chunk_call(identifier, count, selectors) } {
        Ok(selectors) => selectors,
        Err(status) => return status,
    };
    let layout = layout();
    if !all_available(&layout, &selectors) {
        return HR_NOT_FOUND;
    }
    let bytes = selected_bytes(&layout, &selectors);
    // SAFETY: writable, checked.
    unsafe { *out = new_monitor(queue, false, bytes) };
    S_OK
}

unsafe extern "win64" fn close_installation_monitor(_: This, monitor: usize) {
    let removed = monitors().as_mut().and_then(|m| m.remove(&monitor));
    if removed.is_some() {
        // SAFETY: the box `new_monitor` made, closed once.
        drop(unsafe { Box::from_raw(monitor as *mut u64) });
    }
}

/// `XPackageGetInstallationProgress(monitor, &progress)`: everything installed and launchable. The record is
/// total, installed and launch bytes, then the launchable and completed flags.
unsafe extern "win64" fn get_installation_progress(_: This, monitor: usize, out: *mut u8) {
    if out.is_null() {
        return;
    }
    let mut record = [0u8; 32];
    let bytes = monitors()
        .as_ref()
        .and_then(|m| m.get(&monitor))
        .map(|m| m.bytes);
    if let Some(bytes) = bytes {
        for field in 0..3 {
            record[field * 8..field * 8 + 8].copy_from_slice(&bytes.to_le_bytes());
        }
        record[0x18] = 1;
        record[0x19] = 1;
    }
    // SAFETY: the progress record has 32 bytes.
    unsafe { std::ptr::copy_nonoverlapping(record.as_ptr(), out, record.len()) };
}

unsafe extern "win64" fn update_installation_monitor(_: This, _monitor: usize) -> u8 {
    0
}

type ProgressCallback = unsafe extern "win64" fn(*mut c_void, usize);

struct Report {
    callback: usize,
    context: usize,
    monitor: usize,
}

unsafe extern "win64" fn report_progress(context: *mut c_void, canceled: u8) {
    // SAFETY: the box `register_progress` leaked for this one callback.
    let report = unsafe { Box::from_raw(context as *mut Report) };
    let open = monitors()
        .as_ref()
        .is_some_and(|m| m.contains_key(&report.monitor));
    if canceled != 0 || !open {
        return;
    }
    let callback: ProgressCallback =
        // SAFETY: the callback the program registered, of the documented shape.
        unsafe { std::mem::transmute(report.callback) };
    // SAFETY: the program's callback, handed what it registered and its monitor.
    unsafe { callback(report.context as *mut c_void, report.monitor) };
}

/// `XPackageRegisterInstallationProgressChanged(monitor, context, callback, &token)`. The progress of a finished
/// install never changes, so the callback runs once, and only for a monitor an install call made: that is
/// what a caller waiting for the end of its install needs.
unsafe extern "win64" fn register_progress(
    _: This,
    monitor: usize,
    context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    if callback == 0 || token.is_null() {
        return E_INVALIDARG;
    }
    let found = monitors()
        .as_ref()
        .and_then(|m| m.get(&monitor))
        .map(|m| (m.notify, m.queue));
    let Some((notify, queue)) = found else {
        return E_INVALIDARG;
    };
    // SAFETY: writable, checked.
    unsafe { *token = TOKENS.fetch_add(1, Ordering::Relaxed) };
    if notify {
        let report = Box::into_raw(Box::new(Report {
            callback,
            context,
            monitor,
        }));
        // SAFETY: the queue handle of the monitor, or null for the process queue.
        let queued = unsafe {
            tasks::submit(
                queue as *mut c_void,
                0,
                report as usize,
                report_progress as *const () as usize,
            )
        };
        if queued != S_OK {
            // SAFETY: not taken over by a queue.
            drop(unsafe { Box::from_raw(report) });
        }
    }
    S_OK
}

unsafe extern "win64" fn unregister_progress(
    _: This,
    _monitor: usize,
    _token: u64,
    _wait: u8,
) -> u8 {
    1
}

const INSTALL_LABEL: &str = "XPackageInstallChunks";

/// What an install of `selectors` comes to: a finished monitor, or no way to fetch what is missing.
fn install(selectors: &[Selector]) -> (i32, Vec<u8>) {
    let layout = layout();
    if !all_available(&layout, selectors) {
        note_once(
            "XPackageInstallChunks",
            "XPackageInstallChunks: a chunk is not in the game folder and cannot be downloaded offline",
        );
        return (HR_NETWORK_UNREACHABLE, Vec::new());
    }
    let monitor = new_monitor(0, true, selected_bytes(&layout, selectors));
    (S_OK, monitor.to_le_bytes().to_vec())
}

/// `XPackageInstallChunksAsync(identifier, count, selectors, intervalMs, suppressConfirmation, block)`.
unsafe extern "win64" fn install_chunks_async(
    _: This,
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
    _interval: u32,
    _suppress: u8,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's arguments.
    let selectors = match unsafe { chunk_call(identifier, count, selectors) } {
        Ok(selectors) => selectors,
        Err(status) => return status,
    };
    // SAFETY: the program's block.
    unsafe { tasks::begin_one_shot(block, INSTALL_LABEL, move || install(&selectors)) }
}

unsafe extern "win64" fn install_chunks_result(
    _: This,
    block: *mut AsyncBlock,
    monitor: *mut usize,
) -> i32 {
    if monitor.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *monitor = 0 };
    // SAFETY: the program's block and handle.
    unsafe { tasks::one_shot_result(block, INSTALL_LABEL, 8, monitor as *mut c_void) }
}

/// `XPackageInstallChunks(identifier, count, selectors, intervalMs, suppressConfirmation, &monitor)`.
unsafe extern "win64" fn install_chunks(
    _: This,
    identifier: *const u8,
    count: u32,
    selectors: *const RawSelector,
    _interval: u32,
    _suppress: u8,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable, checked.
    unsafe { *out = 0 };
    // SAFETY: the program's arguments.
    let selectors = match unsafe { chunk_call(identifier, count, selectors) } {
        Ok(selectors) => selectors,
        Err(status) => return status,
    };
    let (status, bytes) = install(&selectors);
    if let Ok(bytes) = <[u8; 8]>::try_from(bytes.as_slice()) {
        // SAFETY: writable, checked.
        unsafe { *out = usize::from_le_bytes(bytes) };
    }
    status
}

// ---- packages ----

type PackageCallback = unsafe extern "win64" fn(*mut c_void, *const Details) -> u8;

/// `XPackageDetails`, as the GDK header lays it out (unmeasured: the game never enumerates).
#[repr(C)]
struct Details {
    identifier: *const u8,
    version: [u16; 4],
    kind: u32,
    display_name: *const u8,
    description: *const u8,
    publisher: *const u8,
    store_id: *const u8,
    installing: u8,
}

const KIND_GAME: u32 = 0;

fn text(value: Option<String>) -> CString {
    CString::new(value.unwrap_or_default()).unwrap_or_default()
}

/// `XPackageEnumeratePackages(kind, scope, context, callback)`: the game's own package, and no content.
unsafe extern "win64" fn enumerate_packages(
    _: This,
    kind: u32,
    _scope: u32,
    context: *mut c_void,
    callback: usize,
) -> i32 {
    if callback == 0 {
        return E_INVALIDARG;
    }
    if kind != KIND_GAME {
        return S_OK;
    }
    let config = game_config().unwrap_or_default();
    let identifier = text(Some(package_identifier()));
    let display_name = text(attribute(&config, "ShellVisuals", "DefaultDisplayName"));
    let publisher = text(attribute(&config, "ShellVisuals", "PublisherDisplayName"));
    let store_id = text(element_text(&config, "StoreId"));
    let description = text(None);
    let mut version = [0u16; 4];
    let numbers = attribute(&config, "Identity", "Version").unwrap_or_default();
    for (slot, part) in version.iter_mut().zip(numbers.split('.')) {
        *slot = part.parse().unwrap_or(0);
    }
    let details = Details {
        identifier: identifier.as_ptr() as *const u8,
        version,
        kind: KIND_GAME,
        display_name: display_name.as_ptr() as *const u8,
        description: description.as_ptr() as *const u8,
        publisher: publisher.as_ptr() as *const u8,
        store_id: store_id.as_ptr() as *const u8,
        installing: 0,
    };
    let visit: PackageCallback =
        // SAFETY: the program's callback, of the documented shape.
        unsafe { std::mem::transmute(callback) };
    // SAFETY: the program's callback, handed a record that lives for the call.
    unsafe { visit(context, &details) };
    S_OK
}

/// `XPackageEnumerateFeatures(identifier, context, callback)`: the package declares none.
unsafe extern "win64" fn enumerate_features(
    _: This,
    identifier: *const u8,
    _context: *mut c_void,
    callback: usize,
) -> i32 {
    if callback == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: a string or null.
    match unsafe { own_package(identifier) } {
        Ok(()) => S_OK,
        Err(status) => status,
    }
}

/// A package is only installed by a download, and none finishes while offline, so the callback never runs.
unsafe extern "win64" fn register_package_installed(
    _: This,
    _queue: usize,
    _context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    if callback == 0 || token.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: writable, checked.
    unsafe { *token = TOKENS.fetch_add(1, Ordering::Relaxed) };
    S_OK
}

unsafe extern "win64" fn unregister_package_installed(_: This, _token: u64, _wait: u8) -> u8 {
    1
}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![ClassImpl {
        class: "XPackageImpl",
        root: "IXPackageImpl",
        methods: vec![
            method!(
                "XPackageGetCurrentProcessPackageIdentifier",
                current_identifier
            ),
            method!("XPackageIsPackagedProcess", is_packaged_process),
            method!("XPackageGetUserLocale", user_locale),
            method!("XPackageMount", mount),
            method!("XPackageGetMountPathSize", mount_path_size),
            method!("XPackageGetMountPath", mount_path),
            method!("XPackageCloseMountHandle", close_mount),
            method!("XPackageMountWithUiAsync", mount_with_ui_async),
            method!("XPackageMountWithUiResult", mount_with_ui_result),
            method!("XPackageFindChunkAvailability", find_chunk_availability),
            method!(
                "XPackageEnumerateChunkAvailability",
                enumerate_chunk_availability
            ),
            method!(
                "XPackageChangeChunkInstallOrder",
                change_chunk_install_order
            ),
            method!("XPackageEstimateDownloadSize", estimate_download_size),
            method!("XPackageUninstallChunks", uninstall_chunks),
            method!("XPackageUninstallPackage", uninstall_package),
            method!(
                "XPackageCreateInstallationMonitor",
                create_installation_monitor
            ),
            method!(
                "XPackageCloseInstallationMonitorHandle",
                close_installation_monitor
            ),
            method!("XPackageGetInstallationProgress", get_installation_progress),
            method!(
                "XPackageUpdateInstallationMonitor",
                update_installation_monitor
            ),
            method!(
                "XPackageRegisterInstallationProgressChanged",
                register_progress
            ),
            method!(
                "XPackageUnregisterInstallationProgressChanged",
                unregister_progress
            ),
            method!("XPackageInstallChunks", install_chunks),
            method!("XPackageInstallChunksAsync", install_chunks_async),
            method!("XPackageInstallChunksResult", install_chunks_result),
            method!("XPackageEnumerateFeatures", enumerate_features),
            method!("XPackageEnumeratePackages", enumerate_packages),
            method!("@35", enumerate_packages),
            method!("@39", enumerate_packages),
            method!(
                "XPackageRegisterPackageInstalled",
                register_package_installed
            ),
            method!("@36", register_package_installed),
            method!("@40", register_package_installed),
            method!(
                "XPackageUnregisterPackageInstalled",
                unregister_package_installed
            ),
        ],
        failure: E_NOTIMPL,
    }]
}
