//! XMA2 decoding: the WMA Pro frame coder in 2048-byte packets.

mod frame;
mod stream;
mod tables;

pub use frame::{FrameDecoder, FrameInfo, SAMPLES_PER_FRAME, band_rate_class};
pub use stream::{
    DEFAULT_LEAD_IN, FFMPEG_LEAD_IN, PACKET_SIZE, PacketHeader, Pcm, Xma2Config, Xma2Decoder, f32_to_i16,
};
