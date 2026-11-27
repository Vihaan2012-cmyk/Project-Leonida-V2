//! A small XMA2 writer for tests: frames of one 512-sample subframe per channel, and a packetizer.

use super::frame::{FrameDecoder, tables};
use super::stream::PACKET_SIZE;
use super::tables::*;

pub(crate) const N: usize = 512;
const DATA_BITS: usize = PACKET_SIZE * 8 - 32;

pub(crate) type Bits = Vec<bool>;

pub(crate) fn put(b: &mut Bits, n: u32, v: u32) {
    for i in (0..n).rev() {
        b.push((v >> i) & 1 == 1);
    }
}

pub(crate) fn put_code(b: &mut Bits, (code, len): (u32, u32)) {
    put(b, len, code);
}

pub(crate) struct Rng(pub(crate) u32);
impl Rng {
    pub(crate) fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0 >> 8
    }
}

/// Four magnitudes (0..=15) and signs, one symbol of the vector coder.
#[derive(Clone, Copy)]
pub(crate) struct Tuple {
    mags: [u8; 4],
    neg: [bool; 4],
}

impl Tuple {
    fn packed(&self) -> i32 {
        self.mags.iter().fold(0, |v, &m| (v << 4) | i32::from(m))
    }
}

/// Tuples the vector table can express, without three zeros in a row (that would switch to run-level coding).
pub(crate) fn random_tuples(rng: &mut Rng, count: usize) -> Vec<Tuple> {
    let usable: Vec<u16> = VEC4_SYMS
        .iter()
        .copied()
        .filter(|&s| s > 1)
        .filter(|&s| {
            let p = s - 1;
            let m = [(p >> 12) & 15, (p >> 8) & 15, (p >> 4) & 15, p & 15];
            let zeros_edge = m.iter().take_while(|&&v| v == 0).count();
            let zeros_tail = m.iter().rev().take_while(|&&v| v == 0).count();
            zeros_edge <= 1 && zeros_tail <= 1 && !m.windows(3).any(|w| w.iter().all(|&v| v == 0))
        })
        .collect();
    (0..count)
        .map(|_| {
            let p = usable[(rng.next() as usize) % usable.len()] - 1;
            let mags = [
                (p >> 12) as u8 & 15,
                (p >> 8) as u8 & 15,
                (p >> 4) as u8 & 15,
                p as u8 & 15,
            ];
            let neg = std::array::from_fn(|i| mags[i] != 0 && rng.next() & 1 == 1);
            Tuple { mags, neg }
        })
        .collect()
}

pub(crate) fn coefficients(tuples: &[Tuple]) -> Vec<f64> {
    let mut c = vec![0.0; N];
    for (j, t) in tuples.iter().enumerate() {
        for i in 0..4 {
            let v = f64::from(t.mags[i]);
            c[4 * j + i] = if t.neg[i] { -v } else { v };
        }
    }
    c
}

pub(crate) fn vec4_code(packed: i32) -> (u32, u32) {
    let i = VEC4_SYMS
        .iter()
        .position(|&s| i32::from(s) - 1 == packed)
        .expect("tuple is in the vector table");
    tables().vec4.code_of(i)
}

pub(crate) fn encode_channel_coefficients(b: &mut Bits, tuples: &[Tuple]) {
    put(b, 1, 0);
    for t in tuples {
        put_code(b, vec4_code(t.packed()));
        for i in 0..4 {
            if t.mags[i] != 0 {
                put(b, 1, u32::from(t.neg[i]));
            }
        }
    }
    put_code(b, vec4_code(0));
    let eob = COEF0_SYMS.iter().position(|&s| s == 1).expect("EOB symbol");
    put_code(b, tables().coef[0].code_of(eob));
}

pub(crate) fn encode_scale_factors(b: &mut Bits, bands: usize) {
    put(b, 2, 0);
    let zero = SCALE_TABLE
        .iter()
        .position(|&(s, _)| s == 60)
        .expect("delta 0");
    for _ in 0..bands {
        put_code(b, tables().sf.code_of(zero));
    }
}

