//! The C runtime's file calls: streams, descriptors, `stat`, `access`, `remove`, `rename` and the folder calls.
//! Each maps its names the way the Win32 calls do and then lets the system's C runtime do the work on the
//! mapped file, so a name outside the sandbox never reaches it. A refusal sets `errno` like the call would.

use std::collections::HashMap;

use super::*;

const EPERM: i32 = 1;
const ENOENT: i32 = 2;
const EACCES: i32 = 13;
const EEXIST: i32 = 17;
const EINVAL: i32 = 22;

const O_WRONLY: i32 = 0x1;
const O_RDWR: i32 = 0x2;
const O_APPEND: i32 = 0x8;
const O_TEMPORARY: i32 = 0x40;
const O_CREAT: i32 = 0x100;
const O_TRUNC: i32 = 0x200;
const O_EXCL: i32 = 0x400;
const SH_DENYNO: i32 = 0x40;

static FUNCTIONS: Mutex<Option<HashMap<&'static str, usize>>> = Mutex::new(None);

/// A function of the system's C runtime, found once.
fn real(name: &'static str) -> usize {
    if let Some(found) = FUNCTIONS
        .lock()
        .ok()
        .and_then(|f| f.as_ref().and_then(|f| f.get(name).copied()))
    {
        return found;
    }
    let found = ucrt(name);
    if found != 0
        && let Ok(mut functions) = FUNCTIONS.lock()
    {
        functions
            .get_or_insert_with(HashMap::new)
            .insert(name, found);
    }
    found
}

fn errno_of(code: u32) -> i32 {
    match code {
        2 | 3 | 53 => ENOENT,
        80 | 183 => EEXIST,
        123 => EINVAL,
        1 => EPERM,
        _ => EACCES,
    }
}

pub(super) fn set_errno(code: u32) {
    let function = real("_errno");
    if function != 0 {
        // SAFETY: `_errno` takes nothing and returns the address of the calling thread's `errno`.
        unsafe {
            *std::mem::transmute::<usize, unsafe extern "C" fn() -> *mut i32>(function)() =
                errno_of(code)
        };
    }
}

fn errno() -> i32 {
    let function = real("_errno");
    if function == 0 {
        return EACCES;
    }
    // SAFETY: as above.
    unsafe { *std::mem::transmute::<usize, unsafe extern "C" fn() -> *mut i32>(function)() }
}

fn wide_z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Maps a name for a call that reads, giving the host path as terminated UTF-16, or sets `errno` and gives `None`.
fn read_name(text: &str) -> Option<Vec<u16>> {
    match target(text, false) {
        Ok(mapped) => Some(to_wide(&read_path(&mapped, text))),
        Err(code) => {
            set_errno(code);
            None
        }
    }
}

type OpenW = unsafe extern "C" fn(*const u16, *const u16) -> *mut c_void;
type ShareOpenW = unsafe extern "C" fn(*const u16, *const u16, i32) -> *mut c_void;
type ReopenW = unsafe extern "C" fn(*const u16, *const u16, *mut c_void) -> *mut c_void;
type Close = unsafe extern "C" fn(*mut c_void) -> i32;
type OpenFdW = unsafe extern "C" fn(*mut i32, *const u16, i32, i32, i32) -> i32;
type PathModeW = unsafe extern "C" fn(*const u16, i32) -> i32;
type StatW = unsafe extern "C" fn(*const u16, *mut c_void) -> i32;

/// The host path a stream open goes to.
fn stream_path(text: &str, mode: &str) -> Result<Vec<u16>, u32> {
    let writes = mode.contains(['w', 'a', '+']);
    let mapped = target(text, writes)?;
    if !writes {
        return Ok(to_wide(&read_path(&mapped, text)));
    }
    let keeps = mode.contains(['a', '+']) && !mode.contains('w');
    write_path(&mapped, text, keeps).map(|path| to_wide(&path))
}

