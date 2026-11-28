//! BC6H block reconstruction.
//!
//! Lane 15 carries the mode (`mode - 1`, 0..=13, in the order of the 14 BC6H modes) and the partition. Lanes 0 and
//! 1 carry the backward distances to the block that predicts the endpoints (`de`) and the indices (`di`), as in
//! BC7. A predictor is a block's stored endpoint state (per channel the four fields `w x y z` left-aligned to 16,
//! 10, 6 and 6 bits) or index state (16 pixel indices left-aligned to 4 bits); distance 0 gives the mid-range
//! default and a distance beyond the block number reads the codec's built-in history.
//!
//! Lanes 2-4 carry the 16-bit zigzag delta of `w` per colour channel (two 8-bit symbols, bit-interleaved), lanes
//! 5-7 the 10-bit delta of `x` (two 5-bit symbols) and lanes 8-10 the deltas of `y` and `z` (one 6-bit symbol per
//! element). Lanes 11-14 carry the four index rows as zigzag deltas, two pixels per even/odd bit pair, with the
//! parity swapped on the last row.

use super::blocks::{interleave4, unzig};
use super::bptc::ANCHOR2;
use super::lanes::LaneSymbols;
use super::{BcPackCodec, BcPackError, DecodeOptions, HistoryStatus};
use std::sync::OnceLock;

pub(super) type Endpoints = [[u16; 4]; 3];
pub(super) type Indices = [u8; 16];

/// Endpoint state of a block without a predictor: the centre of every field.
const MID_ENDPOINTS: Endpoints = [[0x8000, 0x200, 0x20, 0x20]; 3];
const MID_INDICES: Indices = [8; 16];

pub(super) struct Mode {
    pub code: u8,
    pub transformed: bool,
    pub base: u32,
    pub delta: [u32; 3],
    pub two_regions: bool,
    pub layout: &'static str,
}