/// One frame with a single 512-sample subframe per channel and no length prefix, stop bit or trailer yet.
pub(crate) fn encode_frame(channels: &[&[Tuple]], step: i32, stereo_transform: bool) -> Bits {
    let bands = FrameDecoder::new(channels.len(), 32000)
        .unwrap()
        .band_count(0);
    let nb = channels.len();
    let mut b = Bits::new();
    put(&mut b, 1, 1);
    put(&mut b, 1, 0);
    if nb > 1 {
        put(&mut b, 1, 0);
    }
    put(&mut b, 8, 0);
    put(&mut b, 1, 0);
    put(&mut b, 1, 0);
    put(&mut b, 1, 0);
    if nb > 1 {
        put(&mut b, 1, 0);
        if stereo_transform {
            put(&mut b, 1, 0);
            put(&mut b, 1, 1);
        } else {
            put(&mut b, 1, 1);
            put(&mut b, 1, 0);
        }
    }
    for _ in 0..nb {
        put(&mut b, 1, 1);
    }
    put(&mut b, 1, 0);
    put(&mut b, 6, (step & 63) as u32);
    if nb > 1 {
        put(&mut b, 3, 0);
        for _ in 0..nb {
            put(&mut b, 1, 0);
        }
    }
    for _ in 0..nb {
        encode_scale_factors(&mut b, bands);
    }
    for tuples in channels {
        encode_channel_coefficients(&mut b, tuples);
    }
    b
}

/// The finished frame bits: length prefix, body, stop bit, trailer (0 until the packetizer decides).
pub(crate) fn finish_frame(body: &Bits) -> Bits {
    let len = body.len() + 15 + 2;
    let mut f = Bits::new();
    put(&mut f, 15, len as u32);
    f.extend_from_slice(body);
    f.push(true);
    f.push(false);
    f
}

pub(crate) struct Pk {
    carry: usize,
    started: usize,
    data: Bits,
}

pub(crate) fn packetize(frames: &[Bits]) -> Vec<Vec<u8>> {
    let mut pks = vec![Pk {
        carry: 0,
        started: 0,
        data: Bits::new(),
    }];
    let mut last_trailer: Option<(usize, usize)> = None;
    for f in frames {
        let cur = pks.len() - 1;
        if DATA_BITS - pks[cur].data.len() <= 17 {
            pks.push(Pk {
                carry: 0,
                started: 0,
                data: Bits::new(),
            });
        }
        let cur = pks.len() - 1;
        if let Some((pi, bi)) = last_trailer
            && pi == cur
        {
            pks[pi].data[bi] = true;
        }
        pks[cur].started += 1;
        let take = f.len().min(DATA_BITS - pks[cur].data.len());
        pks[cur].data.extend_from_slice(&f[..take]);
        let mut rest = &f[take..];
        if rest.is_empty() {
            last_trailer = Some((cur, pks[cur].data.len() - 1));
            continue;
        }
        while !rest.is_empty() {
            let chunk = rest.len().min(DATA_BITS);
            pks.push(Pk {
                carry: chunk,
                started: 0,
                data: rest[..chunk].to_vec(),
            });
            rest = &rest[chunk..];
        }
        let cur = pks.len() - 1;
        last_trailer = Some((cur, pks[cur].data.len() - 1));
    }
    pks.iter()
        .map(|p| {
            let mut out = vec![0u8; PACKET_SIZE];
            let word = ((p.started as u32) << 26) | ((p.carry as u32) << 11) | (1 << 8);
            out[..4].copy_from_slice(&word.to_be_bytes());
            for (i, &bit) in p.data.iter().enumerate() {
                if bit {
                    out[4 + i / 8] |= 0x80 >> (i % 8);
                }
            }
            out
        })
        .collect()
}
