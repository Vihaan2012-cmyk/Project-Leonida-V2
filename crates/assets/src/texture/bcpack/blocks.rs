//! BCn block reconstruction from the decoded lane symbols (BC1, BC3, BC4, BC5).
//!
//! Every block names a predictor through lane 0: nothing (zero or mid-range defaults), an earlier block of the
//! chunk, or an entry of the codec's built-in history. The endpoint lanes are zigzag deltas to the predictor's
//! endpoint fields and the index lanes are ranks into the codebooks. Endpoints of an earlier block are read back
//! from its already written output.

use super::codebooks::{
    BC1_HISTORY, BC1_QUADRANT, BC3_ALPHA_PAIR, BC3_COLOUR_QUADRANT, BC3_HISTORY, BC4_HISTORY,
    BC4_PAIR, BC5_HISTORY, BC5_PAIR, Hist,
};
use super::lanes::LaneSymbols;
use super::{BcPackCodec, BcPackError, DecodeOptions, HistoryStatus};

#[inline(always)]
pub(super) fn unzig(v: u8) -> i32 {
    i32::from(v >> 1) ^ -i32::from(v & 1)
}

const fn spread_table() -> [u8; 16] {
    let mut t = [0u8; 16];
    let mut v = 0;
    while v < 16 {
        let mut k = 0;
        while k < 4 {
            t[v] |= (((v >> k) & 1) as u8) << (2 * k);
            k += 1;
        }
        v += 1;
    }
    t
}

/// Bit `k` of a nibble moved to bit `2k`.
static SPREAD: [u8; 16] = spread_table();

/// Backward distance of a two-element block: the two nibbles are bit-interleaved, then zigzag decoded.
#[inline(always)]
fn distance2(a: u8, b: u8) -> usize {
    let v = SPREAD[usize::from(a & 15)] | SPREAD[usize::from(b & 15)] << 1;
    (unzig(v) & 255) as usize
}

#[inline(always)]
fn distance1(v: u8) -> usize {
    (unzig(v) & 255) as usize
}

/// Where a block's predictor comes from.
enum Base<const N: usize> {
    None,
    Block(usize),
    History([u8; N]),
}

struct Taint(Option<Vec<bool>>);

impl Taint {
    fn mark(&mut self, block: usize, count: usize) {
        self.0.get_or_insert_with(|| vec![false; count])[block] = true;
    }

    fn inherit(&mut self, block: usize, from: usize) {
        if let Some(t) = &mut self.0 {
            t[block] = t[from];
        }
    }

    fn count(&self) -> usize {
        self.0
            .as_ref()
            .map_or(0, |t| t.iter().filter(|b| **b).count())
    }
}

/// Resolve the distance `d` of block `i` to its predictor.
#[inline(always)]
fn base_of<const N: usize>(
    codec: BcPackCodec,
    table: &[Hist<N>; 256],
    i: usize,
    d: usize,
    opts: &DecodeOptions,
    taint: &mut Taint,
    count: usize,
) -> Result<Base<N>, BcPackError> {
    if d == 0 {
        return Ok(Base::None);
    }
    if d <= i {
        taint.inherit(i, i - d);
        return Ok(Base::Block(i - d));
    }
    let entry = 256 + i - d;
    let refuse = |status| BcPackError::History {
        codec,
        block: i,
        entry: entry as u8,
        status,
    };
    match table[entry] {
        Hist::Confirmed(v) => Ok(Base::History(v)),
        Hist::Provisional(v) if opts.allow_provisional => {
            taint.mark(i, count);
            Ok(Base::History(v))
        }
        Hist::Provisional(_) => Err(refuse(HistoryStatus::Provisional)),
        Hist::Unknown => Err(refuse(HistoryStatus::Unknown)),
    }
}

fn require_zero_lane(codec: BcPackCodec, lane: &[u8]) -> Result<(), BcPackError> {
    if lane.iter().any(|s| *s != 0) {
        return Err(BcPackError::UnexpectedLane { codec, lane: 1 });
    }
    Ok(())
}

/// Colour endpoint fields of a BC1 colour word pair: `r0 g0 b0 r1 g1 b1`.
#[inline(always)]
fn colour_fields(c0: u16, c1: u16) -> [u8; 6] {
    [
        (c0 >> 11) as u8,
        (c0 >> 5 & 63) as u8,
        (c0 & 31) as u8,
        (c1 >> 11) as u8,
        (c1 >> 5 & 63) as u8,
        (c1 & 31) as u8,
    ]
}

/// Four quadrant bytes (top-left, top-right, bottom-left, bottom-right) as the block's four index rows.
#[inline(always)]
fn quadrant_rows(tl: u8, tr: u8, bl: u8, br: u8) -> u32 {
    let row = |l: u8, r: u8, shift: u32| u32::from((l >> shift & 15) | (r >> shift & 15) << 4);
    row(tl, tr, 0) | row(tl, tr, 4) << 8 | row(bl, br, 0) << 16 | row(bl, br, 4) << 24
}

