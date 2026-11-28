//! What the shim needs from a Bink 2 decoder, as a trait with the shape of `bink::Movie`, so playback is
//! written and tested before the decoder crate is in.

use std::sync::{Arc, Mutex};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrackInfo {
    pub rate: u32,
    pub channels: u8,
    pub id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub has_alpha: bool,
    pub audio: Vec<AudioTrackInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

/// A decoded frame. `planes` are in the order the game's frame buffers use: Y, Cr, Cb, then alpha when the
/// movie has it. `index` counts from 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub index: u32,
    pub planes: Vec<Plane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct DecodeError(pub String);

pub trait MovieDecoder: Send {
    fn info(&self) -> StreamInfo;
    /// The next frame of the stream, starting at frame 0 and after every `seek`.
    fn decode_next(&mut self) -> Result<DecodedFrame, DecodeError>;
    /// Makes the next `decode_next` return `frame` (counted from 0).
    fn seek(&mut self, frame: u32) -> Result<(), DecodeError>;
    /// The samples of one audio track (index into `info().audio`) that belong to the frame last decoded,
    /// interleaved, `-1..1`.
    fn audio(&mut self, track: usize) -> Vec<f32>;
}

pub type DecoderFactory = fn(Arc<[u8]>) -> Result<Box<dyn MovieDecoder>, DecodeError>;

static FACTORY: Mutex<Option<DecoderFactory>> = Mutex::new(None);

/// Chooses the decoder every movie opened from now on uses instead of the `bink` crate's.
pub fn set_decoder_factory(factory: Option<DecoderFactory>) {
    if let Ok(mut slot) = FACTORY.lock() {
        *slot = factory;
    }
}

/// The installed decoder, or the `bink` crate's.
pub fn decoder_factory() -> Option<DecoderFactory> {
    FACTORY
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .or(Some(super::bink_decoder::open))
}
