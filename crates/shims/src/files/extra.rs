//! The rest of the Win32 file calls a program may make: the 8-bit forms of the calls in the parent module, copies and
//! more kinds of moves, disk space, named pipes and Windows Error Reporting's file list. Every one goes through the
//! same path mapping, so none reaches a path the sandbox keeps shut.

use super::*;
use windows_sys::Win32::Foundation::{ERROR_INVALID_HANDLE, ERROR_NOT_SUPPORTED};

const COPY_FILE_FAIL_IF_EXISTS: u32 = 1;
const S_OK: i32 = 0;

fn widen(name: *const u8) -> Vec<u16> {
    narrow_ptr_to_string(name)
        .encode_utf16()
        .chain([0])
        .collect()
}

fn hresult(code: u32) -> i32 {
    (0x8007_0000 | code) as i32
}

/// A function of the system's kernel library by name, for the calls the sandbox leaves alone when it is off.
fn host_function(name: &str) -> usize {
    let symbol: Vec<u8> = name.bytes().chain([0]).collect();
    let Ok(module) = crate::hostlib::load("kernel32.dll", 0) else {
        return 0;
    };
    // SAFETY: asks the system's kernel library, loaded from the system folder, for one export.
    unsafe {
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            module as *mut c_void,
            symbol.as_ptr(),
        )
        .map_or(0, |f| f as usize)
    }
}

unsafe extern "win64" fn create_file_a(
    name: *const u8,
    access: u32,
    share: u32,
    security: *const SECURITY_ATTRIBUTES,
    disposition: u32,
    flags: u32,
    template: HANDLE,
) -> HANDLE {
    let wide = widen(name);
    // SAFETY: a terminated name; the other arguments are the program's.
    unsafe {
        create_file_w(
            wide.as_ptr(),
            access,
            share,
            security,
            disposition,
            flags,
            template,
        )
    }
}

unsafe extern "win64" fn create_directory_a(name: *const u8, security: *const c_void) -> i32 {
    // SAFETY: a terminated name.
    unsafe { create_directory_w(widen(name).as_ptr(), security) }
}

unsafe extern "win64" fn create_directory_ex_w(
    _template: *const u16,
    name: *const u16,
    security: *const c_void,
) -> i32 {
    // SAFETY: forwarded as given.
    unsafe { create_directory_w(name, security) }
}

unsafe extern "win64" fn create_directory_ex_a(
    _template: *const u8,
    name: *const u8,
    security: *const c_void,
) -> i32 {
    // SAFETY: a terminated name.
    unsafe { create_directory_w(widen(name).as_ptr(), security) }
}

unsafe extern "win64" fn remove_directory_a(name: *const u8) -> i32 {
    // SAFETY: a terminated name.
    unsafe { remove_directory_w(widen(name).as_ptr()) }
}

unsafe extern "win64" fn delete_file_a(name: *const u8) -> i32 {
    // SAFETY: a terminated name.
    unsafe { delete_file_w(widen(name).as_ptr()) }
}

unsafe extern "win64" fn set_file_attributes_a(name: *const u8, attributes: u32) -> i32 {
    // SAFETY: a terminated name.
    unsafe { set_file_attributes_w(widen(name).as_ptr(), attributes) }
}

unsafe extern "win64" fn move_file_w(from: *const u16, to: *const u16) -> i32 {
    // SAFETY: forwarded as given.
    unsafe { move_file_ex_w(from, to, 0) }
}

unsafe extern "win64" fn move_file_a(from: *const u8, to: *const u8) -> i32 {
    // SAFETY: terminated names.
    unsafe { move_file_ex_w(widen(from).as_ptr(), widen(to).as_ptr(), 0) }
}

unsafe extern "win64" fn move_file_ex_a(from: *const u8, to: *const u8, flags: u32) -> i32 {
    // SAFETY: terminated names.
    unsafe { move_file_ex_w(widen(from).as_ptr(), widen(to).as_ptr(), flags) }
}

