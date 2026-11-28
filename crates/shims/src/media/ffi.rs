//! The 23 `Bink2Scarlett.dll` imports, as the executable calls them.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::decoder::{StreamInfo, decoder_factory};
use super::header::{HEADER_BYTES, MovieHeader, PREFIX_BYTES, parse_audio_tracks, parse_header};
use super::player::{Player, SystemClock, WAIT_CAP};
use super::sound::EngineSink;
use super::source::{ByteSource, FileSource, MemorySource, loader, read_fully};
use crate::trace::note;

const WORDS: usize = 0x1C0;
const WIDTH: usize = 0;
const HEIGHT: usize = 1;
const FRAMES: usize = 0x08 / 4;
const FRAME_NUM: usize = 0x0C / 4;
const LAST_FRAME_NUM: usize = 0x10 / 4;
const RATE: usize = 0x14 / 4;
const RATE_DIV: usize = 0x18 / 4;
const AUDIO_TRACKS: usize = 0x38 / 4;
const PAUSED: usize = 0x11C / 4;

/// `BinkOpen` flags: a stream in memory, and the sound-track option list.
const FLAG_FROM_MEMORY: u32 = 0x0400_0000;
const FLAG_SOUND_TRACK: u32 = 0x4000;
const VIDEO_ALPHA: u32 = 0x0010_0000;

type OpenFn = unsafe extern "win64" fn(*mut u64, *const c_void, u32) -> i32;
type ReadFn = unsafe extern "win64" fn(*mut u64, *mut u8, u64) -> u64;
type CloseFn = unsafe extern "win64" fn(*mut u64);

/// The four functions of `BinkSetOSFileCallbacks`. The third (seek) takes `(file, position)`; playback reads
/// each stream once from its start, so it is never called.
#[derive(Clone, Copy)]
struct FileCallbacks {
    open: usize,
    read: usize,
    close: usize,
}

static CALLBACKS: Mutex<Option<FileCallbacks>> = Mutex::new(None);

/// A file the executable's own callbacks opened. The first word of the 256-byte block is the executable's
/// file object, which is all its callbacks touch.
struct CallbackSource {
    callbacks: FileCallbacks,
    file: Box<[u64; 32]>,
}

// SAFETY: the callbacks are the executable's own and are made to be called from Bink's threads.
unsafe impl Send for CallbackSource {}

impl CallbackSource {
    /// # Safety
    /// `name` is what the executable passed to `BinkOpenWithOptions`, which its open callback understands.
    unsafe fn open(
        callbacks: FileCallbacks,
        name: *const c_void,
        flags: u32,
    ) -> Option<CallbackSource> {
        let mut file = Box::new([0u64; 32]);
        // SAFETY: the address was registered as the open callback, which has this shape.
        let open: OpenFn = unsafe { std::mem::transmute(callbacks.open) };
        // SAFETY: as above; `file` is a stable block.
        let ok = unsafe { open(file.as_mut_ptr(), name, flags) } & 0xFF != 0;
        ok.then_some(CallbackSource { callbacks, file })
    }
}

impl ByteSource for CallbackSource {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        // SAFETY: the address was registered as the read callback, which has this shape.
        let read: ReadFn = unsafe { std::mem::transmute(self.callbacks.read) };
        // SAFETY: `buf` is valid for its length.
        let n = unsafe { read(self.file.as_mut_ptr(), buf.as_mut_ptr(), buf.len() as u64) };
        (n as usize).min(buf.len())
    }
}

impl Drop for CallbackSource {
    fn drop(&mut self) {
        // SAFETY: the address was registered as the close callback, which has this shape.
        let close: CloseFn = unsafe { std::mem::transmute(self.callbacks.close) };
        // SAFETY: the block the open callback filled in.
        unsafe { close(self.file.as_mut_ptr()) };
    }
}

struct Movie {
    player: Player,
    image: Box<[u32; WORDS]>,
}

impl Movie {
    fn word(&mut self, index: usize, value: u32) {
        // SAFETY: inside the image; the game reads it without a lock, so the writes are volatile.
        unsafe { self.image.as_mut_ptr().add(index).write_volatile(value) }
    }

    fn sync(&mut self) {
        let (cur, last, paused) = (
            self.player.current(),
            self.player.last_word(),
            self.player.paused(),
        );
        self.word(FRAME_NUM, cur);
        self.word(LAST_FRAME_NUM, last);
        self.word(PAUSED, u32::from(paused));
    }
}

static MOVIES: Mutex<BTreeMap<usize, Arc<Mutex<Movie>>>> = Mutex::new(BTreeMap::new());

static NOTED: [AtomicBool; 6] = [const { AtomicBool::new(false) }; 6];

fn note_once(slot: usize, text: &str) {
    if !NOTED[slot].swap(true, Ordering::Relaxed) {
        note(text.to_string());
    }
}

