//! The file sandbox through the shim functions: what a program can reach with each family of calls once the sandbox
//! is on, and that nothing outside the game folder, `Output/` and the mods folder is read, changed or made.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{WIN32_FIND_DATAA, WIN32_FIND_DATAW};

use super::tests::{
    CREATE_ALWAYS, GENERIC_READ, GENERIC_WRITE, OPEN_EXISTING, SHARE, Setup, close, last_error,
    open, read_all, setup, w,
};
use super::{
    create_directory_w, delete_file_w, find_close, find_first_file_w, get_file_attributes_w,
    move_file_ex_w, remove_directory_w, set_file_attributes_w,
};
use crate::files::{configure, configure_sandboxed};

unsafe extern "C" {
    fn _errno() -> *mut i32;
    fn fclose(stream: *mut c_void) -> i32;
}

fn errno() -> i32 {
    // SAFETY: the calling thread's `errno`.
    unsafe { *_errno() }
}

struct Boxed {
    _s: Setup,
    package: PathBuf,
    output: PathBuf,
    outside: PathBuf,
    mods: PathBuf,
}

fn boxed() -> Boxed {
    let s = setup();
    let plain = |path: &Path| PathBuf::from(path.to_string_lossy().trim_start_matches("\\\\?\\"));
    let (package, output) = (plain(&s.package), plain(&s.output));
    let root = package.parent().expect("root").to_path_buf();
    let outside = root.join("outside");
    let mods = root.join("Mods");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::create_dir_all(mods.join("fix")).expect("mods");
    std::fs::write(outside.join("secret.txt"), b"secret").expect("secret");
    std::fs::write(mods.join("fix").join("m.txt"), b"mod").expect("mod");
    std::fs::write(package.join("data").join("a.txt"), b"package").expect("package file");
    std::fs::create_dir_all(output.join("saves")).expect("saves");
    assert!(configure_sandboxed(
        package.clone(),
        output.clone(),
        vec![mods.clone()]
    ));
    Boxed {
        _s: s,
        package,
        output,
        outside,
        mods,
    }
}

fn secret(b: &Boxed) -> String {
    b.outside.join("secret.txt").to_string_lossy().into_owned()
}

fn find_shim<T: Copy>(dll: &str, name: &str) -> T {
    let address = crate::builtin()
        .iter()
        .find(|shim| shim.dll == dll && shim.name == name)
        .unwrap_or_else(|| panic!("{dll}!{name} has no shim"))
        .address;
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `T` is the function pointer type of the shim's documented shape.
    unsafe { std::mem::transmute_copy(&address) }
}

fn a(text: &str) -> Vec<u8> {
    text.bytes().chain([0]).collect()
}

const KERNEL: &str = "kernel32.dll";
const STDIO: &str = "api-ms-win-crt-stdio-l1-1-0.dll";
const FILESYSTEM: &str = "api-ms-win-crt-filesystem-l1-1-0.dll";

#[test]
fn only_the_game_output_and_mods_folders_are_reachable() {
    let b = boxed();
    let reach = |name: &str| open(name, GENERIC_READ, OPEN_EXISTING);
    for name in [
        "GAME:\\data\\a.txt",
        "data/A.TXT",
        "\\data\\a.txt",
        ".\\data\\..\\data\\a.txt",
        &b.package.join("data").join("a.txt").to_string_lossy(),
        &b.mods.join("fix").join("m.txt").to_string_lossy(),
    ] {
        let handle = reach(name);
        assert_ne!(handle, INVALID_HANDLE_VALUE, "{name}");
        close(handle);
    }
    let saved = b.output.join("saves").join("a.sav");
    let handle = open(
        &saved.to_string_lossy(),
        GENERIC_READ | GENERIC_WRITE,
        CREATE_ALWAYS,
    );
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    close(handle);
    assert!(saved.is_file());
    let first = open("T:\\cache\\x.tmp", GENERIC_WRITE, CREATE_ALWAYS);
    assert_ne!(first, INVALID_HANDLE_VALUE);
    close(first);
}

