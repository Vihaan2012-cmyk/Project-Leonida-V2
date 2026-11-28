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
