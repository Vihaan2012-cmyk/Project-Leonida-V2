//! `LoadLibrary*`, `GetModuleHandle*`, `GetProcAddress`, `FreeLibrary` and `DisableThreadLibraryCalls`.
//!
//! A console DLL (one the shim registry has functions for, and no Windows DLL) is answered with a fake module
//! handle, and `GetProcAddress` on it resolves through the registry. What the offline policy blocks is refused
//! as if the DLL were missing, and so is every DLL that is neither a console DLL nor on the host allow-list. An
//! allowed host DLL is loaded from the system folder. `GetProcAddress` on a host module still prefers a shim of
//! the same name, so the program cannot step around these functions or the file shims by asking for them.

use std::ffi::{CStr, c_char, c_void};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_MOD_NOT_FOUND, SetLastError};
use windows_sys::Win32::System::LibraryLoader as host;

use crate::trace::note;
use crate::{Shim, host_allowed, lower};

/// The console libraries, for the ones no shim is written for yet.
const CONSOLE_DLLS: &[&str] = &[
    "xmem.dll",
    "d3d12_xs.dll",
    "xg_xs.dll",
    "xgameruntime.dll",
    "gameplatformservices.dll",
    "gameinput.dll",
    "xcrdapi.dll",
    "chad.dll",
    "xapu.dll",
    "acphal.dll",
];

/// Every library set and umbrella the program imports these functions from.
const LOADER_DLLS: &[&str] = &[
    "kernel32.dll",
    "kernelbase.dll",
    "api-ms-win-core-libraryloader-l1-1-0.dll",
    "api-ms-win-core-libraryloader-l1-1-1.dll",
    "api-ms-win-core-libraryloader-l1-2-0.dll",
    "api-ms-win-core-libraryloader-l1-2-1.dll",
    "api-ms-win-core-libraryloader-l1-2-2.dll",
    "api-ms-win-core-libraryloader-l2-1-0.dll",
    "ext-ms-win-kernel32-package-current-l1-1-0.dll",
];

const GET_MODULE_HANDLE_EX_UNCHANGED_REFCOUNT: u32 = 0x1;
const GET_MODULE_HANDLE_EX_FROM_ADDRESS: u32 = 0x4;

struct Fake {
    handle: usize,
    dll: String,
    refs: u32,
}

static FAKES: Mutex<Vec<Fake>> = Mutex::new(Vec::new());

fn registry() -> &'static [Shim] {
    static REGISTRY: OnceLock<Vec<Shim>> = OnceLock::new();
    REGISTRY.get_or_init(crate::builtin)
}

fn is_console_dll(dll: &str) -> bool {
    !host_allowed(dll)
        && (CONSOLE_DLLS.contains(&dll) || registry().iter().any(|shim| lower(shim.dll) == dll))
}

/// The fake handle of a console DLL, made on first use.
fn fake_handle(dll: &str, add_ref: bool) -> usize {
    let Ok(mut fakes) = FAKES.lock() else {
        return 0;
    };
    if let Some(fake) = fakes.iter_mut().find(|f| f.dll == dll) {
        if add_ref {
            fake.refs += 1;
        }
        return fake.handle;
    }
    let handle = Box::leak(Box::new([0u64; 2])) as *mut [u64; 2] as usize;
    fakes.push(Fake {
        handle,
        dll: dll.to_string(),
        refs: u32::from(add_ref),
    });
    handle
}

fn fake_name(handle: usize) -> Option<String> {
    FAKES
        .lock()
        .ok()?
        .iter()
        .find(|f| f.handle == handle)
        .map(|f| f.dll.clone())
}

/// What `FreeLibrary` does to a fake handle; `None` when it is not one.
fn fake_release(handle: usize) -> Option<bool> {
    let mut fakes = FAKES.lock().ok()?;
    let fake = fakes.iter_mut().find(|f| f.handle == handle)?;
    fake.refs = fake.refs.saturating_sub(1);
    Some(true)
}

fn fail(error: u32) {
    // SAFETY: sets the calling thread's last error.
    unsafe { SetLastError(error) };
}

