//! Builders for synthetic Bink 2 frames used by the macroblock tests.

use super::{Frame, Plane, VideoDecoder};
use crate::container::Header;
use crate::tables;

/// Packs bit fields LSB first, the order the decoder reads them in.
#[derive(Default)]
pub struct Bits(Vec<bool>);

impl Bits {
    pub fn put(&mut self, value: u32, n: u32) -> &mut Self {
        for i in 0..n {
            self.0.push((value >> i) & 1 != 0);
        }
        self
    }

    /// A prefix-coded value: `p` ones and a zero; values from 4 on add `p - 3` low bits.
    pub fn unsigned(&mut self, v: u32) -> &mut Self {
        if v < 4 {
            return self.put((1 << v) - 1, v + 1);
        }
        let n = (31 - (v - 2).leading_zeros()).max(1);
        let p = n + 3;
        self.put((1 << p) - 1, p + 1);
        self.put(v - (1 << n) - 2, n)
    }

    /// A DC difference group: the presence flag, then magnitude and sign for each value.
    pub fn dc_diffs(&mut self, d: &[i32]) -> &mut Self {
        if d.iter().all(|&v| v == 0) {
            return self.put(0, 1);
        }
        self.put(1, 1);
        for &v in d {
            self.unsigned(v.unsigned_abs());
            if v != 0 {
                self.put(u32::from(v < 0), 1);
            }
        }
        self
    }

    /// One motion vector component as the symbol code, escaping to the long form beyond +-7.
    pub fn mv(&mut self, v: i32) -> &mut Self {
        let symbol = match v {
            0..=7 => v as usize,
            -7..=-1 => (v + 15) as usize,
            _ => 15,
        };
        self.put(
            u32::from(tables::MV_CODES[symbol]),
            u32::from(tables::MV_BITS[symbol]),
        );
        if symbol == 15 {
            let u = if v >= 0 {
                (v as u32) << 1
            } else {
                (((-v - 1) as u32) << 1) | 1
            };
            let bits = (31 - (u + 1).leading_zeros()).max(4);
            self.put(1 << (bits - 4), bits - 3);
            self.put(u + 1 - (1 << bits), bits);
        }
        self
    }

    /// Padded to whole 32-bit words, with a tail of zero bytes for the reader to peek into.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.0.len().div_ceil(32) * 4];
        for (i, &b) in self.0.iter().enumerate() {
            out[i / 8] |= u8::from(b) << (i % 8);
        }
        out
    }
}

pub fn header(width: u32, height: u32) -> Header {
    Header {
        width,
        height,
        frames: 2,
        fps_num: 30,
        fps_den: 1,
        has_alpha: false,
        audio: Vec::new(),
        version: b'n',
        flags: 0,
        largest_frame: 0,
    }
}

pub fn decoder(width: u32, height: u32) -> VideoDecoder {
    VideoDecoder::new(&header(width, height)).unwrap()
}

/// A frame of the decoder's geometry whose samples come from `f(plane, x, y)`.
pub fn frame(dec: &VideoDecoder, f: impl Fn(usize, usize, usize) -> u8) -> Frame {
    let mut frame = dec.blank(0);
    for (p, plane) in frame.planes.iter_mut().enumerate() {
        for y in 0..plane.data.len() / plane.stride {
            for x in 0..plane.stride {
                plane.data[y * plane.stride + x] = f(p, x, y);
            }
        }
    }
    frame
}

/// A frame payload for a picture one macroblock tall: the frame flags, the offset of the empty second slice and
/// the macroblock bits.
pub fn payload(mbs: &Bits) -> Vec<u8> {
    let body = mbs.bytes();
    let mut out = Vec::new();
    out.extend_from_slice(&0x2000u32.to_le_bytes());
    out.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&[0; 8]);
    out
}

pub fn sample(plane: &Plane, x: usize, y: usize) -> u8 {
    plane.data[y * plane.stride + x]
}
