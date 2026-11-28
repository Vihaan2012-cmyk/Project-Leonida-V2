//! Video: `Bink2Scarlett.dll`, and the Media Foundation calls the game makes.
//!
//! The 23 functions the executable imports play movies with the `bink` crate's decoder, or the one installed with
//! [`set_decoder_factory`] (a movie whose stream cannot be read shows black frames that end at once: every frame
//! due at once, `BinkNextFrame` walking the counter to the last frame, which is how a play loop ends).
//!
//! - Opening reads the stream header through the executable's own file callbacks (`BinkSetOSFileCallbacks`),
//!   from a stream in memory, or from a host file. The decode thread ([`worker`]) reads the rest and decodes a
//!   few frames ahead only once a frame is asked for, so opening a movie just to measure it (the game does) is cheap.
//! - [`player`] holds one movie's state: `BinkWait` follows the movie's frame rate on a media clock that stops
//!   while paused, `BinkShouldSkip` is true when the next frame is a whole period late, `BinkGoto` seeks the
//!   decoder (unless the frame is queued), `BinkSetWillLoop` wraps. A decoder error ends the movie: the frame
//!   counter jumps to the last frame and a trace note says why.
//! - Frames go into the game's own mapped textures in the layout [`layout`] documents.
//! - Audio goes to voices on the shared mixer ([`sound`]), one per selected track: each decoded frame's samples are
//!   queued when the frame is played (the first frame brings 0.76 s, so the queue stays ahead of the device).
//!   The movie's clock follows the audio: [`player`] reads how long the first voice has played (a source-frame count
//!   in the mixer, rebased at every seek) when each frame is installed, and moves the wall-based clock by the
//!   smoothed difference, so picture and sound stay together through device clock drift. A stalled or absent
//!   device (the voice plays nothing) hands the clock back to the wall.
//!
//! # What the executable does with Bink (RVAs of game build 1.0.2.1, read statically)
//!
//! - `BinkSetOSFileCallbacks` is called once, from `015e3c10`, with four functions: open `01946220`
//!   `(file*, name, flags) -> bool` (writes the game's file object into `*file`), read `0092ebc0`
//!   `(file*, dest, size) -> bytes` (sequential from position 0; short at the end), a position setter
//!   `0442e890 (file*, position)` and close `01a400f0 (file*)`. `name` is not a string but the game's own block:
//!   `+0x00` raw path, `+0x08` u32, `+0x10` resolved path, `+0x18` the player. The game tries `<name>`, then
//!   `<name>.bk2`, then `<name>.bik`, and holds at most 8 movie files open. Only the first word of `file` is used.
//! - The same function calls `BinkSetMemory(03a1a170, 0169c4d0)` (a 0x20-aligned allocator and its free; unused here),
//!   `BinkStartAsyncThread(i, 0)` for up to 4 threads, and `BinkSetSoundSystem(05f172c0, 0)`.
//! - `BinkOpenWithOptions(name_block_or_stream, options, flags)` is called from `0442ea70` (a probe that opens,
//!   reads `BinkGetFrameBuffersInfo`, and closes at once) and `0442efe0` (the real open, from `0199ef20`).
//!   `flags` is `0x440 | 0x4000`, plus `0x8` when the descriptor's byte `+0x251` is 1, plus `0x08204000` when `+0x250`
//!   is 1, plus `0x04000000` (from memory) when the descriptor holds a stream pointer at `+0x298`. `options`
//!   (built by `0442ed40`) is a zeroed block of at least 0x28 bytes where only `+0x1c` (a u32, 1 when the game
//!   wants audio) and `+0x20` (a pointer to that many u32 track ids, the id being the descriptor's word at `+0x218`)
//!   are set. Audio tracks are therefore picked by id.
//! - Handle words the game reads (`002ef0e0`, `0442f340`, `0442f580`, `0442f6e0`, `0442f9e0`, `0442fa20`, `018bea90`):
//!   `+0x08` frames, `+0x0c` current frame, `+0x10` last decoded frame (the game treats all ones as "none yet"),
//!   `+0x14`/`+0x18` frame rate, `+0x38` audio track count, `+0x11c` paused (nonzero makes the game call `BinkPause(0)`).
//! - Playing: `BinkPause(h, 1)` right after opening (`05536e00`); `BinkSetWillLoop`; then per tick `BinkWait(h) != 0`
//!   means not due, else `BinkNextFrame`, `BinkDoFrameAsyncMulti(h, thread_ids, count)` and
//!   `BinkDoFrameAsyncWait(h, -1)`; when `BinkShouldSkip` the same three run in a loop; `BinkGoto(h, n, 0)` plus the
//!   same two calls seeks; `BinkDoFrame` follows `BinkNextFrame` after a seek (`002ef309`). `BinkGetKeyFrame(h, f, 2)`
//!   (`0442f580`) picks restart frames, and `BinkSetVolume(h, track, v)` (`018bea90`) is called for each track with
//!   `v` clamped to 0x10000 by the game (`018beb30`), so 0x10000 is full scale. `BinkGetPlatformInfo(0x400 + i, &id)`
//!   is expected to give a Bink thread's id; answering 0 makes the game use no Bink threads (`0577fa20`).
//! - Video reaches the GPU as planar 8-bit textures per frame buffer set, written through the CPU: `0442f760` maps
//!   each texture (`05121690`) and hands its address and pitch over in the frame buffer description, and after each
//!   decode `0532f140` only flushes the cache lines of the current set. The textures are made by `01bc2de0` from
//!   the description: one per plane that has its `Allocate` word set, sized from the description, with a 256-byte
//!   aligned pitch. The description is 0x108 bytes (`0442f170` copies exactly that); `0442ec02` and `0442ec11`
//!   read the alpha and fifth plane words of set 0.
//!
//! # Not measured, and what is done instead
//!
//! - Whether the game crops by the movie size or by the buffer size: the description reports the exact movie size,
//!   not a padded one.
//! - Whether a movie has alpha: taken from bit `0x00100000` of the header word at `+0x24`.
//! - How many frame buffer sets Bink itself reports: three here (the game's fence array holds three).
//! - The game's own sound system is measured but not driven. `0442fce0` (from `015e3c10`) calls
//!   `BinkSetSoundSystem(05f172c0, 0)`; `05f172c0` returns `01a9b300`, the game's `BINKSNDOPEN`
//!   `(snd, rate, channels, flags, bink) -> bool`. It zeroes a 0x1c0-byte `BINKSND`, stores rate (`+0x34`), channels
//!   (`+0x38`) and the movie (`+0x74`), sets the ready function `020eeb40` (`+0x170`: room for a segment in the
//!   voice's queue), `0442e820` (`+0x178`), the submit function `00b98f70` (`+0x180`: sends a finished segment of the
//!   ring at `+0xa4` to the voice through `04e64f00`), `0442e850`/`0442e860` (`+0x198`/`+0x1a0`), the close function
//!   `019031b0` (`+0x1a8`) and no-ops (`00004380`) elsewhere. It makes the ring (eight segments of 0.125 s of 16-bit samples,
//!   `01a9b4a0`) and a 0x60-byte streaming voice object in the game's own audio layer (`04f97130`,
//!   `04ee9870`, `04ee9ae0`), and files the sound in a table of up to eight (`01a9b570`). So in the real engine the
//!   decoded samples reach the game's audio layer (its XAudio2 source voices), mixed under the groups `Bink.Quiet`,
//!   `Bink.Normal` and `Bink.Loud` (`05e15400`, from `02425c90`); the voice is named `bink_%u` (`0086a0c0`). The
//!   per-track volume the game sets with `BinkSetVolume` (`018bea90`) is applied to the mixer voice here instead.
//!   Nothing else of Bink's sound API is imported (no `BinkOpenTrack`, `BinkSetPan`, `BinkGetTrackData`).
//! - The plane order and audio contract of the decoder behind [`MovieDecoder`] are assumed (Y, Cr, Cb, alpha; the
//!   audio of the frame just decoded).
//! - The decode thread calls the game's read callback from a thread the game did not start.

