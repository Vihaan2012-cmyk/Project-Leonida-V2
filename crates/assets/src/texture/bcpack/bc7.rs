//! BC7 block reconstruction for chunks that never reach the built-in history.
//!
//! Lane 15 carries two symbols per block that name its mode, partition, rotation and index selection. Lanes 0 and
//! 1 carry the backward distances to the block whose endpoints (`de`) and indices (`di`) predict this one: 0 means
//! zeros, `1..=i` an earlier block of the chunk, anything larger a built-in history entry, which is not recovered
//! for BC7 and so is refused. Endpoint lanes are zigzag deltas on the predictor's endpoint bytes read at this
//! mode's field width, with the p-bit riding as an extra low bit of the first channel; index lanes are zigzag
//! deltas per pixel on 4-bit nibbles that are bit-interleaved in pairs.

use super::blocks::{interleave4, unzig};
use super::bptc::{ANCHOR2, ANCHOR3A, ANCHOR3B, PART2, PART3, tables};
use super::lanes::LaneSymbols;
use super::{BcPackCodec, BcPackError, DecodeOptions, HistoryStatus};

const SUBSETS: [usize; 8] = [3, 2, 3, 2, 1, 1, 1, 2];
const COLOUR_BITS: [u32; 8] = [4, 6, 5, 7, 5, 7, 7, 5];
const ALPHA_BITS: [u32; 8] = [0, 0, 0, 0, 6, 8, 7, 5];
/// 0 no p-bits, 1 one per endpoint, 2 one shared by the two endpoints of a subset.
const PBITS: [u8; 8] = [1, 2, 0, 1, 0, 0, 1, 1];
/// Index bits of the first index set.
const INDEX_BITS: [u32; 8] = [3, 3, 2, 2, 3, 2, 4, 2];
const PARTITION_BITS: [u32; 8] = [4, 6, 6, 6, 0, 0, 0, 6];
/// Lane of each channel of each subset's endpoints, -1 where the channel has none.
const ENDPOINT_LANES: [[i8; 4]; 3] = [[2, 3, 4, 5], [6, 7, 8, 9], [10, 5, 9, -1]];

const fn nibble_table(shift: usize) -> [u8; 256] {
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

/// The even and odd bits of an index symbol, packed to a nibble each.
static EVEN: [u8; 256] = nibble_table(0);
static ODD: [u8; 256] = nibble_table(1);

/// Bit packer for one 128-bit block, least significant bit first.
struct Bits {
    acc: u128,
    pos: u32,
}

impl Bits {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= u128::from(v & ((1u32 << bits) - 1)) << self.pos;
        self.pos += bits;
    }
}

fn fault_history(block: usize, distance: usize) -> BcPackError {
    BcPackError::History {
        codec: BcPackCodec::Bc7,
        block,
        entry: (256 + block - distance) as u8,
        status: HistoryStatus::Unknown,
    }
}