pub(super) fn open_stream_shared(text: &str, mode: &str, share: Option<i32>) -> *mut c_void {
    let function = real(if share.is_some() {
        "_wfsopen"
    } else {
        "_wfopen"
    });
    if function == 0 {
        return std::ptr::null_mut();
    }
    let path = match stream_path(text, mode) {
        Ok(path) => path,
        Err(code) => {
            set_errno(code);
            return std::ptr::null_mut();
        }
    };
    let mode = wide_z(mode);
    // SAFETY: `function` is `_wfopen` or `_wfsopen`; the strings are terminated.
    unsafe {
        match share {
            Some(share) => std::mem::transmute::<usize, ShareOpenW>(function)(
                path.as_ptr(),
                mode.as_ptr(),
                share,
            ),
            None => std::mem::transmute::<usize, OpenW>(function)(path.as_ptr(), mode.as_ptr()),
        }
    }
}

fn open_checked(text: &str, mode: &str, share: Option<i32>, out: *mut *mut c_void) -> i32 {
    let stream = open_stream_shared(text, mode, share);
    if !out.is_null() {
        // SAFETY: the program's output slot.
        unsafe { *out = stream };
    }
    if stream.is_null() { errno() } else { 0 }
}

unsafe extern "win64" fn wfopen_s(
    out: *mut *mut c_void,
    name: *const u16,
    mode: *const u16,
) -> i32 {
    open_checked(
        &wide_ptr_to_string(name),
        &wide_ptr_to_string(mode),
        None,
        out,
    )
}

unsafe extern "win64" fn fopen_s(out: *mut *mut c_void, name: *const u8, mode: *const u8) -> i32 {
    open_checked(
        &narrow_ptr_to_string(name),
        &narrow_ptr_to_string(mode),
        None,
        out,
    )
}

unsafe extern "win64" fn wfsopen(name: *const u16, mode: *const u16, share: i32) -> *mut c_void {
    open_stream_shared(
        &wide_ptr_to_string(name),
        &wide_ptr_to_string(mode),
        Some(share),
    )
}

unsafe extern "win64" fn fsopen(name: *const u8, mode: *const u8, share: i32) -> *mut c_void {
    open_stream_shared(
        &narrow_ptr_to_string(name),
        &narrow_ptr_to_string(mode),
        Some(share),
    )
}

fn reopen(text: &str, mode: &str, stream: *mut c_void) -> *mut c_void {
    let (reopen, close) = (real("_wfreopen"), real("fclose"));
    if reopen == 0 || close == 0 {
        return std::ptr::null_mut();
    }
    match stream_path(text, mode) {
        Ok(path) => {
            let mode = wide_z(mode);
            // SAFETY: the system's `_wfreopen`, the program's stream and terminated strings.
            unsafe {
                std::mem::transmute::<usize, ReopenW>(reopen)(path.as_ptr(), mode.as_ptr(), stream)
            }
        }
        Err(code) => {
            // SAFETY: a failed reopen closes the stream, so it does here too.
            unsafe { std::mem::transmute::<usize, Close>(close)(stream) };
            set_errno(code);
            std::ptr::null_mut()
        }
    }
}

unsafe extern "win64" fn wfreopen(
    name: *const u16,
    mode: *const u16,
    stream: *mut c_void,
) -> *mut c_void {
    reopen(&wide_ptr_to_string(name), &wide_ptr_to_string(mode), stream)
}

unsafe extern "win64" fn freopen(
    name: *const u8,
    mode: *const u8,
    stream: *mut c_void,
) -> *mut c_void {
    reopen(
        &narrow_ptr_to_string(name),
        &narrow_ptr_to_string(mode),
        stream,
    )
}

/// Opens a descriptor; gives the descriptor or -1 with `errno` set.
fn open_descriptor(text: &str, flags: i32, share: i32, mode: i32) -> i32 {
    let function = real("_wsopen_s");
    if function == 0 {
        set_errno(5);
        return -1;
    }
    let writes = flags & (O_WRONLY | O_RDWR | O_APPEND | O_CREAT | O_TRUNC | O_TEMPORARY) != 0;
    let path = target(text, writes).and_then(|mapped| {
        if !writes {
            return Ok(to_wide(&read_path(&mapped, text)));
        }
        if flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL) && visible(&mapped, text) {
            return Err(ERROR_FILE_EXISTS);
        }
        write_path(&mapped, text, flags & O_TRUNC == 0).map(|path| to_wide(&path))
    });
    let path = match path {
        Ok(path) => path,
        Err(code) => {
            set_errno(code);
            return -1;
        }
    };
    let mut descriptor = -1;
    // SAFETY: the system's `_wsopen_s` with a terminated path and a slot for the descriptor.
    let code = unsafe {
        std::mem::transmute::<usize, OpenFdW>(function)(
            &mut descriptor,
            path.as_ptr(),
            flags,
            share,
            mode,
        )
    };
    if code == 0 { descriptor } else { -1 }
}

