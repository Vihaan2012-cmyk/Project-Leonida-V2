//! One WMA Pro style frame of an XMA2 stream: 512 samples per channel, one or two channels.
//!
//! XMA2 is the WMA Pro frame coder with fixed parameters (512 samples per frame, up to four subframes of 128
//! to 512 samples, a 15-bit frame length prefix, 16-bit output scale) and a packet layer in `packet.rs`.

use std::f64::consts::PI;
use std::sync::OnceLock;

use super::tables::*;
use crate::bits::BitReader;
use crate::huffman::Vlc;
use crate::mdct::Imdct;
use crate::{Error, Result};

pub const SAMPLES_PER_FRAME: usize = 512;
pub const MAX_CHANNELS: usize = 2;
const MIN_SUBFRAME: usize = 128;
const MAX_SUBFRAMES: usize = 4;
const NUM_SIZES: usize = 3;
const MAX_BANDS: usize = 29;
const LOG2_FRAME_SIZE: u32 = 15;
const OUT_LEN: usize = SAMPLES_PER_FRAME + SAMPLES_PER_FRAME / 2;
const BASE_QUANT_STEP: i32 = 90;

struct Tables {
    sf: Vlc,
    sf_rl: Vlc,
    coef: [Vlc; 2],
    vec4: Vlc,
    vec2: Vlc,
    vec1: Vlc,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let pairs = |t: &[(u8, u8)], off: i32| {
            Vlc::from_entries(t.iter().map(|&(s, l)| (i32::from(s) + off, l))).expect("table")
        };
        Tables {
            sf: pairs(&SCALE_TABLE, -60),
            sf_rl: pairs(&SCALE_RL_TABLE, 0),
            coef: [
                Vlc::from_entries(
                    COEF0_SYMS
                        .iter()
                        .zip(COEF0_LENS.iter())
                        .map(|(&s, &l)| (i32::from(s), l)),
                )
                .expect("coef0"),
                pairs(&COEF1_TABLE, 0),
            ],
            vec4: Vlc::from_entries(
                VEC4_SYMS
                    .iter()
                    .zip(VEC4_LENS.iter())
                    .map(|(&s, &l)| (i32::from(s) - 1, l)),
            )
            .expect("vec4"),
            vec2: pairs(&VEC2_TABLE, -1),
            vec1: pairs(&VEC1_TABLE, 0),
        }
    })
}

/// Maps a stream's sample rate to the band layout the encoder used (24, 32, 44.1 or 48 kHz).
pub fn band_rate_class(sample_rate: u32) -> u32 {
    [24000u32, 32000, 44100, 48000]
        .into_iter()
        .min_by_key(|&r| r.abs_diff(sample_rate))
        .unwrap_or(48000)
}

#[derive(Clone)]
struct Channel {
    prev_block_len: usize,
    transmit_coefs: bool,
    num_subframes: usize,
    subframe_len: [usize; MAX_SUBFRAMES],
    cur_subframe: usize,
    decoded_samples: usize,
    grouped: bool,
    quant_step: i32,
    reuse_sf: bool,
    scale_factor_step: i32,
    max_scale_factor: i32,
    saved_sf: [[i32; MAX_BANDS]; 2],
    sf_idx: usize,
    sf_cur: usize,
    table_idx: usize,
    coefs_at: usize,
    num_vec_coeffs: usize,
    out: Vec<f32>,
}

impl Channel {
    fn new() -> Self {
        Self {
            prev_block_len: SAMPLES_PER_FRAME,
            transmit_coefs: false,
            num_subframes: 0,
            subframe_len: [0; MAX_SUBFRAMES],
            cur_subframe: 0,
            decoded_samples: 0,
            grouped: false,
            quant_step: 0,
            reuse_sf: false,
            scale_factor_step: 1,
            max_scale_factor: 0,
            saved_sf: [[0; MAX_BANDS]; 2],
            sf_idx: 0,
            sf_cur: 0,
            table_idx: 0,
            coefs_at: 0,
            num_vec_coeffs: 0,
            out: vec![0.0; OUT_LEN],
        }
    }
}

#[derive(Clone, Copy, Default)]
struct ChannelGroup {
    num_channels: usize,
    transform: bool,
    transform_band: [bool; MAX_BANDS],
    members: [usize; MAX_CHANNELS],
}