unsafe extern "win64" fn move_file_with_progress_w(
    from: *const u16,
    to: *const u16,
    _progress: usize,
    _data: usize,
    flags: u32,
) -> i32 {
    // SAFETY: forwarded as given.
    unsafe { move_file_ex_w(from, to, flags) }
}

unsafe extern "win64" fn move_file_with_progress_a(
    from: *const u8,
    to: *const u8,
    _progress: usize,
    _data: usize,
    flags: u32,
) -> i32 {
    // SAFETY: terminated names.
    unsafe { move_file_ex_w(widen(from).as_ptr(), widen(to).as_ptr(), flags) }
}

fn copy_file(from: &str, to: &str, fail_if_exists: bool) -> Result<(), u32> {
    let source = target(from, false)?;
    let destination = target(to, true)?;
    if !visible(&source, from) {
        return Err(ERROR_FILE_NOT_FOUND);
    }
    let location = read_path(&source, from);
    if location.is_dir() {
        return Err(ERROR_ACCESS_DENIED);
    }
    if fail_if_exists && visible(&destination, to) {
        return Err(ERROR_FILE_EXISTS);
    }
    let copy = write_path(&destination, to, false)?;
    std::fs::copy(&location, &copy)
        .map(|_| ())
        .map_err(|e| io_code(&e))
}

fn copied(result: Result<(), u32>) -> i32 {
    match result {
        Ok(()) => TRUE,
        Err(code) => fail_bool(code),
    }
}

unsafe extern "win64" fn copy_file_w(from: *const u16, to: *const u16, fail_if_exists: i32) -> i32 {
    copied(copy_file(
        &wide_ptr_to_string(from),
        &wide_ptr_to_string(to),
        fail_if_exists != 0,
    ))
}

unsafe extern "win64" fn copy_file_a(from: *const u8, to: *const u8, fail_if_exists: i32) -> i32 {
    copied(copy_file(
        &narrow_ptr_to_string(from),
        &narrow_ptr_to_string(to),
        fail_if_exists != 0,
    ))
}

unsafe extern "win64" fn copy_file_ex_w(
    from: *const u16,
    to: *const u16,
    _progress: usize,
    _data: usize,
    _cancel: *mut i32,
    flags: u32,
) -> i32 {
    copied(copy_file(
        &wide_ptr_to_string(from),
        &wide_ptr_to_string(to),
        flags & COPY_FILE_FAIL_IF_EXISTS != 0,
    ))
}

unsafe extern "win64" fn copy_file_ex_a(
    from: *const u8,
    to: *const u8,
    _progress: usize,
    _data: usize,
    _cancel: *mut i32,
    flags: u32,
) -> i32 {
    copied(copy_file(
        &narrow_ptr_to_string(from),
        &narrow_ptr_to_string(to),
        flags & COPY_FILE_FAIL_IF_EXISTS != 0,
    ))
}

unsafe extern "win64" fn find_first_file_a(pattern: *const u8, data: *mut c_void) -> HANDLE {
    // SAFETY: forwarded as given.
    unsafe {
        find_first(
            &narrow_ptr_to_string(pattern),
            0,
            data,
            0,
            std::ptr::null(),
            0,
            true,
        )
    }
}

unsafe extern "win64" fn find_first_file_ex_a(
    pattern: *const u8,
    level: i32,
    data: *mut c_void,
    operation: i32,
    filter: *const c_void,
    flags: u32,
) -> HANDLE {
    // SAFETY: forwarded as given.
    unsafe {
        find_first(
            &narrow_ptr_to_string(pattern),
            level,
            data,
            operation,
            filter,
            flags,
            true,
        )
    }
}

unsafe extern "win64" fn find_next_file_a(handle: HANDLE, data: *mut c_void) -> i32 {
    // SAFETY: forwarded as given.
    unsafe { find_next(handle, data, true) }
}