fn open_descriptor_s(out: *mut i32, text: &str, flags: i32, share: i32, mode: i32) -> i32 {
    let descriptor = open_descriptor(text, flags, share, mode);
    if !out.is_null() {
        // SAFETY: the program's output slot.
        unsafe { *out = descriptor };
    }
    if descriptor == -1 { errno() } else { 0 }
}

unsafe extern "win64" fn wopen(name: *const u16, flags: i32, mode: i32) -> i32 {
    open_descriptor(&wide_ptr_to_string(name), flags, SH_DENYNO, mode)
}

unsafe extern "win64" fn open(name: *const u8, flags: i32, mode: i32) -> i32 {
    open_descriptor(&narrow_ptr_to_string(name), flags, SH_DENYNO, mode)
}

unsafe extern "win64" fn wsopen(name: *const u16, flags: i32, share: i32, mode: i32) -> i32 {
    open_descriptor(&wide_ptr_to_string(name), flags, share, mode)
}

unsafe extern "win64" fn sopen(name: *const u8, flags: i32, share: i32, mode: i32) -> i32 {
    open_descriptor(&narrow_ptr_to_string(name), flags, share, mode)
}

unsafe extern "win64" fn wsopen_s(
    out: *mut i32,
    name: *const u16,
    flags: i32,
    share: i32,
    mode: i32,
) -> i32 {
    open_descriptor_s(out, &wide_ptr_to_string(name), flags, share, mode)
}

unsafe extern "win64" fn sopen_s(
    out: *mut i32,
    name: *const u8,
    flags: i32,
    share: i32,
    mode: i32,
) -> i32 {
    open_descriptor_s(out, &narrow_ptr_to_string(name), flags, share, mode)
}

unsafe extern "win64" fn wcreat(name: *const u16, mode: i32) -> i32 {
    open_descriptor(
        &wide_ptr_to_string(name),
        O_CREAT | O_TRUNC | O_RDWR,
        SH_DENYNO,
        mode,
    )
}

unsafe extern "win64" fn creat(name: *const u8, mode: i32) -> i32 {
    open_descriptor(
        &narrow_ptr_to_string(name),
        O_CREAT | O_TRUNC | O_RDWR,
        SH_DENYNO,
        mode,
    )
}

fn done(result: Result<(), u32>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(code) => {
            set_errno(code);
            -1
        }
    }
}

unsafe extern "win64" fn wremove(name: *const u16) -> i32 {
    done(delete_file(&wide_ptr_to_string(name)))
}

unsafe extern "win64" fn wunlink(name: *const u16) -> i32 {
    done(delete_file(&wide_ptr_to_string(name)))
}

unsafe extern "win64" fn unlink(name: *const u8) -> i32 {
    done(delete_file(&narrow_ptr_to_string(name)))
}

unsafe extern "win64" fn wrename(from: *const u16, to: *const u16) -> i32 {
    done(move_file(
        &wide_ptr_to_string(from),
        &wide_ptr_to_string(to),
        0,
    ))
}

unsafe extern "win64" fn rename(from: *const u8, to: *const u8) -> i32 {
    done(move_file(
        &narrow_ptr_to_string(from),
        &narrow_ptr_to_string(to),
        0,
    ))
}

unsafe extern "win64" fn wmkdir(name: *const u16) -> i32 {
    done(create_directory(&wide_ptr_to_string(name)))
}

unsafe extern "win64" fn mkdir(name: *const u8) -> i32 {
    done(create_directory(&narrow_ptr_to_string(name)))
}

