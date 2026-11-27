//! The runtime classes every title starts with: `XErrorImpl`, `XThreadingImpl`, `XSystemImpl`,
//! `XSystemAnalyticsImpl`, `XGameImpl`, `XGameRuntimeFeatureImpl`, `XPersistentLocalStorageImpl` and
//! `XGameEventImpl`.
//!
//! Signatures follow the public GDK reference. The slots the executable and the package's libraries call were
//! measured (`runtime_imports.md`): `XErrorImpl` slot 3 is `XErrorReport(HRESULT, const char*)`;
//! `XSystemAnalyticsImpl` slot 3 takes the address its result goes to.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use super::com::{ClassImpl, c_str, method, write_c_str};
use super::tasks::{self, AsyncBlock};
use super::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, HR_DISK_FULL, HR_INSUFFICIENT_BUFFER,
    HR_PATH_NOT_FOUND, S_OK,
};
use crate::trace::note;

type This = *mut c_void;

// ---- XErrorImpl ----

static ERROR_CALLBACK: Mutex<(usize, usize)> = Mutex::new((0, 0));
static ERROR_REPORTS: AtomicU32 = AtomicU32::new(0);

/// `XErrorReport`: every report is noted (the first 64) and handed to the callback the program set.
pub(super) unsafe extern "win64" fn error_report(_: This, hr: i32, message: *const u8) -> i32 {
    // SAFETY: a NUL-terminated string or null.
    let text = unsafe { c_str(message) }.unwrap_or_default();
    if ERROR_REPORTS.fetch_add(1, Ordering::Relaxed) < 64 {
        note(format!("XErrorReport({hr:#010x}, {text})"));
    }
    let (callback, context) = *ERROR_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if callback != 0 {
        // SAFETY: the callback the program set, with its documented shape.
        let callback: unsafe extern "win64" fn(i32, *const u8, *mut c_void) -> u8 =
            unsafe { std::mem::transmute(callback) };
        // SAFETY: the program's own callback.
        unsafe { callback(hr, message, context as *mut c_void) };
    }
    S_OK
}

unsafe extern "win64" fn error_set_callback(_: This, callback: usize, context: usize) {
    *ERROR_CALLBACK.lock().unwrap_or_else(|e| e.into_inner()) = (callback, context);
}

unsafe extern "win64" fn error_set_options(_: This, _first: u32, _second: u32) {}

// ---- XThreadingImpl ----

thread_local! {
    static TIME_SENSITIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe extern "win64" fn thread_set_time_sensitive(_: This, value: u8) -> i32 {
    TIME_SENSITIVE.with(|flag| flag.set(value != 0));
    S_OK
}

unsafe extern "win64" fn thread_assert_not_time_sensitive(_: This) {}

unsafe extern "win64" fn thread_is_time_sensitive(_: This) -> u8 {
    u8::from(TIME_SENSITIVE.with(|flag| flag.get()))
}

/// Slot 28 (`XTaskQueueIsEmpty`) is private: the executable's signature is unmeasured, and no argument is read.
unsafe extern "win64" fn task_queue_is_empty(_: This) -> u8 {
    note("XThreadingImpl slot 28 (XTaskQueueIsEmpty) called; answering false".to_string());
    0
}

// ---- XSystemImpl ----

const CONSOLE_ID: &str = "00000000.00000000.00000000.00000000.00";
const SANDBOX_ID: &str = "RETAIL";
const APP_DEVICE_ID: &str = "00000000000000000000000000000000000000000000";

/// A fixed identifier into a `(size, buffer, used)` out parameter; `minimum` is the size the API requires.
unsafe fn copy_id(
    text: &str,
    minimum: usize,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    if buffer.is_null() {
        return E_INVALIDARG;
    }
    if size < minimum.max(text.len() + 1) {
        return HR_INSUFFICIENT_BUFFER;
    }
    // SAFETY: `size` bytes are writable and the text fits.
    unsafe { write_c_str(buffer, size, text) };
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = text.len() + 1 };
    }
    S_OK
}

unsafe extern "win64" fn system_console_id(
    _: This,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    // SAFETY: the program's buffers.
    unsafe { copy_id(CONSOLE_ID, 39, size, buffer, used) }
}

unsafe extern "win64" fn system_sandbox_id(
    _: This,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    // SAFETY: the program's buffers.
    unsafe { copy_id(SANDBOX_ID, 16, size, buffer, used) }
}

unsafe extern "win64" fn system_app_device_id(
    _: This,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    // SAFETY: the program's buffers.
    unsafe { copy_id(APP_DEVICE_ID, 45, size, buffer, used) }
}

/// Nothing is tracked offline, so the callback is accepted and never called.
unsafe extern "win64" fn system_handle_track(_: This, _callback: usize, _context: usize) -> i32 {
    S_OK
}

