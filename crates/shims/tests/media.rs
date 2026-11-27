//! Bink 2 as the executable calls it, with a fake decoder behind the shims: the open, frame buffer, decode,
//! pacing, seek and close calls in the order the game makes them.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use shims::media::testing::{FakeDecoder, FakeGame, stream_header};
use shims::media::{DecodeError, MovieDecoder, parse_header, set_decoder_factory};

type Open = unsafe extern "win64" fn(*const u8, *const c_void, u32) -> usize;
type Handle = unsafe extern "win64" fn(usize) -> i32;
type Goto = unsafe extern "win64" fn(usize, u32, i32);

/// A stream whose width is 777 makes its decoder fail at frame 3.
fn factory(bytes: std::sync::Arc<[u8]>) -> Result<Box<dyn MovieDecoder>, DecodeError> {
    let h = parse_header(&bytes).map_err(|e| DecodeError(e.to_string()))?;
    let mut d = FakeDecoder::new(h.frames, h.width, h.height, h.audio_tracks, false);
    if h.width == 777 {
        d.fail_at = Some(3);
    }
    Ok(Box::new(d))
}

fn install() {
    set_decoder_factory(Some(factory));
}

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

fn open_memory(bytes: &[u8]) -> usize {
    install();
    let open: Open = shim("BinkOpenWithOptions");
    // SAFETY: a complete stream header, which is the whole stream.
    unsafe { open(bytes.as_ptr(), std::ptr::null(), 0x0400_0000) }
}

/// What the game does: wait until due, step, decode; ends on the last frame. Returns the frames shown.
fn play(handle: usize, mut each: impl FnMut(u32)) -> u32 {
    let wait: Handle = shim("BinkWait");
    let next: unsafe extern "win64" fn(usize) = shim("BinkNextFrame");
    let start: unsafe extern "win64" fn(usize, *const u32, u32) -> i32 =
        shim("BinkDoFrameAsyncMulti");
    let finish: unsafe extern "win64" fn(usize, i32) -> i32 = shim("BinkDoFrameAsyncWait");
    let started = Instant::now();
    let mut shown = 0;
    // SAFETY: a live handle.
    unsafe {
        start(handle, std::ptr::null(), 0);
        finish(handle, -1);
        shown += 1;
        each(word(handle, 0x10));
        while word(handle, 0x10) != word(handle, 0x08) {
            while wait(handle) != 0 {
                assert!(started.elapsed() < Duration::from_secs(20), "stuck waiting");
                std::thread::sleep(Duration::from_millis(1));
            }
            next(handle);
            start(handle, std::ptr::null(), 0);
            finish(handle, -1);
            shown += 1;
            each(word(handle, 0x10));
            assert!(shown < 10_000, "the movie never ends");
        }
    }
    shown
}

#[test]
fn a_movie_plays_into_the_games_buffers_at_its_frame_rate() {
    let bytes = stream_header(9, 48, 32, 1);
    let handle = open_memory(&bytes);
    let info = FakeDecoder::new(9, 48, 32, 1, false).info;
    let mut game = FakeGame::new(&info);
    let get: unsafe extern "win64" fn(usize, *mut c_void) = shim("BinkGetFrameBuffersInfo");
    let register: unsafe extern "win64" fn(usize, *const c_void) = shim("BinkRegisterFrameBuffers");
    let pause: unsafe extern "win64" fn(usize, i32) -> i32 = shim("BinkPause");
    let mut described = vec![0u64; 0x108 / 8];
    // SAFETY: the description is 0x108 bytes; the handle is live.
    unsafe {
        get(handle, described.as_mut_ptr() as *mut c_void);
        let u32s = std::slice::from_raw_parts(described.as_ptr() as *const u32, 0x108 / 4);
        assert_eq!(&u32s[..5], &[3, 48, 32, 24, 16]);
        assert_eq!(u32s[(0x18 + 8) / 4], 1, "Y exists");
        assert_eq!(u32s[(0x18 + 0x30 + 8) / 4], 0, "no alpha");
        assert_eq!(word(handle, 0x10), u32::MAX, "nothing decoded yet");
        assert_eq!(word(handle, 0x38), 1);
        register(handle, game.ptr() as *const c_void);
        pause(handle, 1);
        assert_eq!(word(handle, 0x11C), 1, "the paused word");
        pause(handle, 0);
        assert_eq!(word(handle, 0x11C), 0);
    }
    let started = Instant::now();
    let mut sets = Vec::new();
    let shown = play(handle, |last| {
        let set = game.frame_num() as usize;
        sets.push(set);
        let memory = game.plane(set, 0, 3);
        let frame = last - 1;
        assert_eq!(
            memory[5 * 64 + 7],
            FakeDecoder::pixel(0, frame, 7, 5),
            "frame {last}"
        );
    });
    assert_eq!(shown, 9);
    assert_eq!(sets, [0, 1, 2, 0, 1, 2, 0, 1, 2]);
    assert!(
        started.elapsed() >= Duration::from_millis(8 * 33 - 20),
        "nine frames at 30 fps take about 266 ms, not {:?}",
        started.elapsed()
    );
    let close: unsafe extern "win64" fn(usize) = shim("BinkClose");
    // SAFETY: closed once; a second close is ignored.
    unsafe {
        close(handle);
        close(handle);
    }
}

