//! RIFF WAVE writing (16-bit PCM and 32-bit float) with optional cue points and loops, and a small reader.

use std::io::Write;

use crate::{Error, Result};

/// A loop for the `smpl` chunk, in sample frames, `end` inclusive as the chunk defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavLoop {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WavExtras {
    /// Positions written as `cue ` points.
    pub cues: Vec<u32>,
    pub loops: Vec<WavLoop>,
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

fn build(
    format_tag: u16,
    bits: u16,
    rate: u32,
    channels: u16,
    data: Vec<u8>,
    extras: &WavExtras,
) -> Vec<u8> {
    let block_align = channels * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&format_tag.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    chunk(&mut body, b"fmt ", &fmt);
    chunk(&mut body, b"data", &data);
    if !extras.cues.is_empty() {
        let mut cue = Vec::new();
        cue.extend_from_slice(&(extras.cues.len() as u32).to_le_bytes());
        for (i, &pos) in extras.cues.iter().enumerate() {
            cue.extend_from_slice(&(i as u32 + 1).to_le_bytes());
            cue.extend_from_slice(&pos.to_le_bytes());
            cue.extend_from_slice(b"data");
            cue.extend_from_slice(&0u32.to_le_bytes());
            cue.extend_from_slice(&0u32.to_le_bytes());
            cue.extend_from_slice(&pos.to_le_bytes());
        }
        chunk(&mut body, b"cue ", &cue);
    }
    if !extras.loops.is_empty() {
        let mut smpl = Vec::new();
        let period = 1_000_000_000u32 / rate.max(1);
        for v in [0u32, 0, period, 60, 0, 0, 0, extras.loops.len() as u32, 0] {
            smpl.extend_from_slice(&v.to_le_bytes());
        }
        for (i, l) in extras.loops.iter().enumerate() {
            for v in [i as u32, 0, l.start, l.end, 0, 0] {
                smpl.extend_from_slice(&v.to_le_bytes());
            }
        }
        chunk(&mut body, b"smpl", &smpl);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// A complete 16-bit PCM WAV file for interleaved `samples`.
pub fn pcm16_wav(rate: u32, channels: u16, samples: &[i16], extras: &WavExtras) -> Vec<u8> {
    let data = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    build(1, 16, rate, channels, data, extras)
}

/// A complete IEEE float WAV file for interleaved `samples`.
pub fn f32_wav(rate: u32, channels: u16, samples: &[f32], extras: &WavExtras) -> Vec<u8> {
    let data = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    build(3, 32, rate, channels, data, extras)
}

pub fn write_pcm16<W: Write>(w: &mut W, rate: u32, channels: u16, samples: &[i16]) -> Result<()> {
    w.write_all(&pcm16_wav(rate, channels, samples, &WavExtras::default()))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub float: bool,
    pub data: Vec<u8>,
    pub extras: WavExtras,
}

impl Wav {
    pub fn parse(bytes: &[u8]) -> Result<Wav> {
        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(Error::Invalid("not a RIFF WAVE file".into()));
        }
        let mut at = 12;
        let mut fmt: Option<(u16, u16, u32, u16)> = None;
        let mut data = None;
        let mut extras = WavExtras::default();
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size =
                u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
                    as usize;
            let body = bytes.get(at + 8..at + 8 + size).ok_or(Error::Truncated {
                what: "WAV chunk",
                offset: at,
                need: size,
                have: bytes.len().saturating_sub(at + 8),
            })?;
            let u32_at =
                |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            match id {
                b"fmt " if size >= 16 => {
                    fmt = Some((
                        u16::from_le_bytes([body[0], body[1]]),
                        u16::from_le_bytes([body[2], body[3]]),
                        u32_at(body, 4),
                        u16::from_le_bytes([body[14], body[15]]),
                    ));
                }
                b"data" => data = Some(body.to_vec()),
                b"cue " if size >= 4 => {
                    let n = u32_at(body, 0) as usize;
                    for i in 0..n {
                        if 4 + i * 24 + 24 <= size {
                            extras.cues.push(u32_at(body, 4 + i * 24 + 4));
                        }
                    }
                }
                b"smpl" if size >= 36 => {
                    let n = u32_at(body, 28) as usize;
                    for i in 0..n {
                        let o = 36 + i * 24;
                        if o + 24 <= size {
                            extras.loops.push(WavLoop {
                                start: u32_at(body, o + 8),
                                end: u32_at(body, o + 12),
                            });
                        }
                    }
                }
                _ => {}
            }
            at += 8 + size + (size & 1);
        }
        let (tag, channels, sample_rate, bits) =
            fmt.ok_or_else(|| Error::Invalid("WAV without a fmt chunk".into()))?;
        if tag != 1 && tag != 3 {
            return Err(Error::Unsupported(format!("WAV format tag {tag:#x}")));
        }
        Ok(Wav {
            sample_rate,
            channels,
            bits,
            float: tag == 3,
            data: data.ok_or_else(|| Error::Invalid("WAV without a data chunk".into()))?,
            extras,
        })
    }

    pub fn samples_i16(&self) -> Option<Vec<i16>> {
        (!self.float && self.bits == 16).then(|| {
            self.data
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect()
        })
    }

    pub fn samples_f32(&self) -> Option<Vec<f32>> {
        (self.float && self.bits == 32).then(|| {
            self.data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_round_trips_with_cues_and_loops() {
        let samples: Vec<i16> = (0..20).map(|i| (i * 1000 - 9000) as i16).collect();
        let extras = WavExtras {
            cues: vec![0, 5, 9],
            loops: vec![WavLoop { start: 2, end: 8 }],
        };
        let bytes = pcm16_wav(32000, 2, &samples, &extras);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        let wav = Wav::parse(&bytes).unwrap();
        assert_eq!(
            (wav.sample_rate, wav.channels, wav.bits, wav.float),
            (32000, 2, 16, false)
        );
        assert_eq!(wav.samples_i16().unwrap(), samples);
        assert_eq!(wav.extras, extras);
    }

    #[test]
    fn float_wav_keeps_values_above_one() {
        let samples = [0.0f32, 1.5, -3.25, 0.125, 7.0];
        let bytes = f32_wav(48000, 1, &samples, &WavExtras::default());
        let wav = Wav::parse(&bytes).unwrap();
        assert!(wav.float);
        assert_eq!(wav.samples_f32().unwrap(), samples);
        assert_eq!(wav.samples_i16(), None);
    }

    #[test]
    fn odd_sized_chunks_are_padded() {
        let bytes = pcm16_wav(
            8000,
            1,
            &[1, 2, 3],
            &WavExtras {
                cues: vec![1],
                loops: vec![],
            },
        );
        assert_eq!(bytes.len() % 2, 0);
        assert!(Wav::parse(&bytes).is_ok());
        assert!(Wav::parse(&bytes[..20]).is_err());
        assert!(Wav::parse(b"not a wav file at all").is_err());
    }
}