/// Header bit layouts: `rw9-0` is bits 0..=9 of the red `w` endpoint in stream order, `rw11~10` is bit 11 then
/// bit 10, `gy4` is the single bit 4, `d` is the 5-bit partition (the public D3D11 / Khronos BC6H layouts).
pub(super) const MODES: [Mode; 14] = [
    Mode {
        code: 0x00,
        transformed: true,
        base: 10,
        delta: [5, 5, 5],
        two_regions: true,
        layout: "gy4 by4 bz4 rw9-0 gw9-0 bw9-0 rx4-0 gz4 gy3-0 gx4-0 bz0 gz3-0 bx4-0 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Mode {
        code: 0x01,
        transformed: true,
        base: 7,
        delta: [6, 6, 6],
        two_regions: true,
        layout: "gy5 gz4 gz5 rw6-0 bz0 bz1 by4 gw6-0 by5 bz2 gy4 bw6-0 bz3 bz5 bz4 rx5-0 gy3-0 gx5-0 gz3-0 bx5-0 by3-0 ry5-0 rz5-0 d",
    },
    Mode {
        code: 0x02,
        transformed: true,
        base: 11,
        delta: [5, 4, 4],
        two_regions: true,
        layout: "rw9-0 gw9-0 bw9-0 rx4-0 rw10 gy3-0 gx3-0 gw10 bz0 gz3-0 bx3-0 bw10 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Mode {
        code: 0x06,
        transformed: true,
        base: 11,
        delta: [4, 5, 4],
        two_regions: true,
        layout: "rw9-0 gw9-0 bw9-0 rx3-0 rw10 gz4 gy3-0 gx4-0 gw10 gz3-0 bx3-0 bw10 bz1 by3-0 ry3-0 bz0 bz2 rz3-0 gy4 bz3 d",
    },
    Mode {
        code: 0x0A,
        transformed: true,
        base: 11,
        delta: [4, 4, 5],
        two_regions: true,
        layout: "rw9-0 gw9-0 bw9-0 rx3-0 rw10 by4 gy3-0 gx3-0 gw10 bz0 gz3-0 bx4-0 bw10 by3-0 ry3-0 bz1 bz2 rz3-0 bz4 bz3 d",
    },
    Mode {
        code: 0x0E,
        transformed: true,
        base: 9,
        delta: [5, 5, 5],
        two_regions: true,
        layout: "rw8-0 by4 gw8-0 gy4 bw8-0 bz4 rx4-0 gz4 gy3-0 gx4-0 bz0 gz3-0 bx4-0 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Mode {
        code: 0x12,
        transformed: true,
        base: 8,
        delta: [6, 5, 5],
        two_regions: true,
        layout: "rw7-0 gz4 by4 gw7-0 bz2 gy4 bw7-0 bz3 bz4 rx5-0 gy3-0 gx4-0 bz0 gz3-0 bx4-0 bz1 by3-0 ry5-0 rz5-0 d",
    },
    Mode {
        code: 0x16,
        transformed: true,
        base: 8,
        delta: [5, 6, 5],
        two_regions: true,
        layout: "rw7-0 bz0 by4 gw7-0 gy5 gy4 bw7-0 gz5 bz4 rx4-0 gz4 gy3-0 gx5-0 gz3-0 bx4-0 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Mode {
        code: 0x1A,
        transformed: true,
        base: 8,
        delta: [5, 5, 6],
        two_regions: true,
        layout: "rw7-0 bz1 by4 gw7-0 by5 gy4 bw7-0 bz5 bz4 rx4-0 gz4 gy3-0 gx4-0 bz0 gz3-0 bx5-0 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Mode {
        code: 0x1E,
        transformed: false,
        base: 6,
        delta: [6, 6, 6],
        two_regions: true,
        layout: "rw5-0 gz4 bz0 bz1 by4 gw5-0 gy5 by5 bz2 gy4 bw5-0 gz5 bz3 bz5 bz4 rx5-0 gy3-0 gx5-0 gz3-0 bx5-0 by3-0 ry5-0 rz5-0 d",
    },
    Mode {
        code: 0x03,
        transformed: false,
        base: 10,
        delta: [10, 10, 10],
        two_regions: false,
        layout: "rw9-0 gw9-0 bw9-0 rx9-0 gx9-0 bx9-0",
    },
    Mode {
        code: 0x07,
        transformed: true,
        base: 11,
        delta: [9, 9, 9],
        two_regions: false,
        layout: "rw9-0 gw9-0 bw9-0 rx8-0 rw10 gx8-0 gw10 bx8-0 bw10",
    },
    Mode {
        code: 0x0B,
        transformed: true,
        base: 12,
        delta: [8, 8, 8],
        two_regions: false,
        layout: "rw9-0 gw9-0 bw9-0 rx7-0 rw11~10 gx7-0 gw11~10 bx7-0 bw11~10",
    },
    Mode {
        code: 0x0F,
        transformed: true,
        base: 16,
        delta: [4, 4, 4],
        two_regions: false,
        layout: "rw9-0 gw9-0 bw9-0 rx3-0 rw15~10 gx3-0 gw15~10 bx3-0 bw15~10",
    },
];

/// `(channel, slot, field bit, stream position)`; channel 3 is the partition field.
type PlanBit = (u8, u8, u8, u8);