/// What a decoded frame reports besides its samples.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameInfo {
    /// The frame's trailer bit: another frame follows in the same packet.
    pub more_frames: bool,
    pub trim_start: u16,
    pub trim_end: u16,
}

pub struct FrameDecoder {
    nb_channels: usize,
    num_sfb: [usize; NUM_SIZES],
    sfb_offsets: [[usize; MAX_BANDS]; NUM_SIZES],
    sf_offsets: [[[usize; MAX_BANDS]; NUM_SIZES]; NUM_SIZES],
    imdct: [Imdct; NUM_SIZES],
    windows: [Vec<f32>; NUM_SIZES],
    channels: Vec<Channel>,
    tmp: Vec<f32>,
    groups: Vec<ChannelGroup>,
    // current subframe state
    subframe_len: usize,
    members: [usize; MAX_CHANNELS],
    member_count: usize,
    num_bands: usize,
    table_idx: usize,
    transmit_num_vec_coeffs: bool,
    esc_len: u32,
    parsed_all: bool,
    pcm: [[f32; SAMPLES_PER_FRAME]; MAX_CHANNELS],
}

fn log2_floor(v: usize) -> u32 {
    usize::BITS - 1 - v.leading_zeros()
}

impl FrameDecoder {
    pub fn new(nb_channels: usize, sample_rate: u32) -> Result<Self> {
        if nb_channels == 0 || nb_channels > MAX_CHANNELS {
            return Err(Error::Unsupported(format!(
                "{nb_channels} channels in one XMA2 stream"
            )));
        }
        let rate = band_rate_class(sample_rate) as usize;
        let mut num_sfb = [0; NUM_SIZES];
        let mut sfb_offsets = [[0usize; MAX_BANDS]; NUM_SIZES];
        for i in 0..NUM_SIZES {
            let subframe_len = SAMPLES_PER_FRAME >> i;
            let mut band = 1;
            for &freq in CRITICAL_FREQ.iter().take(MAX_BANDS - 1) {
                if sfb_offsets[i][band - 1] >= subframe_len {
                    break;
                }
                let offset = ((subframe_len * 2 * usize::from(freq)) / rate + 2) & !3;
                if offset > sfb_offsets[i][band - 1] {
                    sfb_offsets[i][band] = offset;
                    band += 1;
                }
                if offset >= subframe_len {
                    break;
                }
            }
            sfb_offsets[i][band - 1] = subframe_len;
            num_sfb[i] = band - 1;
        }
        let mut sf_offsets = [[[0usize; MAX_BANDS]; NUM_SIZES]; NUM_SIZES];
        for i in 0..NUM_SIZES {
            for b in 0..num_sfb[i] {
                let offset = ((sfb_offsets[i][b] + sfb_offsets[i][b + 1] - 1) << i) >> 1;
                for x in 0..NUM_SIZES {
                    let mut v = 0;
                    while (sfb_offsets[x][v + 1] << x) < offset {
                        v += 1;
                        if v + 1 >= MAX_BANDS {
                            return Err(Error::Invalid("scale factor band table overflow".into()));
                        }
                    }
                    sf_offsets[i][x][b] = v;
                }
            }
        }
        let imdct = std::array::from_fn(|i| {
            let n = SAMPLES_PER_FRAME >> i;
            Imdct::new(n, (2.0 / n as f64 / 32768.0) as f32)
        });
        let windows = std::array::from_fn(|i| {
            let n = SAMPLES_PER_FRAME >> i;
            (0..n)
                .map(|k| ((k as f64 + 0.5) / (2.0 * n as f64) * PI).sin() as f32)
                .collect()
        });
        Ok(Self {
            nb_channels,
            num_sfb,
            sfb_offsets,
            sf_offsets,
            imdct,
            windows,
            channels: vec![Channel::new(); nb_channels],
            tmp: vec![0.0; SAMPLES_PER_FRAME],
            groups: Vec::new(),
            subframe_len: 0,
            members: [0; MAX_CHANNELS],
            member_count: 0,
            num_bands: 0,
            table_idx: 0,
            transmit_num_vec_coeffs: false,
            esc_len: 0,
            parsed_all: false,
            pcm: [[0.0; SAMPLES_PER_FRAME]; MAX_CHANNELS],
        })
    }

