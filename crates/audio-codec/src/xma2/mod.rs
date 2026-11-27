//! XMA2 decoding: the WMA Pro frame coder in 2048-byte packets.

mod frame;
mod riff;
mod stream;
mod tables;

pub use frame::{FrameDecoder, FrameInfo, SAMPLES_PER_FRAME, band_rate_class};
pub use riff::{Xma2File, Xma2Format, write_xma2_riff};
pub use stream::{
    DEFAULT_LEAD_IN, FFMPEG_LEAD_IN, PACKET_SIZE, PacketHeader, Xma2Config, Xma2Decoder,
};

#[cfg(test)]
pub(crate) mod testenc;
#[cfg(test)]
mod tests;
