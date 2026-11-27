//! Audio decoding and container handling for the game's audio.

pub mod awc;
pub mod bits;
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
