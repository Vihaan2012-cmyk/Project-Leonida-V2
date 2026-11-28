//! The registry as the sandbox shows it: empty. The game reads one machine-wide value to learn which console it
//! runs on (`0x055deb88` in the executable) and treats a missing key as the large console, so every open is
//! answered "not found" without touching the real registry, and every write is refused. With the sandbox off the
//! calls reach Windows.

use super::*;

const ERROR_SUCCESS: i32 = 0;
const FILE_NOT_FOUND: i32 = 2;
const ACCESS_DENIED: i32 = 5;
const INVALID_HANDLE: i32 = 6;

/// The system's own function of this name, only when the sandbox is off.
fn passthrough(name: &str) -> Option<usize> {
    if crate::files::sandboxed() {
        return None;
    }
    let symbol: Vec<u8> = name.bytes().chain([0]).collect();
    let module = crate::hostlib::load("advapi32.dll", 0).ok()?;
    // SAFETY: asks the system's registry library, loaded from the system folder, for one export.
    unsafe {
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            module as *mut c_void,
            symbol.as_ptr(),
        )
    }
    .map(|f| f as usize)
}

fn hive(key: usize) -> String {
    match key as u32 {
        0x8000_0000 => "HKEY_CLASSES_ROOT".into(),
        0x8000_0001 => "HKEY_CURRENT_USER".into(),
        0x8000_0002 => "HKEY_LOCAL_MACHINE".into(),
        0x8000_0003 => "HKEY_USERS".into(),
        0x8000_0005 => "HKEY_CURRENT_CONFIG".into(),
        other => format!("key {other:#x}"),
    }
}

fn refuse_open(root: usize, sub: &str, out: *mut usize) -> i32 {
    if !out.is_null() {
        // SAFETY: the program's output slot.
        unsafe { *out = 0 };
    }
    let name = format!("{}\\{sub}", hive(root));
    note_once(
        &format!("registry:{name}"),
        format!("registry key {name} answered as not found"),
    );
    FILE_NOT_FOUND
}

fn narrow(text: *const u8) -> String {
    narrow_ptr_to_string(text)
}

unsafe extern "win64" fn open_key_w(root: usize, sub: *const u16, out: *mut usize) -> i32 {
    if let Some(function) = passthrough("RegOpenKeyW") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<
                usize,
                unsafe extern "win64" fn(usize, *const u16, *mut usize) -> i32,
            >(function)(root, sub, out)
        };
    }
    refuse_open(root, &wide_ptr_to_string(sub), out)
}

unsafe extern "win64" fn open_key_a(root: usize, sub: *const u8, out: *mut usize) -> i32 {
    if let Some(function) = passthrough("RegOpenKeyA") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<
                usize,
                unsafe extern "win64" fn(usize, *const u8, *mut usize) -> i32,
            >(function)(root, sub, out)
        };
    }
    refuse_open(root, &narrow(sub), out)
}

unsafe extern "win64" fn open_key_ex_w(
    root: usize,
    sub: *const u16,
    options: u32,
    access: u32,
    out: *mut usize,
) -> i32 {
    if let Some(function) = passthrough("RegOpenKeyExW") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<
                usize,
                unsafe extern "win64" fn(usize, *const u16, u32, u32, *mut usize) -> i32,
            >(function)(root, sub, options, access, out)
        };
    }
    refuse_open(root, &wide_ptr_to_string(sub), out)
}

unsafe extern "win64" fn open_key_ex_a(
    root: usize,
    sub: *const u8,
    options: u32,
    access: u32,
    out: *mut usize,
) -> i32 {
    if let Some(function) = passthrough("RegOpenKeyExA") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<
                usize,
                unsafe extern "win64" fn(usize, *const u8, u32, u32, *mut usize) -> i32,
            >(function)(root, sub, options, access, out)
        };
    }
    refuse_open(root, &narrow(sub), out)
}

type Query<T> =
    unsafe extern "win64" fn(usize, *const T, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;

unsafe extern "win64" fn query_value_ex_w(
    key: usize,
    name: *const u16,
    reserved: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    length: *mut u32,
) -> i32 {
    if let Some(function) = passthrough("RegQueryValueExW") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<usize, Query<u16>>(function)(
                key, name, reserved, kind, data, length,
            )
        };
    }
    INVALID_HANDLE
}