    pub fn channels(&self) -> usize {
        self.nb_channels
    }

    /// Forget the overlap state (after a seek or at stream start).
    pub fn reset(&mut self) {
        for c in &mut self.channels {
            c.out.iter_mut().for_each(|v| *v = 0.0);
            c.prev_block_len = SAMPLES_PER_FRAME;
        }
    }

    /// Samples of the last decoded frame, one array per channel.
    pub fn pcm(&self, channel: usize) -> &[f32; SAMPLES_PER_FRAME] {
        &self.pcm[channel]
    }

    /// The overlap tail the next frame would have been added to; the last 256 samples a stream still owes.
    pub fn flush_tail(&self, channel: usize) -> [f32; SAMPLES_PER_FRAME] {
        let mut out = [0.0; SAMPLES_PER_FRAME];
        out[..SAMPLES_PER_FRAME / 2].copy_from_slice(&self.channels[channel].out[..SAMPLES_PER_FRAME / 2]);
        out
    }

    fn decode_subframe_length(&self, br: &mut BitReader<'_>, offset: usize) -> Result<usize> {
        if offset == SAMPLES_PER_FRAME - MIN_SUBFRAME {
            return Ok(MIN_SUBFRAME);
        }
        if br.bits_left() < 1 {
            return Err(Error::Corrupt("frame ends inside the tile header"));
        }
        // four subframes max: the "maximum length" bit, then a 1-bit shift
        let shift = if br.read_bit() { 1 + br.read(1) as usize } else { 0 };
        Ok(SAMPLES_PER_FRAME >> shift)
    }

    fn decode_tile_header(&mut self, br: &mut BitReader<'_>) -> Result<()> {
        let nb = self.nb_channels;
        let mut num_samples = [0usize; MAX_CHANNELS];
        let mut contains = [false; MAX_CHANNELS];
        let mut channels_for_cur = nb;
        let mut min_channel_len = 0usize;
        for c in &mut self.channels {
            c.num_subframes = 0;
        }
        let fixed_layout = br.read_bit();
        loop {
            for c in 0..nb {
                contains[c] = if num_samples[c] == min_channel_len {
                    if fixed_layout
                        || channels_for_cur == 1
                        || min_channel_len == SAMPLES_PER_FRAME - MIN_SUBFRAME
                    {
                        true
                    } else {
                        br.read_bit()
                    }
                } else {
                    false
                };
            }
            let len = self.decode_subframe_length(br, min_channel_len)?;
            min_channel_len += len;
            for c in 0..nb {
                if contains[c] {
                    let ch = &mut self.channels[c];
                    if ch.num_subframes >= MAX_SUBFRAMES {
                        return Err(Error::Corrupt("too many subframes"));
                    }
                    ch.subframe_len[ch.num_subframes] = len;
                    ch.num_subframes += 1;
                    num_samples[c] += len;
                    if num_samples[c] > SAMPLES_PER_FRAME {
                        return Err(Error::Corrupt("channel longer than the frame"));
                    }
                } else if num_samples[c] <= min_channel_len {
                    if num_samples[c] < min_channel_len {
                        channels_for_cur = 0;
                        min_channel_len = num_samples[c];
                    }
                    channels_for_cur += 1;
                }
            }
            if min_channel_len >= SAMPLES_PER_FRAME {
                return Ok(());
            }
        }
    }

    fn decode_channel_transform(&mut self, br: &mut BitReader<'_>) -> Result<()> {
        self.groups.clear();
        if self.nb_channels < 2 {
            return Ok(());
        }
        let mut remaining = self.member_count;
        if br.read_bit() {
            return Err(Error::Unsupported("channel transform bit".into()));
        }
        while remaining > 0 && self.groups.len() < self.member_count {
            let mut g = ChannelGroup::default();
            if remaining > 2 {
                return Err(Error::Unsupported("channel groups of more than 2 channels".into()));
            }
            g.num_channels = remaining;
            let mut n = 0;
            for i in 0..self.member_count {
                let c = self.members[i];
                if !self.channels[c].grouped {
                    g.members[n] = c;
                    n += 1;
                }
                self.channels[c].grouped = true;
            }
            if g.num_channels == 2 {
                if br.read_bit() {
                    if br.read_bit() {
                        return Err(Error::Unsupported("channel transform type".into()));
                    }
                } else {
                    g.transform = true;
                }
            }
            if g.transform {
                if !br.read_bit() {
                    for b in 0..self.num_bands {
                        g.transform_band[b] = br.read_bit();
                    }
                } else {
                    g.transform_band[..self.num_bands].fill(true);
                }
            }
            remaining -= g.num_channels;
            self.groups.push(g);
        }
        Ok(())
    }

