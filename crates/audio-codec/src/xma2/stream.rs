//! XMA2 packets: 2048-byte units holding frames that may cross packet boundaries, and the streaming decoder
//! that feeds packets in and hands PCM out.

use std::collections::VecDeque;

use super::frame::{FrameDecoder, FrameInfo, MAX_CHANNELS, SAMPLES_PER_FRAME};
use crate::{Error, Result};

pub const PACKET_SIZE: usize = 2048;
const HEADER_BITS: usize = 32;
const LENGTH_PREFIX_BITS: usize = 15;
/// Samples dropped from the start by default: none. The three real streams measured declare exactly
/// `frames * 512` samples, so the decoded length equals the declared one and the stream's own first samples
/// are silence.
pub const DEFAULT_LEAD_IN: usize = 0;
/// What FFmpeg drops: the first frame and 64 samples of decoder delay. Set it as `lead_in` to match its output.
pub const FFMPEG_LEAD_IN: usize = SAMPLES_PER_FRAME + 64;

/// The 32-bit header at the start of every packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    /// Frames that start in this packet.
    pub frame_count: u8,
    /// Bits after the header that still belong to the frame begun in an earlier packet.
    pub carry_bits: u16,
    pub metadata: u8,
    /// Packets of other streams between this packet and the next one of the same stream.
    pub skip_count: u8,
}

impl PacketHeader {
    pub fn parse(packet: &[u8]) -> Result<Self> {
        if packet.len() < 4 {
            return Err(Error::Truncated {
                what: "XMA2 packet header",
                offset: 0,
                need: 4,
                have: packet.len(),
            });
        }
        let w = u32::from_be_bytes([packet[0], packet[1], packet[2], packet[3]]);
        Ok(Self {
            frame_count: (w >> 26) as u8,
            carry_bits: ((w >> 11) & 0x7FFF) as u16,
            metadata: ((w >> 8) & 7) as u8,
            skip_count: (w & 0xFF) as u8,
        })
    }
}

fn bit_at(data: &[u8], bit: usize) -> bool {
    data[bit >> 3] & (0x80 >> (bit & 7)) != 0
}

fn read_bits(data: &[u8], bit: usize, n: usize) -> usize {
    (0..n).fold(0, |v, i| (v << 1) | usize::from(bit_at(data, bit + i)))
}

fn copy_bits(src: &[u8], from: usize, n: usize, dst: &mut Vec<u8>, dst_bits: &mut usize) {
    for i in 0..n {
        if (*dst_bits).is_multiple_of(8) {
            dst.push(0);
        }
        if bit_at(src, from + i) {
            let last = dst.len() - 1;
            dst[last] |= 0x80 >> (*dst_bits & 7);
        }
        *dst_bits += 1;
    }
}

/// Decoder state of one 1- or 2-channel XMA2 stream.
struct StreamDecoder {
    frames: FrameDecoder,
    channels: usize,
    reservoir: Vec<u8>,
    reservoir_bits: usize,
    next_packet: usize,
    queue: VecDeque<f32>,
    to_skip: usize,
    last_info: FrameInfo,
}

impl StreamDecoder {
    fn new(channels: usize, sample_rate: u32, first_packet: usize, lead_in: usize) -> Result<Self> {
        Ok(Self {
            frames: FrameDecoder::new(channels, sample_rate)?,
            channels,
            reservoir: Vec::new(),
            reservoir_bits: 0,
            next_packet: first_packet,
            queue: VecDeque::new(),
            to_skip: lead_in,
            last_info: FrameInfo::default(),
        })
    }

    fn clear_reservoir(&mut self) {
        self.reservoir.clear();
        self.reservoir_bits = 0;
    }

    fn emit(&mut self, samples_per_channel: &[&[f32]]) {
        let n = samples_per_channel[0].len();
        let skip = self.to_skip.min(n);
        self.to_skip -= skip;
        for i in skip..n {
            for ch in samples_per_channel {
                self.queue.push_back(ch[i]);
            }
        }
    }

    fn decode_frame_bits(&mut self, data: &[u8], bits: usize) -> Result<FrameInfo> {
        let info = self.frames.decode_frame(data, bits)?;
        self.last_info = info;
        let pcm: Vec<[f32; SAMPLES_PER_FRAME]> =
            (0..self.channels).map(|c| *self.frames.pcm(c)).collect();
        let refs: Vec<&[f32]> = pcm.iter().map(|p| &p[..]).collect();
        self.emit(&refs);
        Ok(info)
    }