unsafe extern "win64" fn system_is_handle_valid(_: This, handle: usize) -> u8 {
    u8::from(handle != 0)
}

unsafe extern "win64" fn system_allow_full_download_bandwidth(_: This, _allow: u8) {}

// ---- XSystemAnalyticsImpl ----

#[repr(C)]
struct SystemAnalyticsInfo {
    os_version: [u16; 4],
    hosting_os_version: [u16; 4],
    family: [u8; 64],
    form: [u8; 64],
}

fn padded<const N: usize>(text: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let bytes = text.as_bytes();
    let count = bytes.len().min(N - 1);
    out[..count].copy_from_slice(&bytes[..count]);
    out
}

/// The executable (RVA 0x2478440) compares the hosting OS version with 10.0.26100.9628 and, from there on, enables
/// device feature 0x11f and skips a startup probe: the console OS it ships for is at least that.
const CONSOLE_OS: [u16; 4] = [10, 0, 26100, 9628];

/// `major.minor.build.revision` from `LEONIDA_HOSTING_OS`, else the console's.
fn parse_os_version(text: &str) -> Option<[u16; 4]> {
    let mut parts = text.trim().split('.');
    let mut out = [0u16; 4];
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

fn os_version() -> [u16; 4] {
    std::env::var("LEONIDA_HOSTING_OS")
        .ok()
        .and_then(|text| parse_os_version(&text))
        .unwrap_or(CONSOLE_OS)
}

/// A member function that returns a struct takes the address for it after `this` and hands it back.
unsafe extern "win64" fn analytics_info(
    _: This,
    result: *mut SystemAnalyticsInfo,
) -> *mut SystemAnalyticsInfo {
    if !result.is_null() {
        let info = SystemAnalyticsInfo {
            os_version: os_version(),
            hosting_os_version: os_version(),
            family: padded("Xbox"),
            form: padded(&super::setting("LEONIDA_ANALYTICS_FORM", "Xbox Series X")),
        };
        // SAFETY: `result` is writable for the whole struct.
        unsafe { result.write_unaligned(info) };
    }
    result
}

// ---- XGameImpl ----

unsafe extern "win64" fn game_title_id(_: This, out: *mut u32) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    match super::title_id() {
        Some(id) => {
            // SAFETY: writable, checked.
            unsafe { *out = id };
            S_OK
        }
        None => {
            // SAFETY: writable, checked.
            unsafe { *out = 0 };
            E_FAIL
        }
    }
}

/// A new game launch is noted and ignored: there is nothing to launch.
unsafe extern "win64" fn game_launch_new(
    _: This,
    executable: *const u8,
    arguments: *const u8,
    _user: usize,
) {
    // SAFETY: strings or null.
    let (executable, arguments) = unsafe { (c_str(executable), c_str(arguments)) };
    note(format!(
        "XLaunchNewGame({}, {}) ignored",
        executable.unwrap_or_default(),
        arguments.unwrap_or_default()
    ));
}

/// A restart after a crash is accepted with nothing registered.
unsafe extern "win64" fn game_restart_on_crash(
    _: This,
    _command_line: *const u8,
    _reserved: u32,
) -> i32 {
    S_OK
}

// ---- XGameRuntimeFeatureImpl ----

/// The class each `XGameRuntimeFeature` value goes through; networking is unavailable offline.
const FEATURES: [Option<&str>; 22] = [
    Some("XAccessibilityImpl"),
    Some("XAppCaptureImpl"),
    Some("XThreadingImpl"),
    Some("XThreadingImpl"),
    Some("XDisplayImpl"),
    Some("XGameImpl"),
    Some("XGameInviteImpl"),
    Some("XGameSaveImpl"),
    Some("XGameUiImpl"),
    Some("XLauncherImpl"),
    None,
    Some("XPackageImpl"),
    Some("XPersistentLocalStorageImpl"),
    Some("XAccessibilityImpl"),
    Some("XStoreImpl"),
    Some("XSystemImpl"),
    Some("XThreadingImpl"),
    Some("XThreadingImpl"),
    Some("XUserImpl"),
    Some("XErrorImpl"),
    Some("XGameEventImpl"),
    Some("XGameStreamingImpl"),
];

unsafe extern "win64" fn feature_available(_: This, feature: u32) -> u8 {
    let class = FEATURES.get(feature as usize).copied().flatten();
    u8::from(class.is_some_and(super::com::served))
}

// ---- XPersistentLocalStorageImpl ----

#[repr(C)]
struct SpaceInfo {
    available_free_bytes: u64,
    total_free_bytes: u64,
    used_bytes: u64,
    total_bytes: u64,
}

