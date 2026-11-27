//! Video: Bink2Scarlett.dll, and the Media Foundation calls the game makes.
//!
//! Bink 2 is proprietary and is not decoded. The 23 functions the executable imports are answered so that
//! every movie behaves as one whose frames are all due at once and show black: `BinkWait` never makes the
//! caller wait, `BinkDoFrame` draws nothing, and `BinkNextFrame` walks the frame counter to the last frame, which
//! is how a play loop ends. A title that skips movies is never stalled, and one that waits for the last frame
//! sees it at once.
//!
//! What the docs measured of the handle (`movies.md`, "ABI over the archive") is mirrored in the 32-bit words
//! a caller may read straight from it: width at +0x00, height at +0x04, frame count at +0x08, the 1-based
//! current frame at +0x0c, the last decoded frame at +0x10, the frame rate numerator and denominator at
//! +0x14 and +0x18, and the audio track count at +0x38. Frame counts and sizes come from the stream's own
//! header when the name is a readable file or the bytes of a stream in memory; otherwise the movie is one
//! 1920x1080 frame, which also ends at once.
//!
//! Unmeasured, so answered with a trace note and no write: `BinkGetFrameBuffersInfo` (the layout of the
//! frame buffer description), `BinkRegisterFrameBuffers`, the options structure `BinkOpenWithOptions` takes,
//! and the callback shapes `BinkSetOSFileCallbacks` and `BinkSetMemory` take. Media Foundation (`MFPlat.DLL`,
//! `MFReadWrite.dll`) is Windows' own and is bound to the host.

mod decoder;
mod header;
pub mod layout;
pub mod player;
pub mod sound;
pub mod source;
#[doc(hidden)]
pub mod testing;
pub mod worker;

pub use decoder::{
    AudioTrackInfo, DecodeError, DecodedFrame, DecoderFactory, MovieDecoder, Plane, StreamInfo,
    decoder_factory, set_decoder_factory,
};
pub use header::{
    HEADER_BYTES, MediaError, MovieHeader, PREFIX_BYTES, parse_audio_tracks, parse_header,
};

use std::sync::atomic::{AtomicBool, Ordering};

use crate::Shim;

const WORDS: usize = 64;
const WIDTH: usize = 0;
const HEIGHT: usize = 1;
const FRAMES: usize = 2;
const FRAME_NUM: usize = 3;
const LAST_FRAME_NUM: usize = 4;
const RATE: usize = 5;
const RATE_DIV: usize = 6;
const AUDIO_TRACKS: usize = 0x38 / 4;

/// `BinkOpen`'s flag for a stream inside a larger file (`movies.md`).
pub const FLAG_FILE_OFFSET: u32 = 0x20;