unsafe extern "win64" fn wrmdir(name: *const u16) -> i32 {
    done(remove_directory(&wide_ptr_to_string(name)))
}

unsafe extern "win64" fn rmdir(name: *const u8) -> i32 {
    done(remove_directory(&narrow_ptr_to_string(name)))
}

fn access(text: &str, mode: i32, name: &'static str) -> i32 {
    let function = real(name);
    let Some(path) = read_name(text).filter(|_| function != 0) else {
        return if name.ends_with("_s") { errno() } else { -1 };
    };
    // SAFETY: the system's `_waccess` or `_waccess_s` on a terminated path.
    unsafe { std::mem::transmute::<usize, PathModeW>(function)(path.as_ptr(), mode) }
}

unsafe extern "win64" fn waccess(name: *const u16, mode: i32) -> i32 {
    access(&wide_ptr_to_string(name), mode, "_waccess")
}

unsafe extern "win64" fn access_narrow(name: *const u8, mode: i32) -> i32 {
    access(&narrow_ptr_to_string(name), mode, "_waccess")
}

unsafe extern "win64" fn waccess_s(name: *const u16, mode: i32) -> i32 {
    access(&wide_ptr_to_string(name), mode, "_waccess_s")
}

unsafe extern "win64" fn access_s(name: *const u8, mode: i32) -> i32 {
    access(&narrow_ptr_to_string(name), mode, "_waccess_s")
}

fn stat(text: &str, buffer: *mut c_void, name: &'static str) -> i32 {
    let function = real(name);
    let Some(path) = read_name(text).filter(|_| function != 0) else {
        return -1;
    };
    // SAFETY: the system's `_wstat*` on a terminated path, with the program's result buffer.
    unsafe { std::mem::transmute::<usize, StatW>(function)(path.as_ptr(), buffer) }
}

macro_rules! stat_calls {
    ($($wide:ident $narrow:ident $wname:literal $nname:literal),* $(,)?) => {
        $(
            unsafe extern "win64" fn $wide(name: *const u16, buffer: *mut c_void) -> i32 {
                stat(&wide_ptr_to_string(name), buffer, $wname)
            }

            unsafe extern "win64" fn $narrow(name: *const u8, buffer: *mut c_void) -> i32 {
                stat(&narrow_ptr_to_string(name), buffer, $wname)
            }
        )*

        fn stats() -> Vec<(&'static str, &'static str, usize, usize)> {
            vec![
            $(($wname, $nname, $wide as *const () as usize, $narrow as *const () as usize)),*
        ]
        }
    };
}

stat_calls! {
    wstat64_call stat64_call "_wstat64" "_stat64",
    wstat64i32_call stat64i32_call "_wstat64i32" "_stat64i32",
    wstat32_call stat32_call "_wstat32" "_stat32",
    wstat32i64_call stat32i64_call "_wstat32i64" "_stat32i64",
    wstat_plain_call stat_plain_call "_wstat" "_stat",
    wstati64_call stati64_call "_wstati64" "_stati64",
}