    fn decode_scale_factors(&mut self, br: &mut BitReader<'_>) -> Result<()> {
        let t = tables();
        for mi in 0..self.member_count {
            let c = self.members[mi];
            let num_bands = self.num_bands;
            let cur_table = self.table_idx;
            let ch = &mut self.channels[c];
            ch.sf_cur = 1 - ch.sf_idx;
            if ch.reuse_sf {
                let offs = &self.sf_offsets[cur_table][ch.table_idx];
                let (a, b) = (ch.sf_idx, ch.sf_cur);
                for band in 0..num_bands {
                    ch.saved_sf[b][band] = ch.saved_sf[a][offs[band]];
                }
            }
            if ch.cur_subframe == 0 || br.read_bit() {
                if !ch.reuse_sf {
                    ch.scale_factor_step = br.read(2) as i32 + 1;
                    let mut val = 45 / ch.scale_factor_step;
                    for band in 0..num_bands {
                        let d = t.sf.decode(br).ok_or(Error::Corrupt("scale factor code"))?;
                        val += d;
                        ch.saved_sf[ch.sf_cur][band] = val;
                    }
                } else {
                    let mut i = 0;
                    while i < num_bands {
                        let idx = t.sf_rl.decode(br).ok_or(Error::Corrupt("scale factor run code"))?;
                        let (skip, val, sign);
                        if idx == 0 {
                            let code = br.read(14) as i32;
                            val = code >> 6;
                            sign = (code & 1) - 1;
                            skip = (code & 0x3f) >> 1;
                        } else if idx == 1 {
                            break;
                        } else {
                            skip = i32::from(SCALE_RL_RUN[idx as usize]);
                            val = i32::from(SCALE_RL_LEVEL[idx as usize]);
                            sign = i32::from(br.read_bit()) - 1;
                        }
                        i += skip as usize;
                        if i >= num_bands {
                            return Err(Error::Corrupt("scale factor run past the last band"));
                        }
                        ch.saved_sf[ch.sf_cur][i] += (val ^ sign) - sign;
                        i += 1;
                    }
                }
                ch.sf_idx = 1 - ch.sf_idx;
                ch.table_idx = cur_table;
                ch.reuse_sf = true;
            }
            ch.max_scale_factor = ch.saved_sf[ch.sf_cur][..num_bands]
                .iter()
                .copied()
                .max()
                .unwrap_or(0);
        }
        Ok(())
    }

    fn read_large_value(br: &mut BitReader<'_>) -> u32 {
        let mut n = 8;
        if br.read_bit() {
            n += 8;
            if br.read_bit() {
                n += 8;
                if br.read_bit() {
                    n += 7;
                }
            }
        }
        br.read(n)
    }

