//! Bink audio: the DCT variant every track of the game's movies uses.
//!
//! A frame's packet for one track is a 4-byte count of the decoded bytes (16-bit samples), then whole blocks, each
//! starting on a 32-bit boundary. A block is two skipped bits, then each channel's spectrum ([`coeffs`]) one after
//! the other. Every spectrum goes through an inverse DCT ([`transform`]) to `frame_len` samples; the first `overlap`
//! of them are cross-faded with the last `overlap` of the channel's previous block, and the first
//! `frame_len - overlap` are the block's output. At 48 kHz that is 2048 and 128: 1920 samples a channel, 7680 bytes
//! of stereo. A movie's first frame carries nineteen blocks as a head start and most later frames one, five in
//! every six; the rest carry none.

mod coeffs;
mod transform;

use crate::bits::BitReader;
use crate::container::AudioTrack;
use crate::error::{Error, Result};

use coeffs::Layout;
use transform::Dct3;

pub struct AudioDecoder {
    channels: usize,
    layout: Layout,
    dct: Dct3,
    previous: Vec<Vec<f32>>,
    first: bool,
    spectrum: Vec<Vec<f32>>,
}

impl AudioDecoder {
    pub fn new(track: &AudioTrack) -> Result<AudioDecoder> {
        if !track.dct {
            return Err(Error::Audio("RDFT tracks are not decoded"));
        }
        let channels = usize::from(track.channels);
        if channels == 0 || channels > 2 || track.rate == 0 {
            return Err(Error::Audio("track layout"));
        }
        let layout = Layout::new(track.rate);
        Ok(AudioDecoder {
            channels,
            dct: Dct3::new(layout.frame_len),
            previous: vec![vec![0.0; layout.overlap]; channels],
            spectrum: vec![vec![0.0; layout.frame_len]; channels],
            layout,
            first: true,
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Samples per channel one block yields.
    pub fn block_samples(&self) -> usize {
        self.layout.frame_len - self.layout.overlap
    }

    /// Forgets the previous block, as after a seek: the next block is output without a cross-fade.
    pub fn reset(&mut self) {
        self.first = true;
    }

    /// Samples per channel a packet says it decodes to (the packet is the bytes after the packet's own size word).
    pub fn packet_samples(&self, packet: &[u8]) -> usize {
        packet
            .first_chunk::<4>()
            .map_or(0, |c| u32::from_le_bytes(*c) as usize / (2 * self.channels))
    }

    /// Decodes a packet, appending interleaved samples in `-1..1` to `out`. Returns how many samples per channel
    /// were added: the count the packet announces.
    pub fn decode_packet(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        let Some(body) = packet.get(4..) else {
            return Ok(0);
        };
        let wanted = self.packet_samples(packet);
        let block = self.block_samples();
        let mut reader = BitReader::new(body);
        let start = out.len();
        for _ in 0..wanted.div_ceil(block) {
            if reader.overrun() || reader.pos() >= reader.bit_len() {
                return Err(Error::Audio("packet ends before its announced samples"));
            }
            self.decode_block(&mut reader, out)?;
            reader.seek(reader.pos().next_multiple_of(32));
        }
        out.truncate(start + wanted * self.channels);
        Ok(wanted)
    }

    fn decode_block(&mut self, reader: &mut BitReader, out: &mut Vec<f32>) -> Result<()> {
        reader.skip(2);
        for spectrum in &mut self.spectrum {
            coeffs::read_channel(reader, &self.layout, spectrum)?;
            self.dct.inverse(spectrum);
        }
        let overlap = self.layout.overlap;
        let count = (overlap * self.channels) as f32;
        for (ch, (samples, previous)) in
            self.spectrum.iter_mut().zip(&mut self.previous).enumerate()
        {
            if !self.first {
                for i in 0..overlap {
                    let j = (i * self.channels + ch) as f32;
                    samples[i] = (previous[i] * (count - j) + samples[i] * j) / count;
                }
            }
            previous.copy_from_slice(&samples[self.layout.frame_len - overlap..]);
        }
        self.first = false;
        out.reserve(self.block_samples() * self.channels);
        for i in 0..self.block_samples() {
            for spectrum in &self.spectrum {
                out.push(spectrum[i].clamp(-1.0, 1.0));
            }
        }
        Ok(())
    }
}

/// Decodes each track's packet of one frame into `out[track]`, replacing what was there. A track that cannot be
/// decoded, or whose packet is damaged, yields silence of the length the packet announces; the number of such
/// tracks is returned.
pub(crate) fn decode_frame(
    decoders: &mut [Option<AudioDecoder>],
    packets: &[&[u8]],
    out: &mut [Vec<f32>],
) -> u32 {
    let mut failed = 0;
    for ((decoder, packet), samples) in decoders.iter_mut().zip(packets).zip(out.iter_mut()) {
        samples.clear();
        let Some(decoder) = decoder else {
            failed += 1;
            continue;
        };
        if decoder.decode_packet(packet, samples).is_err() {
            samples.clear();
            samples.resize(decoder.packet_samples(packet) * decoder.channels(), 0.0);
            decoder.reset();
            failed += 1;
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::coeffs::tests::BitWriter;
    use super::*;

    const QUANT: u32 = 70;

    fn track(channels: u16, dct: bool) -> AudioTrack {
        AudioTrack {
            rate: 48000,
            channels,
            id: 0,
            dct,
            max_decoded_bytes: 0,
        }
    }

    /// Amplitude of a coefficient `magnitude` at quantiser index `quant`, in output samples.
    fn amplitude(layout: &Layout, magnitude: i32, quant: u32) -> f64 {
        f64::from(magnitude)
            * (f64::from(quant) * 0.152_891_647_872_219_54).exp()
            * f64::from(layout.root)
    }

    /// The samples of a block whose only non-zero coefficient is `k` (2..10) with value `magnitude`.
    fn cosine(layout: &Layout, k: usize, magnitude: i32, quant: u32) -> Vec<f64> {
        let a = amplitude(layout, magnitude, quant);
        (0..layout.frame_len)
            .map(|n| a * (PI * (2 * n + 1) as f64 * k as f64 / (2 * layout.frame_len) as f64).cos())
            .collect()
    }

    /// A packet of blocks, each channel with one non-zero coefficient: `(k, magnitude)`; quantiser index `QUANT`.
    fn packet(layout: &Layout, blocks: &[&[(usize, i32)]], channels: usize) -> Vec<u8> {
        let block_bytes = (layout.frame_len - layout.overlap) * channels * 2;
        let mut w = BitWriter::default();
        w.put((blocks.len() * block_bytes) as u32, 32);
        for block in blocks {
            w.put(0, 2);
            for &(k, magnitude) in *block {
                let mut run = [0; 8];
                run[k - 2] = magnitude;
                w.channel(layout, &[QUANT], &run, 6);
            }
            w.align32();
        }
        w.bytes
    }

    fn near(a: f32, b: f64, what: &str) {
        assert!((f64::from(a) - b).abs() < 2e-5, "{what}: {a} vs {b}");
    }

    #[test]
    fn a_block_is_the_cosine_its_spectrum_describes() {
        let decoder = AudioDecoder::new(&track(1, true)).unwrap();
        let layout = decoder.layout.clone();
        let mut decoder = decoder;
        let mut out = Vec::new();
        let n = decoder
            .decode_packet(&packet(&layout, &[&[(5, 3)]], 1), &mut out)
            .unwrap();
        assert_eq!(n, 1920);
        assert_eq!(out.len(), 1920);
        let want = cosine(&layout, 5, 3, QUANT);
        for i in (0..1920).step_by(37) {
            near(out[i], want[i], "sample");
        }
        assert!(want[..1920].iter().any(|v| v.abs() > 1e-3), "not silence");
    }

    #[test]
    fn the_next_block_cross_fades_over_the_previous_tail_across_interleaved_indices() {
        let mut decoder = AudioDecoder::new(&track(2, true)).unwrap();
        let layout = decoder.layout.clone();
        let first: &[(usize, i32)] = &[(4, 6), (3, -5)];
        let second: &[(usize, i32)] = &[(7, -4), (2, 9)];
        let mut out = Vec::new();
        let n = decoder
            .decode_packet(&packet(&layout, &[first, second], 2), &mut out)
            .unwrap();
        assert_eq!(n, 3840);
        let a = [cosine(&layout, 4, 6, QUANT), cosine(&layout, 3, -5, QUANT)];
        let b = [cosine(&layout, 7, -4, QUANT), cosine(&layout, 2, 9, QUANT)];
        let count = (layout.overlap * 2) as f64;
        for i in [0usize, 1, 17, 64, 127] {
            for ch in 0..2 {
                let j = (i * 2 + ch) as f64;
                let want = (a[ch][1920 + i] * (count - j) + b[ch][i] * j) / count;
                near(out[(1920 + i) * 2 + ch], want, "blended");
            }
        }
        for i in [128usize, 700, 1919] {
            for ch in 0..2 {
                near(out[(1920 + i) * 2 + ch], b[ch][i], "after the overlap");
            }
        }
        for i in [0usize, 900] {
            for ch in 0..2 {
                near(out[i * 2 + ch], a[ch][i], "first block is unblended");
            }
        }
    }

    #[test]
    fn a_reset_drops_the_overlap_so_the_next_block_starts_clean() {
        let mut decoder = AudioDecoder::new(&track(1, true)).unwrap();
        let layout = decoder.layout.clone();
        let p = packet(&layout, &[&[(4, 6)]], 1);
        let mut first = Vec::new();
        decoder.decode_packet(&p, &mut first).unwrap();
        let mut blended = Vec::new();
        decoder.decode_packet(&p, &mut blended).unwrap();
        assert_ne!(first[..128], blended[..128]);
        decoder.reset();
        let mut clean = Vec::new();
        decoder.decode_packet(&p, &mut clean).unwrap();
        assert_eq!(first, clean);
    }

    #[test]
    fn the_count_decides_how_much_is_decoded_and_kept() {
        let mut decoder = AudioDecoder::new(&track(1, true)).unwrap();
        let layout = decoder.layout.clone();
        let mut p = packet(&layout, &[&[(5, 3)], &[(5, 3)]], 1);
        let mut out = Vec::new();
        assert_eq!(decoder.packet_samples(&p), 3840);
        p[..4].copy_from_slice(&(2 * 1000u32).to_le_bytes());
        assert_eq!(decoder.decode_packet(&p, &mut out).unwrap(), 1000);
        assert_eq!(out.len(), 1000);
        out.clear();
        assert_eq!(decoder.decode_packet(&[], &mut out).unwrap(), 0);
        assert_eq!(decoder.decode_packet(&[0, 0, 0, 0], &mut out).unwrap(), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn a_packet_that_ends_before_its_count_is_an_error() {
        let mut decoder = AudioDecoder::new(&track(1, true)).unwrap();
        let layout = decoder.layout.clone();
        let mut p = packet(&layout, &[&[(5, 3)]], 1);
        p[..4].copy_from_slice(&(2 * 3840u32).to_le_bytes());
        assert!(decoder.decode_packet(&p, &mut Vec::new()).is_err());
        let short = packet(&layout, &[&[(5, 3)]], 1);
        assert!(
            decoder
                .decode_packet(&short[..short.len() / 2], &mut Vec::new())
                .is_err()
        );
    }

    #[test]
    fn samples_stay_inside_full_scale() {
        let mut decoder = AudioDecoder::new(&track(1, true)).unwrap();
        let layout = decoder.layout.clone();
        let mut w = BitWriter::default();
        w.put(3840, 32);
        w.put(0, 2);
        w.float(31, 0x7f_ffff, false);
        w.float(0, 0, false);
        for _ in 0..layout.band_count() {
            w.put(0, 7);
        }
        w.zero_tail(2, layout.frame_len);
        w.align32();
        let mut out = Vec::new();
        decoder.decode_packet(&w.bytes, &mut out).unwrap();
        assert!(out.iter().all(|v| *v == 1.0), "a huge DC term clips");
    }

    #[test]
    fn only_dct_tracks_are_decoded_and_a_damaged_packet_becomes_silence() {
        assert!(AudioDecoder::new(&track(2, false)).is_err());
        let layout = Layout::new(48000);
        let good = packet(&layout, &[&[(5, 3)]], 1);
        let mut bad = good.clone();
        bad.truncate(40);
        let mut decoders = vec![
            AudioDecoder::new(&track(1, true)).ok(),
            AudioDecoder::new(&track(1, true)).ok(),
            None,
        ];
        let mut out = vec![vec![7.0], Vec::new(), Vec::new()];
        let failed = decode_frame(&mut decoders, &[&good, &bad, &good], &mut out);
        assert_eq!(failed, 2);
        assert_eq!(out[0].len(), 1920);
        assert!(out[0].iter().any(|v| *v != 0.0));
        assert_eq!(out[1], vec![0.0; 1920]);
        assert!(out[2].is_empty());
    }
}