#[test]
fn every_other_path_is_closed_and_answered_as_the_console_would() {
    let b = boxed();
    let outside = secret(&b);
    let upper = outside.to_uppercase();
    let slashes = outside.replace('\\', "/");
    for name in [
        outside.as_str(),
        upper.as_str(),
        slashes.as_str(),
        "C:\\Windows\\win.ini",
        "c:/users/someone/documents/a.docx",
        "Z:\\anything",
        &format!("\\\\?\\{outside}"),
        &format!("\\??\\{outside}"),
        &format!("\\\\?\\{}", b.package.join("data").join("a.txt").display()),
        "\\\\.\\PhysicalDrive0",
        "\\\\.\\C:",
        "\\\\?\\Volume{01234567-89ab-cdef-0123-456789abcdef}\\x",
        "\\\\server\\share\\x.bin",
        "GAME:\\..\\outside\\secret.txt",
        "data\\..\\..\\outside\\secret.txt",
        "GAME:\\data\\a.txt:hidden",
        "GAME:\\data\\a.txt::$DATA",
        "GAME:\\data\\nul",
        "GAME:\\data\\COM1",
        "GAME:\\data\\a.txt.",
        "GAME:\\data\\...\\a.txt",
        "GAME:\\data\\*",
        "GAME:\\data\\a?.txt",
    ] {
        let handle = open(name, GENERIC_READ, OPEN_EXISTING);
        assert_eq!(handle, INVALID_HANDLE_VALUE, "{name}");
        let code = last_error();
        assert!([3, 5, 53, 123, 2].contains(&code), "{name}: {code}");
    }
    let handle = open(&outside, GENERIC_READ, OPEN_EXISTING);
    assert_eq!(handle, INVALID_HANDLE_VALUE);
    assert_eq!(last_error(), 3);
    let handle = open(&outside, GENERIC_WRITE, CREATE_ALWAYS);
    assert_eq!(handle, INVALID_HANDLE_VALUE);
    assert_eq!(last_error(), 5);
    assert_eq!(
        std::fs::read(b.outside.join("secret.txt")).expect("untouched"),
        b"secret"
    );
}