    fn decode_coeffs(&mut self, br: &mut BitReader<'_>, c: usize) -> Result<()> {
        let t = tables();
        let subframe_len = self.subframe_len;
        let transmit_num_vec = self.transmit_num_vec_coeffs;
        let esc_len = self.esc_len;
        let ch = &mut self.channels[c];
        let at = ch.coefs_at;
        let num_vec = ch.num_vec_coeffs;
        let coefs = &mut ch.out[at..at + subframe_len];

        let table = usize::from(br.read_bit());
        let (run, level): (&[u16], &[u16]) = if table == 1 {
            (&COEF1_RUN, &COEF1_LEVEL)
        } else {
            (&COEF0_RUN, &COEF0_LEVEL)
        };
        let mut rl_mode = false;
        let mut cur = 0usize;
        let mut num_zeros = 0usize;
        while (transmit_num_vec || !rl_mode) && cur + 3 < num_vec {
            let mut vals = [0f32; 4];
            let idx = t.vec4.decode(br).ok_or(Error::Corrupt("vector code"))?;
            if idx < 0 {
                for i in (0..4).step_by(2) {
                    let idx2 = t.vec2.decode(br).ok_or(Error::Corrupt("vector pair code"))?;
                    if idx2 < 0 {
                        let mut pair = [0f32; 2];
                        for p in &mut pair {
                            let mut v = t.vec1.decode(br).ok_or(Error::Corrupt("vector single code"))? as u32;
                            if v as usize == VEC1_TABLE.len() - 1 {
                                v += Self::read_large_value(br);
                            }
                            *p = v as f32;
                        }
                        vals[i] = pair[0];
                        vals[i + 1] = pair[1];
                    } else {
                        vals[i] = (idx2 >> 4) as f32;
                        vals[i + 1] = (idx2 & 0xF) as f32;
                    }
                }
            } else {
                vals[0] = (idx >> 12) as f32;
                vals[1] = ((idx >> 8) & 0xF) as f32;
                vals[2] = ((idx >> 4) & 0xF) as f32;
                vals[3] = (idx & 0xF) as f32;
            }
            for v in vals {
                if v != 0.0 {
                    coefs[cur] = if br.read_bit() { -v } else { v };
                    num_zeros = 0;
                } else {
                    coefs[cur] = 0.0;
                    num_zeros += 1;
                    rl_mode |= num_zeros > subframe_len >> 8;
                }
                cur += 1;
            }
        }

        if cur < subframe_len {
            coefs[cur..].fill(0.0);
            let mask = subframe_len - 1;
            let mut offset = cur;
            while offset < subframe_len {
                let code = t.coef[table].decode(br).ok_or(Error::Corrupt("run-level code"))?;
                if code > 1 {
                    let code = code as usize;
                    offset += usize::from(run[code]);
                    let v = f32::from(level[code]);
                    coefs[offset & mask] = if br.read_bit() { -v } else { v };
                } else if code == 1 {
                    break;
                } else {
                    let lvl = Self::read_large_value(br) as f32;
                    if br.read_bit() {
                        if br.read_bit() {
                            if br.read_bit() {
                                return Err(Error::Corrupt("broken escape sequence"));
                            }
                            offset += br.read(esc_len) as usize + 4;
                        } else {
                            offset += br.read(2) as usize + 1;
                        }
                    }
                    coefs[offset & mask] = if br.read_bit() { -lvl } else { lvl };
                }
                offset += 1;
            }
            if offset > subframe_len {
                return Err(Error::Corrupt("run-level data overflows the subframe"));
            }
        }
        Ok(())
    }

    fn inverse_channel_transform(&mut self) {
        for gi in 0..self.groups.len() {
            let g = self.groups[gi];
            if !g.transform {
                continue;
            }
            for b in 0..self.num_bands {
                let start = self.sfb_offsets[self.table_idx][b];
                let end = self.sfb_offsets[self.table_idx][b + 1].min(self.subframe_len);
                let (c0, c1) = (g.members[0], g.members[1]);
                let (o0, o1) = (self.channels[c0].coefs_at, self.channels[c1].coefs_at);
                if g.transform_band[b] {
                    for y in start..end {
                        let a = self.channels[c0].out[o0 + y];
                        let d = self.channels[c1].out[o1 + y];
                        self.channels[c0].out[o0 + y] = a - d;
                        self.channels[c1].out[o1 + y] = a + d;
                    }
                } else if self.nb_channels == 2 {
                    for y in start..end {
                        self.channels[c0].out[o0 + y] *= 181.0 / 128.0;
                        self.channels[c1].out[o1 + y] *= 181.0 / 128.0;
                    }
                }
            }
        }
    }

