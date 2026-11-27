//! Bink 2 as the executable calls it: every movie plays to its end at once and shows black.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io::Write;

use shims::media::{MediaError, parse_header};

type Open = unsafe extern "win64" fn(*const u8, *const c_void, u32) -> usize;
type Handle = unsafe extern "win64" fn(usize) -> i32;
type Goto = unsafe extern "win64" fn(usize, u32, i32);
type KeyFrame = unsafe extern "win64" fn(usize, u32, i32) -> u32;

fn shim<T: Copy>(name: &str) -> T {
    let address = shims::builtin()
        .into_iter()
        .find(|s| s.name == name && s.dll.starts_with("Bink"))
        .unwrap_or_else(|| panic!("{name} registered"))
        .address;
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `T` is the function pointer type of the shim's shape.
    unsafe { std::mem::transmute_copy(&address) }
}

fn word(handle: usize, byte: usize) -> u32 {
    // SAFETY: a handle the shim made, read at a documented offset.
    unsafe { ((handle + byte) as *const u32).read() }
}

fn header(frames: u32, width: u32, height: u32, tracks: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x30];
    bytes[..4].copy_from_slice(b"KB2n");
    bytes[8..12].copy_from_slice(&frames.to_le_bytes());
    bytes[0x10..0x14].copy_from_slice(&frames.to_le_bytes());
    bytes[0x14..0x18].copy_from_slice(&width.to_le_bytes());
    bytes[0x18..0x1C].copy_from_slice(&height.to_le_bytes());
    bytes[0x1C..0x20].copy_from_slice(&30u32.to_le_bytes());
    bytes[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
    bytes[0x28..0x2C].copy_from_slice(&tracks.to_le_bytes());
    bytes
}

/// The loop a player runs: wait, decode, stop on the last frame, else advance.
fn play(handle: usize) -> u32 {
    let wait: Handle = shim("BinkWait");
    let do_frame: Handle = shim("BinkDoFrame");
    let next: unsafe extern "win64" fn(usize) = shim("BinkNextFrame");
    let mut shown = 0;
    loop {
        // SAFETY: a live handle.
        unsafe {
            assert_eq!(wait(handle), 0, "a frame is always due");
            assert_eq!(do_frame(handle), 0);
        }
        shown += 1;
        if word(handle, 0x0C) == word(handle, 0x08) {
            return shown;
        }
        assert!(shown < 100_000, "the movie never ends");
        // SAFETY: as above.
        unsafe { next(handle) };
    }
}

#[test]
fn headers_parse_and_nonsense_is_rejected() {
    let h = parse_header(&header(450, 1920, 1080, 2)).unwrap();
    assert_eq!(
        (h.frames, h.width, h.height, h.audio_tracks),
        (450, 1920, 1080, 2)
    );
    assert_eq!((h.rate, h.rate_divisor, h.revision), (30, 1, b'n'));
    assert_eq!(parse_header(b"BIKi"), Err(MediaError::NotBink));
    assert_eq!(parse_header(b"KB2n"), Err(MediaError::Short));
    assert_eq!(
        parse_header(&header(0, 1920, 1080, 0)),
        Err(MediaError::Implausible)
    );
    assert_eq!(
        parse_header(&header(10, 4, 1080, 0)),
        Err(MediaError::Implausible)
    );
    let mut mismatch = header(10, 640, 480, 0);
    mismatch[0x10] = 11;
    assert_eq!(parse_header(&mismatch), Err(MediaError::Implausible));
}

#[test]
fn a_stream_in_memory_plays_all_its_frames_then_ends() {
    let open: Open = shim("BinkOpenWithOptions");
    let close: unsafe extern "win64" fn(usize) = shim("BinkClose");
    let bytes = header(45, 1280, 720, 1);
    // SAFETY: a header-sized buffer, and a handle that is closed once.
    unsafe {
        let handle = open(bytes.as_ptr(), std::ptr::null(), 0);
        assert_ne!(handle, 0);
        assert_eq!(word(handle, 0x00), 1280);
        assert_eq!(word(handle, 0x04), 720);
        assert_eq!(word(handle, 0x08), 45);
        assert_eq!(word(handle, 0x0C), 1, "frames count from one");
        assert_eq!(word(handle, 0x14), 30);
        assert_eq!(word(handle, 0x18), 1);
        assert_eq!(word(handle, 0x38), 1);
        assert_eq!(play(handle), 45);
        assert_eq!(word(handle, 0x10), 45, "the last decoded frame");
        let next: unsafe extern "win64" fn(usize) = shim("BinkNextFrame");
        next(handle);
        assert_eq!(word(handle, 0x0C), 45, "the end holds");
        close(handle);
        close(handle);
    }
}