/// The root of the drive a call about disk space is answered for, `None` for the program's own drive.
fn disk_root(name: *const u16) -> Result<Option<Vec<u16>>, u32> {
    if name.is_null() {
        return Ok(None);
    }
    let text = wide_ptr_to_string(name);
    let mapped = target(&text, false)?;
    let location = read_path(&mapped, &text).to_string_lossy().into_owned();
    let location = location.trim_start_matches("\\\\?\\");
    let drive = location.as_bytes();
    if drive.len() < 2 || drive[1] != b':' {
        return Err(ERROR_PATH_NOT_FOUND);
    }
    Ok(Some(
        format!("{}:\\", drive[0] as char)
            .encode_utf16()
            .chain([0])
            .collect(),
    ))
}

unsafe extern "win64" fn get_disk_free_space_w(
    root: *const u16,
    sectors: *mut u32,
    bytes: *mut u32,
    free: *mut u32,
    total: *mut u32,
) -> i32 {
    match disk_root(root) {
        Ok(drive) => {
            let pointer = drive.as_ref().map_or(std::ptr::null(), |d| d.as_ptr());
            // SAFETY: a terminated drive root or null; the outputs are the program's.
            unsafe { fs_api::GetDiskFreeSpaceW(pointer, sectors, bytes, free, total) }
        }
        Err(code) => fail_bool(code),
    }
}

unsafe extern "win64" fn get_disk_free_space_a(
    root: *const u8,
    sectors: *mut u32,
    bytes: *mut u32,
    free: *mut u32,
    total: *mut u32,
) -> i32 {
    let wide = (!root.is_null()).then(|| widen(root));
    let pointer = wide.as_ref().map_or(std::ptr::null(), |w| w.as_ptr());
    // SAFETY: a terminated name or null; the outputs are the program's.
    unsafe { get_disk_free_space_w(pointer, sectors, bytes, free, total) }
}

unsafe extern "win64" fn get_disk_free_space_ex_a(
    name: *const u8,
    available: *mut u64,
    total: *mut u64,
    free: *mut u64,
) -> i32 {
    let wide = (!name.is_null()).then(|| widen(name));
    let pointer = wide.as_ref().map_or(std::ptr::null(), |w| w.as_ptr());
    // SAFETY: a terminated name or null; the outputs are the program's.
    unsafe { get_disk_free_space_ex_w(pointer, available, total, free) }
}

type WaitPipe = unsafe extern "win64" fn(*const u16, u32) -> i32;
type PipeState = unsafe extern "win64" fn(HANDLE, *mut u32, *mut u32, *mut u32) -> i32;
type RegisterFile = unsafe extern "win64" fn(*const u16, i32, u32) -> i32;

/// Named pipes lead to other programs, so the sandbox answers as if no server were listening.
unsafe extern "win64" fn wait_named_pipe_w(name: *const u16, timeout: u32) -> i32 {
    if crate::files::sandboxed() {
        note_refusal(Refusal::Pipe, &wide_ptr_to_string(name));
        return fail_bool(ERROR_FILE_NOT_FOUND);
    }
    let function = host_function("WaitNamedPipeW");
    if function == 0 {
        return fail_bool(ERROR_NOT_SUPPORTED);
    }
    // SAFETY: the system's own function of this name and shape.
    unsafe { std::mem::transmute::<usize, WaitPipe>(function)(name, timeout) }
}

unsafe extern "win64" fn wait_named_pipe_a(name: *const u8, timeout: u32) -> i32 {
    // SAFETY: a terminated name.
    unsafe { wait_named_pipe_w(widen(name).as_ptr(), timeout) }
}

