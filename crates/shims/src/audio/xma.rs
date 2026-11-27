//! The hardware XMA decoder's requests. The decode itself belongs to the `audio-codec` crate.

use std::sync::Mutex;

use audio_codec::xma2::{Xma2Config, Xma2Decoder};

/// A stream of XMA2 data to decode: whole packets from the start of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmaRequest<'a> {
    pub data: &'a [u8],
    pub sample_rate: u32,
    pub channels: u8,
    /// Frames the stream declares, used when the decoder is not available.
    pub frames: u32,
}

/// Interleaved 16-bit PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub pcm: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("no XMA decoder is linked in")]
    Unavailable,
    #[error("XMA decode failed: {0}")]
    Failed(String),
}

pub type Decoder = fn(&XmaRequest<'_>) -> Result<Decoded, DecodeError>;

static OVERRIDE: Mutex<Option<Decoder>> = Mutex::new(None);

/// Replaces the decoder, for tests and for hosts that bring their own. `None` restores the default.
pub fn set_decoder(decoder: Option<Decoder>) {
    if let Ok(mut slot) = OVERRIDE.lock() {
        *slot = decoder;
    }
}

/// Decodes a request. When no decoder is available the result is silence of the declared length, so the
/// voice still completes on time.
pub fn decode(request: &XmaRequest<'_>) -> Result<Decoded, DecodeError> {
    let decoder = OVERRIDE.lock().ok().and_then(|slot| *slot);
    let outcome = match decoder {
        Some(decoder) => decoder(request),
        None => decode_backend(request),
    };
    match outcome {
        Err(DecodeError::Unavailable) => Ok(silence(request)),
        other => other,
    }
}

fn silence(request: &XmaRequest<'_>) -> Decoded {
    Decoded {
        pcm: vec![0; request.frames as usize * usize::from(request.channels.max(1))],
        sample_rate: request.sample_rate,
        channels: request.channels.max(1),
    }
}

/// The one place that calls the XMA decoder: `audio-codec`'s streaming XMA2 decoder over the whole stream. A stream
/// with no whole packet in it, which the hardware would also have nothing to decode, is silence of the declared
/// length.
fn decode_backend(request: &XmaRequest<'_>) -> Result<Decoded, DecodeError> {
    let failed = |error: audio_codec::Error| DecodeError::Failed(error.to_string());
    if request.channels == 0 {
        return Err(DecodeError::Failed("an XMA stream with no channels".into()));
    }
    let channels = usize::from(request.channels);
    let declared = (request.frames > 0).then_some(u64::from(request.frames));
    let config = Xma2Config::new(request.sample_rate, channels, declared);
    let mut decoder = Xma2Decoder::new(config).map_err(failed)?;
    let pushed = decoder.push_data(request.data);
    let frames = decoder.available();
    if frames == 0 {
        return match pushed {
            Err(error) => Err(failed(error)),
            Ok(()) => Ok(silence(request)),
        };
    }
    let mut pcm = vec![0i16; frames * channels];
    let written = decoder.pull_i16(&mut pcm);
    pcm.truncate(written * channels);
    Ok(Decoded {
        pcm,
        sample_rate: request.sample_rate,
        channels: request.channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> XmaRequest<'static> {
        XmaRequest {
            data: &[1, 2, 3],
            sample_rate: 32_000,
            channels: 2,
            frames: 100,
        }
    }

    #[test]
    fn a_stream_with_no_whole_packet_is_silence_of_the_declared_length() {
        let decoded = decode_backend(&request()).unwrap();
        assert_eq!(decoded.pcm.len(), 200);
        assert!(decoded.pcm.iter().all(|s| *s == 0));
        assert_eq!((decoded.sample_rate, decoded.channels), (32_000, 2));
        assert_eq!(decode(&request()).unwrap(), decoded);
    }

    #[test]
    fn a_stream_with_no_channels_or_rate_is_refused() {
        let mut bad = request();
        bad.channels = 0;
        assert!(matches!(decode_backend(&bad), Err(DecodeError::Failed(_))));
        let mut bad = request();
        bad.sample_rate = 0;
        assert!(matches!(decode_backend(&bad), Err(DecodeError::Failed(_))));
        let mut bad = request();
        bad.channels = 17;
        assert!(matches!(decode_backend(&bad), Err(DecodeError::Failed(_))));
    }

    #[test]
    fn a_whole_packet_of_zeros_is_decoded_or_refused_never_longer_than_declared() {
        let zeros = vec![0u8; 2048];
        let mut packet = request();
        packet.data = &zeros;
        if let Ok(decoded) = decode_backend(&packet) {
            assert_eq!(decoded.channels, 2);
            assert!(decoded.pcm.len() <= 200);
        }
    }
}
