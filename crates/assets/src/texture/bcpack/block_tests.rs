//! Block reconstruction on hand-made blocks: a test-only block encoder turns BCn blocks into lane symbols with
//! the inverse of every codebook, the lane array is encoded and replayed, and `decode` must return the blocks.

use super::codebooks::*;
use super::testenc::{Frag, Rng, encode_flush, interleave};
use super::{BcPackCodec, BcPackError, DecodeOptions, HistoryStatus, decode, decode_with};

#[derive(Clone, Copy)]
enum Pred {
    None,
    Back(usize),
    History(usize),
}

fn zig(delta: i32, modulus: i32) -> u8 {
    let d = delta.rem_euclid(modulus);
    let signed = if d >= modulus / 2 { d - modulus } else { d };
    (if signed < 0 {
        -signed * 2 - 1
    } else {
        signed * 2
    }) as u8
}

/// Lane symbol for a backward distance `d` (0 = none, 1..=255 = blocks back or `256 + i - entry`).
fn distance_symbol(d: usize) -> u8 {
    if d < 128 {
        (d * 2) as u8
    } else {
        ((256 - d) * 2 - 1) as u8
    }
}

fn distance_of(i: usize, p: Pred) -> usize {
    match p {
        Pred::None => 0,
        Pred::Back(d) => d,
        Pred::History(entry) => 256 + i - entry,
    }
}

/// The two nibbles that carry a 8-bit distance symbol in a two-element block.
fn split_nibbles(v: u8) -> (u8, u8) {
    let pick = |v: u8| (0..4).fold(0, |acc, k| acc | (v >> (2 * k) & 1) << k);
    (pick(v), pick(v >> 1))
}

