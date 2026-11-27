//! RIFF XMA2 files (`.xma.wav`): a `fmt ` chunk with the XMA2WAVEFORMATEX fields and the raw packets in `data`.

use super::stream::{PACKET_SIZE, Xma2Config};
use crate::{Error, Result};

const FORMAT_TAG_XMA2: u16 = 0x0166;
const WAVEFORMATEX_SIZE: usize = 18;
const EXTRA_SIZE: usize = 34;

/// The 34 bytes after a WAVEFORMATEX in an XMA2 `fmt ` chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xma2Format {
    pub num_streams: u16,
    pub channel_mask: u32,
    pub samples_encoded: u32,
    pub bytes_per_block: u32,
    pub play_begin: u32,
    pub play_length: u32,
    pub loop_begin: u32,
    pub loop_length: u32,
    pub loop_count: u8,
    pub encoder_version: u8,
    pub block_count: u16,
}

/// A parsed `.xma.wav`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xma2File<'a> {
    pub channels: u16,
    pub sample_rate: u32,
    pub format: Xma2Format,
    pub data: &'a [u8],
    /// The `seek` chunk if present, raw (its byte order is not measured).
    pub seek: Option<&'a [u8]>,
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl<'a> Xma2File<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(Error::Invalid("not a RIFF WAVE file".into()));
        }
        let mut at = 12;
        let mut fmt = None;
        let mut data = None;
        let mut seek = None;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size = u32_at(bytes, at + 4) as usize;
            let body_end = (at + 8 + size).min(bytes.len());
            let body = &bytes[at + 8..body_end];
            match id {
                b"fmt " => fmt = Some(body),
                b"data" => data = Some(body),
                b"seek" => seek = Some(body),
                _ => {}
            }
            at += 8 + size + (size & 1);
        }
        let fmt = fmt.ok_or_else(|| Error::Invalid("XMA2 file without a fmt chunk".into()))?;
        if fmt.len() < WAVEFORMATEX_SIZE || u16::from_le_bytes([fmt[0], fmt[1]]) != FORMAT_TAG_XMA2
        {
            return Err(Error::Invalid("fmt chunk is not XMA2".into()));
        }
        let cb_size = usize::from(u16::from_le_bytes([fmt[16], fmt[17]]));
        if cb_size != EXTRA_SIZE || fmt.len() < WAVEFORMATEX_SIZE + EXTRA_SIZE {
            return Err(Error::Unsupported(format!(
                "XMA2 fmt chunk with {cb_size} extra bytes (only the 34-byte XMA2WAVEFORMATEX is read)"
            )));
        }
        let e = &fmt[WAVEFORMATEX_SIZE..];
        Ok(Self {
            channels: u16::from_le_bytes([fmt[2], fmt[3]]),
            sample_rate: u32_at(fmt, 4),
            format: Xma2Format {
                num_streams: u16::from_le_bytes([e[0], e[1]]),
                channel_mask: u32_at(e, 2),
                samples_encoded: u32_at(e, 6),
                bytes_per_block: u32_at(e, 10),
                play_begin: u32_at(e, 14),
                play_length: u32_at(e, 18),
                loop_begin: u32_at(e, 22),
                loop_length: u32_at(e, 26),
                loop_count: e[30],
                encoder_version: e[31],
                block_count: u16::from_le_bytes([e[32], e[33]]),
            },
            data: data.ok_or_else(|| Error::Invalid("XMA2 file without a data chunk".into()))?,
            seek,
        })
    }

    /// Decoder settings: the play range becomes the lead-in and length.
    pub fn config(&self) -> Xma2Config {
        let f = &self.format;
        let mut c = Xma2Config::new(
            self.sample_rate,
            usize::from(self.channels),
            Some(if f.play_length != 0 {
                u64::from(f.play_length)
            } else {
                u64::from(f.samples_encoded).saturating_sub(u64::from(f.play_begin))
            }),
        );
        c.lead_in = f.play_begin as usize;
        c
    }

    /// The packets of `data`, which must be whole 2048-byte packets.
    pub fn packets(&self) -> Result<std::slice::ChunksExact<'a, u8>> {
        if !self.data.len().is_multiple_of(PACKET_SIZE) {
            return Err(Error::Invalid(format!(
                "XMA2 data of {} bytes is not a whole number of {PACKET_SIZE}-byte packets",
                self.data.len()
            )));
        }
        Ok(self.data.chunks_exact(PACKET_SIZE))
    }
}

