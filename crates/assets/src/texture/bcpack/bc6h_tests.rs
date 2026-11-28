//! BC6H block stage on hand-made chunks: a test-only encoder turns block specs into lane symbols (zigzag deltas
//! against chosen predictors), the lane array is encoded and replayed, and the stage must return the blocks the
//! BC6H packer writes for the same specs. Block colours are cross-checked with the BC6H block decoder of `bcn`.

#![allow(clippy::needless_range_loop)]

use super::bc6h::{Endpoints, Entry, History, Indices, MODES, pack, rebuild, stream_w};
use super::bptc::ANCHOR2;
use super::testenc::{Frag, Rng, encode_flush, interleave};
use super::{BcPackCodec, BcPackError, DecodeOptions, HistoryStatus, decode_with, lanes};
use crate::texture::bcn;

/// One block: the mode index, the partition, BCPack's `w` (natural bit order) and the `x y z` fields per channel.
#[derive(Clone, Copy)]
struct Spec {
    mode: usize,
    part: usize,
    field: [[u32; 4]; 3],
    ix: [u32; 16],
}

#[derive(Clone, Copy, Debug)]
enum Pred {
    None,
    Back(usize),
    History(usize),
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

fn spread(d: u32, bits: u32) -> u32 {
    (0..bits).fold(0, |acc, k| acc | (d >> k & 1) << (2 * k))
}

/// Two symbols whose bits interleave to `v` (the first takes the even bits).
fn split_wide(v: u32, bits: u32) -> (u8, u8) {
    let pick = |v: u32| (0..bits).fold(0, |acc, k| acc | (v >> (2 * k) & 1) << k) as u8;
    (pick(v), pick(v >> 1))
}

fn mode_info(m: usize) -> (u32, [u32; 3], bool, bool) {
    let mode = &MODES[m];
    (mode.base, mode.delta, mode.transformed, mode.two_regions)
}

/// The predictor state a spec leaves behind (the same state the stage keeps).
fn stored(s: &Spec) -> (Endpoints, Indices) {
    let (base, delta, transformed, two) = mode_info(s.mode);
    let w_shift = 16 - base;
    let mut e = [[0u16; 4]; 3];
    for c in 0..3 {
        let dc = delta[c];
        let sh = 6u32.saturating_sub(dc);
        let w = s.field[c][0];
        let w_nat = nat_w(s.mode, w);
        let x10 = (s.field[c][1] << (10 - dc)) as u16;
        e[c] = if two {
            [
                (w_nat << w_shift) as u16,
                x10,
                (s.field[c][2] << sh) as u16,
                (s.field[c][3] << sh) as u16,
            ]
        } else {
            let md = (1u32 << dc) - 1;
            let v = (w_nat & md) ^ if transformed { 1 << (dc - 1) } else { 0 };
            let low_y = if dc >= 6 { v >> (dc - 6) } else { v << sh };
            [(w_nat << w_shift) as u16, x10, low_y as u16, x10 >> 4]
        };
    }
    let shift = if two { 1 } else { 0 };
    (e, s.ix.map(|v| (v << shift) as u8))
}

/// BCPack's own `w` of a spec whose `field[0]` is stored in block bit order.
fn nat_w(mode: usize, stored_w: u32) -> u32 {
    stream_w(mode, stored_w)
}

const MID_E: Endpoints = [[0x8000, 0x200, 0x20, 0x20]; 3];
const MID_I: Indices = [8; 16];

fn encode(specs: &[Spec], preds: &[(Pred, Pred)], hist: &History) -> Vec<Vec<u8>> {
    let n = specs.len();
    let mut lanes = vec![vec![0u8; 2 * n]; 16];
    let kept: Vec<_> = specs.iter().map(stored).collect();
    let dist = |i: usize, p: Pred| match p {
        Pred::None => 0,
        Pred::Back(d) => d,
        Pred::History(entry) => 256 + i - entry,
    };
    for (i, s) in specs.iter().enumerate() {
        let (a, b) = (2 * i, 2 * i + 1);
        let (pe, pi) = preds[i];
        let (de, di) = (dist(i, pe), dist(i, pi));
        let rep = match pe {
            Pred::None => MID_E,
            Pred::Back(d) => kept[i - d].0,
            Pred::History(k) => match hist.endpoints[k] {
                Entry::Confirmed(v) | Entry::Provisional(v) => v,
                Entry::Unknown => unreachable!("test history"),
            },
        };
        let rix = match pi {
            Pred::None => MID_I,
            Pred::Back(d) => kept[i - d].1,
            Pred::History(k) => match hist.indices[k] {
                Entry::Confirmed(v) | Entry::Provisional(v) => v,
                Entry::Unknown => unreachable!("test history"),
            },
        };
        let (base, delta, _, two) = mode_info(s.mode);
        let (x, y) = split_nibbles(zig(de as i32 - di as i32, 256) as u8);
        (lanes[0][a], lanes[0][b]) = (x, y);
        let (x, y) = split_nibbles(di as u8);
        (lanes[1][a], lanes[1][b]) = (x, y);
        (lanes[15][a], lanes[15][b]) = (s.mode as u8, s.part as u8);
        for c in 0..3 {
            let dc = delta[c];
            let sh = 6u32.saturating_sub(dc);
            let r = rep[c].map(i32::from);
            let w = nat_w(s.mode, s.field[c][0]) as i32;
            let dz0 = zig(w - (r[0] >> (16 - base)), 1 << base);
            let (p, q) = split_wide(dz0, 8);
            (lanes[2 + c][a], lanes[2 + c][b]) = (p, q);
            let dz1 = zig(s.field[c][1] as i32 - (r[1] >> (10 - dc)), 1 << dc);
            let (p, q) = split_wide(dz1, 5);
            (lanes[5 + c][a], lanes[5 + c][b]) = (p, q);
            if two {
                lanes[8 + c][a] = zig(s.field[c][2] as i32 - (r[2] >> sh), 1 << dc) as u8;
                lanes[8 + c][b] = zig(s.field[c][3] as i32 - (r[3] >> sh), 1 << dc) as u8;
            }
        }
        let (ibits, ishift) = if two { (3, 1) } else { (4, 0) };
        for y in 0..4 {
            let d: Vec<u32> = (0..4)
                .map(|off| {
                    let p = y * 4 + off;
                    zig(s.ix[p] as i32 - i32::from(rix[p] >> ishift), 1 << ibits)
                })
                .collect();
            let sym = |hi: u32, lo: u32| {
                if y < 3 {
                    spread(hi, 4) << 1 | spread(lo, 4)
                } else {
                    spread(lo, 4) << 1 | spread(hi, 4)
                }
            };
            lanes[11 + y][a] = sym(d[0], d[1]) as u8;
            lanes[11 + y][b] = sym(d[2], d[3]) as u8;
        }
    }
    lanes
}

fn pack_lanes(lanes: &[Vec<u8>], n: usize) -> Vec<u8> {
    let widths = BcPackCodec::Bc6h.lane_widths().unwrap();
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

fn random_spec(rng: &mut Rng, mode: usize) -> Spec {
    let (base, delta, _, two) = mode_info(mode);
    let part = if two { (rng.next() % 32) as usize } else { 0 };
    let mut field = [[0u32; 4]; 3];
    for c in 0..3 {
        let dc = delta[c];
        field[c][0] = rng.next() & ((1 << base) - 1);
        field[c][1] = rng.next() & ((1 << dc) - 1);
        if two {
            field[c][2] = rng.next() & ((1 << dc) - 1);
            field[c][3] = rng.next() & ((1 << dc) - 1);
        }
    }
    let (ibits, anchor_bit) = if two { (3, 4) } else { (4, 8) };
    let mut ix = [0u32; 16];
    for v in &mut ix {
        *v = rng.next() & ((1 << ibits) - 1);
    }
    ix[0] &= !anchor_bit;
    if two {
        ix[usize::from(ANCHOR2[part])] &= !anchor_bit;
    }
    Spec {
        mode,
        part,
        field,
        ix,
    }
}

fn run(specs: &[Spec], preds: &[(Pred, Pred)], hist: &History, opts: &DecodeOptions) -> Vec<u8> {
    let n = specs.len();
    let packed = pack_lanes(&encode(specs, preds, hist), n);
    let syms = lanes::decode_slice(&packed, BcPackCodec::Bc6h.lane_widths().unwrap(), 2 * n)
        .expect("lanes decode");
    let mut out = vec![0u8; 16 * n];
    rebuild(&syms, &mut out, hist, opts, None).expect("block stage");
    out
}

fn expected(specs: &[Spec]) -> Vec<u8> {
    specs
        .iter()
        .flat_map(|s| pack(s.mode, s.part, &s.field, &s.ix))
        .collect()
}

fn history() -> History {
    let mut h = History::empty();
    let mut rng = Rng(77);
    for k in [3usize, 40, 200, 255] {
        let mut e = [[0u16; 4]; 3];
        for c in 0..3 {
            e[c] = [
                (rng.next() & 0xFFFF) as u16,
                (rng.next() & 0x3FF) as u16,
                (rng.next() & 0x3F) as u16,
                (rng.next() & 0x3F) as u16,
            ];
        }
        h.endpoints[k] = Entry::Confirmed(e);
        h.indices[k] = Entry::Confirmed(std::array::from_fn(|_| (rng.next() & 15) as u8));
    }
    h
}

#[test]
fn every_mode_round_trips_against_the_default_and_earlier_blocks() {
    let mut rng = Rng(9);
    let mut specs = Vec::new();
    let mut preds = Vec::new();
    for _ in 0..4 {
        for mode in 0..14 {
            let i = specs.len();
            let s = random_spec(&mut rng, mode);
            let back = |rng: &mut Rng| {
                if i == 0 {
                    Pred::None
                } else {
                    match rng.next() % 3 {
                        0 => Pred::None,
                        1 => Pred::Back(1),
                        _ => Pred::Back(1 + rng.next() as usize % i.min(200)),
                    }
                }
            };
            preds.push((back(&mut rng), back(&mut rng)));
            specs.push(s);
        }
    }
    let opts = DecodeOptions::default();
    let out = run(&specs, &preds, &history(), &opts);
    for (i, (got, want)) in out.chunks(16).zip(expected(&specs).chunks(16)).enumerate() {
        assert_eq!(got, want, "block {i} mode {}", specs[i].mode + 1);
    }
}

#[test]
fn built_in_history_predicts_the_first_blocks() {
    let mut rng = Rng(21);
    let hist = history();
    let specs: Vec<Spec> = (0..6).map(|i| random_spec(&mut rng, i * 2)).collect();
    let preds = [
        (Pred::History(3), Pred::History(40)),
        (Pred::History(200), Pred::History(200)),
        (Pred::History(255), Pred::History(3)),
        (Pred::History(40), Pred::Back(1)),
        (Pred::Back(2), Pred::History(255)),
        (Pred::History(40), Pred::None),
    ];
    let out = run(&specs, &preds, &hist, &DecodeOptions::default());
    assert_eq!(out, expected(&specs));
}

#[test]
fn unknown_and_provisional_history_follow_the_options() {
    let mut rng = Rng(5);
    let mut hist = history();
    hist.endpoints[40] = Entry::Provisional(MID_E);
    let specs = vec![random_spec(&mut rng, 10), random_spec(&mut rng, 12)];
    let preds = [(Pred::History(40), Pred::None), (Pred::Back(1), Pred::None)];
    let strict = DecodeOptions::default();
    let n = specs.len();
    let packed = pack_lanes(&encode(&specs, &preds, &hist), n);
    let syms =
        lanes::decode_slice(&packed, BcPackCodec::Bc6h.lane_widths().unwrap(), 2 * n).unwrap();
    let mut out = vec![0u8; 32];
    assert_eq!(
        rebuild(&syms, &mut out, &hist, &strict, None),
        Err(BcPackError::History {
            codec: BcPackCodec::Bc6h,
            block: 0,
            entry: 40,
            status: HistoryStatus::Provisional,
        })
    );
    let loose = DecodeOptions {
        allow_provisional: true,
    };
    assert_eq!(rebuild(&syms, &mut out, &hist, &loose, None), Ok(2));
    assert_eq!(out, expected(&specs));
    // The empty built-in table refuses every history reference.
    let empty = History::empty();
    let syms = lanes::decode_slice(
        &pack_lanes(
            &encode(
                &specs,
                &[(Pred::History(3), Pred::None), (Pred::None, Pred::None)],
                &hist,
            ),
            n,
        ),
        BcPackCodec::Bc6h.lane_widths().unwrap(),
        2 * n,
    )
    .unwrap();
    assert_eq!(
        rebuild(&syms, &mut out, &empty, &loose, None),
        Err(BcPackError::History {
            codec: BcPackCodec::Bc6h,
            block: 0,
            entry: 3,
            status: HistoryStatus::Unknown,
        })
    );
}

#[test]
fn self_contained_chunks_decode_without_history() {
    let mut rng = Rng(12);
    let specs: Vec<Spec> = (0..14).map(|m| random_spec(&mut rng, m)).collect();
    let preds: Vec<_> = (0..14)
        .map(|i| {
            if i == 0 {
                (Pred::None, Pred::None)
            } else {
                (Pred::Back(i.min(3)), Pred::None)
            }
        })
        .collect();
    let out = run(&specs, &preds, &History::empty(), &DecodeOptions::default());
    assert_eq!(out, expected(&specs));
}

#[test]
fn the_public_entry_uses_the_fitted_history() {
    use super::bc6h_history::{ENDPOINTS, INDICES};
    let k = (3..256)
        .find(|&k| {
            matches!(ENDPOINTS[k], Entry::Provisional(_))
                && matches!(INDICES[k], Entry::Provisional(_))
        })
        .expect("a fully fitted entry");
    let mut rng = Rng(99);
    let specs: Vec<Spec> = (0..3).map(|m| random_spec(&mut rng, m * 4)).collect();
    let mut hist = History::empty();
    let (Entry::Provisional(e), Entry::Provisional(i)) = (ENDPOINTS[k], INDICES[k]) else {
        unreachable!()
    };
    hist.endpoints[k] = Entry::Confirmed(e);
    hist.indices[k] = Entry::Confirmed(i);
    let preds = [
        (Pred::History(k), Pred::History(k)),
        (Pred::Back(1), Pred::Back(1)),
        (Pred::Back(2), Pred::None),
    ];
    let packed = pack_lanes(&encode(&specs, &preds, &hist), 3);
    let strict = DecodeOptions::default();
    assert_eq!(
        decode_with(BcPackCodec::Bc6h, &packed, 48, &strict),
        Err(BcPackError::History {
            codec: BcPackCodec::Bc6h,
            block: 0,
            entry: k as u8,
            status: HistoryStatus::Provisional,
        })
    );
    let loose = DecodeOptions {
        allow_provisional: true,
    };
    let got = decode_with(BcPackCodec::Bc6h, &packed, 48, &loose).unwrap();
    assert_eq!(got.data, expected(&specs));
    assert_eq!(got.provisional_blocks, 3);
}

/// Interpolation weights of the public BC6H format.
const W3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const W4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn unquantize(v: i32, bits: u32) -> i32 {
    if bits >= 15 {
        v
    } else if v == 0 {
        0
    } else if v == (1 << bits) - 1 {
        0xFFFF
    } else {
        ((v << 16) + 0x8000) >> bits
    }
}

/// Pixel colours of a spec computed straight from the public format description, endpoint fields in stored
/// order: `x y z` are deltas against `w` in transformed modes.
fn reference_colours(s: &Spec) -> [[f32; 3]; 16] {
    let (base, delta, transformed, two) = mode_info(s.mode);
    let mask = (1i32 << base) - 1;
    let mut eps = [[0i32; 4]; 3];
    for c in 0..3 {
        let w = s.field[c][0] as i32;
        eps[c][0] = unquantize(w, base);
        for q in 1..if two { 4 } else { 2 } {
            let raw = s.field[c][q] as i32;
            let dc = delta[c];
            let e = if transformed {
                let signed = (raw ^ (1 << (dc - 1))) - (1 << (dc - 1));
                (w + signed) & mask
            } else {
                raw
            };
            eps[c][q] = unquantize(e, base);
        }
    }
    std::array::from_fn(|p| {
        let (weight, sub) = if two {
            let sub = usize::from(super::bptc::PART2[s.part] >> p & 1);
            (W3[s.ix[p] as usize], sub * 2)
        } else {
            (W4[s.ix[p] as usize], 0)
        };
        std::array::from_fn(|c| {
            let v = ((64 - weight) * eps[c][sub] + weight * eps[c][sub + 1] + 32) >> 6;
            bcn::half_to_f32(((v.max(0) * 31) >> 6).min(0x7BFF) as u16)
        })
    })
}

#[test]
fn packed_blocks_decode_to_the_colours_the_format_defines() {
    let mut rng = Rng(31);
    for mode in 0..14 {
        for _ in 0..40 {
            let mut s = random_spec(&mut rng, mode);
            // Modes 13 and 14 keep reversed high bits: pick `w` with those bits clear so that the stored
            // and natural orders agree.
            if mode >= 12 {
                for c in 0..3 {
                    s.field[c][0] &= 0x3FF;
                }
            }
            let block = pack(s.mode, s.part, &s.field, &s.ix);
            let got = bcn::bc6h_block(&block, false);
            let want = reference_colours(&s);
            for p in 0..16 {
                for c in 0..3 {
                    let (g, w) = (got[p][c], want[p][c]);
                    assert!(
                        g == w
                            || (g.is_nan() && w.is_nan())
                            || (g.is_infinite() && w.is_infinite()),
                        "mode {} pixel {p} channel {c}: {g} vs {w}",
                        mode + 1
                    );
                }
            }
        }
    }
}
