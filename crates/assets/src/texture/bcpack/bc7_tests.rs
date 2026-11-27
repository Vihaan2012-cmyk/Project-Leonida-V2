//! BC7 block stage on hand-made chunks: a test-only encoder turns block specs into lane symbols (zigzag deltas
//! against chosen predictors), the lane array is encoded and replayed, and `decode_with` must return the blocks
//! a plain BC7 packer writes for the same specs.

#![allow(clippy::needless_range_loop)]

use super::bptc::{ANCHOR2, ANCHOR3A, ANCHOR3B, PART2, PART3, tables};
use super::testenc::{Frag, Rng, encode_flush, interleave};
use super::{BcPackCodec, BcPackError, DecodeOptions, HistoryStatus, decode_with};
use crate::texture::bcn;

const SUBSETS: [usize; 8] = [3, 2, 3, 2, 1, 1, 1, 2];
const COLOUR_BITS: [u32; 8] = [4, 6, 5, 7, 5, 7, 7, 5];
const ALPHA_BITS: [u32; 8] = [0, 0, 0, 0, 6, 8, 7, 5];
const PBITS: [u8; 8] = [1, 2, 0, 1, 0, 0, 1, 1];
const INDEX_BITS: [u32; 8] = [3, 3, 2, 2, 3, 2, 4, 2];
const PARTITION_BITS: [u32; 8] = [4, 6, 6, 6, 0, 0, 0, 6];
const LANES: [[i8; 4]; 3] = [[2, 3, 4, 5], [6, 7, 8, 9], [10, 5, 9, -1]];

#[derive(Clone, Copy)]
struct Spec {
    mode: usize,
    part: usize,
    rot: usize,
    isel: usize,
    /// Endpoint values per subset, endpoint and lane channel, with the p-bit as the low bit of channel 0.
    value: [[[u32; 4]; 2]; 3],
    idx1: [u32; 16],
    idx2: [u32; 16],
}

fn widths(s: &Spec) -> [u32; 4] {
    let (cb, ab) = (COLOUR_BITS[s.mode], ALPHA_BITS[s.mode]);
    let mut w = [cb, cb, cb, ab];
    if s.rot != 0 {
        w[s.rot - 1] = ab;
        w[3] = cb;
    }
    w
}

fn field_bits(s: &Spec, c: usize) -> u32 {
    widths(s)[c] + u32::from(PBITS[s.mode] != 0 && c == 0)
}

fn subset_of(s: &Spec, p: usize) -> usize {
    match SUBSETS[s.mode] {
        2 => usize::from(PART2[s.part] >> p & 1),
        3 => (PART3[s.part] >> (2 * p) & 3) as usize,
        _ => 0,
    }
}

fn anchor_of(s: &Spec, sub: usize) -> usize {
    match (SUBSETS[s.mode], sub) {
        (_, 0) => 0,
        (2, _) => usize::from(ANCHOR2[s.part]),
        (_, 1) => usize::from(ANCHOR3A[s.part]),
        _ => usize::from(ANCHOR3B[s.part]),
    }
}

struct Writer {
    acc: u128,
    pos: u32,
}

impl Writer {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= u128::from(v & ((1 << bits) - 1)) << self.pos;
        self.pos += bits;
    }
}

/// The standard BC7 bit layout of a spec.
fn pack_block(s: &Spec) -> [u8; 16] {
    let m = s.mode;
    let ns = SUBSETS[m];
    let mut w = Writer {
        acc: 1 << m,
        pos: m as u32 + 1,
    };
    w.put(s.part as u32, PARTITION_BITS[m]);
    if m == 4 || m == 5 {
        w.put(s.rot as u32, 2);
    }
    if m == 4 {
        w.put(s.isel as u32, 1);
    }
    let (cb, ab) = (COLOUR_BITS[m], ALPHA_BITS[m]);
    let stream = |v: u32, c: usize| if PBITS[m] != 0 && c == 0 { v >> 1 } else { v };
    for slot in 0..3 {
        let c = if s.rot != 0 && slot == s.rot - 1 {
            3
        } else {
            slot
        };
        for sub in 0..ns {
            for e in 0..2 {
                w.put(stream(s.value[sub][e][c], c), cb);
            }
        }
    }
    if ab != 0 {
        let c = if s.rot != 0 { s.rot - 1 } else { 3 };
        for sub in 0..ns {
            for e in 0..2 {
                w.put(stream(s.value[sub][e][c], c), ab);
            }
        }
    }
    match PBITS[m] {
        1 => {
            for sub in 0..ns {
                for e in 0..2 {
                    w.put(s.value[sub][e][0] & 1, 1);
                }
            }
        }
        2 => {
            for sub in 0..ns {
                w.put(s.value[sub][0][0] & 1, 1);
            }
        }
        _ => {}
    }
    let (first, fb, second, sb) = match m {
        4 => (&s.idx2, 2, Some(&s.idx1), 3),
        5 => (&s.idx1, 2, Some(&s.idx2), 2),
        _ => (&s.idx1, INDEX_BITS[m], None, 0),
    };
    for p in 0..16 {
        let anchor = p == anchor_of(s, subset_of(s, p));
        w.put(first[p], if anchor { fb - 1 } else { fb });
    }
    if let Some(sec) = second {
        for (p, &v) in sec.iter().enumerate() {
            w.put(v, if p == 0 { sb - 1 } else { sb });
        }
    }
    assert_eq!(w.pos, 128);
    w.acc.to_le_bytes()
}