#[cfg(windows)]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::io::Read;
    use std::sync::Mutex;

    use super::*;
    use crate::trace::note;

    struct Movie {
        will_loop: bool,
        paused: bool,
    }

    static MOVIES: Mutex<BTreeMap<usize, Movie>> = Mutex::new(BTreeMap::new());

    static NOTED: [AtomicBool; 6] = [const { AtomicBool::new(false) }; 6];

    fn note_once(slot: usize, text: &str) {
        if !NOTED[slot].swap(true, Ordering::Relaxed) {
            note(text.to_string());
        }
    }

    fn word(handle: usize, index: usize) -> u32 {
        // SAFETY: only handles this module made are looked up, and each is a live image of `WORDS` words.
        unsafe { (handle as *const u32).add(index).read_volatile() }
    }

    fn set_word(handle: usize, index: usize, value: u32) {
        // SAFETY: as in `word`.
        unsafe { (handle as *mut u32).add(index).write_volatile(value) }
    }

    /// Reads the name or the stream bytes the caller handed to `BinkOpenWithOptions`.
    fn read_header(source: *const u8) -> Option<MovieHeader> {
        if source.is_null() {
            return None;
        }
        let mut stream = true;
        for at in 0..4 {
            // SAFETY: a name is NUL-terminated, so a byte is read only after every byte before it matched
            // `KB2` and a revision letter, which no name starts with.
            let byte = unsafe { source.add(at).read() };
            let wanted = b"KB2"[..]
                .get(at)
                .map_or(byte.is_ascii_lowercase(), |&w| byte == w);
            if !wanted {
                stream = false;
                break;
            }
        }
        if stream {
            let mut head = [0u8; HEADER_BYTES];
            // SAFETY: a stream in memory holds at least its fixed header.
            unsafe { std::ptr::copy_nonoverlapping(source, head.as_mut_ptr(), HEADER_BYTES) };
            return parse_header(&head).ok();
        }
        let mut name = Vec::new();
        for at in 0..1024 {
            // SAFETY: bounded read of a NUL-terminated string.
            let byte = unsafe { source.add(at).read() };
            if byte == 0 {
                break;
            }
            name.push(byte);
        }
        let path = String::from_utf8(name).ok()?;
        let mut head = [0u8; HEADER_BYTES];
        std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
        parse_header(&head).ok()
    }

    /// `HBINK BinkOpenWithOptions(name_or_stream, options, flags)`
    pub extern "win64" fn open_with_options(
        source: *const u8,
        _options: *const c_void,
        _flags: u32,
    ) -> usize {
        let header = read_header(source).unwrap_or_else(|| {
            note_once(
                0,
                "Bink: a movie's header could not be read (a name the host cannot open, or a stream behind \
                 the title's own file callbacks); it is treated as one 1920x1080 frame",
            );
            MovieHeader::FALLBACK
        });
        let image = Box::into_raw(Box::new([0u32; WORDS]));
        let handle = image as usize;
        set_word(handle, WIDTH, header.width);
        set_word(handle, HEIGHT, header.height);
        set_word(handle, FRAMES, header.frames);
        set_word(handle, FRAME_NUM, 1);
        set_word(handle, RATE, header.rate);
        set_word(handle, RATE_DIV, header.rate_divisor);
        set_word(handle, AUDIO_TRACKS, header.audio_tracks);
        if let Ok(mut movies) = MOVIES.lock() {
            movies.insert(
                handle,
                Movie {
                    will_loop: false,
                    paused: false,
                },
            );
        }
        handle
    }

    fn live(handle: usize) -> bool {
        MOVIES.lock().is_ok_and(|m| m.contains_key(&handle))
    }

    pub extern "win64" fn close(handle: usize) {
        let known = MOVIES.lock().ok().and_then(|mut m| m.remove(&handle));
        if known.is_some() {
            // SAFETY: the image `open_with_options` boxed, removed from the table so it is freed once.
            drop(unsafe { Box::from_raw(handle as *mut [u32; WORDS]) });
        }
    }

    pub extern "win64" fn do_frame(handle: usize) -> i32 {
        if live(handle) {
            set_word(handle, LAST_FRAME_NUM, word(handle, FRAME_NUM));
        }
        0
    }

    pub extern "win64" fn do_frame_async_multi(
        _binks: *const usize,
        _threads: *const u32,
        _count: u32,
    ) -> i32 {
        1
    }

    pub extern "win64" fn do_frame_async_wait(_handle: usize, _microseconds: i32) -> i32 {
        1
    }

    pub extern "win64" fn free_globals() {}

    pub extern "win64" fn get_frame_buffers_info(_handle: usize, _info: *mut c_void) {
        note_once(
            1,
            "Bink: BinkGetFrameBuffersInfo left the frame buffer description alone (its layout is unmeasured)",
        );
    }

    pub extern "win64" fn get_key_frame(handle: usize, frame: u32, _flags: i32) -> u32 {
        if !live(handle) {
            return 0;
        }
        let frames = word(handle, FRAMES);
        match frame {
            0 => word(handle, FRAME_NUM),
            f => f.min(frames),
        }
    }

    pub extern "win64" fn get_platform_info(_which: i32, _out: *mut c_void) -> i32 {
        0
    }

    pub extern "win64" fn goto(handle: usize, frame: u32, _flags: i32) {
        if live(handle) {
            set_word(
                handle,
                FRAME_NUM,
                frame.clamp(1, word(handle, FRAMES).max(1)),
            );
        }
    }

    pub extern "win64" fn next_frame(handle: usize) {
        let Some(will_loop) = MOVIES
            .lock()
            .ok()
            .and_then(|m| m.get(&handle).map(|m| m.will_loop))
        else {
            return;
        };
        let (at, frames) = (word(handle, FRAME_NUM), word(handle, FRAMES));
        if at < frames {
            set_word(handle, FRAME_NUM, at + 1);
        } else if will_loop {
            set_word(handle, FRAME_NUM, 1);
        }
    }

    pub extern "win64" fn pause(handle: usize, pause: i32) -> i32 {
        if let Ok(mut movies) = MOVIES.lock()
            && let Some(movie) = movies.get_mut(&handle)
        {
            movie.paused = pause != 0;
            return i32::from(movie.paused);
        }
        0
    }

    pub extern "win64" fn register_frame_buffers(_handle: usize, _buffers: *const c_void) {
        note_once(
            2,
            "Bink: BinkRegisterFrameBuffers took no buffers (nothing is decoded into them)",
        );
    }

    pub extern "win64" fn request_stop_async_threads_multi(_count: u32) -> i32 {
        1
    }

    pub extern "win64" fn set_memory(_alloc: *const c_void, _free: *const c_void) -> i32 {
        note_once(
            3,
            "Bink: BinkSetMemory's allocators are not used (the shim allocates its own handles)",
        );
        1
    }

    pub extern "win64" fn set_os_file_callbacks(
        _callbacks: *const c_void,
        _more: *const c_void,
    ) -> i32 {
        note_once(
            4,
            "Bink: BinkSetOSFileCallbacks' callbacks are not called (their shape is unmeasured)",
        );
        1
    }

    pub extern "win64" fn set_sound_system(_open: *const c_void, _parameter: usize) -> i32 {
        1
    }

    pub extern "win64" fn set_volume(_handle: usize, _track: u32, _volume: i32) -> i32 {
        1
    }

    pub extern "win64" fn set_will_loop(handle: usize, on: i32) {
        if let Ok(mut movies) = MOVIES.lock()
            && let Some(movie) = movies.get_mut(&handle)
        {
            movie.will_loop = on != 0;
        }
    }

    pub extern "win64" fn should_skip(_handle: usize) -> i32 {
        0
    }

    pub extern "win64" fn start_async_thread(_thread: i32, _parameter: *const c_void) -> i32 {
        1
    }

    pub extern "win64" fn wait(_handle: usize) -> i32 {
        0
    }

    pub extern "win64" fn wait_stop_async_threads_multi(_count: u32) -> i32 {
        1
    }
}

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let list: [(&str, usize); 23] = [
            ("BinkClose", imp::close as *const () as usize),
            ("BinkDoFrame", imp::do_frame as *const () as usize),
            (
                "BinkDoFrameAsyncMulti",
                imp::do_frame_async_multi as *const () as usize,
            ),
            (
                "BinkDoFrameAsyncWait",
                imp::do_frame_async_wait as *const () as usize,
            ),
            ("BinkFreeGlobals", imp::free_globals as *const () as usize),
            (
                "BinkGetFrameBuffersInfo",
                imp::get_frame_buffers_info as *const () as usize,
            ),
            ("BinkGetKeyFrame", imp::get_key_frame as *const () as usize),
            (
                "BinkGetPlatformInfo",
                imp::get_platform_info as *const () as usize,
            ),
            ("BinkGoto", imp::goto as *const () as usize),
            ("BinkNextFrame", imp::next_frame as *const () as usize),
            (
                "BinkOpenWithOptions",
                imp::open_with_options as *const () as usize,
            ),
            ("BinkPause", imp::pause as *const () as usize),
            (
                "BinkRegisterFrameBuffers",
                imp::register_frame_buffers as *const () as usize,
            ),
            (
                "BinkRequestStopAsyncThreadsMulti",
                imp::request_stop_async_threads_multi as *const () as usize,
            ),
            ("BinkSetMemory", imp::set_memory as *const () as usize),
            (
                "BinkSetOSFileCallbacks",
                imp::set_os_file_callbacks as *const () as usize,
            ),
            (
                "BinkSetSoundSystem",
                imp::set_sound_system as *const () as usize,
            ),
            ("BinkSetVolume", imp::set_volume as *const () as usize),
            ("BinkSetWillLoop", imp::set_will_loop as *const () as usize),
            ("BinkShouldSkip", imp::should_skip as *const () as usize),
            (
                "BinkStartAsyncThread",
                imp::start_async_thread as *const () as usize,
            ),
            ("BinkWait", imp::wait as *const () as usize),
            (
                "BinkWaitStopAsyncThreadsMulti",
                imp::wait_stop_async_threads_multi as *const () as usize,
            ),
        ];
        list.into_iter()
            .map(|(name, address)| Shim {
                dll: "Bink2Scarlett.dll",
                name,
                address,
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
