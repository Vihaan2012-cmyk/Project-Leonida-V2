//! Audio decoding and container handling for the game's audio.

pub mod awc;
pub mod bits;
pub mod error;
pub mod huffman;
pub mod mdct;
pub mod ogg;
pub mod wav;
pub mod xma2;

pub use error::{Error, Result};