    fn window(&mut self) {
        for mi in 0..self.member_count {
            let c = self.members[mi];
            let subframe_len = self.subframe_len;
            let ch = &mut self.channels[c];
            let mut winlen = ch.prev_block_len;
            let mut start = ch.coefs_at - winlen / 2;
            if subframe_len < winlen {
                start += (winlen - subframe_len) / 2;
                winlen = subframe_len;
            }
            let window = &self.windows[SAMPLES_PER_FRAME.trailing_zeros() as usize
                - winlen.trailing_zeros() as usize];
            let len = winlen / 2;
            for a in 0..len {
                let s0 = ch.out[start + a];
                let s1 = ch.out[start + 2 * len - 1 - a];
                let wi = window[a];
                let wj = window[2 * len - 1 - a];
                ch.out[start + a] = s0 * wj - s1 * wi;
                ch.out[start + 2 * len - 1 - a] = s0 * wi + s1 * wj;
            }
            ch.prev_block_len = subframe_len;
        }
    }

    fn decode_subframe(&mut self, br: &mut BitReader<'_>) -> Result<()> {
        let nb = self.nb_channels;
        let mut offset = SAMPLES_PER_FRAME;
        let mut subframe_len = SAMPLES_PER_FRAME;
        let mut total_samples = SAMPLES_PER_FRAME * nb;

        for c in &mut self.channels {
            c.grouped = false;
            if offset > c.decoded_samples {
                if c.cur_subframe >= c.num_subframes {
                    return Err(Error::Corrupt("subframe index past the tile layout"));
                }
                offset = c.decoded_samples;
                subframe_len = c.subframe_len[c.cur_subframe];
            }
        }

        self.member_count = 0;
        for i in 0..nb {
            let c = &mut self.channels[i];
            total_samples -= c.decoded_samples;
            if c.cur_subframe < c.num_subframes
                && offset == c.decoded_samples
                && subframe_len == c.subframe_len[c.cur_subframe]
            {
                total_samples -= subframe_len;
                c.decoded_samples += subframe_len;
                self.members[self.member_count] = i;
                self.member_count += 1;
            }
        }
        if total_samples == 0 {
            self.parsed_all = true;
        }

        self.table_idx = (SAMPLES_PER_FRAME / subframe_len).trailing_zeros() as usize;
        self.num_bands = self.num_sfb[self.table_idx];
        offset += SAMPLES_PER_FRAME / 2;
        for mi in 0..self.member_count {
            self.channels[self.members[mi]].coefs_at = offset;
        }
        self.subframe_len = subframe_len;
        self.esc_len = log2_floor(subframe_len - 1) + 1;

        if br.read_bit() {
            let mut fill = br.read(2) as usize;
            if fill == 0 {
                let len = br.read(4);
                fill = br.read(len) as usize + 1;
            }
            if br.pos() + fill > br.bit_len() {
                return Err(Error::Corrupt("fill bits past the end of the frame"));
            }
            br.skip(fill);
        }
        if br.read_bit() {
            return Err(Error::Unsupported("reserved subframe bit".into()));
        }

        self.decode_channel_transform(br)?;

        let mut transmit_coeffs = false;
        for mi in 0..self.member_count {
            let c = self.members[mi];
            self.channels[c].transmit_coefs = br.read_bit();
            transmit_coeffs |= self.channels[c].transmit_coefs;
        }

        if transmit_coeffs {
            let mut quant_step = BASE_QUANT_STEP;
            self.transmit_num_vec_coeffs = br.read_bit();
            if self.transmit_num_vec_coeffs {
                let num_bits = log2_floor(subframe_len.div_ceil(4)) + 1;
                for mi in 0..self.member_count {
                    let n = (br.read(num_bits) as usize) << 2;
                    if n > subframe_len {
                        return Err(Error::Corrupt("vector coefficient count above the subframe"));
                    }
                    self.channels[self.members[mi]].num_vec_coeffs = n;
                }
            } else {
                for mi in 0..self.member_count {
                    self.channels[self.members[mi]].num_vec_coeffs = subframe_len;
                }
            }
            let mut step = br.read_signed(6);
            quant_step += step;
            if step == -32 || step == 31 {
                let sign = i32::from(step == 31) - 1;
                let mut quant = 0;
                while br.pos() + 5 < br.bit_len() {
                    step = br.read(5) as i32;
                    if step != 31 {
                        break;
                    }
                    quant += 31;
                }
                quant_step += ((quant + step) ^ sign) - sign;
            }
            if self.member_count == 1 {
                self.channels[self.members[0]].quant_step = quant_step;
            } else {
                let modifier_len = br.read(3);
                for mi in 0..self.member_count {
                    let c = self.members[mi];
                    self.channels[c].quant_step = quant_step;
                    if br.read_bit() {
                        if modifier_len > 0 {
                            self.channels[c].quant_step += br.read(modifier_len) as i32 + 1;
                        } else {
                            self.channels[c].quant_step += 1;
                        }
                    }
                }
            }
            self.decode_scale_factors(br)?;
        }

        for mi in 0..self.member_count {
            let c = self.members[mi];
            let at = self.channels[c].coefs_at;
            if self.channels[c].transmit_coefs && (br.pos() as isize) < br.bit_len() as isize {
                self.decode_coeffs(br, c)?;
            } else {
                self.channels[c].out[at..at + subframe_len].fill(0.0);
            }
        }

        if transmit_coeffs {
            self.inverse_channel_transform();
            for mi in 0..self.member_count {
                let c = self.members[mi];
                let ch = &self.channels[c];
                let sf = &ch.saved_sf[ch.sf_cur];
                for b in 0..self.num_bands {
                    let start = self.sfb_offsets[self.table_idx][b];
                    let end = self.sfb_offsets[self.table_idx][b + 1].min(subframe_len);
                    let exp = ch.quant_step - (ch.max_scale_factor - sf[b]) * ch.scale_factor_step;
                    let quant = 10f64.powf(f64::from(exp) / 20.0) as f32;
                    for y in start..end {
                        self.tmp[y] = ch.out[ch.coefs_at + y] * quant;
                    }
                }
                let at = ch.coefs_at;
                let imdct = &self.imdct[self.table_idx];
                let ch = &mut self.channels[c];
                imdct.process(&self.tmp[..subframe_len], &mut ch.out[at..at + subframe_len]);
            }
        }

        self.window();

        for mi in 0..self.member_count {
            let ch = &mut self.channels[self.members[mi]];
            if ch.cur_subframe >= ch.num_subframes {
                return Err(Error::Corrupt("subframe count mismatch"));
            }
            ch.cur_subframe += 1;
        }
        Ok(())
    }