#[test]
fn creating_a_file_outside_makes_nothing() {
    let b = boxed();
    for access in [GENERIC_WRITE, GENERIC_READ | GENERIC_WRITE] {
        let target = b.outside.join("new.bin");
        let handle = open(&target.to_string_lossy(), access, CREATE_ALWAYS);
        assert_eq!(handle, INVALID_HANDLE_VALUE);
        assert_eq!(last_error(), 5);
        assert!(!target.exists());
    }
    assert!(!b.output.join("overlay").join("host").join("e").exists());
    let narrow: unsafe extern "win64" fn(
        *const u8,
        u32,
        u32,
        *const c_void,
        u32,
        u32,
        HANDLE,
    ) -> HANDLE = find_shim(KERNEL, "CreateFileA");
    let name = a(&b.outside.join("new2.bin").to_string_lossy());
    // SAFETY: a terminated name.
    let handle = unsafe {
        narrow(
            name.as_ptr(),
            GENERIC_WRITE,
            SHARE,
            std::ptr::null(),
            CREATE_ALWAYS,
            0,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(handle, INVALID_HANDLE_VALUE);
    assert!(!b.outside.join("new2.bin").exists());
}

#[test]
fn queries_and_listings_of_outside_paths_find_nothing() {
    let b = boxed();
    let file = w(&secret(&b));
    let folder = w(&b.outside.to_string_lossy());
    let pattern = w(&format!("{}\\*", b.outside.display()));
    // SAFETY: terminated names.
    unsafe {
        assert_eq!(get_file_attributes_w(file.as_ptr()), u32::MAX);
        assert_eq!(last_error(), 3);
        assert_eq!(get_file_attributes_w(folder.as_ptr()), u32::MAX);
        let ex_w: unsafe extern "win64" fn(*const u16, i32, *mut c_void) -> i32 =
            find_shim(KERNEL, "GetFileAttributesExW");
        let mut info = [0u8; 40];
        assert_eq!(ex_w(file.as_ptr(), 0, info.as_mut_ptr().cast()), 0);
        let ex_a: unsafe extern "win64" fn(*const u8, i32, *mut c_void) -> i32 =
            find_shim(KERNEL, "GetFileAttributesExA");
        assert_eq!(
            ex_a(a(&secret(&b)).as_ptr(), 0, info.as_mut_ptr().cast()),
            0
        );
        let attributes_a: unsafe extern "win64" fn(*const u8) -> u32 =
            find_shim(KERNEL, "GetFileAttributesA");
        assert_eq!(attributes_a(a(&secret(&b)).as_ptr()), u32::MAX);
        let mut data: WIN32_FIND_DATAW = std::mem::zeroed();
        let handle = find_first_file_w(
            pattern.as_ptr(),
            (&mut data as *mut WIN32_FIND_DATAW).cast(),
        );
        assert_eq!(handle, INVALID_HANDLE_VALUE);
        assert_eq!(last_error(), 3);
        let first_a: unsafe extern "win64" fn(*const u8, *mut c_void) -> HANDLE =
            find_shim(KERNEL, "FindFirstFileA");
        let mut narrow_data: WIN32_FIND_DATAA = std::mem::zeroed();
        let handle = first_a(
            a(&format!("{}\\*", b.outside.display())).as_ptr(),
            (&mut narrow_data as *mut WIN32_FIND_DATAA).cast(),
        );
        assert_eq!(handle, INVALID_HANDLE_VALUE);
        let ex_first_a: unsafe extern "win64" fn(
            *const u8,
            i32,
            *mut c_void,
            i32,
            *const c_void,
            u32,
        ) -> HANDLE = find_shim(KERNEL, "FindFirstFileExA");
        let handle = ex_first_a(
            a(&format!("{}\\*", b.outside.display())).as_ptr(),
            0,
            (&mut narrow_data as *mut WIN32_FIND_DATAA).cast(),
            0,
            std::ptr::null(),
            0,
        );
        assert_eq!(handle, INVALID_HANDLE_VALUE);
        let parent = w("GAME:\\data\\..");
        let handle =
            find_first_file_w(parent.as_ptr(), (&mut data as *mut WIN32_FIND_DATAW).cast());
        assert_eq!(handle, INVALID_HANDLE_VALUE);
    }
}

#[test]
fn the_8_bit_listing_of_the_package_works() {
    let _b = boxed();
    let first: unsafe extern "win64" fn(*const u8, *mut c_void) -> HANDLE =
        find_shim(KERNEL, "FindFirstFileA");
    let next: unsafe extern "win64" fn(HANDLE, *mut c_void) -> i32 =
        find_shim(KERNEL, "FindNextFileA");
    let pattern = a("GAME:\\data\\*.txt");
    // SAFETY: a terminated pattern and a record to fill.
    unsafe {
        let mut data: WIN32_FIND_DATAA = std::mem::zeroed();
        let handle = first(
            pattern.as_ptr(),
            (&mut data as *mut WIN32_FIND_DATAA).cast(),
        );
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let name: Vec<u8> = data
            .cFileName
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        assert_eq!(name, b"a.txt");
        assert_eq!(data.nFileSizeLow, 7);
        assert_eq!(next(handle, (&mut data as *mut WIN32_FIND_DATAA).cast()), 0);
        assert_eq!(last_error(), 18);
        find_close(handle);
    }
}

#[test]
fn changing_outside_paths_is_refused_and_leaves_them_alone() {
    let b = boxed();
    let file = secret(&b);
    let made = b.outside.join("sub");
    let package_file = "GAME:\\data\\a.txt";
    // SAFETY: terminated names.
    unsafe {
        let name = w(&made.to_string_lossy());
        assert_eq!(create_directory_w(name.as_ptr(), std::ptr::null()), 0);
        assert_eq!(last_error(), 5);
        assert!(!made.exists());
        let create_a: unsafe extern "win64" fn(*const u8, *const c_void) -> i32 =
            find_shim(KERNEL, "CreateDirectoryA");
        assert_eq!(
            create_a(a(&made.to_string_lossy()).as_ptr(), std::ptr::null()),
            0
        );
        let create_ex: unsafe extern "win64" fn(*const u16, *const u16, *const c_void) -> i32 =
            find_shim(KERNEL, "CreateDirectoryExW");
        assert_eq!(
            create_ex(w(package_file).as_ptr(), name.as_ptr(), std::ptr::null()),
            0
        );
        assert!(!made.exists());
        assert_eq!(delete_file_w(w(&file).as_ptr()), 0);
        assert_eq!(last_error(), 5);
        let delete_a: unsafe extern "win64" fn(*const u8) -> i32 = find_shim(KERNEL, "DeleteFileA");
        assert_eq!(delete_a(a(&file).as_ptr()), 0);
        assert_eq!(
            remove_directory_w(w(&b.outside.to_string_lossy()).as_ptr()),
            0
        );
        let remove_a: unsafe extern "win64" fn(*const u8) -> i32 =
            find_shim(KERNEL, "RemoveDirectoryA");
        assert_eq!(remove_a(a(&b.outside.to_string_lossy()).as_ptr()), 0);
        assert_eq!(set_file_attributes_w(w(&file).as_ptr(), 0x2), 0);
        assert_eq!(last_error(), 5);
        let set_a: unsafe extern "win64" fn(*const u8, u32) -> i32 =
            find_shim(KERNEL, "SetFileAttributesA");
        assert_eq!(set_a(a(&file).as_ptr(), 0x2), 0);
    }
    assert!(b.outside.join("secret.txt").is_file());
    assert!(
        !std::fs::metadata(b.outside.join("secret.txt"))
            .expect("meta")
            .permissions()
            .readonly()
    );
    assert!(b.outside.is_dir());
}

#[test]
fn moves_and_copies_across_the_boundary_in_either_direction_are_refused() {
    let b = boxed();
    let secret_file = secret(&b);
    let inside = b.output.join("saves").join("in.txt");
    std::fs::write(&inside, b"in").expect("inside");
    let inside_name = inside.to_string_lossy().into_owned();
    let copy_w: unsafe extern "win64" fn(*const u16, *const u16, i32) -> i32 =
        find_shim(KERNEL, "CopyFileW");
    let copy_a: unsafe extern "win64" fn(*const u8, *const u8, i32) -> i32 =
        find_shim(KERNEL, "CopyFileA");
    let copy_ex: unsafe extern "win64" fn(
        *const u16,
        *const u16,
        usize,
        usize,
        *mut i32,
        u32,
    ) -> i32 = find_shim(KERNEL, "CopyFileExW");
    let move_w: unsafe extern "win64" fn(*const u16, *const u16) -> i32 =
        find_shim(KERNEL, "MoveFileW");
    let move_a: unsafe extern "win64" fn(*const u8, *const u8) -> i32 =
        find_shim(KERNEL, "MoveFileA");
    let move_progress: unsafe extern "win64" fn(*const u16, *const u16, usize, usize, u32) -> i32 =
        find_shim(KERNEL, "MoveFileWithProgressW");
    let pulled = b.output.join("saves").join("pulled.txt");
    let pushed = b.outside.join("pushed.txt");
    // SAFETY: terminated names.
    unsafe {
        assert_eq!(
            copy_w(
                w(&secret_file).as_ptr(),
                w(&pulled.to_string_lossy()).as_ptr(),
                0
            ),
            0
        );
        assert_eq!(last_error(), 3);
        assert_eq!(
            copy_a(
                a(&inside_name).as_ptr(),
                a(&pushed.to_string_lossy()).as_ptr(),
                0
            ),
            0
        );
        assert_eq!(last_error(), 5);
        assert_eq!(
            copy_ex(
                w("GAME:\\data\\a.txt").as_ptr(),
                w(&pushed.to_string_lossy()).as_ptr(),
                0,
                0,
                std::ptr::null_mut(),
                0
            ),
            0
        );
        assert_eq!(
            move_w(
                w(&secret_file).as_ptr(),
                w(&pulled.to_string_lossy()).as_ptr()
            ),
            0
        );
        assert_eq!(
            move_a(
                a(&inside_name).as_ptr(),
                a(&pushed.to_string_lossy()).as_ptr()
            ),
            0
        );
        assert_eq!(
            move_progress(
                w(&inside_name).as_ptr(),
                w(&pushed.to_string_lossy()).as_ptr(),
                0,
                0,
                1
            ),
            0
        );
        assert_eq!(
            move_file_ex_w(
                w(&secret_file).as_ptr(),
                w("GAME:\\data\\stolen.txt").as_ptr(),
                1
            ),
            0
        );
    }
    assert!(!pulled.exists() && !pushed.exists());
    assert!(inside.is_file());
    assert!(b.outside.join("secret.txt").is_file());
    assert!(!b.package.join("data").join("stolen.txt").exists());
    assert!(
        !b.output
            .join("overlay")
            .join("data")
            .join("stolen.txt")
            .exists()
    );
}

#[test]
fn copies_and_moves_inside_the_allowed_folders_work() {
    let b = boxed();
    let copy_w: unsafe extern "win64" fn(*const u16, *const u16, i32) -> i32 =
        find_shim(KERNEL, "CopyFileW");
    let target = b.output.join("saves").join("copy.txt");
    let name = w(&target.to_string_lossy());
    // SAFETY: terminated names.
    unsafe {
        assert_eq!(
            copy_w(w("GAME:\\data\\a.txt").as_ptr(), name.as_ptr(), 1),
            1
        );
        assert_eq!(std::fs::read(&target).expect("copy"), b"package");
        assert_eq!(
            copy_w(w("GAME:\\data\\a.txt").as_ptr(), name.as_ptr(), 1),
            0
        );
        assert_eq!(last_error(), 80);
        let into_package = w("GAME:\\data\\b.txt");
        assert_eq!(copy_w(name.as_ptr(), into_package.as_ptr(), 1), 1);
    }
    assert!(
        b.output
            .join("overlay")
            .join("data")
            .join("b.txt")
            .is_file()
    );
    assert!(!b.package.join("data").join("b.txt").exists());
}

#[test]
fn disk_space_is_only_reported_for_allowed_paths() {
    let b = boxed();
    let free_w: unsafe extern "win64" fn(
        *const u16,
        *mut u32,
        *mut u32,
        *mut u32,
        *mut u32,
    ) -> i32 = find_shim(KERNEL, "GetDiskFreeSpaceW");
    let free_a: unsafe extern "win64" fn(*const u8, *mut u32, *mut u32, *mut u32, *mut u32) -> i32 =
        find_shim(KERNEL, "GetDiskFreeSpaceA");
    let free_ex_a: unsafe extern "win64" fn(*const u8, *mut u64, *mut u64, *mut u64) -> i32 =
        find_shim(KERNEL, "GetDiskFreeSpaceExA");
    let (mut x, mut y, mut z, mut t) = (0u32, 0u32, 0u32, 0u32);
    let (mut big_a, mut big_b, mut big_c) = (0u64, 0u64, 0u64);
    // SAFETY: terminated names and output slots.
    unsafe {
        let outside = w(&b.outside.to_string_lossy());
        assert_eq!(free_w(outside.as_ptr(), &mut x, &mut y, &mut z, &mut t), 0);
        assert_eq!(last_error(), 3);
        assert_eq!(
            free_a(a("C:\\").as_ptr(), &mut x, &mut y, &mut z, &mut t),
            0
        );
        assert_eq!(
            free_ex_a(
                a(&b.outside.to_string_lossy()).as_ptr(),
                &mut big_a,
                &mut big_b,
                &mut big_c
            ),
            0
        );
        assert_eq!(
            free_w(w("GAME:\\").as_ptr(), &mut x, &mut y, &mut z, &mut t),
            1
        );
        assert_ne!(x, 0);
        assert_eq!(
            free_ex_a(a("GAME:\\").as_ptr(), &mut big_a, &mut big_b, &mut big_c),
            1
        );
    }
}

#[test]
fn the_c_runtime_streams_follow_the_same_rules() {
    let b = boxed();
    let secret_file = secret(&b);
    let fopen: unsafe extern "win64" fn(*const u8, *const u8) -> *mut c_void =
        find_shim(STDIO, "fopen");
    let wfopen: unsafe extern "win64" fn(*const u16, *const u16) -> *mut c_void =
        find_shim(STDIO, "_wfopen");
    let wfopen_s: unsafe extern "win64" fn(*mut *mut c_void, *const u16, *const u16) -> i32 =
        find_shim(STDIO, "_wfopen_s");
    let fopen_s: unsafe extern "win64" fn(*mut *mut c_void, *const u8, *const u8) -> i32 =
        find_shim(STDIO, "fopen_s");
    let fsopen: unsafe extern "win64" fn(*const u8, *const u8, i32) -> *mut c_void =
        find_shim(STDIO, "_fsopen");
    // SAFETY: terminated names and slots; streams are closed with the system's `fclose`.
    unsafe {
        assert!(fopen(a(&secret_file).as_ptr(), a("rb").as_ptr()).is_null());
        assert_eq!(errno(), 2);
        assert!(wfopen(w(&secret_file).as_ptr(), w("r").as_ptr()).is_null());
        let new_file = b.outside.join("made.txt").to_string_lossy().into_owned();
        assert!(fopen(a(&new_file).as_ptr(), a("wb").as_ptr()).is_null());
        assert_eq!(errno(), 13);
        assert!(!Path::new(&new_file).exists());
        let mut stream = 1usize as *mut c_void;
        assert_eq!(
            wfopen_s(&mut stream, w(&secret_file).as_ptr(), w("r").as_ptr()),
            2
        );
        assert!(stream.is_null());
        assert_eq!(
            fopen_s(&mut stream, a(&new_file).as_ptr(), a("w").as_ptr()),
            13
        );
        assert!(fsopen(a(&secret_file).as_ptr(), a("r").as_ptr(), 0x40).is_null());
        assert!(fsopen(a(&new_file).as_ptr(), a("w").as_ptr(), 0x40).is_null());
        let good = fopen(a("GAME:\\data\\a.txt").as_ptr(), a("rb").as_ptr());
        assert!(!good.is_null());
        fclose(good);
        assert_eq!(
            fopen_s(
                &mut stream,
                a("GAME:\\data\\a.txt").as_ptr(),
                a("r").as_ptr()
            ),
            0
        );
        assert!(!stream.is_null());
        fclose(stream);
        let saved = b
            .output
            .join("saves")
            .join("s.bin")
            .to_string_lossy()
            .into_owned();
        let out = fopen(a(&saved).as_ptr(), a("wb").as_ptr());
        assert!(!out.is_null());
        fclose(out);
        assert!(Path::new(&saved).is_file());
        let freopen: unsafe extern "win64" fn(*const u8, *const u8, *mut c_void) -> *mut c_void =
            find_shim(STDIO, "freopen");
        let again = fopen(a("GAME:\\data\\a.txt").as_ptr(), a("rb").as_ptr());
        assert!(freopen(a(&secret_file).as_ptr(), a("rb").as_ptr(), again).is_null());
    }
    assert!(b.outside.join("secret.txt").is_file());
}

#[test]
fn the_c_runtime_descriptors_and_path_calls_follow_the_same_rules() {
    let b = boxed();
    let secret_file = secret(&b);
    let wopen: unsafe extern "win64" fn(*const u16, i32, i32) -> i32 = find_shim(STDIO, "_wopen");
    let open_narrow: unsafe extern "win64" fn(*const u8, i32, i32) -> i32 =
        find_shim(STDIO, "_open");
    let wsopen_s: unsafe extern "win64" fn(*mut i32, *const u16, i32, i32, i32) -> i32 =
        find_shim(STDIO, "_wsopen_s");
    let wcreat: unsafe extern "win64" fn(*const u16, i32) -> i32 = find_shim(STDIO, "_wcreat");
    let wstat: unsafe extern "win64" fn(*const u16, *mut c_void) -> i32 =
        find_shim(FILESYSTEM, "_wstat64");
    let stat_narrow: unsafe extern "win64" fn(*const u8, *mut c_void) -> i32 =
        find_shim(FILESYSTEM, "_stat64i32");
    let waccess: unsafe extern "win64" fn(*const u16, i32) -> i32 =
        find_shim(FILESYSTEM, "_waccess");
    let access_s: unsafe extern "win64" fn(*const u8, i32) -> i32 =
        find_shim(FILESYSTEM, "_access_s");
    let remove: unsafe extern "win64" fn(*const u8) -> i32 = find_shim(FILESYSTEM, "remove");
    let wremove: unsafe extern "win64" fn(*const u16) -> i32 = find_shim(FILESYSTEM, "_wremove");
    let unlink: unsafe extern "win64" fn(*const u8) -> i32 = find_shim(FILESYSTEM, "_unlink");
    let rename: unsafe extern "win64" fn(*const u8, *const u8) -> i32 =
        find_shim(FILESYSTEM, "rename");
    let mkdir: unsafe extern "win64" fn(*const u8) -> i32 = find_shim(FILESYSTEM, "_mkdir");
    let wrmdir: unsafe extern "win64" fn(*const u16) -> i32 = find_shim(FILESYSTEM, "_wrmdir");
    let new_dir = b.outside.join("d").to_string_lossy().into_owned();
    let mut buffer = [0u8; 128];
    // SAFETY: terminated names and slots.
    unsafe {
        assert_eq!(wopen(w(&secret_file).as_ptr(), 0, 0), -1);
        assert_eq!(errno(), 2);
        assert_eq!(
            open_narrow(a(&secret_file).as_ptr(), 0x8000 | 0x100 | 2, 0o600),
            -1
        );
        assert_eq!(errno(), 13);
        let mut fd = 5;
        assert_eq!(wsopen_s(&mut fd, w(&secret_file).as_ptr(), 0, 0x40, 0), 2);
        assert_eq!(fd, -1);
        assert_eq!(
            wcreat(
                w(&b.outside.join("c.bin").to_string_lossy()).as_ptr(),
                0o600
            ),
            -1
        );
        assert!(!b.outside.join("c.bin").exists());
        assert_eq!(
            wstat(w(&secret_file).as_ptr(), buffer.as_mut_ptr().cast()),
            -1
        );
        assert_eq!(
            stat_narrow(a(&secret_file).as_ptr(), buffer.as_mut_ptr().cast()),
            -1
        );
        assert_eq!(waccess(w(&secret_file).as_ptr(), 0), -1);
        assert_eq!(access_s(a(&secret_file).as_ptr(), 0), 2);
        assert_eq!(remove(a(&secret_file).as_ptr()), -1);
        assert_eq!(wremove(w(&secret_file).as_ptr()), -1);
        assert_eq!(unlink(a(&secret_file).as_ptr()), -1);
        assert_eq!(errno(), 13);
        assert_eq!(
            rename(a(&secret_file).as_ptr(), a("GAME:\\data\\x").as_ptr()),
            -1
        );
        assert_eq!(mkdir(a(&new_dir).as_ptr()), -1);
        assert!(!Path::new(&new_dir).exists());
        assert_eq!(wrmdir(w(&b.outside.to_string_lossy()).as_ptr()), -1);
        let fd = wopen(w("GAME:\\data\\a.txt").as_ptr(), 0, 0);
        assert!(fd >= 0);
        assert_eq!(
            wstat(w("GAME:\\data\\a.txt").as_ptr(), buffer.as_mut_ptr().cast()),
            0
        );
        assert_eq!(waccess(w("GAME:\\data\\a.txt").as_ptr(), 0), 0);
        assert_eq!(access_s(a("GAME:\\data\\a.txt").as_ptr(), 0), 0);
    }
    assert!(b.outside.join("secret.txt").is_file());
}

#[test]
fn the_cpp_stream_open_follows_the_same_rules() {
    let b = boxed();
    let secret_file = secret(&b);
    let narrow: unsafe extern "win64" fn(*const u8, i32, i32) -> *mut c_void =
        find_shim("msvcp140.dll", "?_Fiopen@std@@YAPEAU_iobuf@@PEBDHH@Z");
    let wide: unsafe extern "win64" fn(*const u16, i32, i32) -> *mut c_void =
        find_shim("msvcp140.dll", "?_Fiopen@std@@YAPEAU_iobuf@@PEB_WHH@Z");
    let (read, write, binary) = (0x01, 0x02, 0x20);
    // SAFETY: terminated names; streams are closed with the system's `fclose`.
    unsafe {
        assert!(narrow(a(&secret_file).as_ptr(), read, 64).is_null());
        assert!(wide(w(&secret_file).as_ptr(), read | binary, 64).is_null());
        let new_file = b.outside.join("f.bin").to_string_lossy().into_owned();
        assert!(narrow(a(&new_file).as_ptr(), write | binary, 64).is_null());
        assert!(!Path::new(&new_file).exists());
        let stream = narrow(a("GAME:\\data\\a.txt").as_ptr(), read | binary, 64);
        assert!(!stream.is_null());
        fclose(stream);
        let stream = wide(w("GAME:\\data\\a.txt").as_ptr(), read, 64);
        assert!(!stream.is_null());
        fclose(stream);
        assert!(
            narrow(
                a("GAME:\\data\\a.txt").as_ptr(),
                read | write | 0x10 | 0x08,
                64
            )
            .is_null()
        );
    }
    assert_eq!(super::crt::fiopen_mode(0x01).as_deref(), Some("r"));
    assert_eq!(super::crt::fiopen_mode(0x02 | 0x20).as_deref(), Some("wb"));
    assert_eq!(super::crt::fiopen_mode(0x02 | 0x08).as_deref(), Some("a"));
    assert_eq!(super::crt::fiopen_mode(0x01 | 0x02).as_deref(), Some("r+"));
    assert_eq!(
        super::crt::fiopen_mode(0x01 | 0x02 | 0x10).as_deref(),
        Some("w+")
    );
    assert_eq!(
        super::crt::fiopen_mode(0x01 | 0x02 | 0x08).as_deref(),
        Some("a+")
    );
    assert_eq!(super::crt::fiopen_mode(0x01 | 0x10), None);
    assert_eq!(super::crt::fiopen_mode(0), None);
}

#[test]
fn named_pipes_are_closed() {
    let _b = boxed();
    let handle = open(
        "\\\\.\\pipe\\crashpad_1234",
        GENERIC_READ | GENERIC_WRITE,
        OPEN_EXISTING,
    );
    assert_eq!(handle, INVALID_HANDLE_VALUE);
    assert_eq!(last_error(), 2);
    let wait_w: unsafe extern "win64" fn(*const u16, u32) -> i32 =
        find_shim("api-ms-win-core-namedpipe-l1-1-0.dll", "WaitNamedPipeW");
    let wait_a: unsafe extern "win64" fn(*const u8, u32) -> i32 =
        find_shim(KERNEL, "WaitNamedPipeA");
    let state: unsafe extern "win64" fn(HANDLE, *mut u32, *mut u32, *mut u32) -> i32 = find_shim(
        "api-ms-win-core-namedpipe-l1-1-0.dll",
        "SetNamedPipeHandleState",
    );
    // SAFETY: terminated names.
    unsafe {
        assert_eq!(wait_w(w("\\\\.\\pipe\\crashpad_1234").as_ptr(), 100), 0);
        assert_eq!(last_error(), 2);
        assert_eq!(wait_a(a("\\\\.\\pipe\\x").as_ptr(), 0), 0);
        let mut mode = 1u32;
        assert_eq!(
            state(
                0x1234 as HANDLE,
                &mut mode,
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            0
        );
        assert_eq!(last_error(), 6);
    }
}

#[test]
fn error_reporting_registers_no_file_it_may_not_have() {
    let b = boxed();
    let register: unsafe extern "win64" fn(*const u16, i32, u32) -> i32 = find_shim(
        "api-ms-win-core-windowserrorreporting-l1-1-0.dll",
        "WerRegisterFile",
    );
    // SAFETY: terminated names.
    unsafe {
        assert_eq!(
            register(
                w(&b.output.join("log.txt").to_string_lossy()).as_ptr(),
                1,
                0
            ),
            0
        );
        assert_eq!(register(w(&secret(&b)).as_ptr(), 1, 0) as u32, 0x8007_0003);
    }
}

#[test]
fn the_registry_is_empty_and_read_only() {
    let _b = boxed();
    let open_key: unsafe extern "win64" fn(usize, *const u16, *mut usize) -> i32 =
        find_shim("advapi32.dll", "RegOpenKeyW");
    let query: unsafe extern "win64" fn(
        usize,
        *const u16,
        *mut u32,
        *mut u32,
        *mut u8,
        *mut u32,
    ) -> i32 = find_shim("advapi32.dll", "RegQueryValueExW");
    let close_key: unsafe extern "win64" fn(usize) -> i32 =
        find_shim("advapi32.dll", "RegCloseKey");
    let open_ex: unsafe extern "win64" fn(usize, *const u16, u32, u32, *mut usize) -> i32 =
        find_shim("advapi32.dll", "RegOpenKeyExW");
    let set_value: unsafe extern "win64" fn(usize, *const u16, u32, u32, *const u8, u32) -> i32 =
        find_shim("advapi32.dll", "RegSetValueExW");
    let mut key = 0xAAAAusize;
    // SAFETY: terminated names and slots.
    unsafe {
        let name = w("HARDWARE\\Description\\System\\Leonida");
        assert_eq!(open_key(0x8000_0002, name.as_ptr(), &mut key), 2);
        assert_eq!(key, 0);
        assert_eq!(open_ex(0x8000_0001, name.as_ptr(), 0, 0x20019, &mut key), 2);
        let mut kind = 0u32;
        let mut value = 0u32;
        let mut length = 4u32;
        assert_eq!(
            query(
                key,
                w("Value").as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                (&mut value as *mut u32).cast(),
                &mut length
            ),
            6
        );
        assert_eq!(close_key(0x8000_0002), 0);
        assert_eq!(close_key(0x1234), 6);
        assert_eq!(
            set_value(0x8000_0002, w("x").as_ptr(), 0, 4, [0u8; 4].as_ptr(), 4),
            5
        );
    }
    let notes = crate::trace::notes();
    let line = "registry key HKEY_LOCAL_MACHINE\\HARDWARE\\Description\\System\\Leonida answered as not found";
    assert_eq!(
        notes.iter().filter(|n| n.as_str() == line).count(),
        1,
        "{notes:?}"
    );
}

#[test]
fn a_refused_path_is_noted_once() {
    let b = boxed();
    let name = b
        .outside
        .join("noted-once-probe.txt")
        .to_string_lossy()
        .into_owned();
    for _ in 0..4 {
        assert_eq!(
            open(&name, GENERIC_READ, OPEN_EXISTING),
            INVALID_HANDLE_VALUE
        );
    }
    let _ = open(&name.to_uppercase(), GENERIC_READ, OPEN_EXISTING);
    let notes = crate::trace::notes();
    let count = notes
        .iter()
        .filter(|n| n.to_lowercase().contains("noted-once-probe"))
        .count();
    assert_eq!(count, 1, "{notes:?}");
    assert!(crate::trace::files().iter().any(|f| f == &name));
}

fn junction(link: &Path, to: &Path) -> bool {
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(to)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[test]
fn a_junction_inside_an_allowed_folder_cannot_lead_out() {
    let b = boxed();
    let via_package = b.package.join("data").join("link");
    let via_output = b.output.join("saves").join("link");
    if !junction(&via_package, &b.outside) {
        eprintln!("skipped: no junction could be made here");
        return;
    }
    assert!(junction(&via_output, &b.outside));
    for name in [
        "GAME:\\data\\link\\secret.txt",
        &via_output.join("secret.txt").to_string_lossy(),
    ] {
        assert_eq!(
            open(name, GENERIC_READ, OPEN_EXISTING),
            INVALID_HANDLE_VALUE,
            "{name}"
        );
        assert_eq!(last_error(), 3);
        assert_eq!(
            open(name, GENERIC_WRITE, CREATE_ALWAYS),
            INVALID_HANDLE_VALUE,
            "{name}"
        );
        assert_eq!(last_error(), 5);
    }
    let attributes = w("GAME:\\data\\link");
    // SAFETY: a terminated name.
    assert_eq!(
        unsafe { get_file_attributes_w(attributes.as_ptr()) },
        u32::MAX
    );
    assert!(list_folder("GAME:\\data\\link\\*").is_empty());
    assert_eq!(
        std::fs::read(b.outside.join("secret.txt")).expect("kept"),
        b"secret"
    );
    let handle = open("GAME:\\data\\a.txt", GENERIC_READ, OPEN_EXISTING);
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    assert_eq!(read_all(handle), b"package");
    close(handle);
    let _ = std::fs::remove_dir(&via_package);
    let _ = std::fs::remove_dir(&via_output);
}

fn list_folder(pattern: &str) -> Vec<String> {
    super::tests::list(pattern)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn a_link_to_another_place_inside_the_package_is_allowed() {
    let b = boxed();
    std::fs::create_dir_all(b.package.join("real")).expect("real");
    std::fs::write(b.package.join("real").join("f.txt"), b"x").expect("f");
    if !junction(
        &b.package.join("data").join("inner"),
        &b.package.join("real"),
    ) {
        return;
    }
    let handle = open("GAME:\\data\\inner\\f.txt", GENERIC_READ, OPEN_EXISTING);
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    close(handle);
    let _ = std::fs::remove_dir(b.package.join("data").join("inner"));
}

#[test]
fn turning_the_sandbox_off_restores_the_old_reach() {
    let b = boxed();
    let secret_file = secret(&b);
    assert_eq!(
        open(&secret_file, GENERIC_READ, OPEN_EXISTING),
        INVALID_HANDLE_VALUE
    );
    assert!(crate::files::sandboxed());
    assert!(configure(b.package.clone(), b.output.clone()));
    assert!(!crate::files::sandboxed());
    let handle = open(&secret_file, GENERIC_READ, OPEN_EXISTING);
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    assert_eq!(read_all(handle), b"secret");
    close(handle);
    let wait_w: unsafe extern "win64" fn(*const u16, u32) -> i32 =
        find_shim(KERNEL, "WaitNamedPipeW");
    // SAFETY: a terminated name.
    assert_eq!(
        unsafe { wait_w(w("\\\\.\\pipe\\leonida-absent-pipe").as_ptr(), 1) },
        0
    );
    let open_key: unsafe extern "win64" fn(usize, *const u16, *mut usize) -> i32 =
        find_shim("advapi32.dll", "RegOpenKeyW");
    let mut key = 0usize;
    // SAFETY: a terminated name and a slot.
    let code = unsafe { open_key(0x8000_0002, w("SOFTWARE\\Microsoft").as_ptr(), &mut key) };
    assert_eq!(code, 0, "the real registry answers with the sandbox off");
    let close_key: unsafe extern "win64" fn(usize) -> i32 =
        find_shim("advapi32.dll", "RegCloseKey");
    // SAFETY: a key just opened.
    unsafe { close_key(key) };
}