/// The predictor bytes a block leaves behind: endpoints scaled to 8 bits, indices scaled to 4 and 2 bits.
fn stored(s: &Spec) -> ([u8; 24], [u8; 32]) {
    let mut e = [0u8; 24];
    let ns = SUBSETS[s.mode];
    for sub in 0..ns {
        for ep in 0..2 {
            for c in 0..4 {
                let lane = LANES[sub][c];
                if widths(s)[c] == 0 || lane < 0 {
                    continue;
                }
                e[sub * 8 + ep * 4 + c] = (s.value[sub][ep][c] << (8 - field_bits(s, c))) as u8;
            }
        }
    }
    for sub in ns..3 {
        e.copy_within(0..8, sub * 8);
    }
    let mut ix = [0u8; 32];
    for p in 0..16 {
        ix[p] = (s.idx1[p] << (4 - INDEX_BITS[s.mode])) as u8;
        if s.mode == 4 || s.mode == 5 {
            ix[16 + p] = (s.idx2[p] << 2) as u8;
        }
    }
    (e, ix)
}

fn zig(delta: i32, modulus: i32) -> u32 {
    let d = delta.rem_euclid(modulus);
    let signed = if d >= modulus / 2 { d - modulus } else { d };
    (if signed < 0 {
        -signed * 2 - 1
    } else {
        signed * 2
    }) as u32
}

fn split_nibbles(v: u8) -> (u8, u8) {
    let pick = |v: u8| (0..4).fold(0, |acc, k| acc | (v >> (2 * k) & 1) << k);
    (pick(v), pick(v >> 1))
}

fn spread(d: u32) -> u8 {
    (0..4).fold(0, |acc, k| acc | ((d >> k & 1) as u8) << (2 * k))
}

/// One row of an index set: four zigzag deltas, two per symbol, even and odd bits swapping roles on the last row.
#[allow(clippy::too_many_arguments)]
fn put_indices(
    lanes: &mut [Vec<u8>],
    (a, b): (usize, usize),
    y: usize,
    lane: usize,
    set: &[u32; 16],
    before: &[u8],
    shift: u32,
    bits: u32,
) {
    let d: Vec<u32> = (0..4)
        .map(|off| {
            let p = y * 4 + off;
            zig(set[p] as i32 - i32::from(before[p] >> shift), 1 << bits)
        })
        .collect();
    let sym = |hi: u32, lo: u32| {
        if y < 3 {
            spread(hi) << 1 | spread(lo)
        } else {
            spread(lo) << 1 | spread(hi)
        }
    };
    lanes[lane][a] = sym(d[0], d[1]);
    lanes[lane][b] = sym(d[2], d[3]);
}