/// No pipe handle is ever handed out while the sandbox is on, so none can be changed.
unsafe extern "win64" fn set_named_pipe_handle_state(
    pipe: HANDLE,
    mode: *mut u32,
    collection: *mut u32,
    timeout: *mut u32,
) -> i32 {
    if crate::files::sandboxed() {
        return fail_bool(ERROR_INVALID_HANDLE);
    }
    let function = host_function("SetNamedPipeHandleState");
    if function == 0 {
        return fail_bool(ERROR_NOT_SUPPORTED);
    }
    // SAFETY: the system's own function of this name and shape.
    unsafe { std::mem::transmute::<usize, PipeState>(function)(pipe, mode, collection, timeout) }
}

/// Windows Error Reporting would keep the file for its reports. Nothing is registered: a path the sandbox allows
/// is acknowledged, one it keeps shut gets the error the same call on a file would.
unsafe extern "win64" fn wer_register_file(name: *const u16, kind: i32, flags: u32) -> i32 {
    if !crate::files::sandboxed() {
        let function = host_function("WerRegisterFile");
        if function == 0 {
            return hresult(ERROR_NOT_SUPPORTED);
        }
        // SAFETY: the system's own function of this name and shape.
        return unsafe { std::mem::transmute::<usize, RegisterFile>(function)(name, kind, flags) };
    }
    match target(&wide_ptr_to_string(name), false) {
        Ok(_) => S_OK,
        Err(code) => hresult(code),
    }
}

pub(super) fn shims() -> Vec<Shim> {
    let functions: &[(&str, usize)] = &[
        ("CreateFileA", create_file_a as *const () as usize),
        ("CreateDirectoryA", create_directory_a as *const () as usize),
        (
            "CreateDirectoryExW",
            create_directory_ex_w as *const () as usize,
        ),
        (
            "CreateDirectoryExA",
            create_directory_ex_a as *const () as usize,
        ),
        ("RemoveDirectoryA", remove_directory_a as *const () as usize),
        ("DeleteFileA", delete_file_a as *const () as usize),
        (
            "SetFileAttributesA",
            set_file_attributes_a as *const () as usize,
        ),
        ("MoveFileW", move_file_w as *const () as usize),
        ("MoveFileA", move_file_a as *const () as usize),
        ("MoveFileExA", move_file_ex_a as *const () as usize),
        (
            "MoveFileWithProgressW",
            move_file_with_progress_w as *const () as usize,
        ),
        (
            "MoveFileWithProgressA",
            move_file_with_progress_a as *const () as usize,
        ),
        ("CopyFileW", copy_file_w as *const () as usize),
        ("CopyFileA", copy_file_a as *const () as usize),
        ("CopyFileExW", copy_file_ex_w as *const () as usize),
        ("CopyFileExA", copy_file_ex_a as *const () as usize),
        ("FindFirstFileA", find_first_file_a as *const () as usize),
        (
            "FindFirstFileExA",
            find_first_file_ex_a as *const () as usize,
        ),
        ("FindNextFileA", find_next_file_a as *const () as usize),
        (
            "GetDiskFreeSpaceW",
            get_disk_free_space_w as *const () as usize,
        ),
        (
            "GetDiskFreeSpaceA",
            get_disk_free_space_a as *const () as usize,
        ),
        (
            "GetDiskFreeSpaceExA",
            get_disk_free_space_ex_a as *const () as usize,
        ),
        ("WaitNamedPipeW", wait_named_pipe_w as *const () as usize),
        ("WaitNamedPipeA", wait_named_pipe_a as *const () as usize),
        (
            "SetNamedPipeHandleState",
            set_named_pipe_handle_state as *const () as usize,
        ),
        ("WerRegisterFile", wer_register_file as *const () as usize),
    ];
    let mut shims = Vec::new();
    for (name, address) in functions {
        for dll in FILE_SETS.iter().chain(
            [
                "api-ms-win-core-namedpipe-l1-1-0.dll",
                "api-ms-win-core-windowserrorreporting-l1-1-0.dll",
                "wer.dll",
            ]
            .iter(),
        ) {
            shims.push(Shim {
                dll,
                name,
                address: *address,
            });
        }
    }
    shims
}
