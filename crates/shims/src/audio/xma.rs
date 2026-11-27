//! The hardware XMA decoder's requests. The decode itself belongs to the `audio-codec` crate.

use std::sync::Mutex;

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
        Err(DecodeError::Unavailable) => Ok(Decoded {
            pcm: vec![0; request.frames as usize * usize::from(request.channels.max(1))],
            sample_rate: request.sample_rate,
            channels: request.channels.max(1),
        }),
        other => other,
    }
}

// integration
/// The one place that calls the XMA decoder; `audio-codec` replaces this body.
fn decode_backend(_request: &XmaRequest<'_>) -> Result<Decoded, DecodeError> {
    Err(DecodeError::Unavailable)
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
    fn without_a_decoder_the_stream_is_silence_of_the_declared_length() {
        let decoded = decode_backend(&request());
        assert_eq!(decoded, Err(DecodeError::Unavailable));
        let silent = decode(&request()).unwrap();
        assert_eq!(silent.pcm.len(), 200);
        assert!(silent.pcm.iter().all(|s| *s == 0));
        assert_eq!((silent.sample_rate, silent.channels), (32_000, 2));
    }
}