/// Lane symbols of every block against the given distances (`de`, `di`); history distances predict from zeros.
fn encode_lanes(specs: &[Spec], dists: &[(usize, usize)]) -> Vec<Vec<u8>> {
    let n = specs.len();
    let mut lanes = vec![vec![0u8; 2 * n]; 16];
    let kept: Vec<_> = specs.iter().map(stored).collect();
    for (i, s) in specs.iter().enumerate() {
        let (a, b) = (2 * i, 2 * i + 1);
        let (de, di) = dists[i];
        let rep = if de == 0 || de > i {
            [0u8; 24]
        } else {
            kept[i - de].0
        };
        let rix = if di == 0 || di > i {
            [0u8; 32]
        } else {
            kept[i - di].1
        };
        let (x, y) = split_nibbles(zig(de as i32 - di as i32, 256) as u8);
        (lanes[0][a], lanes[0][b]) = (x, y);
        let (x, y) = split_nibbles(di as u8);
        (lanes[1][a], lanes[1][b]) = (x, y);
        let (x, y) = tables().cell_of(s.mode as u8, s.part as u8, s.rot as u8, s.isel as u8);
        (lanes[15][a], lanes[15][b]) = (x, y);
        for sub in 0..SUBSETS[s.mode] {
            for c in 0..4 {
                let lane = LANES[sub][c];
                if widths(s)[c] == 0 || lane < 0 {
                    continue;
                }
                let bits = field_bits(s, c);
                for e in 0..2 {
                    let before = i32::from(rep[sub * 8 + e * 4 + c]) >> (8 - bits);
                    let sym = zig(s.value[sub][e][c] as i32 - before, 1 << bits);
                    lanes[lane as usize][a + e] = sym as u8;
                }
            }
        }
        let rows2 = [6, 7, 8, 10];
        let ib = INDEX_BITS[s.mode];
        for yy in 0..4 {
            put_indices(
                &mut lanes,
                (a, b),
                yy,
                11 + yy,
                &s.idx1,
                &rix[..16],
                4 - ib,
                ib,
            );
            if s.mode == 4 || s.mode == 5 {
                put_indices(&mut lanes, (a, b), yy, rows2[yy], &s.idx2, &rix[16..], 2, 2);
            }
        }
    }
    lanes
}

fn pack_lanes(lanes: &[Vec<u8>], n: usize) -> Vec<u8> {
    let widths = BcPackCodec::Bc7.lane_widths().unwrap();
    let bytes = widths
        .iter()
        .zip(lanes)
        .map(|(&w, d)| {
            for &s in d {
                assert!(u32::from(s) < 1 << w, "symbol {s} exceeds {w} bits");
            }
            let chunks: Vec<(Frag, &[u8])> = d
                .chunks(3000)
                .map(|c| {
                    (
                        Frag {
                            z: 0,
                            q: 6,
                            count: 0,
                        },
                        c,
                    )
                })
                .collect();
            encode_flush(w, &chunks)
        })
        .collect();
    interleave(widths, bytes, 2 * n)
}

fn expected(specs: &[Spec]) -> Vec<u8> {
    specs.iter().flat_map(pack_block).collect()
}

fn random_spec(
    rng: &mut Rng,
    mode: usize,
    prior: (&[u8; 24], &[u8; 32]),
    de: bool,
    di: bool,
) -> Spec {
    let part = loop {
        let part = match mode {
            0 => 1 + (rng.next() % 15) as usize,
            4..=6 => 0,
            _ => (rng.next() % 64) as usize,
        };
        let (a, b) = tables().cell_of(mode as u8, part as u8, 0, 0);
        if !tables().signature(a, b).unwrap().inferred {
            break part;
        }
    };
    let (rot, isel) = match mode {
        4 => ((rng.next() % 4) as usize, (rng.next() % 2) as usize),
        5 => ((rng.next() % 4) as usize, 0),
        _ => (0, 0),
    };
    let mut s = Spec {
        mode,
        part,
        rot,
        isel,
        value: [[[0; 4]; 2]; 3],
        idx1: [0; 16],
        idx2: [0; 16],
    };
    for sub in 0..SUBSETS[mode] {
        for e in 0..2 {
            for c in 0..4 {
                if widths(&s)[c] == 0 || LANES[sub][c] < 0 {
                    continue;
                }
                let bits = field_bits(&s, c);
                let mask = (1i32 << bits) - 1;
                let v = if de {
                    (i32::from(prior.0[sub * 8 + e * 4 + c]) >> (8 - bits))
                        + (rng.next() % 7) as i32
                        - 3
                } else {
                    (rng.next() % 8) as i32
                };
                s.value[sub][e][c] = (v & mask) as u32;
            }
        }
        if PBITS[mode] == 2 {
            let low = s.value[sub][0][0] & 1;
            s.value[sub][1][0] = s.value[sub][1][0] & !1 | low;
        }
    }
    let ib = INDEX_BITS[mode];
    for p in 0..16 {
        let base = if di {
            i32::from(prior.1[p]) >> (4 - ib)
        } else {
            0
        };
        let v = (base + (rng.next() % 5) as i32 - 2) & ((1 << ib) - 1);
        s.idx1[p] = v as u32;
        if mode == 4 || mode == 5 {
            let base = if di {
                i32::from(prior.1[16 + p]) >> 2
            } else {
                0
            };
            s.idx2[p] = ((base + (rng.next() % 3) as i32 - 1) & 3) as u32;
        }
    }
    let (first_bits, second_bits) = match mode {
        4 => (2, 3),
        5 => (2, 2),
        _ => (ib, 0),
    };
    for p in 0..16 {
        if p == anchor_of(&s, subset_of(&s, p)) {
            let set = if mode == 4 { &mut s.idx2 } else { &mut s.idx1 };
            set[p] &= (1 << (first_bits - 1)) - 1;
        }
    }
    if second_bits != 0 {
        let set = if mode == 4 { &mut s.idx1 } else { &mut s.idx2 };
        set[0] &= (1 << (second_bits - 1)) - 1;
    }
    s
}