#[test]
fn going_to_a_frame_and_looping_decode_the_right_pictures() {
    let bytes = stream_header(30, 32, 32, 0);
    let handle = open_memory(&bytes);
    let info = FakeDecoder::new(30, 32, 32, 0, false).info;
    let mut game = FakeGame::new(&info);
    let register: unsafe extern "win64" fn(usize, *const c_void) = shim("BinkRegisterFrameBuffers");
    let goto: Goto = shim("BinkGoto");
    let do_frame: Handle = shim("BinkDoFrame");
    let next: unsafe extern "win64" fn(usize) = shim("BinkNextFrame");
    let looping: unsafe extern "win64" fn(usize, i32) = shim("BinkSetWillLoop");
    // SAFETY: a live handle and the game's buffers.
    unsafe {
        register(handle, game.ptr() as *const c_void);
        goto(handle, 20, 0);
        assert_eq!(word(handle, 0x0C), 20);
        do_frame(handle);
        assert_eq!(word(handle, 0x10), 20);
        let set = game.frame_num() as usize;
        assert_eq!(
            game.plane(set, 0, 3)[3 * 64 + 3],
            FakeDecoder::pixel(0, 19, 3, 3)
        );
        goto(handle, 999, 0);
        assert_eq!(word(handle, 0x0C), 30);
        looping(handle, 1);
        next(handle);
        assert_eq!(word(handle, 0x0C), 1, "a looping movie wraps");
        do_frame(handle);
        let set = game.frame_num() as usize;
        assert_eq!(game.plane(set, 0, 3)[0], FakeDecoder::pixel(0, 0, 0, 0));
    }
}

#[test]
fn a_decoder_error_ends_the_movie_for_the_play_loop() {
    let bytes = stream_header(12, 777, 32, 0);
    let handle = open_memory(&bytes);
    assert_eq!(
        play(handle, |_| {}),
        4,
        "three frames, then the fourth ends it"
    );
    assert_eq!(word(handle, 0x0C), 12);
    assert!(
        shims::trace::notes()
            .iter()
            .any(|n| n.contains("decoder failed"))
    );
}

#[test]
fn an_unreadable_movie_is_one_frame_and_ends_at_once() {
    let _names = NAMES.lock().unwrap_or_else(|p| p.into_inner());
    install();
    let open: Open = shim("BinkOpenWithOptions");
    let name = b"nowhere:/missing.bk2\0";
    // SAFETY: a NUL-terminated name.
    let handle = unsafe { open(name.as_ptr(), std::ptr::null(), 0) };
    assert_ne!(handle, 0);
    assert_eq!((word(handle, 8), word(handle, 0x0C)), (1, 1));
    assert_eq!(play(handle, |_| {}), 1);
}

#[test]
fn small_calls_on_a_dead_handle_do_nothing() {
    for name in [
        "BinkWait",
        "BinkShouldSkip",
        "BinkClose",
        "BinkDoFrame",
        "BinkNextFrame",
    ] {
        let call: unsafe extern "win64" fn(usize) -> i32 = shim(name);
        // SAFETY: the handle is unknown to the shim, which ignores it.
        unsafe { call(0x1234) };
    }
    let pause: unsafe extern "win64" fn(usize, i32) -> i32 = shim("BinkPause");
    // SAFETY: as above.
    assert_eq!(unsafe { pause(0x1234, 1) }, 0);
}

