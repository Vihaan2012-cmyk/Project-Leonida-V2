//! Audio decoding and container handling for the game's audio.
//!
//! - [`xma2`]: streaming XMA2 decoder (packets in, PCM out) for the runtime's audio shims and the exporter.
//! - [`awc`]: the `ADAT` bank container: streams, tags, markers, seek tables, loudness and peaks.
//! - [`decode`]: whole-stream decoding of PCM16, PCM float and XMA2 to [`Pcm`] and WAV.
//! - [`opus`] and [`ogg`]: lossless Ogg Opus repackaging of plain and streamed-music Opus (no Opus decoder).
//! - [`wav`]: WAV writing and reading with cue and loop chunks.
//!
//! Not implemented: decoding Opus to PCM, and any XMA2 feature the bitstream flags as reserved; those return
//! [`Error::Unsupported`] instead of producing audio.

pub mod awc;
pub mod bits;
pub mod decode;
pub mod error;
pub mod huffman;
pub mod mdct;
pub mod ogg;
pub mod opus;
pub mod pcm;
pub mod wav;
pub mod xma2;

pub use error::{Error, Result};
pub use pcm::{Pcm, f32_to_i16};