fn read_u16(block: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([block[at], block[at + 1]])
}

pub fn bc1(
    lanes: &LaneSymbols,
    out: &mut [u8],
    opts: &DecodeOptions,
) -> Result<usize, BcPackError> {
    let codec = BcPackCodec::Bc1;
    let n = out.len() / 8;
    require_zero_lane(codec, lanes.lane(1))?;
    let l0 = lanes.lane(0);
    let cols: [&[u8]; 6] = std::array::from_fn(|k| lanes.lane(2 + k));
    let quads: [&[u8]; 4] = std::array::from_fn(|k| lanes.lane(8 + k));
    let mut taint = Taint(None);
    for i in 0..n {
        let base = match base_of(
            codec,
            &BC1_HISTORY,
            i,
            distance1(l0[i]),
            opts,
            &mut taint,
            n,
        )? {
            Base::None => [0; 6],
            Base::History(v) => v,
            Base::Block(j) => colour_fields(read_u16(out, j * 8), read_u16(out, j * 8 + 2)),
        };
        let e = |k: usize, mask: i32| ((i32::from(base[k]) + unzig(cols[k][i])) & mask) as u16;
        let c0 = e(0, 31) << 11 | e(1, 63) << 5 | e(2, 31);
        let c1 = e(3, 31) << 11 | e(4, 63) << 5 | e(5, 31);
        let rows = quadrant_rows(
            BC1_QUADRANT[usize::from(quads[0][i])],
            BC1_QUADRANT[usize::from(quads[1][i])],
            BC1_QUADRANT[usize::from(quads[2][i])],
            BC1_QUADRANT[usize::from(quads[3][i])],
        );
        let block = &mut out[i * 8..i * 8 + 8];
        block[0..2].copy_from_slice(&c0.to_le_bytes());
        block[2..4].copy_from_slice(&c1.to_le_bytes());
        block[4..8].copy_from_slice(&rows.to_le_bytes());
    }
    Ok(taint.count())
}

/// Four rows of two row symbols each, every symbol standing for two 3-bit indices, as 48 bits.
#[inline(always)]
fn index_bits(pairs: &[[u8; 2]; 64], rows: [(u8, u8); 4]) -> u64 {
    let mut bits = 0u64;
    for (r, (left, right)) in rows.into_iter().enumerate() {
        let l = pairs[usize::from(left) & 63];
        let rt = pairs[usize::from(right) & 63];
        let v =
            u64::from(l[0]) | u64::from(l[1]) << 3 | u64::from(rt[0]) << 6 | u64::from(rt[1]) << 9;
        bits |= v << (12 * r);
    }
    bits
}

#[inline(always)]
fn put_alpha(block: &mut [u8], a0: u8, a1: u8, bits: u64) {
    block[0] = a0;
    block[1] = a1;
    block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
}

pub fn bc3(
    lanes: &LaneSymbols,
    out: &mut [u8],
    opts: &DecodeOptions,
) -> Result<usize, BcPackError> {
    let codec = BcPackCodec::Bc3;
    let n = out.len() / 16;
    require_zero_lane(codec, lanes.lane(1))?;
    let l0 = lanes.lane(0);
    let alpha = lanes.lane(2);
    let rows: [&[u8]; 4] = std::array::from_fn(|k| lanes.lane(3 + k));
    let (lr, lg, lb) = (lanes.lane(7), lanes.lane(8), lanes.lane(9));
    let (top, bottom) = (lanes.lane(10), lanes.lane(11));
    let mut taint = Taint(None);
    for i in 0..n {
        let (a, b) = (2 * i, 2 * i + 1);
        let distance = distance2(l0[a], l0[b]);
        let base = match base_of(codec, &BC3_HISTORY, i, distance, opts, &mut taint, n)? {
            Base::None => [128, 128, 0, 0, 0, 0, 0, 0],
            Base::History(v) => v,
            Base::Block(j) => {
                let blk = &out[j * 16..j * 16 + 16];
                let c = colour_fields(read_u16(blk, 8), read_u16(blk, 10));
                [blk[0], blk[1], c[0], c[1], c[2], c[3], c[4], c[5]]
            }
        };
        let d = |k: usize, sym: u8, mask: i32| ((i32::from(base[k]) + unzig(sym)) & mask) as u16;
        let (a0, a1) = (d(0, alpha[a], 255) as u8, d(1, alpha[b], 255) as u8);
        let c0 = d(2, lr[a], 31) << 11 | d(3, lg[a], 63) << 5 | d(4, lb[a], 31);
        let c1 = d(5, lr[b], 31) << 11 | d(6, lg[b], 63) << 5 | d(7, lb[b], 31);
        let bits = index_bits(
            &BC3_ALPHA_PAIR,
            [
                (rows[0][a], rows[0][b]),
                (rows[1][a], rows[1][b]),
                (rows[2][a], rows[2][b]),
                (rows[3][a], rows[3][b]),
            ],
        );
        let quad = quadrant_rows(
            BC3_COLOUR_QUADRANT[usize::from(top[a])],
            BC3_COLOUR_QUADRANT[usize::from(top[b])],
            BC3_COLOUR_QUADRANT[usize::from(bottom[a])],
            BC3_COLOUR_QUADRANT[usize::from(bottom[b])],
        );
        let block = &mut out[i * 16..i * 16 + 16];
        put_alpha(&mut block[0..8], a0, a1, bits);
        block[8..10].copy_from_slice(&c0.to_le_bytes());
        block[10..12].copy_from_slice(&c1.to_le_bytes());
        block[12..16].copy_from_slice(&quad.to_le_bytes());
    }
    Ok(taint.count())
}