fn plans() -> &'static [(Vec<PlanBit>, u32); 14] {
    static PLANS: OnceLock<[(Vec<PlanBit>, u32); 14]> = OnceLock::new();
    PLANS.get_or_init(|| {
        std::array::from_fn(|m| {
            let mode = &MODES[m];
            let mut pos: u32 = if mode.code < 2 { 2 } else { 5 };
            let mut plan = Vec::new();
            for tok in mode.layout.split(' ') {
                if tok == "d" {
                    for b in 0..5 {
                        plan.push((3, 0, b, pos as u8));
                        pos += 1;
                    }
                    continue;
                }
                let name = tok.as_bytes();
                let chan = match name[0] {
                    b'r' => 0,
                    b'g' => 1,
                    _ => 2,
                };
                let slot = match name[1] {
                    b'w' => 0,
                    b'x' => 1,
                    b'y' => 2,
                    _ => 3,
                };
                let body = &tok[2..];
                let bits: Vec<u8> = if let Some((hi, lo)) = body.split_once('~') {
                    let (hi, lo): (u8, u8) = (hi.parse().unwrap(), lo.parse().unwrap());
                    (lo..=hi).rev().collect()
                } else if let Some((hi, lo)) = body.split_once('-') {
                    let (hi, lo): (u8, u8) = (hi.parse().unwrap(), lo.parse().unwrap());
                    (lo..=hi).collect()
                } else {
                    vec![body.parse().unwrap()]
                };
                for b in bits {
                    plan.push((chan, slot, b, pos as u8));
                    pos += 1;
                }
            }
            (plan, pos)
        })
    })
}

/// BCPack keeps the two reversed high-bit groups of modes 13 and 14 in natural order: swap them into the order
/// the block stores them in. The mapping is its own inverse.
pub(super) fn stream_w(mode: usize, w: u32) -> u32 {
    match mode {
        12 => (w & 0x3FF) | (w >> 10 & 1) << 11 | (w >> 11 & 1) << 10,
        13 => (0..6).fold(w & 0x3FF, |out, k| out | (w >> (10 + k) & 1) << (15 - k)),
        _ => w,
    }
}

/// How well a built-in history entry is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Entry<T> {
    Unknown,
    Provisional(T),
    Confirmed(T),
}

pub(super) struct History {
    pub endpoints: Vec<Entry<Endpoints>>,
    pub indices: Vec<Entry<Indices>>,
}

impl History {
    /// No built-in entry known.
    pub fn empty() -> History {
        History {
            endpoints: vec![Entry::Unknown; 256],
            indices: vec![Entry::Unknown; 256],
        }
    }
}

/// Where each block's endpoint and index chains start (a history entry, or -1 for none or an earlier block chain
/// that starts at the default), for measuring which entries a chunk needs.
#[derive(Default)]
pub(super) struct Trace {
    pub endpoint_root: Vec<i16>,
    pub index_root: Vec<i16>,
    pub mode: Vec<u8>,
    pub faulty: usize,
}

/// The BC6H block of mode index `m` (0..=13): endpoint fields `[w, x, y, z]` per channel in stored bit order, the
/// partition and the 16 pixel indices (3 bits in two-region modes, 4 otherwise).
pub(super) fn pack(m: usize, part: usize, field: &[[u32; 4]; 3], ix: &[u32; 16]) -> [u8; 16] {
    let mode = &MODES[m];
    let (plan, header) = &plans()[m];
    let mut acc = u128::from(mode.code);
    for &(c, q, bit, pos) in plan {
        let v = if c == 3 {
            part as u32
        } else {
            field[usize::from(c)][usize::from(q)]
        };
        acc |= u128::from(v >> bit & 1) << pos;
    }
    let (ibits, second) = if mode.two_regions {
        (3, usize::from(ANCHOR2[part]))
    } else {
        (4, usize::MAX)
    };
    let mut at = *header;
    for (p, v) in ix.iter().enumerate() {
        let nb = if p == 0 || p == second {
            ibits - 1
        } else {
            ibits
        };
        acc |= u128::from(v & ((1 << nb) - 1)) << at;
        at += nb;
    }
    debug_assert_eq!(at, 128);
    acc.to_le_bytes()
}

#[inline(always)]
fn interleave(a: u8, b: u8, bits: u32) -> u32 {
    (0..bits).fold(0, |acc, k| {
        acc | u32::from(a >> k & 1) << (2 * k) | u32::from(b >> k & 1) << (2 * k + 1)
    })
}

#[inline(always)]
fn unzig_wide(v: u32) -> i32 {
    (v >> 1) as i32 ^ -((v & 1) as i32)
}