    fn try_complete_reservoir(&mut self) -> Result<()> {
        if self.reservoir_bits < LENGTH_PREFIX_BITS {
            return Ok(());
        }
        let len = read_bits(&self.reservoir, 0, LENGTH_PREFIX_BITS);
        if len == 0 || self.reservoir_bits < len {
            return Ok(());
        }
        let data = std::mem::take(&mut self.reservoir);
        self.reservoir_bits = 0;
        self.decode_frame_bits(&data, len).map(|_| ())
    }

    fn process_packet(&mut self, packet: &[u8], index: usize) -> Result<()> {
        let hdr = PacketHeader::parse(packet)?;
        self.next_packet = index + 1 + usize::from(hdr.skip_count);
        let total = packet.len() * 8;
        let mut pos = HEADER_BITS;
        let carry = usize::from(hdr.carry_bits);
        let mut result = Ok(());

        if carry > 0 {
            let n = carry.min(total - pos);
            if self.reservoir_bits > 0 {
                copy_bits(
                    packet,
                    pos,
                    n,
                    &mut self.reservoir,
                    &mut self.reservoir_bits,
                );
                result = self.try_complete_reservoir();
            }
            pos += n;
        }
        // whatever is still pending was not completed by the carry bits
        self.clear_reservoir();
        result?;

        while pos + LENGTH_PREFIX_BITS < total {
            let len = read_bits(packet, pos, LENGTH_PREFIX_BITS);
            if len == 0 {
                break;
            }
            let rem = total - pos;
            if len > rem {
                copy_bits(
                    packet,
                    pos,
                    rem,
                    &mut self.reservoir,
                    &mut self.reservoir_bits,
                );
                break;
            }
            let mut frame = Vec::with_capacity(len.div_ceil(8));
            let mut bits = 0;
            copy_bits(packet, pos, len, &mut frame, &mut bits);
            pos += len;
            let info = self.decode_frame_bits(&frame, len)?;
            if !info.more_frames {
                break;
            }
        }
        Ok(())
    }
}

/// Parameters of an XMA2 stream as the containers give them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xma2Config {
    pub sample_rate: u32,
    pub channels: usize,
    /// Samples per channel the stream declares (after the lead-in); output stops there. `None` returns all.
    pub total_samples: Option<u64>,
    /// Samples dropped from the start, see [`DEFAULT_LEAD_IN`] and [`FFMPEG_LEAD_IN`].
    pub lead_in: usize,
}

impl Xma2Config {
    pub fn new(sample_rate: u32, channels: usize, total_samples: Option<u64>) -> Self {
        Self {
            sample_rate,
            channels,
            total_samples,
            lead_in: DEFAULT_LEAD_IN,
        }
    }

    /// Channels of each XMA2 stream: pairs, with a trailing mono stream for an odd count.
    pub fn stream_channels(&self) -> Vec<usize> {
        (0..self.channels.div_ceil(MAX_CHANNELS))
            .map(|s| (self.channels - s * MAX_CHANNELS).min(MAX_CHANNELS))
            .collect()
    }
}

/// Streaming XMA2 decoder: feed packets in file order with [`Xma2Decoder::push_packet`], pull PCM with
/// [`Xma2Decoder::pull_f32`] or [`Xma2Decoder::pull_i16`].
pub struct Xma2Decoder {
    config: Xma2Config,
    streams: Vec<StreamDecoder>,
    packet_index: usize,
    emitted: u64,
}

impl Xma2Decoder {
    pub fn new(config: Xma2Config) -> Result<Self> {
        if config.channels == 0 || config.channels > 16 {
            return Err(Error::Unsupported(format!(
                "{} XMA2 channels",
                config.channels
            )));
        }
        if config.sample_rate == 0 {
            return Err(Error::Invalid("XMA2 stream with a zero sample rate".into()));
        }
        let mut d = Self {
            config,
            streams: Vec::new(),
            packet_index: 0,
            emitted: 0,
        };
        d.reset()?;
        Ok(d)
    }

    pub fn config(&self) -> &Xma2Config {
        &self.config
    }

    pub fn channels(&self) -> usize {
        self.config.channels
    }

    /// Back to the start of the stream: the next packet pushed is packet 0.
    pub fn reset(&mut self) -> Result<()> {
        self.restart_at_packet(0)
    }