/// Write a `.xma.wav` for `data` (whole packets).
pub fn write_xma2_riff(
    sample_rate: u32,
    channels: u16,
    samples_encoded: u32,
    play_begin: u32,
    play_length: u32,
    data: &[u8],
) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&FORMAT_TAG_XMA2.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&sample_rate.to_le_bytes());
    fmt.extend_from_slice(&(sample_rate * u32::from(channels) * 2 / 4).to_le_bytes());
    fmt.extend_from_slice(&(PACKET_SIZE as u16).to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    fmt.extend_from_slice(&(EXTRA_SIZE as u16).to_le_bytes());
    fmt.extend_from_slice(&u16::from(channels).div_ceil(2).to_le_bytes());
    fmt.extend_from_slice(&0u32.to_le_bytes());
    fmt.extend_from_slice(&samples_encoded.to_le_bytes());
    fmt.extend_from_slice(&(PACKET_SIZE as u32).to_le_bytes());
    fmt.extend_from_slice(&play_begin.to_le_bytes());
    fmt.extend_from_slice(&play_length.to_le_bytes());
    fmt.extend_from_slice(&[0; 8]);
    fmt.push(0);
    fmt.push(4);
    fmt.extend_from_slice(&((data.len() / PACKET_SIZE) as u16).to_le_bytes());
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVEfmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(data);
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_parse() {
        let data = vec![7u8; PACKET_SIZE * 3];
        let bytes = write_xma2_riff(32000, 1, 1536, 0, 1536, &data);
        let f = Xma2File::parse(&bytes).unwrap();
        assert_eq!((f.channels, f.sample_rate), (1, 32000));
        assert_eq!(f.format.block_count, 3);
        assert_eq!(f.format.num_streams, 1);
        assert_eq!(f.format.bytes_per_block, 2048);
        assert_eq!(f.format.samples_encoded, 1536);
        assert_eq!(f.data, &data[..]);
        assert_eq!(f.packets().unwrap().count(), 3);
        let c = f.config();
        assert_eq!(
            (c.sample_rate, c.channels, c.total_samples, c.lead_in),
            (32000, 1, Some(1536), 0)
        );
    }

    #[test]
    fn play_range_sets_lead_in_and_length() {
        let bytes = write_xma2_riff(48000, 2, 4000, 128, 3000, &[0; PACKET_SIZE]);
        let c = Xma2File::parse(&bytes).unwrap().config();
        assert_eq!((c.lead_in, c.total_samples), (128, Some(3000)));
        let bytes = write_xma2_riff(48000, 2, 4000, 128, 0, &[0; PACKET_SIZE]);
        assert_eq!(
            Xma2File::parse(&bytes).unwrap().config().total_samples,
            Some(3872)
        );
    }

    #[test]
    fn rejects_other_formats_and_ragged_data() {
        assert!(Xma2File::parse(b"RIFF\0\0\0\0WAVE").is_err());
        let mut bytes = write_xma2_riff(32000, 1, 100, 0, 100, &[0; PACKET_SIZE]);
        let at = bytes.windows(4).position(|w| w == b"fmt ").unwrap() + 8;
        bytes[at] = 1;
        assert!(Xma2File::parse(&bytes).is_err());
        let ragged = write_xma2_riff(32000, 1, 100, 0, 100, &[0; 100]);
        assert!(Xma2File::parse(&ragged).unwrap().packets().is_err());
    }
}