const fn parity_table(shift: usize) -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut s = 0;
    while s < 256 {
        let mut k = 0;
        while k < 4 {
            t[s] |= (((s >> (2 * k + shift)) & 1) as u8) << k;
            k += 1;
        }
        s += 1;
    }
    t
}

static EVEN: [u8; 256] = parity_table(0);
static ODD: [u8; 256] = parity_table(1);

fn missing(block: usize, distance: usize) -> BcPackError {
    BcPackError::History {
        codec: BcPackCodec::Bc6h,
        block,
        entry: (256 + block - distance) as u8,
        status: HistoryStatus::Unknown,
    }
}

/// The built-in history of BC6H: nothing is recovered yet.
fn builtin() -> &'static History {
    static HISTORY: OnceLock<History> = OnceLock::new();
    HISTORY.get_or_init(History::empty)
}

/// Whether any built-in history entry is recovered; without one no real chunk can be decoded.
pub fn has_history() -> bool {
    let h = builtin();
    h.endpoints.iter().any(|e| *e != Entry::Unknown)
}

pub fn bc6h(
    lanes: &LaneSymbols,
    out: &mut [u8],
    opts: &DecodeOptions,
) -> Result<usize, BcPackError> {
    rebuild(lanes, out, builtin(), opts, None)
}

