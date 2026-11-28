//! The fixed part of a Bink 2 stream header and the audio descriptors after it (`movies.md`, section 2).

use thiserror::Error;

use super::decoder::AudioTrackInfo;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MediaError {
    #[error("not a Bink 2 stream")]
    NotBink,
    #[error("the Bink 2 header is shorter than its fixed part")]
    Short,
    #[error("the Bink 2 header holds an impossible size or frame count")]
    Implausible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovieHeader {
    pub revision: u8,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub rate: u32,
    pub rate_divisor: u32,
    pub audio_tracks: u32,
    /// The word at +0x24; its 0x00100000 bit is taken to be Bink's alpha flag.
    pub video_flags: u32,
    /// Bytes of the whole stream: the word at +0x04 plus 8.
    pub stream_bytes: u64,
}

pub const HEADER_BYTES: usize = 0x2C;

/// Revisions from `i` on have one more word between the fixed header and the audio descriptors.
const EXTRA_WORD: usize = 4;

/// Enough of the stream's start for the fixed header and the descriptors of 32 audio tracks.
pub const PREFIX_BYTES: usize = HEADER_BYTES + EXTRA_WORD + 32 * 12;

impl MovieHeader {
    /// What a movie whose header cannot be read looks like: one frame that ends it at once.
    pub const FALLBACK: MovieHeader = MovieHeader {
        revision: b'n',
        frames: 1,
        width: 1920,
        height: 1080,
        rate: 30,
        rate_divisor: 1,
        audio_tracks: 0,
        video_flags: 0,
        stream_bytes: 0,
    };
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

pub fn parse_header(bytes: &[u8]) -> Result<MovieHeader, MediaError> {
    if bytes.len() < 3 || &bytes[..3] != b"KB2" {
        return Err(MediaError::NotBink);
    }
    if bytes.len() < HEADER_BYTES {
        return Err(MediaError::Short);
    }
    let header = MovieHeader {
        revision: bytes[3],
        frames: word(bytes, 0x08),
        width: word(bytes, 0x14),
        height: word(bytes, 0x18),
        rate: word(bytes, 0x1C),
        rate_divisor: word(bytes, 0x20),
        audio_tracks: word(bytes, 0x28),
        video_flags: word(bytes, 0x24),
        stream_bytes: u64::from(word(bytes, 0x04)) + 8,
    };
    let sane = |v: u32| (16..=8192).contains(&v);
    if header.frames == 0
        || word(bytes, 0x10) != header.frames
        || !sane(header.width)
        || !sane(header.height)
        || header.rate == 0
        || header.rate_divisor == 0
        || header.audio_tracks > 32
    {
        return Err(MediaError::Implausible);
    }
    Ok(header)
}

/// The per-track sample rate, channel count and id, from the region after the fixed header (and the extra word of
/// revisions `i` and later): `count` maximum sizes, `count` pairs of u16 (rate, flags; 0x2000 is stereo), then
/// `count` ids. Empty when the bytes stop short.
pub fn parse_audio_tracks(bytes: &[u8], count: u32) -> Vec<AudioTrackInfo> {
    let count = count as usize;
    let extra = if bytes.get(3).is_some_and(|r| *r >= b'i') {
        EXTRA_WORD
    } else {
        0
    };
    let rates = HEADER_BYTES + extra + count * 4;
    let ids = rates + count * 4;
    if bytes.len() < ids + count * 4 {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            let rate = u16::from_le_bytes([bytes[rates + i * 4], bytes[rates + i * 4 + 1]]);
            let flags = u16::from_le_bytes([bytes[rates + i * 4 + 2], bytes[rates + i * 4 + 3]]);
            AudioTrackInfo {
                rate: u32::from(rate),
                channels: if flags & 0x2000 != 0 { 2 } else { 1 },
                id: word(bytes, ids + i * 4),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::testing::stream_header;

    #[test]
    fn the_fixed_header_parses_and_nonsense_is_rejected() {
        let bytes = stream_header(450, 1920, 1080, 2);
        let h = parse_header(&bytes).unwrap();
        assert_eq!(
            (h.frames, h.width, h.height, h.audio_tracks),
            (450, 1920, 1080, 2)
        );
        assert_eq!(h.stream_bytes, bytes.len() as u64);
        assert_eq!(parse_header(b"BIKi"), Err(MediaError::NotBink));
        assert_eq!(parse_header(b"KB2n"), Err(MediaError::Short));
        assert_eq!(
            parse_header(&stream_header(0, 1920, 1080, 0)),
            Err(MediaError::Implausible)
        );
        let mut mismatch = stream_header(10, 640, 480, 0);
        mismatch[0x10] = 11;
        assert_eq!(parse_header(&mismatch), Err(MediaError::Implausible));
    }

    #[test]
    fn audio_descriptors_give_rate_channels_and_id() {
        let tracks = parse_audio_tracks(&stream_header(10, 640, 480, 2), 2);
        assert_eq!(
            tracks,
            vec![
                AudioTrackInfo {
                    rate: 48000,
                    channels: 2,
                    id: 0
                },
                AudioTrackInfo {
                    rate: 48000,
                    channels: 2,
                    id: 3
                },
            ]
        );
        assert!(parse_audio_tracks(&stream_header(10, 640, 480, 2)[..0x40], 2).is_empty());
    }

    #[test]
    fn revisions_before_i_have_no_extra_word_before_the_descriptors() {
        let mut bytes = stream_header(10, 640, 480, 1);
        bytes[3] = b'h';
        bytes.drain(HEADER_BYTES..HEADER_BYTES + 4);
        let tracks = parse_audio_tracks(&bytes, 1);
        assert_eq!(tracks.len(), 1);
        assert_eq!((tracks[0].rate, tracks[0].channels), (48000, 2));
    }
}