fn folder_bytes(folder: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn storage_folder() -> Option<String> {
    let folder = super::persistent_dir();
    std::fs::create_dir_all(&folder).ok()?;
    Some(folder.to_string_lossy().into_owned())
}

unsafe extern "win64" fn storage_path_size(_: This, size: *mut usize) -> i32 {
    if size.is_null() {
        return E_POINTER;
    }
    let Some(folder) = storage_folder() else {
        // SAFETY: writable, checked.
        unsafe { *size = 0 };
        return HR_PATH_NOT_FOUND;
    };
    // SAFETY: writable, checked.
    unsafe { *size = folder.len() + 1 };
    S_OK
}

unsafe extern "win64" fn storage_path(
    _: This,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> i32 {
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = 0 };
    }
    if buffer.is_null() {
        return E_POINTER;
    }
    let Some(folder) = storage_folder() else {
        return HR_PATH_NOT_FOUND;
    };
    // SAFETY: `size` bytes are writable.
    if !unsafe { write_c_str(buffer, size, &folder) } {
        return HR_INSUFFICIENT_BUFFER;
    }
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = folder.len() + 1 };
    }
    S_OK
}

fn space() -> Option<SpaceInfo> {
    let folder = super::persistent_dir();
    std::fs::create_dir_all(&folder).ok()?;
    let total = super::persistent_quota();
    let used = folder_bytes(&folder);
    let left = total.saturating_sub(used);
    Some(SpaceInfo {
        available_free_bytes: left,
        total_free_bytes: left,
        used_bytes: used,
        total_bytes: total,
    })
}

unsafe extern "win64" fn storage_space_info(_: This, info: *mut SpaceInfo) -> i32 {
    if info.is_null() {
        return E_POINTER;
    }
    match space() {
        Some(found) => {
            // SAFETY: writable, checked.
            unsafe { info.write_unaligned(found) };
            S_OK
        }
        None => HR_PATH_NOT_FOUND,
    }
}

/// Nobody can be asked for more room offline: a request the quota still holds succeeds, any other is a full disk.
unsafe extern "win64" fn storage_prompt_async(
    _: This,
    requested: u64,
    block: *mut AsyncBlock,
) -> i32 {
    if block.is_null() {
        return E_POINTER;
    }
    // SAFETY: the program's block.
    unsafe {
        tasks::begin_one_shot(
            block,
            "XPersistentLocalStoragePromptUserForSpaceAsync",
            move || {
                let fits = space().is_some_and(|s| requested <= s.total_free_bytes);
                (if fits { S_OK } else { HR_DISK_FULL }, Vec::new())
            },
        )
    }
}

unsafe extern "win64" fn storage_prompt_result(_: This, block: *mut AsyncBlock) -> i32 {
    // SAFETY: the program's block.
    unsafe {
        tasks::one_shot_result(
            block,
            "XPersistentLocalStoragePromptUserForSpaceAsync",
            0,
            std::ptr::null_mut(),
        )
    }
}

// ---- XGameEventImpl ----

static EVENTS: AtomicU32 = AtomicU32::new(0);

/// `XGameEventWrite(user, serviceConfigId, playSessionId, eventName, dimensionsJson, measurementsJson)`: the
/// events are dropped; the first one's name is noted.
unsafe extern "win64" fn game_event_write(
    _: This,
    _user: usize,
    _config: *const u8,
    _session: *const u8,
    name: *const u8,
    dimensions: *const u8,
    measurements: *const u8,
) -> i32 {
    // SAFETY: strings or null.
    let name = unsafe { c_str(name) };
    let Some(name) = name else {
        return E_INVALIDARG;
    };
    let logging = std::env::var("LEONIDA_LOG_GAME_EVENTS").is_ok_and(|v| v == "1");
    if logging {
        // SAFETY: strings or null.
        let (d, m) = unsafe { (c_str(dimensions), c_str(measurements)) };
        note(format!(
            "game event {name} dimensions={} measurements={}",
            d.unwrap_or_default(),
            m.unwrap_or_default()
        ));
    } else if EVENTS.fetch_add(1, Ordering::Relaxed) == 0 {
        note(format!("game events are dropped offline (first: {name})"));
    }
    S_OK
}