/// A chunk of `n` blocks of random modes (or one fixed mode) chained to earlier blocks.
fn chunk(seed: u64, n: usize, fixed: Option<usize>) -> (Vec<Spec>, Vec<(usize, usize)>) {
    let mut rng = Rng(seed);
    let mut specs: Vec<Spec> = Vec::new();
    let mut dists = Vec::new();
    for i in 0..n {
        let pick = |rng: &mut Rng| match rng.next() % 3 {
            0 => 0,
            1 => usize::from(i > 0),
            _ => 1 + rng.next() as usize % i.clamp(1, 255),
        };
        let de = if i == 0 { 0 } else { pick(&mut rng) };
        let di = if i == 0 { 0 } else { pick(&mut rng) };
        let mode = fixed.unwrap_or((rng.next() % 8) as usize);
        let zero = ([0u8; 24], [0u8; 32]);
        let (ke, ki) = (
            if de > 0 {
                stored(&specs[i - de]).0
            } else {
                zero.0
            },
            if di > 0 {
                stored(&specs[i - di]).1
            } else {
                zero.1
            },
        );
        specs.push(random_spec(&mut rng, mode, (&ke, &ki), de > 0, di > 0));
        dists.push((de, di));
    }
    (specs, dists)
}

fn decode(
    specs: &[Spec],
    lanes: Vec<Vec<u8>>,
    opts: DecodeOptions,
) -> Result<super::Decoded, BcPackError> {
    let packed = pack_lanes(&lanes, specs.len());
    decode_with(BcPackCodec::Bc7, &packed, specs.len() * 16, &opts)
}

#[test]
fn every_mode_round_trips_through_chained_blocks() {
    for mode in 0..8 {
        let (specs, dists) = chunk(0x9e37 + mode as u64, 48, Some(mode));
        let got = decode(
            &specs,
            encode_lanes(&specs, &dists),
            DecodeOptions::default(),
        )
        .unwrap();
        assert_eq!(got.provisional_blocks, 0);
        assert_eq!(got.data, expected(&specs), "mode {mode}");
    }
}

