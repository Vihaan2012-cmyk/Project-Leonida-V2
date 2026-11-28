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
use crate::container::{AudioTrack, Container};
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

/// The audio of a whole movie: one decoder per track and the samples of the frame decoded last.
pub(crate) struct Audio {
    decoders: Vec<Option<AudioDecoder>>,
    /// Per track: samples from before the frame a seek lands on that still belong after it.
    preroll: Vec<Vec<f32>>,
    frame: Vec<Vec<f32>>,
    failures: u32,
}

impl Audio {
    pub fn new(tracks: &[AudioTrack]) -> Audio {
        Audio {
            decoders: tracks.iter().map(|t| AudioDecoder::new(t).ok()).collect(),
            preroll: vec![Vec::new(); tracks.len()],
            frame: vec![Vec::new(); tracks.len()],
            failures: 0,
        }
    }

    /// How many track packets were replaced by silence because they could not be decoded.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// The samples of the frame decoded last for `track`, handed over once.
    pub fn take(&mut self, track: usize) -> Vec<f32> {
        self.frame
            .get_mut(track)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Decodes each track's packet of the next frame; a track that cannot be decoded, or whose packet is damaged,
    /// yields silence of the length the packet announces.
    pub fn decode_frame(&mut self, packets: &[&[u8]]) {
        let tracks = self
            .decoders
            .iter_mut()
            .zip(packets)
            .zip(self.frame.iter_mut());
        for (t, ((decoder, packet), samples)) in tracks.enumerate() {
            samples.clear();
            let Some(decoder) = decoder else {
                self.failures += 1;
                continue;
            };
            if decoder.decode_packet(packet, samples).is_err() {
                samples.clear();
                samples.resize(decoder.packet_samples(packet) * decoder.channels(), 0.0);
                decoder.reset();
                self.failures += 1;
            }
            if !self.preroll[t].is_empty() {
                let mut joined = std::mem::take(&mut self.preroll[t]);
                joined.append(samples);
                *samples = joined;
            }
        }
    }

    /// Positions the audio for decoding to continue at frame `next`. The packets run ahead of the pictures (a
    /// movie's first frame carries the first 0.7 s of sound), so the sound that belongs at `next` is in the
    /// packets of earlier frames: it is decoded from there and kept to go out with the audio of `next`.
    pub fn seek(&mut self, container: &Container, next: u32) {
        let header = &container.header;
        for (t, decoder) in self.decoders.iter_mut().enumerate() {
            self.preroll[t].clear();
            self.frame[t].clear();
            let Some(decoder) = decoder else {
                continue;
            };
            decoder.reset();
            let rate = u64::from(header.audio[t].rate);
            let target =
                u64::from(next) * u64::from(header.fps_den) * rate / u64::from(header.fps_num);
            let counts: Vec<u64> = (0..next)
                .map(|f| {
                    container
                        .packets(f)
                        .ok()
                        .and_then(|p| p.audio.get(t).map(|a| decoder.packet_samples(a) as u64))
                        .unwrap_or(0)
                })
                .collect();
            let mut before = 0;
            let mut containing = None;
            for (f, count) in counts.iter().enumerate() {
                if before + count > target {
                    containing = Some(f);
                    break;
                }
                before += count;
            }
            let Some(containing) = containing else {
                continue;
            };
            let first = counts[..containing]
                .iter()
                .rposition(|c| *c > 0)
                .unwrap_or(containing);
            let first_before: u64 = counts[..first].iter().sum();
            let channels = decoder.channels();
            let mut samples = Vec::new();
            for (f, count) in counts.iter().enumerate().skip(first) {
                let packet = container
                    .packets(f as u32)
                    .ok()
                    .and_then(|p| p.audio.get(t).copied());
                let Some(packet) = packet else {
                    continue;
                };
                if decoder.decode_packet(packet, &mut samples).is_err() {
                    let missing = *count as usize * channels;
                    samples.resize(samples.len() + missing, 0.0);
                    decoder.reset();
                    self.failures += 1;
                }
            }
            let skip = ((target - first_before) as usize * channels).min(samples.len());
            samples.drain(..skip);
            self.preroll[t] = samples;
        }
    }
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
        let mut audio = Audio::new(&[track(1, true), track(1, true), track(2, false)]);
        audio.decode_frame(&[&good, &bad, &good]);
        assert_eq!(audio.failures(), 2);
        let first = audio.take(0);
        assert_eq!(first.len(), 1920);
        assert!(first.iter().any(|v| *v != 0.0));
        assert_eq!(audio.take(1), vec![0.0; 1920]);
        assert!(audio.take(2).is_empty());
        assert!(audio.take(0).is_empty(), "handed over once");
    }

