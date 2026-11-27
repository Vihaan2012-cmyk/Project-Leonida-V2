//! BCPack BC1 block rebuild: the per-block stage after the entropy decoder.
//!
//! The rANS stage turns a chunk into twelve lanes of small symbols (lane widths 8,8,5,6,5,5,6,5,8,8,8,8); it is
//! sequential per lane and stays on the CPU. What follows is per block: lane 0 names an earlier block (or a
//! built-in history entry) whose six endpoint fields are the base, lanes 2-7 are signed zigzag deltas to it, and
//! lanes 8-11 index a 256-entry table of 2x2 index quadrants. The tables are recovered data, so they are
//! arguments here, not constants (docs/textures.md, "From a chunk entry to `bcpack.decode`").

use crate::PassError;

pub const LANES: usize = 12;
const MODS: [i32; 6] = [32, 64, 32, 32, 64, 32];

/// The two lookup tables of the BC1 rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bc1Tables {
    /// Lane symbol to one byte of quadrant indices.
    pub quadrant: [u8; 256],
    /// Built-in history, entry 1..=255 (entry 0 is unused): the six endpoint fields r0 g0 b0 r1 g1 b1, or
    /// `None` for an entry that is not recovered.
    pub history: [Option<[u8; 6]>; 256],
}

impl Bc1Tables {
    /// The tables as the shader reads them: 64 words of quadrants, then two words per history entry.
    pub fn pack(&self) -> Vec<u32> {
        let mut words = vec![0u32; 64 + 512];
        for (i, q) in self.quadrant.iter().enumerate() {
            words[i / 4] |= (*q as u32) << (8 * (i % 4));
        }
        for (entry, e) in self.history.iter().enumerate() {
            if let Some(e) = e {
                words[64 + entry * 2] = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                words[64 + entry * 2 + 1] = e[4] as u32 | (e[5] as u32) << 8 | 1 << 16;
            }
        }
        words
    }
}

/// Twelve lanes of `blocks` symbols, packed lane-major with each lane padded to a multiple of four bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lanes {
    pub blocks: u32,
    pub lane_stride: u32,
    pub words: Vec<u32>,
}

impl Lanes {
    pub fn pack(lanes: &[&[u8]; LANES]) -> Result<Lanes, PassError> {
        let blocks = lanes[0].len();
        if lanes.iter().any(|l| l.len() != blocks) {
            return Err(PassError::LaneLengths);
        }
        let stride = blocks.next_multiple_of(4);
        let mut bytes = vec![0u8; stride * LANES];
        for (i, lane) in lanes.iter().enumerate() {
            bytes[i * stride..i * stride + blocks].copy_from_slice(lane);
        }
        let words = bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        Ok(Lanes {
            blocks: blocks as u32,
            lane_stride: stride as u32,
            words,
        })
    }
}

fn unzig(v: u8) -> i32 {
    let v = v as i32;
    if v & 1 != 0 { -((v + 1) >> 1) } else { v >> 1 }
}

/// The block-by-block reference: each block's base is the finished block `d` back, a built-in entry, or zero.
/// Fails on the first block whose history entry is not recovered.
pub fn rebuild_cpu(lanes: &[&[u8]; LANES], tables: &Bc1Tables) -> Result<Vec<u8>, PassError> {
    let blocks = lanes[0].len();
    if lanes.iter().any(|l| l.len() != blocks) {
        return Err(PassError::LaneLengths);
    }
    let mut done: Vec<[u8; 6]> = Vec::with_capacity(blocks);
    let mut out = Vec::with_capacity(blocks * 8);
    for i in 0..blocks {
        let v = lanes[0][i] as usize;
        let d = if v & 1 != 0 {
            256 - ((v + 1) >> 1)
        } else {
            v >> 1
        };
        let base = if d == 0 {
            [0u8; 6]
        } else if i >= d {
            done[i - d]
        } else {
            let entry = 256 + i - d;
            tables.history[entry].ok_or(PassError::UnknownHistory { block: i, entry })?
        };
        let mut e = [0u8; 6];
        for c in 0..6 {
            e[c] = (base[c] as i32 + unzig(lanes[2 + c][i])).rem_euclid(MODS[c]) as u8;
        }
        done.push(e);
        let c0 = (e[0] as u16) << 11 | (e[1] as u16) << 5 | e[2] as u16;
        let c1 = (e[3] as u16) << 11 | (e[4] as u16) << 5 | e[5] as u16;
        let q = |lane: usize| tables.quadrant[lanes[lane][i] as usize];
        let (a, b, c, dd) = (q(8), q(9), q(10), q(11));
        out.extend_from_slice(&c0.to_le_bytes());
        out.extend_from_slice(&c1.to_le_bytes());
        out.push((a & 15) | ((b & 15) << 4));
        out.push((a >> 4) | ((b >> 4) << 4));
        out.push((c & 15) | ((dd & 15) << 4));
        out.push((c >> 4) | ((dd >> 4) << 4));
    }
    Ok(out)
}