pub(super) fn impls() -> Vec<ClassImpl> {
    vec![
        ClassImpl {
            class: "XErrorImpl",
            root: "IXErrorImpl",
            methods: vec![
                method!("@3", error_report),
                method!("XErrorSetCallback", error_set_callback),
                method!("XErrorSetOptions", error_set_options),
            ],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XThreadingImpl",
            root: "IXThreadingImpl",
            methods: vec![
                method!("XAsyncGetStatus", tasks::m_async_get_status),
                method!("XAsyncGetResultSize", tasks::m_async_get_result_size),
                method!("XAsyncCancel", tasks::m_async_cancel),
                method!("XAsyncRun", tasks::m_async_run),
                method!("XAsyncBegin", tasks::m_async_begin),
                method!("XAsyncSchedule", tasks::m_async_schedule),
                method!("XAsyncComplete", tasks::m_async_complete),
                method!("XAsyncGetResult", tasks::m_async_get_result),
                method!("XTaskQueueCreate", tasks::m_queue_create),
                method!("XTaskQueueCreateComposite", tasks::m_queue_create_composite),
                method!("XTaskQueueGetPort", tasks::m_queue_get_port),
                method!("XTaskQueueDuplicateHandle", tasks::m_queue_duplicate),
                method!("XTaskQueueDispatch", tasks::m_queue_dispatch),
                method!("XTaskQueueCloseHandle", tasks::m_queue_close),
                method!("XTaskQueueSubmitCallback", tasks::m_queue_submit),
                method!(
                    "XTaskQueueSubmitDelayedCallback",
                    tasks::m_queue_submit_delayed
                ),
                method!("XTaskQueueRegisterWaiter", tasks::m_queue_register_waiter),
                method!(
                    "XTaskQueueUnregisterWaiter",
                    tasks::m_queue_unregister_waiter
                ),
                method!("XTaskQueueTerminate", tasks::m_queue_terminate),
                method!("XTaskQueueRegisterMonitor", tasks::m_queue_register_monitor),
                method!(
                    "XTaskQueueUnregisterMonitor",
                    tasks::m_queue_unregister_monitor
                ),
                method!(
                    "XTaskQueueGetCurrentProcessTaskQueue",
                    tasks::m_queue_get_process
                ),
                method!(
                    "XTaskQueueSetCurrentProcessTaskQueue",
                    tasks::m_queue_set_process
                ),
                method!("XThreadSetTimeSensitive", thread_set_time_sensitive),
                method!("XTaskQueueIsEmpty", task_queue_is_empty),
                method!(
                    "XThreadAssertNotTimeSensitive",
                    thread_assert_not_time_sensitive
                ),
                method!("XThreadIsTimeSensitive", thread_is_time_sensitive),
            ],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XSystemImpl",
            root: "IXSystemImpl",
            methods: vec![
                method!("XSystemGetConsoleId", system_console_id),
                method!("XSystemGetXboxLiveSandboxId", system_sandbox_id),
                method!("XSystemGetAppSpecificDeviceId", system_app_device_id),
                method!("XSystemHandleTrack", system_handle_track),
                method!("XSystemIsHandleValid", system_is_handle_valid),
                method!(
                    "XSystemAllowFullDownloadBandwidth",
                    system_allow_full_download_bandwidth
                ),
            ],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XSystemAnalyticsImpl",
            root: "IXSystemAnalyticsImpl",
            methods: vec![method!("XSystemGetAnalyticsInfo", analytics_info)],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XGameImpl",
            root: "IXGameImpl",
            methods: vec![
                method!("XGameGetXboxTitleId", game_title_id),
                method!("XLaunchNewGame", game_launch_new),
                method!("XLaunchRestartOnCrash", game_restart_on_crash),
            ],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XGameRuntimeFeatureImpl",
            root: "IXGameRuntimeFeatureImpl",
            methods: vec![method!("XGameRuntimeIsFeatureAvailable", feature_available)],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XPersistentLocalStorageImpl",
            root: "IXPersistentLocalStorageImpl",
            methods: vec![
                method!("XPersistentLocalStorageGetPathSize", storage_path_size),
                method!("XPersistentLocalStorageGetPath", storage_path),
                method!("XPersistentLocalStorageGetSpaceInfo", storage_space_info),
                method!(
                    "XPersistentLocalStoragePromptUserForSpaceAsync",
                    storage_prompt_async
                ),
                method!(
                    "XPersistentLocalStoragePromptUserForSpaceResult",
                    storage_prompt_result
                ),
            ],
            failure: E_NOTIMPL,
        },
        ClassImpl {
            class: "XGameEventImpl",
            root: "IXGameEventImpl",
            methods: vec![method!("XGameEventWrite", game_event_write)],
            failure: E_NOTIMPL,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(version: [u16; 4]) -> u64 {
        u64::from(version[0]) << 48
            | u64::from(version[1]) << 32
            | u64::from(version[2]) << 16
            | u64::from(version[3])
    }

    #[test]
    fn default_os_version_passes_the_executables_gate() {
        assert!(packed(CONSOLE_OS) >= 0x000a_0000_65f4_259c);
        assert!(packed([10, 0, 22000, 0]) < 0x000a_0000_65f4_259c);
    }

    #[test]
    fn os_version_text_needs_four_numbers() {
        assert_eq!(parse_os_version("10.0.26100.1"), Some([10, 0, 26100, 1]));
        assert_eq!(parse_os_version("10.0.26100"), None);
        assert_eq!(parse_os_version("10.0.26100.1.2"), None);
        assert_eq!(parse_os_version("10.0.x.1"), None);
    }
}