    /// A stream of `frames` frames at 30 fps with one mono 48 kHz DCT track and no picture data.
    fn movie(frame_packets: &[Vec<u8>]) -> Container {
        let frames = frame_packets.len();
        let index = 0x3C;
        let mut bytes = vec![0u8; index + 4 * frames];
        bytes[..4].copy_from_slice(b"KB2n");
        let put = |bytes: &mut Vec<u8>, at: usize, v: u32| {
            bytes[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        put(&mut bytes, 8, frames as u32);
        put(&mut bytes, 0x10, frames as u32);
        put(&mut bytes, 0x14, 16);
        put(&mut bytes, 0x18, 16);
        put(&mut bytes, 0x1C, 30);
        put(&mut bytes, 0x20, 1);
        put(&mut bytes, 0x28, 1);
        put(&mut bytes, 0x34, 48000 | 0x1000 << 16);
        for (f, packet) in frame_packets.iter().enumerate() {
            let at = bytes.len() as u32;
            put(&mut bytes, index + 4 * f, at | u32::from(f == 0));
            bytes.extend_from_slice(&(packet.len() as u32).to_le_bytes());
            bytes.extend_from_slice(packet);
        }
        Container::parse(bytes.into()).unwrap()
    }

    fn sounding_movie(frames: usize) -> Container {
        let layout = Layout::new(48000);
        let packets: Vec<Vec<u8>> = (0..frames)
            .map(|f| packet(&layout, &[&[(3 + f % 5, 1 + (f % 7) as i32)]], 1))
            .collect();
        movie(&packets)
    }

    fn everything(container: &Container) -> Vec<f32> {
        let mut decoder = AudioDecoder::new(&container.header.audio[0]).unwrap();
        let mut all = Vec::new();
        for f in 0..container.header.frames {
            decoder
                .decode_packet(container.packets(f).unwrap().audio[0], &mut all)
                .unwrap();
        }
        all
    }

    #[test]
    fn after_a_seek_the_sound_for_the_frame_comes_from_the_packets_before_it() {
        let container = sounding_movie(40);
        let reference = everything(&container);
        for next in [3usize, 12, 30, 39] {
            let mut audio = Audio::new(&container.header.audio);
            audio.seek(&container, next as u32);
            let packets = container.packets(next as u32).unwrap();
            audio.decode_frame(&packets.audio);
            let got = audio.take(0);
            let start = next * 1600;
            let end = 1920 * (next + 1);
            assert_eq!(got.len(), end - start, "frame {next}");
            assert_eq!(got[..], reference[start..end], "frame {next}");
            assert_eq!(audio.failures(), 0);
        }
    }

    #[test]
    fn seeking_to_the_start_keeps_nothing_and_the_first_frame_is_decoded_whole() {
        let container = sounding_movie(8);
        let reference = everything(&container);
        let mut audio = Audio::new(&container.header.audio);
        audio.seek(&container, 0);
        audio.decode_frame(&container.packets(0).unwrap().audio);
        assert_eq!(audio.take(0), reference[..1920]);
    }
}