fn wide_text(name: *const u16) -> Option<String> {
    if name.is_null() {
        return None;
    }
    let mut length = 0;
    // SAFETY: the program passes a NUL-terminated string.
    unsafe {
        while *name.add(length) != 0 {
            length += 1;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(
            name, length,
        )))
    }
}

fn narrow_text(name: *const u8) -> Option<String> {
    if name.is_null() {
        return None;
    }
    // SAFETY: the program passes a NUL-terminated string.
    Some(
        unsafe { CStr::from_ptr(name as *const c_char) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// A name as the shim registry spells DLLs: no folder, lower case, with `.dll`.
fn dll_name(name: &str) -> String {
    lower(name)
}

/// Why a name may not be loaded at all, as a trace note.
fn refused(name: &str, why: &str) {
    note(format!("library {name} refused: {why}"));
    fail(ERROR_MOD_NOT_FOUND);
}

fn load(name: Option<String>, flags: u32) -> usize {
    let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
        fail(ERROR_INVALID_PARAMETER);
        return 0;
    };
    if let Some(reason) = offline::import(&name, None) {
        refused(&name, reason.describe());
        return 0;
    }
    if let Some(why) = path_refusal(&name) {
        refused(&name, why);
        return 0;
    }
    let dll = dll_name(&name);
    if is_console_dll(&dll) {
        return fake_handle(&dll, true);
    }
    if host_allowed(&dll) {
        return match crate::hostlib::load(&dll, flags) {
            Ok(module) => module,
            Err(error) => {
                refused(&name, &error.to_string());
                0
            }
        };
    }
    refused(&name, "not a console or allowed Windows library");
    0
}

/// Why a library named with a folder may not be loaded. With the file sandbox on, only a name without a folder, a
/// path into the Windows system folder, or (for a console library, which is never loaded from there) a path into
/// the game folder is accepted.
fn path_refusal(name: &str) -> Option<&'static str> {
    if !name.contains(['\\', '/', ':']) || !crate::files::sandboxed() {
        return None;
    }
    let folder = name
        .rsplit_once(['\\', '/'])
        .map_or("", |(folder, _)| folder);
    let in_system = crate::hostlib::system_directory().is_some_and(|system| {
        let plain = |text: &str| {
            text.trim_end_matches(['\\', '/'])
                .replace('/', "\\")
                .to_lowercase()
        };
        plain(folder) == plain(&system.to_string_lossy())
    });
    let console = is_console_dll(&dll_name(name));
    if in_system || (console && crate::files::in_package(name)) {
        None
    } else {
        Some("a library path outside the Windows system folder is not allowed")
    }
}

extern "win64" fn load_library_a(name: *const u8) -> usize {
    load(narrow_text(name), 0)
}

extern "win64" fn load_library_w(name: *const u16) -> usize {
    load(wide_text(name), 0)
}

extern "win64" fn load_library_ex_a(name: *const u8, _file: usize, flags: u32) -> usize {
    load(narrow_text(name), flags)
}

extern "win64" fn load_library_ex_w(name: *const u16, _file: usize, flags: u32) -> usize {
    load(wide_text(name), flags)
}

fn module_handle(name: Option<String>, add_ref: bool) -> usize {
    let Some(name) = name else {
        let main = super::main_module();
        // SAFETY: the handle of this process's own executable.
        return if main != 0 {
            main
        } else {
            unsafe { host::GetModuleHandleW(std::ptr::null()) as usize }
        };
    };
    if let Some(reason) = offline::import(&name, None) {
        refused(&name, reason.describe());
        return 0;
    }
    let dll = dll_name(&name);
    if is_console_dll(&dll) {
        return fake_handle(&dll, add_ref);
    }
    if host_allowed(&dll) {
        // SAFETY: asks Windows for a module the allow-list names.
        return unsafe { host::GetModuleHandleW(wide(&dll).as_ptr()) as usize };
    }
    refused(&name, "not a console or allowed Windows library");
    0
}

extern "win64" fn get_module_handle_a(name: *const u8) -> usize {
    module_handle(narrow_text(name), false)
}

extern "win64" fn get_module_handle_w(name: *const u16) -> usize {
    module_handle(wide_text(name), false)
}

fn module_handle_ex(flags: u32, name: Option<String>, from_address: usize, out: *mut usize) -> i32 {
    if out.is_null() {
        fail(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let handle = if flags & GET_MODULE_HANDLE_EX_FROM_ADDRESS != 0 {
        let mut module = std::ptr::null_mut();
        // SAFETY: asks Windows which module holds an address, without changing a reference count.
        let found = unsafe {
            host::GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FROM_ADDRESS | GET_MODULE_HANDLE_EX_UNCHANGED_REFCOUNT,
                from_address as *const u16,
                &mut module,
            )
        };
        if found == 0 { 0 } else { module as usize }
    } else {
        module_handle(name, flags & GET_MODULE_HANDLE_EX_UNCHANGED_REFCOUNT == 0)
    };
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = handle };
    i32::from(handle != 0)
}

extern "win64" fn get_module_handle_ex_a(flags: u32, name: *const u8, out: *mut usize) -> i32 {
    let from_address = name as usize;
    let text = if flags & GET_MODULE_HANDLE_EX_FROM_ADDRESS != 0 {
        None
    } else {
        narrow_text(name)
    };
    module_handle_ex(flags, text, from_address, out)
}

extern "win64" fn get_module_handle_ex_w(flags: u32, name: *const u16, out: *mut usize) -> i32 {
    let from_address = name as usize;
    let text = if flags & GET_MODULE_HANDLE_EX_FROM_ADDRESS != 0 {
        None
    } else {
        wide_text(name)
    };
    module_handle_ex(flags, text, from_address, out)
}

fn host_module_name(module: usize) -> Option<String> {
    let mut buffer = [0u16; 520];
    // SAFETY: fills a buffer of the length given.
    let length = unsafe {
        host::GetModuleFileNameW(
            module as *mut c_void,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    (length > 0 && length < buffer.len())
        .then(|| dll_name(&String::from_utf16_lossy(&buffer[..length])))
}

/// The shim that stands in for `function` of a Windows module, if the registry has one.
fn host_shim(module_dll: &str, function: &str) -> Option<usize> {
    let umbrella = matches!(module_dll, "kernel32.dll" | "kernelbase.dll");
    registry()
        .iter()
        .find(|shim| {
            shim.name == function
                && (lower(shim.dll) == module_dll || (umbrella && host_allowed(shim.dll)))
        })
        .map(|shim| shim.address)
}

extern "win64" fn get_proc_address(module: usize, name: *const u8) -> usize {
    if module == 0 || name.is_null() {
        fail(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let ordinal = (name as usize) < 0x1_0000;
    let function = if ordinal { None } else { narrow_text(name) };
    let function_text = function.as_deref().unwrap_or("");
    if let Some(dll) = fake_name(module) {
        if ordinal {
            fail(windows_sys::Win32::Foundation::ERROR_PROC_NOT_FOUND);
            return 0;
        }
        if let Some(reason) = offline::import(&dll, Some(function_text)) {
            note(format!(
                "{dll}!{function_text} refused: {}",
                reason.describe()
            ));
            fail(windows_sys::Win32::Foundation::ERROR_PROC_NOT_FOUND);
            return 0;
        }
        let found = registry()
            .iter()
            .find(|shim| lower(shim.dll) == dll && shim.name == function_text);
        return match found {
            Some(shim) => shim.address,
            None => {
                note(format!(
                    "{dll}!{function_text} asked for at run time; nothing provides it"
                ));
                fail(windows_sys::Win32::Foundation::ERROR_PROC_NOT_FOUND);
                0
            }
        };
    }
    if !ordinal {
        let blocked = host_module_name(module)
            .and_then(|dll| offline::import(&dll, Some(function_text)))
            .or_else(|| offline::function(function_text));
        if let Some(reason) = blocked {
            note(format!("{function_text} refused: {}", reason.describe()));
            fail(windows_sys::Win32::Foundation::ERROR_PROC_NOT_FOUND);
            return 0;
        }
        if crate::files::sandboxed() && crate::files::blocked_api(function_text) {
            note(format!(
                "{function_text} refused: the file sandbox does not allow it"
            ));
            fail(windows_sys::Win32::Foundation::ERROR_PROC_NOT_FOUND);
            return 0;
        }
        if let Some(dll) = host_module_name(module)
            && let Some(address) = host_shim(&dll, function_text)
        {
            return address;
        }
    }
    // SAFETY: asks a module Windows loaded for one export.
    unsafe { host::GetProcAddress(module as *mut c_void, name) }.map_or(0, |f| f as usize)
}

/// The file name the program is told for `module` with the sandbox on: the game's executable and the console
/// libraries lie in the game folder, so Leonida's own folder never shows. `None` leaves the answer to Windows.
fn sandboxed_module_path(module: usize) -> Option<std::path::PathBuf> {
    if !crate::files::sandboxed() {
        return None;
    }
    let main = super::main_module();
    if module == 0 || (main != 0 && module == main) {
        return super::main_path();
    }
    let dll = fake_name(module)?;
    Some(super::game_dir()?.join(dll))
}

/// Copies `text` into the program's buffer the way `GetModuleFileName` does, truncating with a terminator.
fn put_module_name<T: Copy + Default>(text: &[T], out: *mut T, size: u32) -> u32 {
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    if out.is_null() || size == 0 {
        fail(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    let room = size as usize - 1;
    let copied = text.len().min(room);
    // SAFETY: `out` holds `size` elements, and `copied + 1 <= size`.
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), out, copied);
        *out.add(copied) = T::default();
    }
    if copied < text.len() {
        fail(ERROR_INSUFFICIENT_BUFFER);
        return size;
    }
    copied as u32
}

extern "win64" fn get_module_file_name_w(module: usize, out: *mut u16, size: u32) -> u32 {
    match sandboxed_module_path(module) {
        Some(path) => {
            let text: Vec<u16> = path.to_string_lossy().encode_utf16().collect();
            put_module_name(&text, out, size)
        }
        // SAFETY: the program's own buffer and size.
        None => unsafe { host::GetModuleFileNameW(module as *mut c_void, out, size) },
    }
}

extern "win64" fn get_module_file_name_a(module: usize, out: *mut u8, size: u32) -> u32 {
    match sandboxed_module_path(module) {
        Some(path) => {
            let text: Vec<u8> = path
                .to_string_lossy()
                .chars()
                .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
                .collect();
            put_module_name(&text, out, size)
        }
        // SAFETY: the program's own buffer and size.
        None => unsafe { host::GetModuleFileNameA(module as *mut c_void, out, size) },
    }
}

extern "win64" fn free_library(module: usize) -> i32 {
    if let Some(done) = fake_release(module) {
        return i32::from(done);
    }
    // SAFETY: releases a module Windows loaded.
    unsafe { windows_sys::Win32::Foundation::FreeLibrary(module as *mut c_void) }
}

extern "win64" fn disable_thread_library_calls(module: usize) -> i32 {
    if fake_name(module).is_some() {
        return 1;
    }
    // SAFETY: a plain query of Windows for one of its modules.
    unsafe { host::DisableThreadLibraryCalls(module as *mut c_void) }
}

pub fn shims() -> Vec<Shim> {
    let functions: [(&'static str, usize); 13] = [
        ("LoadLibraryA", load_library_a as *const () as usize),
        ("LoadLibraryW", load_library_w as *const () as usize),
        ("LoadLibraryExA", load_library_ex_a as *const () as usize),
        ("LoadLibraryExW", load_library_ex_w as *const () as usize),
        (
            "GetModuleHandleA",
            get_module_handle_a as *const () as usize,
        ),
        (
            "GetModuleHandleW",
            get_module_handle_w as *const () as usize,
        ),
        (
            "GetModuleHandleExA",
            get_module_handle_ex_a as *const () as usize,
        ),
        (
            "GetModuleHandleExW",
            get_module_handle_ex_w as *const () as usize,
        ),
        ("GetProcAddress", get_proc_address as *const () as usize),
        ("FreeLibrary", free_library as *const () as usize),
        (
            "GetModuleFileNameW",
            get_module_file_name_w as *const () as usize,
        ),
        (
            "GetModuleFileNameA",
            get_module_file_name_a as *const () as usize,
        ),
        (
            "DisableThreadLibraryCalls",
            disable_thread_library_calls as *const () as usize,
        ),
    ];
    let mut shims = Vec::new();
    for dll in LOADER_DLLS {
        for (name, address) in functions {
            shims.push(Shim { dll, name, address });
        }
    }
    shims
}