    /// Decode one frame from `data` (bits `0..bit_len`, starting at the frame's 15-bit length prefix).
    /// The 512 output samples per channel are then available from [`FrameDecoder::pcm`].
    pub fn decode_frame(&mut self, data: &[u8], bit_len: usize) -> Result<FrameInfo> {
        let mut br = BitReader::with_bit_len(data, bit_len);
        let len = br.read(LOG2_FRAME_SIZE) as usize;
        if len > bit_len || len < LOG2_FRAME_SIZE as usize + 2 {
            return Err(Error::Corrupt("frame length prefix"));
        }
        self.decode_tile_header(&mut br)?;

        if self.nb_channels > 1 && br.read_bit() && br.read_bit() {
            br.skip(self.nb_channels * self.nb_channels * 4);
        }
        // dynamic range compression gain, unused
        br.skip(8);
        let mut info = FrameInfo::default();
        if br.read_bit() {
            if br.read_bit() {
                info.trim_start = br.read(log2_floor(SAMPLES_PER_FRAME * 2)) as u16;
            }
            if br.read_bit() {
                info.trim_end = br.read(log2_floor(SAMPLES_PER_FRAME * 2)) as u16;
            }
        }

        self.parsed_all = false;
        for c in &mut self.channels {
            c.decoded_samples = 0;
            c.cur_subframe = 0;
            c.reuse_sf = false;
        }
        while !self.parsed_all {
            self.decode_subframe(&mut br)?;
            if br.overrun() {
                return Err(Error::Corrupt("frame data ends early"));
            }
        }

        for c in 0..self.nb_channels {
            self.pcm[c].copy_from_slice(&self.channels[c].out[..SAMPLES_PER_FRAME]);
            let ch = &mut self.channels[c];
            ch.out.copy_within(SAMPLES_PER_FRAME..OUT_LEN, 0);
        }

        // the frame ends with one stop bit and the trailer bit
        if br.pos() + 2 > len {
            return Err(Error::Corrupt("frame body longer than its length prefix"));
        }
        br.set_pos(len - 1);
        info.more_frames = br.read_bit();
        Ok(info)
    }
}