fn find(handle: usize) -> Option<Arc<Mutex<Movie>>> {
    MOVIES.lock().ok()?.get(&handle).cloned()
}

fn with<R>(handle: usize, f: impl FnOnce(&mut Movie) -> R) -> Option<R> {
    let movie = find(handle)?;
    let mut movie = movie.lock().unwrap_or_else(|p| p.into_inner());
    let result = f(&mut movie);
    movie.sync();
    Some(result)
}

/// The caller's name or stream: opens whatever it is and reads its first bytes.
fn open_source(source: *const u8, flags: u32) -> Option<Box<dyn ByteSource>> {
    if source.is_null() {
        return None;
    }
    let callbacks = CALLBACKS.lock().ok().and_then(|c| *c);
    let magic = |at: usize| {
        // SAFETY: a name is NUL-terminated, so a byte is read only after every byte before it matched
        // `KB2` and a revision letter, which no name starts with.
        unsafe { source.add(at).read() }
    };
    let in_memory = flags & FLAG_FROM_MEMORY != 0
        || (magic(0) == b'K'
            && magic(1) == b'B'
            && magic(2) == b'2'
            && magic(3).is_ascii_lowercase());
    if in_memory {
        // SAFETY: a stream in memory holds at least its fixed header; the size is raised once it is read.
        return Some(Box::new(unsafe { MemorySource::new(source, HEADER_BYTES) }));
    }
    if let Some(callbacks) = callbacks {
        // SAFETY: the executable's own name block, passed through to its own callback.
        let file = unsafe { CallbackSource::open(callbacks, source as *const c_void, flags) }?;
        return Some(Box::new(file));
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
    let file = crate::files::open_read(&String::from_utf8(name).ok()?).ok()?;
    Some(Box::new(FileSource(file)))
}

/// The audio tracks the options pick: `+0x1c` is how many ids and `+0x20` points at them. Without options
/// (or without the sound-track flag) the first track plays.
fn selected_tracks(options: *const u8, flags: u32, info: &StreamInfo) -> Vec<usize> {
    if info.audio.is_empty() {
        return Vec::new();
    }
    if options.is_null() || flags & FLAG_SOUND_TRACK == 0 {
        return vec![0];
    }
    // SAFETY: the options block is at least 0x28 bytes (`movies.md` notes in the module docs).
    let (count, ids) = unsafe {
        (
            (options.add(0x1C) as *const u32).read_unaligned(),
            (options.add(0x20) as *const usize).read_unaligned(),
        )
    };
    let mut selected = Vec::new();
    for i in 0..count.min(32) as usize {
        // SAFETY: `count` ids at the pointer.
        let id = unsafe { ((ids + i * 4) as *const u32).read_unaligned() };
        if let Some(track) = info.audio.iter().position(|t| t.id == id)
            && !selected.contains(&track)
        {
            selected.push(track);
        }
    }
    selected
}

/// `HBINK BinkOpenWithOptions(name_or_stream, options, flags)`
pub extern "win64" fn open_with_options(
    source: *const u8,
    options: *const c_void,
    flags: u32,
) -> usize {
    let mut byte_source = open_source(source, flags);
    let mut prefix = vec![0u8; PREFIX_BYTES];
    let header = byte_source.as_mut().and_then(|s| {
        let n = read_fully(s.as_mut(), &mut prefix[..HEADER_BYTES]);
        prefix.truncate(n);
        let header = parse_header(&prefix).ok()?;
        s.set_limit(header.stream_bytes as usize);
        let more = (header.stream_bytes as usize).min(PREFIX_BYTES);
        let have = prefix.len();
        prefix.resize(more.max(have), 0);
        let n = read_fully(s.as_mut(), &mut prefix[have..]);
        prefix.truncate(have + n);
        Some(header)
    });
    let (header, byte_source) = match (header, byte_source) {
        (Some(header), Some(source)) => (header, Some(source)),
        _ => {
            note_once(
                0,
                "Bink: a movie's header could not be read; it is treated as one 1920x1080 frame",
            );
            (MovieHeader::FALLBACK, None)
        }
    };
    let audio = parse_audio_tracks(&prefix, header.audio_tracks);
    let info = StreamInfo {
        width: header.width,
        height: header.height,
        frames: header.frames,
        fps_num: header.rate,
        fps_den: header.rate_divisor,
        has_alpha: header.video_flags & VIDEO_ALPHA != 0,
        audio,
    };
    let selected = selected_tracks(options as *const u8, flags, &info);
    let loader = match (byte_source, decoder_factory()) {
        (Some(source), Some(factory)) => Some(loader(source, prefix, header.stream_bytes, factory)),
        (Some(_), None) => {
            note_once(
                1,
                "Bink: no Bink 2 decoder is installed, so every movie ends at once and shows black",
            );
            None
        }
        (None, _) => None,
    };
    let player = Player::new(
        info,
        selected,
        loader,
        Box::new(EngineSink::new()),
        Arc::new(SystemClock::new()),
    );
    let mut movie = Movie {
        player,
        image: Box::new([0u32; WORDS]),
    };
    let info = movie.player.info().clone();
    movie.word(WIDTH, info.width);
    movie.word(HEIGHT, info.height);
    movie.word(FRAMES, info.frames);
    movie.word(RATE, info.fps_num);
    movie.word(RATE_DIV, info.fps_den);
    let tracks = movie.player.track_count();
    movie.word(AUDIO_TRACKS, tracks);
    movie.sync();
    let handle = movie.image.as_ptr() as usize;
    if let Ok(mut movies) = MOVIES.lock() {
        movies.insert(handle, Arc::new(Mutex::new(movie)));
    }
    handle
}

pub extern "win64" fn close(handle: usize) {
    let movie = MOVIES.lock().ok().and_then(|mut m| m.remove(&handle));
    if let Some(movie) = movie {
        movie
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .player
            .close();
    }
}

pub extern "win64" fn do_frame(handle: usize) -> i32 {
    with(handle, |m| m.player.do_frame());
    0
}

pub extern "win64" fn do_frame_async_multi(
    handle: usize,
    _threads: *const u32,
    _count: u32,
) -> i32 {
    with(handle, |m| m.player.do_frame_start());
    1
}

/// A negative time waits as long as the player allows.
pub extern "win64" fn do_frame_async_wait(handle: usize, microseconds: i32) -> i32 {
    let patience = if microseconds < 0 {
        WAIT_CAP
    } else {
        Duration::from_micros(microseconds as u64).min(WAIT_CAP)
    };
    with(handle, |m| i32::from(m.player.do_frame_wait(patience))).unwrap_or(1)
}

pub extern "win64" fn free_globals() {}

pub extern "win64" fn get_frame_buffers_info(handle: usize, info: *mut c_void) {
    if info.is_null() {
        return;
    }
    // SAFETY: the game passes its 0x108-byte description.
    with(handle, |m| unsafe { m.player.describe(info as *mut u8) });
}

pub extern "win64" fn get_key_frame(handle: usize, frame: u32, _flags: i32) -> u32 {
    with(handle, |m| match frame {
        0 => m.player.current(),
        f => f.min(m.player.info().frames),
    })
    .unwrap_or(0)
}

pub extern "win64" fn get_platform_info(_which: i32, _out: *mut c_void) -> i32 {
    0
}

pub extern "win64" fn goto(handle: usize, frame: u32, _flags: i32) {
    with(handle, |m| m.player.goto(frame));
}

pub extern "win64" fn next_frame(handle: usize) {
    with(handle, |m| m.player.next_frame());
}

pub extern "win64" fn pause(handle: usize, pause: i32) -> i32 {
    with(handle, |m| i32::from(m.player.pause(pause != 0))).unwrap_or(0)
}

pub extern "win64" fn register_frame_buffers(handle: usize, buffers: *const c_void) {
    if buffers.is_null() {
        return;
    }
    // SAFETY: the description is the game's own, which it keeps until it closes the movie.
    with(handle, |m| unsafe { m.player.register(buffers as usize) });
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
    open: usize,
    read: usize,
    _seek: usize,
    close: usize,
) -> i32 {
    if let Ok(mut callbacks) = CALLBACKS.lock() {
        *callbacks =
            (open != 0 && read != 0 && close != 0).then_some(FileCallbacks { open, read, close });
    }
    1
}

pub extern "win64" fn set_sound_system(_open: *const c_void, _parameter: usize) -> i32 {
    note_once(
        2,
        "Bink: BinkSetSoundSystem's sound system (a BINKSND table) is not used; movie audio goes to the shared mixer",
    );
    1
}

pub extern "win64" fn set_volume(handle: usize, track: u32, volume: i32) -> i32 {
    with(handle, |m| m.player.set_volume(track as usize, volume));
    1
}

pub extern "win64" fn set_will_loop(handle: usize, on: i32) {
    with(handle, |m| m.player.set_will_loop(on != 0));
}

pub extern "win64" fn should_skip(handle: usize) -> i32 {
    with(handle, |m| i32::from(m.player.should_skip())).unwrap_or(0)
}

pub extern "win64" fn start_async_thread(_thread: i32, _parameter: *const c_void) -> i32 {
    1
}

pub extern "win64" fn wait(handle: usize) -> i32 {
    with(handle, |m| i32::from(m.player.wait())).unwrap_or(0)
}

pub extern "win64" fn wait_stop_async_threads_multi(_count: u32) -> i32 {
    1
}
