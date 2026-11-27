//! `xgameruntime.dll` and `gameplatformservices.dll` as the program finds them with `LoadLibraryW` and
//! `GetProcAddress`: the initializers, `QueryApiImpl`, `XErrorReport`, and the version query of the
//! second library.
//!
//! `xgameruntime.lib`'s `XGameRuntimeInitialize` looks up `InitializeApiImpl`, `InitializeApiImplEx`,
//! `InitializeApiImplEx2`, `QueryApiImpl`, `UninitializeApiImpl` and `XErrorReport`, and a missing
//! `XErrorReport` is fatal, so all six are served. The initializers succeed offline.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::com::{Guid, query};
use super::{E_POINTER, S_OK, system, tasks};
use crate::Shim;
use crate::trace::note;

static INITIALIZED: AtomicUsize = AtomicUsize::new(0);

fn initialize(which: &str, gdk: u32, services: u32, mode: u32) -> i32 {
    if INITIALIZED.fetch_add(1, Ordering::Relaxed) == 0 {
        tasks::prepare();
        note(format!(
            "{which}(gdk {gdk}, gaming services {services}, mode {mode:#x}): initialized offline"
        ));
    }
    S_OK
}

extern "win64" fn initialize_api_impl(gdk: u32, services: u32) -> i32 {
    initialize("InitializeApiImpl", gdk, services, 0xFF)
}

extern "win64" fn initialize_api_impl_ex(gdk: u32, services: u32, mode: i8) -> i32 {
    initialize("InitializeApiImplEx", gdk, services, u32::from(mode as u8))
}

extern "win64" fn initialize_api_impl_ex2(
    gdk: u32,
    services: u32,
    mode: i8,
    _options: *const c_void,
) -> i32 {
    initialize("InitializeApiImplEx2", gdk, services, u32::from(mode as u8))
}

extern "win64" fn uninitialize_api_impl() -> i32 {
    let _ = INITIALIZED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    S_OK
}

extern "win64" fn query_api_impl(
    class: *const u8,
    interface: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: each is null or points at a GUID.
    let (class, interface) = unsafe { (Guid::read(class), Guid::read(interface)) };
    match (class, interface) {
        (Some(class), Some(interface)) => query(&class, &interface, out),
        _ => {
            // SAFETY: `out` is writable, checked above.
            unsafe { *out = std::ptr::null_mut() };
            E_POINTER
        }
    }
}

extern "win64" fn x_error_report(hr: i32, message: *const u8) -> i32 {
    // SAFETY: the same function the class serves, which needs no `this`.
    unsafe { system::error_report(std::ptr::null_mut(), hr, message) }
}

extern "win64" fn game_runtime_initialize() -> i32 {
    initialize("XGameRuntimeInitialize", 0, 0, 0)
}

extern "win64" fn game_runtime_uninitialize() {
    uninitialize_api_impl();
}

/// The same pair every time: the offline runtime has one version.
extern "win64" fn get_api_version_impl(first: *mut u64, second: *mut u64) -> i32 {
    if !first.is_null() {
        // SAFETY: writable, checked.
        unsafe { *first = 1 };
    }
    if !second.is_null() {
        // SAFETY: writable, checked.
        unsafe { *second = 1 };
    }
    S_OK
}

pub fn shims() -> Vec<Shim> {
    let entry = |dll: &'static str, name: &'static str, address: usize| Shim { dll, name, address };
    vec![
        entry(
            "xgameruntime.dll",
            "InitializeApiImpl",
            initialize_api_impl as *const () as usize,
        ),
        entry(
            "xgameruntime.dll",
            "InitializeApiImplEx",
            initialize_api_impl_ex as *const () as usize,
        ),
        entry(
            "xgameruntime.dll",
            "InitializeApiImplEx2",
            initialize_api_impl_ex2 as *const () as usize,
        ),
        entry(
            "xgameruntime.dll",
            "QueryApiImpl",
            query_api_impl as *const () as usize,
        ),
        entry(
            "xgameruntime.dll",
            "UninitializeApiImpl",
            uninitialize_api_impl as *const () as usize,
        ),
        entry(
            "xgameruntime.dll",
            "XErrorReport",
            x_error_report as *const () as usize,
        ),
        entry(
            "xgameruntime.dll",
            "XGameRuntimeInitialize",
            game_runtime_initialize as *const () as usize,
        ),
        entry(
            "xgameruntime.dll",
            "XGameRuntimeUninitialize",
            game_runtime_uninitialize as *const () as usize,
        ),
        entry(
            "gameplatformservices.dll",
            "QueryApiImpl",
            query_api_impl as *const () as usize,
        ),
        entry(
            "gameplatformservices.dll",
            "GetApiVersionImpl",
            get_api_version_impl as *const () as usize,
        ),
    ]
}