unsafe extern "win64" fn query_value_ex_a(
    key: usize,
    name: *const u8,
    reserved: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    length: *mut u32,
) -> i32 {
    if let Some(function) = passthrough("RegQueryValueExA") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<usize, Query<u8>>(function)(
                key, name, reserved, kind, data, length,
            )
        };
    }
    INVALID_HANDLE
}

unsafe extern "win64" fn get_value_w(
    key: usize,
    sub: *const u16,
    name: *const u16,
    flags: u32,
    kind: *mut u32,
    data: *mut c_void,
    length: *mut u32,
) -> i32 {
    if let Some(function) = passthrough("RegGetValueW") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<
                usize,
                unsafe extern "win64" fn(
                    usize,
                    *const u16,
                    *const u16,
                    u32,
                    *mut u32,
                    *mut c_void,
                    *mut u32,
                ) -> i32,
            >(function)(key, sub, name, flags, kind, data, length)
        };
    }
    refuse_open(key, &wide_ptr_to_string(sub), std::ptr::null_mut())
}

unsafe extern "win64" fn close_key(key: usize) -> i32 {
    if let Some(function) = passthrough("RegCloseKey") {
        // SAFETY: the system's function of this name and shape.
        return unsafe {
            std::mem::transmute::<usize, unsafe extern "win64" fn(usize) -> i32>(function)(key)
        };
    }
    if key as u32 >= 0x8000_0000 {
        ERROR_SUCCESS
    } else {
        INVALID_HANDLE
    }
}

/// Every call that would write: refused, or passed on when the sandbox is off.
macro_rules! writes {
    ($($name:ident $text:literal ($($arg:ident: $kind:ty),*)),* $(,)?) => {
        $(
            unsafe extern "win64" fn $name($($arg: $kind),*) -> i32 {
                if let Some(function) = passthrough($text) {
                    // SAFETY: the system's function of this name and shape.
                    return unsafe {
                        std::mem::transmute::<usize, unsafe extern "win64" fn($($kind),*) -> i32>(function)($($arg),*)
                    };
                }
                note_once(
                    concat!("registry:", $text),
                    concat!("registry call ", $text, " refused").to_string(),
                );
                ACCESS_DENIED
            }
        )*
    };
}

writes! {
    create_key_ex_w "RegCreateKeyExW" (key: usize, sub: *const u16, reserved: u32, class: *const u16, options: u32, access: u32, security: *const c_void, out: *mut usize, disposition: *mut u32),
    set_value_ex_w "RegSetValueExW" (key: usize, name: *const u16, reserved: u32, kind: u32, data: *const u8, length: u32),
    delete_key_w "RegDeleteKeyW" (key: usize, sub: *const u16),
    delete_value_w "RegDeleteValueW" (key: usize, name: *const u16),
}

pub(super) fn shims() -> Vec<Shim> {
    let functions: &[(&str, usize)] = &[
        ("RegOpenKeyW", open_key_w as *const () as usize),
        ("RegOpenKeyA", open_key_a as *const () as usize),
        ("RegOpenKeyExW", open_key_ex_w as *const () as usize),
        ("RegOpenKeyExA", open_key_ex_a as *const () as usize),
        ("RegQueryValueExW", query_value_ex_w as *const () as usize),
        ("RegQueryValueExA", query_value_ex_a as *const () as usize),
        ("RegGetValueW", get_value_w as *const () as usize),
        ("RegCloseKey", close_key as *const () as usize),
        ("RegCreateKeyExW", create_key_ex_w as *const () as usize),
        ("RegSetValueExW", set_value_ex_w as *const () as usize),
        ("RegDeleteKeyW", delete_key_w as *const () as usize),
        ("RegDeleteValueW", delete_value_w as *const () as usize),
    ];
    let mut shims = Vec::new();
    for (name, address) in functions {
        for dll in [
            "advapi32.dll",
            "kernelbase.dll",
            "api-ms-win-core-registry-l1-1-0.dll",
        ] {
            shims.push(Shim {
                dll,
                name,
                address: *address,
            });
        }
    }
    shims
}