#[test]
fn mixed_modes_round_trip_with_distances_up_to_255() {
    let (specs, dists) = chunk(0xbc7, 520, None);
    assert!(dists.iter().any(|d| d.0 > 128 || d.1 > 128));
    let got = decode(
        &specs,
        encode_lanes(&specs, &dists),
        DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(got.data, expected(&specs));
    let modes: std::collections::BTreeSet<_> = specs.iter().map(|s| s.mode).collect();
    assert_eq!(modes.len(), 8);
}

#[test]
fn the_packer_agrees_with_the_reference_block_decoder() {
    let mut s = Spec {
        mode: 6,
        part: 0,
        rot: 0,
        isel: 0,
        value: [[[0; 4]; 2]; 3],
        idx1: [0; 16],
        idx2: [0; 16],
    };
    s.value[0][0] = [0x81, 0x40, 0x23, 0x5f];
    s.value[0][1] = [0x10, 0x20, 0x30, 0x40];
    let px = bcn::bc7_block(&pack_block(&s));
    assert_eq!(px[0], [0x81, 0x81, 0x47, 0xbf]);
    s.mode = 5;
    s.rot = 1;
    s.value[0][0] = [0xe5, 0x40, 0x23, 0x55];
    s.value[0][1] = [0; 4];
    let px = bcn::bc7_block(&pack_block(&s));
    assert_eq!(px[0], [0xe5, 0x81, 0x46, 0xab]);
}

#[test]
fn an_all_zero_mode_zero_block_stays_zero() {
    let zero = Spec {
        mode: 0,
        part: 0,
        rot: 0,
        isel: 0,
        value: [[[0; 4]; 2]; 3],
        idx1: [0; 16],
        idx2: [0; 16],
    };
    let mut rng = Rng(5);
    let live = random_spec(&mut rng, 6, (&[0; 24], &[0; 32]), false, false);
    let specs = [live, zero, live];
    let lanes = encode_lanes(&specs, &[(0, 0); 3]);
    let got = decode(&specs, lanes, DecodeOptions::default()).unwrap();
    assert_eq!(&got.data[16..32], &[0u8; 16]);
    assert_eq!(&got.data[..16], &pack_block(&live));
}

#[test]
fn a_built_in_history_reference_is_refused() {
    let (specs, dists) = chunk(11, 8, Some(6));
    for index_side in [false, true] {
        let mut d = dists.clone();
        if index_side {
            d[3].1 = 5;
        } else {
            d[3].0 = 9;
        }
        let err = decode(&specs, encode_lanes(&specs, &d), DecodeOptions::default()).unwrap_err();
        let entry = (256 + 3 - if index_side { 5 } else { 9 }) as u8;
        assert_eq!(
            err,
            BcPackError::History {
                codec: BcPackCodec::Bc7,
                block: 3,
                entry,
                status: HistoryStatus::Unknown
            }
        );
        assert!(err.is_missing_constants());
    }
}

#[test]
fn signatures_outside_the_table_are_refused() {
    let (specs, dists) = chunk(12, 6, Some(6));
    let mut lanes = encode_lanes(&specs, &dists);
    (lanes[15][4], lanes[15][5]) = (31, 15);
    let err = decode(&specs, lanes, DecodeOptions::default()).unwrap_err();
    assert_eq!(
        err,
        BcPackError::Signature {
            codec: BcPackCodec::Bc7,
            block: 2
        }
    );
    assert!(err.is_missing_constants());
}

#[test]
fn inferred_signatures_need_the_provisional_option() {
    let (specs, _) = chunk(13, 6, Some(6));
    let mut lanes = encode_lanes(&specs, &[(0, 0); 6]);
    let (a, b) = (0..32u8)
        .flat_map(|a| (0..16u8).map(move |b| (a, b)))
        .find(|&(a, b)| tables().signature(a, b).is_some_and(|s| s.inferred))
        .unwrap();
    (lanes[15][0], lanes[15][1]) = (a, b);
    let strict = decode(&specs, lanes.clone(), DecodeOptions::default());
    assert!(matches!(
        strict,
        Err(BcPackError::Signature { block: 0, .. })
    ));
    let loose = decode(
        &specs,
        lanes,
        DecodeOptions {
            allow_provisional: true,
        },
    )
    .unwrap();
    assert_eq!(loose.provisional_blocks, 1);
    assert_eq!(&loose.data[16..], &expected(&specs)[16..]);
}

#[test]
fn a_set_anchor_index_bit_is_reported() {
    let (mut specs, dists) = chunk(14, 5, Some(6));
    specs[2].idx1[0] |= 8;
    let err = decode(
        &specs,
        encode_lanes(&specs, &dists),
        DecodeOptions::default(),
    )
    .unwrap_err();
    assert_eq!(
        err,
        BcPackError::Inconsistent {
            codec: BcPackCodec::Bc7,
            block: 2,
            blocks: 1
        }
    );
}

#[test]
fn unequal_shared_p_bits_are_reported() {
    let (mut specs, dists) = chunk(15, 5, Some(1));
    specs[1].value[0][1][0] ^= 1;
    let err = decode(
        &specs,
        encode_lanes(&specs, &dists),
        DecodeOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        BcPackError::Inconsistent {
            block: 1,
            blocks: 1,
            ..
        }
    ));
}

#[test]
fn bc7_is_a_supported_codec_and_bc6h_is_not() {
    assert!(super::is_supported(BcPackCodec::Bc7));
    assert!(!super::is_supported(BcPackCodec::Bc6h));
}
