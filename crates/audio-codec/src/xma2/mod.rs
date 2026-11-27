//! XMA2 decoding: the WMA Pro frame coder in 2048-byte packets.

mod frame;
mod tables;

pub use frame::{FrameDecoder, FrameInfo, SAMPLES_PER_FRAME, band_rate_class};
