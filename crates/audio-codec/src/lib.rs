//! Audio decoding and container handling for the game's audio.

pub mod bits;
pub mod error;
pub mod huffman;
pub mod mdct;

pub use error::{Error, Result};