pub(super) fn rebuild(
    lanes: &LaneSymbols,
    out: &mut [u8],
    history: &History,
    opts: &DecodeOptions,
    mut trace: Option<&mut Trace>,
) -> Result<usize, BcPackError> {
    let codec = BcPackCodec::Bc6h;
    let n = out.len() / 16;
    let (l0, l1, l15) = (lanes.lane(0), lanes.lane(1), lanes.lane(15));
    let mut ends: Vec<Endpoints> = vec![[[0; 4]; 3]; n];
    let mut idxs: Vec<Indices> = vec![[0; 16]; n];
    let mut tainted = vec![(false, false); n];
    let (mut e_root, mut i_root) = (vec![-1i16; n], vec![-1i16; n]);
    let (mut faulty, mut first_fault) = (0usize, 0usize);
    if let Some(t) = trace.as_deref_mut() {
        *t = Trace::default();
    }

    for i in 0..n {
        let (a, b) = (2 * i, 2 * i + 1);
        let m = usize::from(l15[a]);
        if m > 13 {
            return Err(BcPackError::Signature { codec, block: i });
        }
        let mode = &MODES[m];
        let part = usize::from(l15[b].min(31));
        let di = usize::from(interleave4(l1[a], l1[b]));
        let de = (di as i32 + unzig(interleave4(l0[a], l0[b]))) as usize & 255;

        let unknown = |status, d| BcPackError::History {
            codec,
            block: i,
            entry: (256 + i - d) as u8,
            status,
        };
        let eref = if de == 0 {
            MID_ENDPOINTS
        } else if de <= i {
            tainted[i].0 = tainted[i - de].0;
            e_root[i] = e_root[i - de];
            ends[i - de]
        } else {
            let entry = 256 + i - de;
            e_root[i] = entry as i16;
            match history.endpoints[entry] {
                Entry::Confirmed(v) => v,
                Entry::Provisional(v) if opts.allow_provisional => {
                    tainted[i].0 = true;
                    v
                }
                Entry::Provisional(_) => return Err(unknown(HistoryStatus::Provisional, de)),
                Entry::Unknown => return Err(missing(i, de)),
            }
        };
        let iref = if di == 0 {
            MID_INDICES
        } else if di <= i {
            tainted[i].1 = tainted[i - di].1;
            i_root[i] = i_root[i - di];
            idxs[i - di]
        } else {
            let entry = 256 + i - di;
            i_root[i] = entry as i16;
            match history.indices[entry] {
                Entry::Confirmed(v) => v,
                Entry::Provisional(v) if opts.allow_provisional => {
                    tainted[i].1 = true;
                    v
                }
                Entry::Provisional(_) => return Err(unknown(HistoryStatus::Provisional, di)),
                Entry::Unknown => return Err(missing(i, di)),
            }
        };

        let (base, w_shift) = (mode.base, 16 - mode.base);
        let mw = (1i32 << base) - 1;
        let mut field = [[0u32; 4]; 3];
        let mut state: Endpoints = [[0; 4]; 3];
        for c in 0..3 {
            let dc = mode.delta[c];
            let md = (1i32 << dc) - 1;
            let sh = 6u32.saturating_sub(dc);
            let dz0 = unzig_wide(interleave(lanes.lane(2 + c)[a], lanes.lane(2 + c)[b], 8));
            let dz1 = unzig_wide(interleave(lanes.lane(5 + c)[a], lanes.lane(5 + c)[b], 5));
            let dz2 = unzig(lanes.lane(8 + c)[a]);
            let dz3 = unzig(lanes.lane(8 + c)[b]);
            let r = eref[c].map(i32::from);
            let w = ((r[0] >> w_shift) + dz0) & mw;
            let x = ((r[1] >> (10 - dc)) + dz1) & md;
            let x10 = (x << (10 - dc)) as u16;
            if mode.two_regions {
                let y = ((r[2] >> sh) + dz2) & md;
                let z = ((r[3] >> sh) + dz3) & md;
                state[c] = [
                    (w << w_shift) as u16,
                    x10,
                    (y << sh) as u16,
                    (z << sh) as u16,
                ];
                field[c] = [stream_w(m, w as u32), x as u32, y as u32, z as u32];
            } else {
                let v = (w & md) ^ if mode.transformed { 1 << (dc - 1) } else { 0 };
                let low_y = if dc >= 6 { v >> (dc - 6) } else { v << sh };
                state[c] = [(w << w_shift) as u16, x10, low_y as u16, x10 >> 4];
                field[c] = [stream_w(m, w as u32), x as u32, 0, 0];
            }
        }
        ends[i] = state;

        let (ibits, ishift) = if mode.two_regions { (3, 1) } else { (4, 0) };
        let imask = (1i32 << ibits) - 1;
        let mut ix = [0u32; 16];
        let mut kept = [0u8; 16];
        for y in 0..4 {
            let (hi, lo) = if y < 3 { (&ODD, &EVEN) } else { (&EVEN, &ODD) };
            let (sa, sb) = (
                usize::from(lanes.lane(11 + y)[a]),
                usize::from(lanes.lane(11 + y)[b]),
            );
            let deltas = [hi[sa], lo[sa], hi[sb], lo[sb]];
            for (off, d) in deltas.into_iter().enumerate() {
                let p = y * 4 + off;
                let v = ((i32::from(iref[p]) >> ishift) + unzig(d)) & imask;
                ix[p] = v as u32;
                kept[p] = (v << ishift) as u8;
            }
        }
        idxs[i] = kept;

        let second = if mode.two_regions {
            usize::from(ANCHOR2[part])
        } else {
            usize::MAX
        };
        let mut bad = ix[0] >> (ibits - 1) != 0;
        if mode.two_regions {
            bad |= ix[second] >> 2 != 0;
        }
        let block = pack(m, part, &field, &ix);
        if bad {
            if faulty == 0 {
                first_fault = i;
            }
            faulty += 1;
        }
        out[i * 16..i * 16 + 16].copy_from_slice(&block);
        if let Some(t) = trace.as_deref_mut() {
            t.mode.push(m as u8);
        }
    }

    if let Some(t) = trace {
        t.endpoint_root = e_root;
        t.index_root = i_root;
        t.faulty = faulty;
        return Ok(tainted.iter().filter(|t| t.0 || t.1).count());
    }
    let provisional = tainted.iter().filter(|t| t.0 || t.1).count();
    if faulty > 0 && (provisional == 0 || faulty > (n / 1024).max(2)) {
        return Err(BcPackError::Inconsistent {
            codec,
            block: first_fault,
            blocks: faulty,
        });
    }
    Ok(provisional)
}