/// Tests that open by name take this, because the file callbacks are process-wide.
static NAMES: Mutex<()> = Mutex::new(());
static OPENED: AtomicUsize = AtomicUsize::new(0);
static CLOSED: AtomicUsize = AtomicUsize::new(0);
static SERVED: Mutex<(Vec<u8>, usize)> = Mutex::new((Vec::new(), 0));

unsafe extern "win64" fn open_callback(file: *mut u64, name: *const c_void, _flags: u32) -> i32 {
    // The name block's first word is the path, like the game's.
    // SAFETY: the test passes a block whose first field is a pointer to a NUL-terminated name.
    let path = unsafe { std::ffi::CStr::from_ptr(*(name as *const *const std::ffi::c_char)) };
    if path.to_bytes() != b"movies:/served" {
        return 0;
    }
    OPENED.fetch_add(1, Ordering::SeqCst);
    SERVED.lock().unwrap().1 = 0;
    // SAFETY: the shim hands over a block at least one word long.
    unsafe { *file = 0xF11E };
    1
}

unsafe extern "win64" fn read_callback(file: *mut u64, dest: *mut u8, size: u64) -> u64 {
    // SAFETY: as above.
    assert_eq!(unsafe { *file }, 0xF11E, "the game's own object comes back");
    let mut served = SERVED.lock().unwrap();
    let n = (size as usize).min(served.0.len() - served.1).min(1000);
    // SAFETY: `dest` holds `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(served.0.as_ptr().add(served.1), dest, n) };
    served.1 += n;
    n as u64
}

unsafe extern "win64" fn seek_callback(_file: *mut u64, _position: u64) {}

unsafe extern "win64" fn close_callback(_file: *mut u64) {
    CLOSED.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn the_games_file_callbacks_serve_the_stream() {
    let _names = NAMES.lock().unwrap_or_else(|p| p.into_inner());
    install();
    let mut bytes = stream_header(5, 32, 32, 1);
    let size = bytes.len() as u32 + 3000 - 8;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    bytes.resize(bytes.len() + 3000, 7);
    SERVED.lock().unwrap().0 = bytes;
    let set: unsafe extern "win64" fn(usize, usize, usize, usize) -> i32 =
        shim("BinkSetOSFileCallbacks");
    let open: Open = shim("BinkOpenWithOptions");
    let close: unsafe extern "win64" fn(usize) = shim("BinkClose");
    let name = b"movies:/served\0";
    let block: [*const u8; 4] = [
        name.as_ptr(),
        std::ptr::null(),
        std::ptr::null(),
        std::ptr::null(),
    ];
    // SAFETY: the callbacks have the shapes the shim calls; the name block outlives the open.
    let handle = unsafe {
        set(
            open_callback as *const () as usize,
            read_callback as *const () as usize,
            seek_callback as *const () as usize,
            close_callback as *const () as usize,
        );
        open(block.as_ptr() as *const u8, std::ptr::null(), 0x4000)
    };
    assert_eq!(OPENED.load(Ordering::SeqCst), 1);
    assert_eq!(
        (word(handle, 0), word(handle, 4), word(handle, 8)),
        (32, 32, 5)
    );
    assert_eq!(
        word(handle, 0x38),
        1,
        "the first track plays without an option list"
    );
    assert_eq!(
        CLOSED.load(Ordering::SeqCst),
        0,
        "the file stays open until it is read"
    );
    assert_eq!(play(handle, |_| {}), 5);
    // SAFETY: a live handle.
    unsafe { close(handle) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while CLOSED.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(CLOSED.load(Ordering::SeqCst), 1);
    let unreadable = b"movies:/other\0";
    let block: [*const u8; 4] = [
        unreadable.as_ptr(),
        std::ptr::null(),
        std::ptr::null(),
        std::ptr::null(),
    ];
    // SAFETY: as above.
    let handle = unsafe { open(block.as_ptr() as *const u8, std::ptr::null(), 0x4000) };
    assert_eq!(
        word(handle, 8),
        1,
        "a name the game cannot open is one frame"
    );
    // SAFETY: unregisters the callbacks again.
    unsafe { set(0, 0, 0, 0) };
}

#[test]
fn the_threading_and_setup_calls_succeed() {
    for name in [
        "BinkSetSoundSystem",
        "BinkSetMemory",
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