fn pack(codec: BcPackCodec, lanes: &[Vec<u8>], per_lane: usize) -> Vec<u8> {
    let widths = codec.lane_widths().unwrap();
    let bytes = widths
        .iter()
        .zip(lanes)
        .map(|(&w, d)| {
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
    interleave(widths, bytes, per_lane)
}

fn inverse_bytes(table: &[u8; 256]) -> [u8; 256] {
    let mut inv = [0u8; 256];
    for (rank, v) in table.iter().enumerate() {
        inv[usize::from(*v)] = rank as u8;
    }
    inv
}

fn inverse_pairs(table: &[[u8; 2]; 64]) -> [[u8; 8]; 8] {
    let mut inv = [[0u8; 8]; 8];
    for (rank, p) in table.iter().enumerate() {
        inv[usize::from(p[0])][usize::from(p[1])] = rank as u8;
    }
    inv
}

/// Rank of the index pair at pixels `2k, 2k + 1` of row `r` of a 48-bit index field.
fn row_pairs(bits: u64, r: usize) -> [(usize, usize); 2] {
    let row = bits >> (12 * r);
    let at = |s: u32| (row >> s & 7) as usize;
    [(at(0), at(3)), (at(6), at(9))]
}

fn fields(hist: &[Hist<6>; 256], entry: usize) -> [u8; 6] {
    match hist[entry] {
        Hist::Confirmed(v) | Hist::Provisional(v) => v,
        Hist::Unknown => unreachable!(),
    }
}

fn colour_of(block: &[u8]) -> [u8; 6] {
    let (c0, c1) = (
        u16::from_le_bytes([block[0], block[1]]),
        u16::from_le_bytes([block[2], block[3]]),
    );
    [
        (c0 >> 11) as u8,
        (c0 >> 5 & 63) as u8,
        (c0 & 31) as u8,
        (c1 >> 11) as u8,
        (c1 >> 5 & 63) as u8,
        (c1 & 31) as u8,
    ]
}

fn random_bc1(rng: &mut Rng, n: usize) -> Vec<u8> {
    (0..n * 8).map(|_| rng.next() as u8).collect()
}

fn encode_bc1(blocks: &[u8], preds: &[Pred]) -> Vec<u8> {
    let n = blocks.len() / 8;
    let inv = inverse_bytes(&BC1_QUADRANT);
    let mut lanes = vec![Vec::new(); 12];
    for i in 0..n {
        let b = &blocks[i * 8..i * 8 + 8];
        let base = match preds[i] {
            Pred::None => [0; 6],
            Pred::Back(d) => colour_of(&blocks[(i - d) * 8..]),
            Pred::History(e) => fields(&BC1_HISTORY, e),
        };
        lanes[0].push(distance_symbol(distance_of(i, preds[i])));
        lanes[1].push(0);
        let mods = [32, 64, 32, 32, 64, 32];
        for (k, f) in colour_of(b).into_iter().enumerate() {
            lanes[2 + k].push(zig(i32::from(f) - i32::from(base[k]), mods[k]));
        }
        let (r0, r1, r2, r3) = (b[4], b[5], b[6], b[7]);
        let quads = [
            (r0 & 15) | (r1 & 15) << 4,
            (r0 >> 4) | (r1 >> 4) << 4,
            (r2 & 15) | (r3 & 15) << 4,
            (r2 >> 4) | (r3 >> 4) << 4,
        ];
        for (k, q) in quads.into_iter().enumerate() {
            lanes[8 + k].push(inv[usize::from(q)]);
        }
    }
    pack(BcPackCodec::Bc1, &lanes, n)
}

fn mixed_preds(n: usize, rng: &mut Rng) -> Vec<Pred> {
    (0..n)
        .map(|i| match rng.next() % 4 {
            0 => Pred::None,
            1 if i >= 1 => Pred::Back(1),
            2 if i >= 3 => Pred::Back(1 + (rng.next() as usize) % i.min(200)),
            _ => Pred::None,
        })
        .collect()
}

#[test]
fn bc1_blocks_round_trip_with_every_predictor_kind() {
    let mut rng = Rng(0xB1C1);
    let n = 700;
    let blocks = random_bc1(&mut rng, n);
    let mut preds = mixed_preds(n, &mut rng);
    // Confirmed history entries (1 and 255 are both confirmed) for blocks that can reach them.
    preds[0] = Pred::History(255);
    preds[5] = Pred::History(250);
    preds[9] = Pred::History(10);
    let packed = encode_bc1(&blocks, &preds);
    let out = decode(BcPackCodec::Bc1, &packed, n * 8).unwrap();
    assert_eq!(out, blocks);
}

#[test]
fn a_provisional_history_entry_is_refused_unless_allowed_and_counted() {
    let mut rng = Rng(3);
    let n = 40;
    let blocks = random_bc1(&mut rng, n);
    assert!(matches!(BC1_HISTORY[6], Hist::Provisional(_)));
    let mut preds = vec![Pred::None; n];
    preds[2] = Pred::History(6);
    preds[3] = Pred::Back(1);
    preds[10] = Pred::Back(8);
    let packed = encode_bc1(&blocks, &preds);
    assert_eq!(
        decode(BcPackCodec::Bc1, &packed, n * 8),
        Err(BcPackError::History {
            codec: BcPackCodec::Bc1,
            block: 2,
            entry: 6,
            status: HistoryStatus::Provisional
        })
    );
    let opts = DecodeOptions {
        allow_provisional: true,
    };
    let d = decode_with(BcPackCodec::Bc1, &packed, n * 8, &opts).unwrap();
    assert_eq!(d.data, blocks);
    // Blocks 2, 3 (one back) and 10 (eight back from 10 is block 2) inherit it.
    assert_eq!(d.provisional_blocks, 3);
}

#[test]
fn an_unrecovered_history_entry_is_refused_even_when_provisional_is_allowed() {
    let entry = (1..256)
        .find(|e| BC4_HISTORY[*e] == Hist::Unknown)
        .expect("BC4 has unrecovered entries");
    let mut lanes = vec![vec![0u8; 1]; 12];
    lanes[0][0] = distance_symbol(256 - entry);
    let packed = pack(BcPackCodec::Bc4, &lanes, 1);
    let opts = DecodeOptions {
        allow_provisional: true,
    };
    assert_eq!(
        decode_with(BcPackCodec::Bc4, &packed, 8, &opts),
        Err(BcPackError::History {
            codec: BcPackCodec::Bc4,
            block: 0,
            entry: entry as u8,
            status: HistoryStatus::Unknown
        })
    );
}

#[test]
fn history_tables_have_the_recovered_counts() {
    let count = |c: usize, p: usize, c_n: usize, p_n: usize| (c, p) == (c_n, p_n);
    let tally = |f: &dyn Fn(usize) -> u8| {
        (0..256).fold((0, 0), |(c, p), i| match f(i) {
            2 => (c + 1, p),
            1 => (c, p + 1),
            _ => (c, p),
        })
    };
    let h6 = |t: &[Hist<6>; 256]| tally(&|i| status(&t[i]));
    let (c, p) = h6(&BC1_HISTORY);
    assert!(count(c, p, 252, 3), "BC1 {c}/{p}");
    let (c, p) = tally(&|i| status(&BC3_HISTORY[i]));
    assert_eq!((c, p), (0, 35));
    let (c, p) = tally(&|i| status(&BC4_HISTORY[i]));
    assert_eq!((c, p), (2, 170));
    let (c, p) = tally(&|i| status(&BC5_HISTORY[i]));
    assert_eq!((c, p), (5, 209));
}

fn status<const N: usize>(h: &Hist<N>) -> u8 {
    match h {
        Hist::Confirmed(_) => 2,
        Hist::Provisional(_) => 1,
        Hist::Unknown => 0,
    }
}

#[test]
fn codebooks_are_permutations() {
    for table in [&BC1_QUADRANT, &BC3_COLOUR_QUADRANT] {
        let mut seen = [false; 256];
        for v in table.iter() {
            assert!(!std::mem::replace(&mut seen[usize::from(*v)], true));
        }
    }
    for table in [&BC3_ALPHA_PAIR, &BC4_PAIR, &BC5_PAIR] {
        let mut seen = [[false; 8]; 8];
        for p in table.iter() {
            assert!(p[0] < 8 && p[1] < 8);
            assert!(!std::mem::replace(
                &mut seen[usize::from(p[0])][usize::from(p[1])],
                true
            ));
        }
    }
}

#[test]
fn a_quadrant_symbol_lays_out_a_block_row_by_row() {
    // Symbol 3 of BC1 is quadrant byte 0x55: every pixel of the quadrant has index 1.
    assert_eq!(BC1_QUADRANT[3], 0x55);
    let mut lanes = vec![vec![0u8; 1]; 12];
    lanes[8][0] = 3;
    lanes[9][0] = 2; // 0x00
    lanes[10][0] = 1; // 0xff
    lanes[11][0] = 0; // 0xaa
    let packed = pack(BcPackCodec::Bc1, &lanes, 1);
    let out = decode(BcPackCodec::Bc1, &packed, 8).unwrap();
    assert_eq!(&out[..4], &[0, 0, 0, 0]);
    // Rows: top-left 0x55 beside top-right 0x00, then bottom-left 0xff beside bottom-right 0xaa.
    assert_eq!(&out[4..], &[0x05, 0x05, 0xaf, 0xaf]);
}

fn alpha_bits(block: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b[..6].copy_from_slice(&block[2..8]);
    u64::from_le_bytes(b)
}

fn push_alpha_rows(
    lanes: &mut [Vec<u8>],
    first: usize,
    stride: usize,
    inv: &[[u8; 8]; 8],
    bits: u64,
) {
    // Lane `first + stride * r` holds row r.
    for r in 0..4 {
        let p = row_pairs(bits, r);
        lanes[first + stride * r].push(inv[p[0].0][p[0].1]);
        lanes[first + stride * r].push(inv[p[1].0][p[1].1]);
    }
}

#[test]
fn bc4_blocks_round_trip() {
    let mut rng = Rng(44);
    let n = 300;
    let blocks: Vec<u8> = (0..n * 8).map(|_| rng.next() as u8).collect();
    let inv = inverse_pairs(&BC4_PAIR);
    let mut lanes = vec![Vec::new(); 12];
    for i in 0..n {
        let b = &blocks[i * 8..i * 8 + 8];
        let (pred, base) = match i % 3 {
            1 => (
                Pred::Back(1),
                [blocks[(i - 1) * 8], blocks[(i - 1) * 8 + 1]],
            ),
            _ => (Pred::None, [128, 128]),
        };
        lanes[0].push(distance_symbol(distance_of(i, pred)));
        lanes[1].push(0);
        lanes[2].push(zig(i32::from(b[0]) - i32::from(base[0]), 256));
        lanes[3].push(zig(i32::from(b[1]) - i32::from(base[1]), 256));
        let bits = alpha_bits(b);
        // BC4 rows hold one pair each: lanes 4 + 2r (left pair) and 5 + 2r (right pair).
        for r in 0..4 {
            let p = row_pairs(bits, r);
            lanes[4 + 2 * r].push(inv[p[0].0][p[0].1]);
            lanes[5 + 2 * r].push(inv[p[1].0][p[1].1]);
        }
    }
    let packed = pack(BcPackCodec::Bc4, &lanes, n);
    assert_eq!(decode(BcPackCodec::Bc4, &packed, n * 8).unwrap(), blocks);
}

#[test]
fn bc3_and_bc5_blocks_round_trip() {
    let mut rng = Rng(35);
    let n = 250;
    let blocks: Vec<u8> = (0..n * 16).map(|_| rng.next() as u8).collect();
    let preds: Vec<Pred> = (0..n)
        .map(|i| {
            if i % 4 == 1 {
                Pred::Back(1)
            } else if i % 5 == 3 {
                Pred::Back(2)
            } else {
                Pred::None
            }
        })
        .collect();
    let dist_lanes = |lanes: &mut Vec<Vec<u8>>, i: usize| {
        let v = distance_symbol(distance_of(i, preds[i]));
        let (a, b) = split_nibbles(v);
        lanes[0].extend([a, b]);
        lanes[1].extend([0, 0]);
    };

    // BC3
    let inv_alpha = inverse_pairs(&BC3_ALPHA_PAIR);
    let inv_quad = inverse_bytes(&BC3_COLOUR_QUADRANT);
    let mut lanes = vec![Vec::new(); 12];
    for i in 0..n {
        let b = &blocks[i * 16..i * 16 + 16];
        let base = match preds[i] {
            Pred::None => [128, 128, 0, 0, 0, 0, 0, 0],
            Pred::Back(d) => {
                let p = &blocks[(i - d) * 16..(i - d) * 16 + 16];
                let c = colour_of(&p[8..]);
                [p[0], p[1], c[0], c[1], c[2], c[3], c[4], c[5]]
            }
            Pred::History(_) => unreachable!(),
        };
        dist_lanes(&mut lanes, i);
        let c = colour_of(&b[8..]);
        let want = [b[0], b[1], c[0], c[1], c[2], c[3], c[4], c[5]];
        let mods = [256, 256, 32, 64, 32, 32, 64, 32];
        let z = |k: usize| zig(i32::from(want[k]) - i32::from(base[k]), mods[k]);
        lanes[2].extend([z(0), z(1)]);
        lanes[7].extend([z(2), z(5)]);
        lanes[8].extend([z(3), z(6)]);
        lanes[9].extend([z(4), z(7)]);
        push_alpha_rows(&mut lanes, 3, 1, &inv_alpha, alpha_bits(b));
        let (r0, r1, r2, r3) = (b[12], b[13], b[14], b[15]);
        let q = [
            (r0 & 15) | (r1 & 15) << 4,
            (r0 >> 4) | (r1 >> 4) << 4,
            (r2 & 15) | (r3 & 15) << 4,
            (r2 >> 4) | (r3 >> 4) << 4,
        ];
        lanes[10].extend([inv_quad[usize::from(q[0])], inv_quad[usize::from(q[1])]]);
        lanes[11].extend([inv_quad[usize::from(q[2])], inv_quad[usize::from(q[3])]]);
    }
    let packed = pack(BcPackCodec::Bc3, &lanes, n * 2);
    assert_eq!(decode(BcPackCodec::Bc3, &packed, n * 16).unwrap(), blocks);

    // BC5: two BC4-style halves; lanes 4..8 are the red rows and 8..12 the green rows.
    let inv_pair = inverse_pairs(&BC5_PAIR);
    let mut lanes = vec![Vec::new(); 12];
    for i in 0..n {
        let b = &blocks[i * 16..i * 16 + 16];
        let base = match preds[i] {
            Pred::None => [128; 4],
            Pred::Back(d) => {
                let p = &blocks[(i - d) * 16..(i - d) * 16 + 16];
                [p[0], p[1], p[8], p[9]]
            }
            Pred::History(_) => unreachable!(),
        };
        dist_lanes(&mut lanes, i);
        let want = [b[0], b[1], b[8], b[9]];
        let z = |k: usize| zig(i32::from(want[k]) - i32::from(base[k]), 256);
        lanes[2].extend([z(0), z(1)]);
        lanes[3].extend([z(2), z(3)]);
        push_alpha_rows(&mut lanes, 4, 1, &inv_pair, alpha_bits(&b[..8]));
        push_alpha_rows(&mut lanes, 8, 1, &inv_pair, alpha_bits(&b[8..]));
    }
    let packed = pack(BcPackCodec::Bc5, &lanes, n * 2);
    assert_eq!(decode(BcPackCodec::Bc5, &packed, n * 16).unwrap(), blocks);
}

#[test]
fn a_non_zero_lane_one_is_refused() {
    let mut lanes = vec![vec![0u8; 1]; 12];
    lanes[1][0] = 1;
    let packed = pack(BcPackCodec::Bc1, &lanes, 1);
    assert_eq!(
        decode(BcPackCodec::Bc1, &packed, 8),
        Err(BcPackError::UnexpectedLane {
            codec: BcPackCodec::Bc1,
            lane: 1
        })
    );
}

#[test]
fn bc3_history_entries_are_provisional_only() {
    // Block 0 of a BC3 chunk pointing at history entry 255 (distance 1 + 255 - ... = entry 255 -> d = 1).
    let entry = 26usize;
    assert!(matches!(BC3_HISTORY[entry], Hist::Provisional(_)));
    let v = distance_symbol(256 - entry);
    let (a, b) = split_nibbles(v);
    let mut lanes = vec![vec![0u8; 2]; 12];
    lanes[0] = vec![a, b];
    let packed = pack(BcPackCodec::Bc3, &lanes, 2);
    assert!(matches!(
        decode(BcPackCodec::Bc3, &packed, 16),
        Err(BcPackError::History {
            status: HistoryStatus::Provisional,
            entry: 26,
            ..
        })
    ));
    let opts = DecodeOptions {
        allow_provisional: true,
    };
    let d = decode_with(BcPackCodec::Bc3, &packed, 16, &opts).unwrap();
    assert_eq!(d.provisional_blocks, 1);
    let Hist::Provisional(h) = BC3_HISTORY[entry] else {
        unreachable!()
    };
    assert_eq!(&d.data[..2], &[h[0], h[1]]);
}