mod bink_decoder;
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

use crate::Shim;

/// `BinkOpen`'s flag for a stream inside a larger file (`movies.md`).
pub const FLAG_FILE_OFFSET: u32 = 0x20;

#[cfg(windows)]
mod ffi;

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let list: [(&str, usize); 23] = [
            ("BinkClose", ffi::close as *const () as usize),
            ("BinkDoFrame", ffi::do_frame as *const () as usize),
            (
                "BinkDoFrameAsyncMulti",
                ffi::do_frame_async_multi as *const () as usize,
            ),
            (
                "BinkDoFrameAsyncWait",
                ffi::do_frame_async_wait as *const () as usize,
            ),
            ("BinkFreeGlobals", ffi::free_globals as *const () as usize),
            (
                "BinkGetFrameBuffersInfo",
                ffi::get_frame_buffers_info as *const () as usize,
            ),
            ("BinkGetKeyFrame", ffi::get_key_frame as *const () as usize),
            (
                "BinkGetPlatformInfo",
                ffi::get_platform_info as *const () as usize,
            ),
            ("BinkGoto", ffi::goto as *const () as usize),
            ("BinkNextFrame", ffi::next_frame as *const () as usize),
            (
                "BinkOpenWithOptions",
                ffi::open_with_options as *const () as usize,
            ),
            ("BinkPause", ffi::pause as *const () as usize),
            (
                "BinkRegisterFrameBuffers",
                ffi::register_frame_buffers as *const () as usize,
            ),
            (
                "BinkRequestStopAsyncThreadsMulti",
                ffi::request_stop_async_threads_multi as *const () as usize,
            ),
            ("BinkSetMemory", ffi::set_memory as *const () as usize),
            (
                "BinkSetOSFileCallbacks",
                ffi::set_os_file_callbacks as *const () as usize,
            ),
            (
                "BinkSetSoundSystem",
                ffi::set_sound_system as *const () as usize,
            ),
            ("BinkSetVolume", ffi::set_volume as *const () as usize),
            ("BinkSetWillLoop", ffi::set_will_loop as *const () as usize),
            ("BinkShouldSkip", ffi::should_skip as *const () as usize),
            (
                "BinkStartAsyncThread",
                ffi::start_async_thread as *const () as usize,
            ),
            ("BinkWait", ffi::wait as *const () as usize),
            (
                "BinkWaitStopAsyncThreadsMulti",
                ffi::wait_stop_async_threads_multi as *const () as usize,
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
