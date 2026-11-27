use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    ReadFile, WIN32_FIND_DATAW, WriteFile,
};

use super::*;
use crate::files::configure;

static LOCK: Mutex<()> = Mutex::new(());

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const CREATE_ALWAYS: u32 = 2;
const CREATE_NEW: u32 = 1;
const SHARE: u32 = 7;

struct Setup {
    _guard: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    package: PathBuf,
    output: PathBuf,
}

fn setup() -> Setup {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
    std::fs::create_dir_all(&scratch).expect("scratch");
    let dir = tempfile::Builder::new()
        .prefix("files-")
        .tempdir_in(&scratch)
        .expect("temp dir");
    let root = dir.path().canonicalize().expect("canonical");
    let package = root.join("package");
    let output = root.join("Output");
    std::fs::create_dir_all(package.join("data")).expect("package");
    std::fs::create_dir_all(&output).expect("output");
    assert!(configure(package.clone(), output.clone()));
    Setup {
        _guard: guard,
        _dir: dir,
        package,
        output,
    }
}

fn w(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn open(name: &str, access: u32, disposition: u32) -> HANDLE {
    let name = w(name);
    // SAFETY: a terminated name; the shim is the one under test.
    unsafe {
        create_file_w(
            name.as_ptr(),
            access,
            SHARE,
            std::ptr::null(),
            disposition,
            0,
            std::ptr::null_mut(),
        )
    }
}

fn read_all(handle: HANDLE) -> Vec<u8> {
    let mut buffer = vec![0u8; 4096];
    let mut got = 0;
    // SAFETY: a host handle and a buffer of the stated size.
    let ok = unsafe {
        ReadFile(
            handle,
            buffer.as_mut_ptr().cast(),
            4096,
            &mut got,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(ok, 0);
    buffer.truncate(got as usize);
    buffer
}

fn write_all(handle: HANDLE, bytes: &[u8]) {
    let mut done = 0;
    // SAFETY: a host handle and the bytes' own length.
    let ok = unsafe {
        WriteFile(
            handle,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            &mut done,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(ok, 0);
}

fn close(handle: HANDLE) {
    // SAFETY: a handle this test opened.
    unsafe { CloseHandle(handle) };
}

fn last_error() -> u32 {
    // SAFETY: reads the thread's last error.
    unsafe { GetLastError() }
}

fn list(pattern: &str) -> Vec<(String, u32)> {
    let wide = w(pattern);
    // SAFETY: an all-zero find record is valid.
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let data_ptr = (&mut data as *mut WIN32_FIND_DATAW).cast();
    // SAFETY: a terminated pattern and a record to fill.
    let handle = unsafe { find_first_file_w(wide.as_ptr(), data_ptr) };
    if handle == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut found = Vec::new();
    loop {
        let end = data.cFileName.iter().position(|c| *c == 0).expect("terminated");
        let name = String::from_utf16_lossy(&data.cFileName[..end]);
        if name != "." && name != ".." {
            found.push((name, data.nFileSizeLow));
        }
        // SAFETY: the listing handle from above.
        if unsafe { find_next_file_w(handle, data_ptr) } == 0 {
            assert_eq!(last_error(), 18);
            break;
        }
    }
    // SAFETY: the listing handle from above.
    unsafe { find_close(handle) };
    found.sort();
    found
}

#[test]
fn reads_package_files_through_console_paths() {
    let s = setup();
    std::fs::write(s.package.join("data").join("a.txt"), b"package bytes").expect("write");
    for name in ["GAME:\\data\\a.txt", "g:/data/a.txt", "data\\a.txt", "\\data\\A.TXT"] {
        let handle = open(name, GENERIC_READ, OPEN_EXISTING);
        assert_ne!(handle, INVALID_HANDLE_VALUE, "{name}");
        assert_eq!(read_all(handle), b"package bytes");
        close(handle);
    }
    let missing = open("GAME:\\data\\none.txt", GENERIC_READ, OPEN_EXISTING);
    assert_eq!(missing, INVALID_HANDLE_VALUE);
    assert_eq!(last_error(), 2);
    let attributes = w("GAME:\\data\\a.txt");
    // SAFETY: a terminated name.
    assert_ne!(unsafe { get_file_attributes_w(attributes.as_ptr()) }, u32::MAX);
}

#[test]
fn a_write_open_of_a_package_file_goes_to_the_overlay() {
    let s = setup();
    let original = s.package.join("data").join("a.txt");
    std::fs::write(&original, b"0123456789").expect("write");
    let handle = open("GAME:\\data\\a.txt", GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING);
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    write_all(handle, b"AB");
    close(handle);
    assert_eq!(std::fs::read(&original).expect("read"), b"0123456789");
    let copy = s.output.join("overlay").join("data").join("a.txt");
    assert_eq!(std::fs::read(copy).expect("overlay"), b"AB23456789");
    let handle = open("GAME:\\data\\a.txt", GENERIC_READ, OPEN_EXISTING);
    assert_eq!(read_all(handle), b"AB23456789");
    close(handle);
}

#[test]
fn new_files_land_in_output_and_need_a_visible_folder() {
    let s = setup();
    let handle = open("GAME:\\data\\new.bin", GENERIC_WRITE, CREATE_NEW);
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    write_all(handle, b"fresh");
    close(handle);
    assert!(!s.package.join("data").join("new.bin").exists());
    assert_eq!(
        std::fs::read(s.output.join("overlay").join("data").join("new.bin")).expect("read"),
        b"fresh"
    );
    let again = open("GAME:\\data\\new.bin", GENERIC_WRITE, CREATE_NEW);
    assert_eq!(again, INVALID_HANDLE_VALUE);
    assert_eq!(last_error(), 80);
    let orphan = open("GAME:\\nowhere\\x.bin", GENERIC_WRITE, CREATE_ALWAYS);
    assert_eq!(orphan, INVALID_HANDLE_VALUE);
    assert_eq!(last_error(), 3);
    let scratch = open("T:\\cache\\deep\\x.tmp", GENERIC_WRITE, CREATE_ALWAYS);
    assert_ne!(scratch, INVALID_HANDLE_VALUE);
    close(scratch);
    assert!(s.output.join("temp").join("cache").join("deep").join("x.tmp").is_file());
}

#[test]
fn listings_merge_the_overlay_and_deletes_hide_package_files() {
    let s = setup();
    let data = s.package.join("data");
    std::fs::write(data.join("a.bin"), b"1").expect("a");
    std::fs::write(data.join("b.bin"), b"22").expect("b");
    let overlay = s.output.join("overlay").join("data");
    std::fs::create_dir_all(&overlay).expect("overlay");
    std::fs::write(overlay.join("b.bin"), b"333").expect("b2");
    std::fs::write(overlay.join("c.bin"), b"4444").expect("c");
    assert_eq!(
        list("GAME:\\data\\*.bin"),
        vec![("a.bin".into(), 1), ("b.bin".into(), 3), ("c.bin".into(), 4)]
    );
    let a = w("GAME:\\data\\a.bin");
    // SAFETY: a terminated name.
    assert_eq!(unsafe { delete_file_w(a.as_ptr()) }, 1);
    assert!(data.join("a.bin").exists());
    assert_eq!(list("GAME:\\data\\*.bin").len(), 2);
    let gone = open("GAME:\\data\\a.bin", GENERIC_READ, OPEN_EXISTING);
    assert_eq!(gone, INVALID_HANDLE_VALUE);
    assert_eq!(last_error(), 2);
    let b = w("GAME:\\data\\b.bin");
    // SAFETY: a terminated name.
    assert_eq!(unsafe { delete_file_w(b.as_ptr()) }, 1);
    assert!(!overlay.join("b.bin").exists());
    assert!(list("GAME:\\data\\*.bin").iter().all(|(n, _)| n == "c.bin"));
    assert!(list("GAME:\\missing\\*").is_empty());
    assert_eq!(last_error(), 3);
}

#[test]
fn directories_are_made_in_the_overlay() {
    let s = setup();
    let existing = w("GAME:\\data");
    // SAFETY: terminated names.
    unsafe {
        assert_eq!(create_directory_w(existing.as_ptr(), std::ptr::null()), 0);
        assert_eq!(last_error(), 183);
        let fresh = w("GAME:\\data\\sub");
        assert_eq!(create_directory_w(fresh.as_ptr(), std::ptr::null()), 1);
        assert!(s.output.join("overlay").join("data").join("sub").is_dir());
        assert!(!s.package.join("data").join("sub").exists());
        assert_eq!(remove_directory_w(fresh.as_ptr()), 1);
        assert_eq!(remove_directory_w(existing.as_ptr()), 0);
        assert_eq!(last_error(), 5);
    }
}

#[test]
fn attributes_and_moves_leave_the_package_alone() {
    let s = setup();
    let original = s.package.join("data").join("a.txt");
    std::fs::write(&original, b"x").expect("write");
    let name = w("GAME:\\data\\a.txt");
    // SAFETY: terminated names.
    unsafe {
        assert_eq!(set_file_attributes_w(name.as_ptr(), 0x2), 1);
        assert_ne!(get_file_attributes_w(name.as_ptr()) & 0x2, 0);
        let to = w("GAME:\\data\\b.txt");
        assert_eq!(move_file_ex_w(name.as_ptr(), to.as_ptr(), 0), 1);
    }
    assert!(original.exists());
    assert!(s.output.join("overlay").join("data").join("b.txt").is_file());
    assert_eq!(list("GAME:\\data\\*.txt").len(), 1);
}

#[test]
fn network_paths_are_refused_and_names_are_traced() {
    let _s = setup();
    for name in ["\\\\server\\share\\x.bin", "tcpip:1.2.3.4/x", "https://example.com/x"] {
        assert_eq!(open(name, GENERIC_READ, OPEN_EXISTING), INVALID_HANDLE_VALUE);
        assert_eq!(last_error(), 53, "{name}");
    }
    let escape = w("GAME:\\..\\outside.bin");
    // SAFETY: a terminated name.
    assert_eq!(unsafe { get_file_attributes_w(escape.as_ptr()) }, u32::MAX);
    assert_eq!(last_error(), 123);
    let seen = crate::trace::files();
    assert!(seen.iter().any(|f| f == "tcpip:1.2.3.4/x"));
}