#[test]
fn a_named_file_is_read_for_its_header() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join("bink-header-probe.bk2");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&header(30, 512, 512, 0))
        .unwrap();
    let mut name = path.to_str().unwrap().as_bytes().to_vec();
    name.push(0);
    let open: Open = shim("BinkOpenWithOptions");
    // SAFETY: a NUL-terminated name.
    let handle = unsafe { open(name.as_ptr(), std::ptr::null(), 0) };
    assert_eq!(
        (word(handle, 0), word(handle, 4), word(handle, 8)),
        (512, 512, 30)
    );
    assert_eq!(play(handle), 30);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn an_unreadable_movie_is_one_frame_and_ends_at_once() {
    let open: Open = shim("BinkOpenWithOptions");
    let name = b"nowhere:/missing.bk2\0";
    // SAFETY: a NUL-terminated name.
    let handle = unsafe { open(name.as_ptr(), std::ptr::null(), 0) };
    assert_ne!(handle, 0);
    assert_eq!((word(handle, 8), word(handle, 0x0C)), (1, 1));
    assert_eq!(play(handle), 1);
}

#[test]
fn seeking_looping_and_the_small_calls() {
    let open: Open = shim("BinkOpenWithOptions");
    let goto: Goto = shim("BinkGoto");
    let looping: unsafe extern "win64" fn(usize, i32) = shim("BinkSetWillLoop");
    let pause: unsafe extern "win64" fn(usize, i32) -> i32 = shim("BinkPause");
    let skip: Handle = shim("BinkShouldSkip");
    let key: KeyFrame = shim("BinkGetKeyFrame");
    let next: unsafe extern "win64" fn(usize) = shim("BinkNextFrame");
    let bytes = header(20, 640, 360, 0);
    // SAFETY: valid header buffer; live handle.
    unsafe {
        let handle = open(bytes.as_ptr(), std::ptr::null(), 0);
        goto(handle, 15, 0);
        assert_eq!(word(handle, 0x0C), 15);
        goto(handle, 999, 0);
        assert_eq!(word(handle, 0x0C), 20, "seeks clamp to the last frame");
        goto(handle, 0, 0);
        assert_eq!(word(handle, 0x0C), 1);
        assert_eq!(skip(handle), 0);
        assert_eq!(key(handle, 7, 0), 7);
        assert_eq!(key(handle, 70, 0), 20);
        assert_eq!(pause(handle, 1), 1);
        assert_eq!(pause(handle, 0), 0);
        goto(handle, 20, 0);
        looping(handle, 1);
        next(handle);
        assert_eq!(word(handle, 0x0C), 1, "a looping movie wraps");
    }
}

#[test]
fn the_threading_and_setup_calls_succeed() {
    for name in [
        "BinkSetSoundSystem",
        "BinkSetMemory",
        "BinkSetOSFileCallbacks",
        "BinkStartAsyncThread",
        "BinkRequestStopAsyncThreadsMulti",
        "BinkWaitStopAsyncThreadsMulti",
        "BinkDoFrameAsyncMulti",
        "BinkDoFrameAsyncWait",
        "BinkSetVolume",
    ] {
        let call: unsafe extern "win64" fn(usize, usize, usize) -> i32 = shim(name);
        // SAFETY: the calls ignore pointers they are given, and are given null.
        assert_eq!(unsafe { call(0, 0, 0) }, 1, "{name}");
    }
    let free: unsafe extern "win64" fn() = shim("BinkFreeGlobals");
    // SAFETY: no arguments.
    unsafe { free() };
}

#[test]
fn every_imported_function_is_provided() {
    let names: Vec<_> = shims::builtin()
        .into_iter()
        .filter(|s| s.dll.starts_with("Bink"))
        .map(|s| s.name)
        .collect();
    assert_eq!(names.len(), 23);
}