pub fn bc7(
    lanes: &LaneSymbols,
    out: &mut [u8],
    opts: &DecodeOptions,
) -> Result<usize, BcPackError> {
    let codec = BcPackCodec::Bc7;
    let n = out.len() / 16;
    let tables = tables();
    let (l0, l1, l15) = (lanes.lane(0), lanes.lane(1), lanes.lane(15));
    let rows1: [&[u8]; 4] = std::array::from_fn(|k| lanes.lane(11 + k));
    let rows2: [&[u8]; 4] = [lanes.lane(6), lanes.lane(7), lanes.lane(8), lanes.lane(10)];
    let mut endpoints = vec![[0u8; 24]; n];
    let mut indices = vec![[0u8; 32]; n];
    let mut tainted = vec![false; n];
    let (mut faulty, mut first_fault) = (0usize, 0usize);

    for i in 0..n {
        let (a, b) = (2 * i, 2 * i + 1);
        let sig = tables
            .signature(l15[a], l15[b])
            .ok_or(BcPackError::Signature { codec, block: i })?;
        if sig.inferred && !opts.allow_provisional {
            return Err(BcPackError::Signature { codec, block: i });
        }
        let di = usize::from(interleave4(l1[a], l1[b]));
        let de = (di as i32 + unzig(interleave4(l0[a], l0[b]))) as usize & 255;
        for d in [de, di] {
            if d > i {
                return Err(fault_history(i, d));
            }
        }
        tainted[i] = sig.inferred || (de > 0 && tainted[i - de]) || (di > 0 && tainted[i - di]);
        let rep = if de == 0 {
            [0u8; 24]
        } else {
            endpoints[i - de]
        };
        let rix = if di == 0 { [0u8; 32] } else { indices[i - di] };

        let mode = usize::from(sig.mode);
        let (part, rot) = (usize::from(sig.part), usize::from(sig.rot));
        let ns = SUBSETS[mode];
        let (cb, ab) = (COLOUR_BITS[mode], ALPHA_BITS[mode]);
        let mut width = [cb, cb, cb, ab];
        if rot != 0 {
            width[rot - 1] = ab;
            width[3] = cb;
        }

        let mut value = [[[0u32; 4]; 2]; 3];
        let mut stored = [0u8; 24];
        let mut pbit = [[0u32; 2]; 3];
        let mut bad = false;
        for sub in 0..ns {
            for c in 0..4 {
                let lane = ENDPOINT_LANES[sub][c];
                if width[c] == 0 || lane < 0 {
                    continue;
                }
                let bits = width[c] + u32::from(PBITS[mode] != 0 && c == 0);
                let (shift, mask) = (8 - bits, (1i32 << bits) - 1);
                let sym = lanes.lane(lane as usize);
                for e in 0..2 {
                    let at = sub * 8 + e * 4 + c;
                    let v = ((i32::from(rep[at]) >> shift) + unzig(sym[a + e])) & mask;
                    value[sub][e][c] = v as u32;
                    stored[at] = (v << shift) as u8;
                }
            }
            match PBITS[mode] {
                1 => pbit[sub] = [value[sub][0][0] & 1, value[sub][1][0] & 1],
                2 => {
                    let p = value[sub][0][0] & 1;
                    pbit[sub] = [p, p];
                    bad |= value[sub][1][0] & 1 != p;
                }
                _ => {}
            }
        }
        for sub in ns..3 {
            stored.copy_within(0..8, sub * 8);
        }
        endpoints[i] = stored;

        let ib = INDEX_BITS[mode];
        let two_sets = mode == 4 || mode == 5;
        let mut first = [0u32; 16];
        let mut second = [0u32; 16];
        let mut kept = [0u8; 32];
        for y in 0..4 {
            let (hi, lo) = if y < 3 { (&ODD, &EVEN) } else { (&EVEN, &ODD) };
            let split = |sa: u8, sb: u8| {
                [
                    hi[usize::from(sa)],
                    lo[usize::from(sa)],
                    hi[usize::from(sb)],
                    lo[usize::from(sb)],
                ]
            };
            for (off, dv) in split(rows1[y][a], rows1[y][b]).into_iter().enumerate() {
                let p = y * 4 + off;
                let shift = 4 - ib;
                let v = ((i32::from(rix[p]) >> shift) + unzig(dv)) & ((1 << ib) - 1);
                first[p] = v as u32;
                kept[p] = (v << shift) as u8;
            }
            if two_sets {
                for (off, dv) in split(rows2[y][a], rows2[y][b]).into_iter().enumerate() {
                    let p = y * 4 + off;
                    let v = ((i32::from(rix[16 + p]) >> 2) + unzig(dv)) & 3;
                    second[p] = v as u32;
                    kept[16 + p] = (v << 2) as u8;
                }
            }
        }
        indices[i] = kept;

        if mode == 0 && part == 0 && stored.iter().all(|v| *v == 0) && first.iter().all(|v| *v == 0)
        {
            continue;
        }

        let subset_of = |p: usize| match ns {
            2 => usize::from(PART2[part] >> p & 1),
            3 => (PART3[part] >> (2 * p) & 3) as usize,
            _ => 0,
        };
        let anchors = [
            0,
            usize::from(if ns == 3 {
                ANCHOR3A[part]
            } else {
                ANCHOR2[part]
            }),
            usize::from(ANCHOR3B[part]),
        ];
        let (primary, primary_bits, secondary, secondary_bits) = match mode {
            4 => (&second, 2, Some(&first), 3),
            5 => (&first, 2, Some(&second), 2),
            _ => (&first, ib, None, 0),
        };

        let mut w = Bits {
            acc: 1 << mode,
            pos: mode as u32 + 1,
        };
        w.put(part as u32, PARTITION_BITS[mode]);
        if two_sets {
            w.put(rot as u32, 2);
        }
        if mode == 4 {
            w.put(u32::from(sig.isel), 1);
        }
        let drops_pbit = |channel: usize| PBITS[mode] != 0 && channel == 0;
        for slot in 0..3 {
            let channel = if rot != 0 && slot == rot - 1 { 3 } else { slot };
            for endpoint in value[..ns].iter().flatten() {
                let v = endpoint[channel];
                w.put(if drops_pbit(channel) { v >> 1 } else { v }, cb);
            }
        }
        if ab != 0 {
            let channel = if rot != 0 { rot - 1 } else { 3 };
            for endpoint in value[..ns].iter().flatten() {
                let v = endpoint[channel];
                w.put(if drops_pbit(channel) { v >> 1 } else { v }, ab);
            }
        }
        match PBITS[mode] {
            1 => (0..ns).for_each(|sub| {
                w.put(pbit[sub][0], 1);
                w.put(pbit[sub][1], 1);
            }),
            2 => (0..ns).for_each(|sub| w.put(pbit[sub][0], 1)),
            _ => {}
        }
        for p in 0..16 {
            if p == anchors[subset_of(p)] {
                bad |= primary[p] >> (primary_bits - 1) != 0;
                w.put(primary[p], primary_bits - 1);
            } else {
                w.put(primary[p], primary_bits);
            }
        }
        if let Some(sec) = secondary {
            for (p, &v) in sec.iter().enumerate() {
                if p == 0 {
                    bad |= v >> (secondary_bits - 1) != 0;
                    w.put(v, secondary_bits - 1);
                } else {
                    w.put(v, secondary_bits);
                }
            }
        }
        bad |= w.pos != 128;
        if bad {
            if faulty == 0 {
                first_fault = i;
            }
            faulty += 1;
        }
        out[i * 16..i * 16 + 16].copy_from_slice(&w.acc.to_le_bytes());
    }

    let provisional = tainted.iter().filter(|t| **t).count();
    if faulty > 0 && (provisional == 0 || faulty > (n / 1024).max(2)) {
        return Err(BcPackError::Inconsistent {
            codec,
            block: first_fault,
            blocks: faulty,
        });
    }
    Ok(provisional)
}