    /// Resume at `packet` (a packet boundary, for example from a seek table). Carry-over bits of that packet
    /// are dropped and the overlap state is empty, so the first frame is a priming frame like at stream start.
    /// Only a single-stream (mono or stereo) file can start anywhere but packet 0.
    pub fn restart_at_packet(&mut self, packet: usize) -> Result<()> {
        let layout = self.config.stream_channels();
        if packet != 0 && layout.len() > 1 {
            return Err(Error::Unsupported(
                "seeking inside a multi-stream XMA2 file".into(),
            ));
        }
        self.streams = layout
            .iter()
            .enumerate()
            .map(|(s, &ch)| {
                StreamDecoder::new(ch, self.config.sample_rate, packet + s, self.config.lead_in)
            })
            .collect::<Result<_>>()?;
        self.packet_index = packet;
        self.emitted = 0;
        Ok(())
    }

    /// Position the decoder so the output continues at `sample` (per channel, counted from the start of the
    /// stream) using a seek table of per-packet start samples (tag `0xa3`). Returns the packet index to push
    /// next. Decoding restarts one packet early so the overlap state is warm at `sample`.
    pub fn seek(&mut self, table: &[u32], sample: u64) -> Result<usize> {
        if table.is_empty() {
            return Err(Error::Invalid("empty seek table".into()));
        }
        let block = crate::awc::seek_block(table, sample.min(u64::from(u32::MAX)) as u32);
        let start = block.saturating_sub(1);
        self.restart_at_packet(start)?;
        let drop = sample.saturating_sub(u64::from(table[start])) as usize;
        for s in &mut self.streams {
            s.to_skip += drop;
        }
        self.emitted = sample;
        Ok(start)
    }

    /// Next packet index `push_packet` expects.
    pub fn packet_index(&self) -> usize {
        self.packet_index
    }

    /// Feed the next packet (2048 bytes) of the file. A packet no stream owns (skipped by the skip counts) is
    /// ignored. On `Err` the decoder stays usable: the rest of that packet is lost.
    pub fn push_packet(&mut self, packet: &[u8]) -> Result<()> {
        if packet.len() != PACKET_SIZE {
            return Err(Error::Invalid(format!(
                "XMA2 packets are {PACKET_SIZE} bytes, got {}",
                packet.len()
            )));
        }
        let index = self.packet_index;
        self.packet_index += 1;
        match self.streams.iter_mut().find(|s| s.next_packet == index) {
            Some(stream) => stream.process_packet(packet, index),
            None => Ok(()),
        }
    }

    /// Feed a whole run of packets (a multiple of 2048 bytes).
    pub fn push_data(&mut self, data: &[u8]) -> Result<()> {
        let mut first_error = None;
        for packet in data.chunks(PACKET_SIZE) {
            if packet.len() == PACKET_SIZE
                && let Err(e) = self.push_packet(packet)
            {
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Samples per channel that [`pull_f32`](Self::pull_f32) can return now.
    pub fn available(&self) -> usize {
        let ready = self
            .streams
            .iter()
            .map(|s| s.queue.len() / s.channels)
            .min()
            .unwrap_or(0);
        match self.config.total_samples {
            Some(total) => ready.min(total.saturating_sub(self.emitted) as usize),
            None => ready,
        }
    }

    /// Interleaved samples (`channels` per frame). Returns the number of frames written.
    pub fn pull_f32(&mut self, out: &mut [f32]) -> usize {
        let channels = self.config.channels;
        let n = self.available().min(out.len() / channels);
        for f in 0..n {
            let mut o = f * channels;
            for s in &mut self.streams {
                for _ in 0..s.channels {
                    out[o] = s.queue.pop_front().unwrap_or(0.0);
                    o += 1;
                }
            }
        }
        self.emitted += n as u64;
        n
    }

    pub fn pull_i16(&mut self, out: &mut [i16]) -> usize {
        let channels = self.config.channels;
        let mut tmp = vec![0f32; out.len()];
        let n = self.pull_f32(&mut tmp);
        for (o, &v) in out.iter_mut().zip(&tmp).take(n * channels) {
            *o = f32_to_i16(v);
        }
        n
    }

    /// Decode a complete stream held in memory.
    pub fn decode_all(config: Xma2Config, data: &[u8]) -> Result<Pcm> {
        let mut d = Self::new(config.clone())?;
        d.push_data(data)?;
        let frames = d.available();
        let mut samples = vec![0f32; frames * config.channels];
        d.pull_f32(&mut samples);
        Ok(Pcm {
            sample_rate: config.sample_rate,
            channels: config.channels,
            samples,
        })
    }
}

/// Decoded interleaved PCM.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn to_i16(&self) -> Vec<i16> {
        self.samples.iter().map(|&v| f32_to_i16(v)).collect()
    }
}

pub fn f32_to_i16(v: f32) -> i16 {
    (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}