/// The shims for the descriptor, rename, folder and `stat` calls and the extra stream opens, by library name.
pub(super) fn shims() -> Vec<Shim> {
    let mut calls: Vec<(String, usize)> = vec![
        ("_wfopen_s".into(), wfopen_s as *const () as usize),
        ("fopen_s".into(), fopen_s as *const () as usize),
        ("_wfsopen".into(), wfsopen as *const () as usize),
        ("_fsopen".into(), fsopen as *const () as usize),
        ("_wfreopen".into(), wfreopen as *const () as usize),
        ("freopen".into(), freopen as *const () as usize),
        ("_wopen".into(), wopen as *const () as usize),
        ("_open".into(), open as *const () as usize),
        ("_wsopen".into(), wsopen as *const () as usize),
        ("_sopen".into(), sopen as *const () as usize),
        ("_wsopen_s".into(), wsopen_s as *const () as usize),
        ("_sopen_s".into(), sopen_s as *const () as usize),
        ("_wcreat".into(), wcreat as *const () as usize),
        ("_creat".into(), creat as *const () as usize),
        ("_wremove".into(), wremove as *const () as usize),
        ("_wunlink".into(), wunlink as *const () as usize),
        ("_unlink".into(), unlink as *const () as usize),
        ("_wrename".into(), wrename as *const () as usize),
        ("rename".into(), rename as *const () as usize),
        ("_wmkdir".into(), wmkdir as *const () as usize),
        ("_mkdir".into(), mkdir as *const () as usize),
        ("_wrmdir".into(), wrmdir as *const () as usize),
        ("_rmdir".into(), rmdir as *const () as usize),
        ("_waccess".into(), waccess as *const () as usize),
        ("_access".into(), access_narrow as *const () as usize),
        ("_waccess_s".into(), waccess_s as *const () as usize),
        ("_access_s".into(), access_s as *const () as usize),
    ];
    for (wide_name, narrow_name, wide_fn, narrow_fn) in stats() {
        calls.push((wide_name.to_string(), wide_fn));
        calls.push((narrow_name.to_string(), narrow_fn));
    }
    let mut shims = Vec::new();
    for (name, address) in calls {
        let name: &'static str = Box::leak(name.into_boxed_str());
        for dll in CRT_SETS {
            shims.push(Shim { dll, name, address });
        }
    }
    shims
}

pub(super) const CRT_SETS: &[&str] = &[
    "api-ms-win-crt-stdio-l1-1-0.dll",
    "api-ms-win-crt-filesystem-l1-1-0.dll",
    "ucrtbase.dll",
];

/// The calls of the C++ library that open a file by name, `std::_Fiopen`, with the `ios_base::openmode` bits it takes.
const IN: i32 = 0x01;
const OUT: i32 = 0x02;
const ATE: i32 = 0x04;
const APP: i32 = 0x08;
const TRUNC: i32 = 0x10;
const BINARY: i32 = 0x20;
const NOREPLACE: i32 = 0x40;

/// The `fopen` mode `std::_Fiopen` picks for an open mode, `None` for a combination it refuses.
pub(super) fn fiopen_mode(mode: i32) -> Option<String> {
    let base = match mode & !(ATE | BINARY | NOREPLACE) {
        m if m == IN => "r",
        m if m == OUT || m == OUT | TRUNC => "w",
        m if m == OUT | APP || m == APP => "a",
        m if m == IN | OUT => "r+",
        m if m == IN | OUT | TRUNC => "w+",
        m if m == IN | OUT | APP || m == IN | APP => "a+",
        _ => return None,
    };
    let mut text = base.to_string();
    if mode & BINARY != 0 {
        text.push('b');
    }
    Some(text)
}

fn fiopen(text: &str, mode: i32) -> *mut c_void {
    let Some(fopen_mode) = fiopen_mode(mode) else {
        return std::ptr::null_mut();
    };
    let stream = open_stream_shared(text, &fopen_mode, None);
    let seek = real("fseek");
    if !stream.is_null() && mode & ATE != 0 && seek != 0 {
        // SAFETY: the system's `fseek` on the stream just opened; 2 is `SEEK_END`.
        unsafe {
            std::mem::transmute::<usize, unsafe extern "C" fn(*mut c_void, i32, i32) -> i32>(seek)(
                stream, 0, 2,
            )
        };
    }
    stream
}

unsafe extern "win64" fn fiopen_narrow(
    name: *const u8,
    mode: i32,
    _protection: i32,
) -> *mut c_void {
    fiopen(&narrow_ptr_to_string(name), mode)
}

unsafe extern "win64" fn fiopen_wide(name: *const u16, mode: i32, _protection: i32) -> *mut c_void {
    fiopen(&wide_ptr_to_string(name), mode)
}

pub(super) fn cpp_shims() -> Vec<Shim> {
    vec![
        Shim {
            dll: "msvcp140.dll",
            name: "?_Fiopen@std@@YAPEAU_iobuf@@PEBDHH@Z",
            address: fiopen_narrow as *const () as usize,
        },
        Shim {
            dll: "msvcp140.dll",
            name: "?_Fiopen@std@@YAPEAU_iobuf@@PEB_WHH@Z",
            address: fiopen_wide as *const () as usize,
        },
    ]
}