pub fn bc4(
    lanes: &LaneSymbols,
    out: &mut [u8],
    opts: &DecodeOptions,
) -> Result<usize, BcPackError> {
    let codec = BcPackCodec::Bc4;
    let n = out.len() / 8;
    require_zero_lane(codec, lanes.lane(1))?;
    let l0 = lanes.lane(0);
    let (e0, e1) = (lanes.lane(2), lanes.lane(3));
    let rows: [&[u8]; 8] = std::array::from_fn(|k| lanes.lane(4 + k));
    let mut taint = Taint(None);
    for i in 0..n {
        let base = match base_of(
            codec,
            &BC4_HISTORY,
            i,
            distance1(l0[i]),
            opts,
            &mut taint,
            n,
        )? {
            Base::None => [128, 128],
            Base::History(v) => v,
            Base::Block(j) => [out[j * 8], out[j * 8 + 1]],
        };
        let a0 = (i32::from(base[0]) + unzig(e0[i])) as u8;
        let a1 = (i32::from(base[1]) + unzig(e1[i])) as u8;
        let bits = index_bits(
            &BC4_PAIR,
            [
                (rows[0][i], rows[1][i]),
                (rows[2][i], rows[3][i]),
                (rows[4][i], rows[5][i]),
                (rows[6][i], rows[7][i]),
            ],
        );
        put_alpha(&mut out[i * 8..i * 8 + 8], a0, a1, bits);
    }
    Ok(taint.count())
}

pub fn bc5(
    lanes: &LaneSymbols,
    out: &mut [u8],
    opts: &DecodeOptions,
) -> Result<usize, BcPackError> {
    let codec = BcPackCodec::Bc5;
    let n = out.len() / 16;
    require_zero_lane(codec, lanes.lane(1))?;
    let l0 = lanes.lane(0);
    let (red, green) = (lanes.lane(2), lanes.lane(3));
    let rows: [&[u8]; 8] = std::array::from_fn(|k| lanes.lane(4 + k));
    let mut taint = Taint(None);
    for i in 0..n {
        let (a, b) = (2 * i, 2 * i + 1);
        let distance = distance2(l0[a], l0[b]);
        let base = match base_of(codec, &BC5_HISTORY, i, distance, opts, &mut taint, n)? {
            Base::None => [128; 4],
            Base::History(v) => v,
            Base::Block(j) => {
                let blk = &out[j * 16..j * 16 + 16];
                [blk[0], blk[1], blk[8], blk[9]]
            }
        };
        let d = |k: usize, sym: u8| (i32::from(base[k]) + unzig(sym)) as u8;
        let ends = [d(0, red[a]), d(1, red[b]), d(2, green[a]), d(3, green[b])];
        let block = &mut out[i * 16..i * 16 + 16];
        for ch in 0..2 {
            let r = &rows[4 * ch..4 * ch + 4];
            let bits = index_bits(
                &BC5_PAIR,
                [
                    (r[0][a], r[0][b]),
                    (r[1][a], r[1][b]),
                    (r[2][a], r[2][b]),
                    (r[3][a], r[3][b]),
                ],
            );
            put_alpha(
                &mut block[8 * ch..8 * ch + 8],
                ends[2 * ch],
                ends[2 * ch + 1],
                bits,
            );
        }
    }
    Ok(taint.count())
}

/// Two nibbles bit-interleaved: `a` supplies the even bits and `b` the odd bits.
#[inline(always)]
pub(super) fn interleave4(a: u8, b: u8) -> u8 {
    SPREAD[usize::from(a & 15)] | SPREAD[usize::from(b & 15)] << 1
}
