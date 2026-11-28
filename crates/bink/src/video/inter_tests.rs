use super::testutil::{Bits, decoder, frame, payload, payload_with_flags, sample};
use super::{BlockKind, Frame, TypeLru};
use crate::bits::BitReader;
use crate::error::Error;

const W: u32 = 64;
const H: u32 = 32;

fn texture(p: usize, x: usize, y: usize) -> u8 {
    ((x * 5 + y * 3 + p * 40) % 180 + 30) as u8
}

fn run(mbs: &Bits) -> (Frame, Frame) {
    let dec = decoder(W, H);
    let reference = frame(&dec, texture);
    let out = dec
        .decode(1, &payload(mbs), false, Some(&reference))
        .unwrap();
    (reference, out)
}

// Block type codes as (bits, count): the type's position in the recency list, as zeros ended by a one.
// Starting list: motion, residue, skip, intra.
const MOTION: (u32, u32) = (0b1, 1);
const RESIDUE: (u32, u32) = (0b10, 2);
const INTRA: (u32, u32) = (0b100, 3);
const SKIP: (u32, u32) = (0b000, 3);

fn kind(b: &mut Bits, k: (u32, u32)) {
    b.put(k.0, k.1);
}

fn single_mv(b: &mut Bits, x: i32, y: i32) {
    b.put(1, 1).mv(x).mv(y);
}

fn six_tap(s: [i32; 6]) -> u8 {
    let v = ((s[2] + s[3]) * 19 / 2 - (s[1] + s[4]) * 2 + (s[0] + s[5]) / 2 + 8) / 16;
    v.clamp(0, 255) as u8
}

/// The bits of an intra macroblock with no coefficients: quantiser unchanged, empty patterns, the luma DC
/// differences `d` and flat chroma.
fn intra_bits(b: &mut Bits, d: &[i32; 16]) {
    b.put(1, 1);
    b.put(1, 1).put(0, 1);
    b.dc_diffs(d);
    for _ in 0..2 {
        b.put(1, 1).dc_diffs(&[0; 4]);
    }
}

#[test]
fn skipped_macroblocks_copy_the_reference() {
    let mut b = Bits::default();
    kind(&mut b, SKIP);
    b.put(0b10, 2);
    let (reference, out) = run(&b);
    for p in 0..3 {
        assert_eq!(out.planes[p].data, reference.planes[p].data, "plane {p}");
    }
}

#[test]
fn whole_pel_motion_shifts_the_block() {
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    single_mv(&mut b, 4, 4);
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    for y in 0..32 {
        for x in 0..32 {
            let want = sample(&reference.planes[0], x + 2, (y + 2).min(31));
            assert_eq!(sample(&out.planes[0], x, y), want, "luma {x},{y}");
        }
    }
    for p in 1..3 {
        for y in 0..16 {
            for x in 0..16 {
                let want = sample(&reference.planes[p], x + 1, (y + 1).min(15));
                assert_eq!(sample(&out.planes[p], x, y), want, "chroma {p} {x},{y}");
            }
        }
    }
    for y in 0..32 {
        for x in 32..64 {
            assert_eq!(
                sample(&out.planes[0], x, y),
                sample(&reference.planes[0], x, y)
            );
        }
    }
}

#[test]
fn vectors_pointing_outside_replicate_the_edge() {
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    single_mv(&mut b, -12, -6);
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    for y in 0..16usize {
        for x in 0..16usize {
            let want = sample(
                &reference.planes[0],
                x.saturating_sub(6),
                y.saturating_sub(3),
            );
            assert_eq!(sample(&out.planes[0], x, y), want, "luma {x},{y}");
        }
    }
}

#[test]
fn half_pel_luma_uses_the_six_tap_filter() {
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    single_mv(&mut b, 1, 0);
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    let px = &reference.planes[0];
    for y in 0..32 {
        for x in 8..24 {
            let s = std::array::from_fn(|k| i32::from(sample(px, x + k - 2, y)));
            assert_eq!(sample(&out.planes[0], x, y), six_tap(s), "{x},{y}");
        }
    }
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    single_mv(&mut b, 0, 1);
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    let px = &reference.planes[0];
    for y in 8..24 {
        for x in 0..32 {
            let s = std::array::from_fn(|k| i32::from(sample(px, x, y + k - 2)));
            assert_eq!(sample(&out.planes[0], x, y), six_tap(s), "{x},{y}");
        }
    }
}

#[test]
fn diagonal_half_pel_filters_rows_then_columns() {
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    single_mv(&mut b, 1, 1);
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    let px = &reference.planes[0];
    for y in 8..24 {
        for x in 8..24 {
            let col: [i32; 6] = std::array::from_fn(|r| {
                let s = std::array::from_fn(|k| i32::from(sample(px, x + k - 2, y + r - 2)));
                i32::from(six_tap(s))
            });
            assert_eq!(sample(&out.planes[0], x, y), six_tap(col), "{x},{y}");
        }
    }
}

#[test]
fn quarter_pel_chroma_blends_two_samples() {
    for (mv, wa, wb) in [(1, 6, 2), (2, 4, 4), (3, 2, 6)] {
        let mut b = Bits::default();
        kind(&mut b, MOTION);
        single_mv(&mut b, mv, 0);
        kind(&mut b, SKIP);
        let (reference, out) = run(&b);
        let px = &reference.planes[2];
        for y in 0..16 {
            for x in 0..15 {
                let (a, c) = (i32::from(sample(px, x, y)), i32::from(sample(px, x + 1, y)));
                let want = ((wa * a + wb * c + 4) >> 3) as u8;
                assert_eq!(sample(&out.planes[2], x, y), want, "mv {mv} at {x},{y}");
            }
        }
    }
}

#[test]
fn four_vectors_move_the_quarters_separately() {
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    // Differences (2,0), (-2,0), (-2,2), (0,0) become the vectors (2,0), (0,0), (0,2), (0,0).
    b.put(0, 1);
    for x in [2, -2, -2, 0] {
        b.mv(x);
    }
    for y in [0, 0, 2, 0] {
        b.mv(y);
    }
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    let px = &reference.planes[0];
    assert_eq!(sample(&out.planes[0], 3, 3), sample(px, 4, 3));
    assert_eq!(sample(&out.planes[0], 20, 3), sample(px, 20, 3));
    assert_eq!(sample(&out.planes[0], 3, 20), sample(px, 3, 21));
    assert_eq!(sample(&out.planes[0], 20, 20), sample(px, 20, 20));
}

#[test]
fn the_next_macroblock_predicts_its_vector_from_the_left() {
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    single_mv(&mut b, 2, 0);
    kind(&mut b, MOTION);
    single_mv(&mut b, 0, 0);
    let (reference, out) = run(&b);
    for y in 0..32 {
        for x in 32..60 {
            let want = sample(&reference.planes[0], x + 1, y);
            assert_eq!(sample(&out.planes[0], x, y), want, "{x},{y}");
        }
    }
}

#[test]
fn escaped_vector_components_round_trip() {
    let mut b = Bits::default();
    kind(&mut b, MOTION);
    single_mv(&mut b, 8, -10);
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    let want = |x: usize, y: usize| sample(&reference.planes[0], x + 4, y.saturating_sub(5));
    assert_eq!(sample(&out.planes[0], 10, 12), want(10, 12));
    assert_eq!(sample(&out.planes[0], 2, 30), want(2, 30));
}

#[test]
fn residue_adds_dc_differences_to_the_prediction() {
    let mut b = Bits::default();
    kind(&mut b, RESIDUE);
    single_mv(&mut b, 0, 0);
    b.put(1, 1); // inter quantiser unchanged: 16
    b.put(1, 1).put(0, 1); // empty luma pattern, no repeat
    let mut d = [0; 16];
    d[0] = 3;
    b.dc_diffs(&d);
    b.put(0, 1); // no chroma residue
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    // A difference of three at quantiser 16 is 48, which every block inherits: six samples.
    for y in 0..32 {
        for x in 0..32 {
            let want = (i32::from(sample(&reference.planes[0], x, y)) + 6).min(255) as u8;
            assert_eq!(sample(&out.planes[0], x, y), want, "{x},{y}");
        }
    }
    for p in 1..3 {
        assert_eq!(out.planes[p].data[..16], reference.planes[p].data[..16]);
    }
}

#[test]
fn residue_chroma_follows_its_flag() {
    let mut b = Bits::default();
    kind(&mut b, RESIDUE);
    single_mv(&mut b, 0, 0);
    b.put(1, 1);
    b.put(1, 1).put(0, 1).put(0, 1); // luma: nothing coded, no DC
    b.put(1, 1); // chroma present
    for _ in 0..2 {
        b.put(1, 1); // pattern repeated from the table: nothing coded
        b.dc_diffs(&[-5, 0, 0, 0]);
    }
    kind(&mut b, SKIP);
    let (reference, out) = run(&b);
    // -5 at quantiser 16 is -80, ten samples down in each chroma block.
    for p in 1..3 {
        for y in 0..16 {
            for x in 0..16 {
                let want = (i32::from(sample(&reference.planes[p], x, y)) - 10).max(0) as u8;
                assert_eq!(sample(&out.planes[p], x, y), want, "plane {p} {x},{y}");
            }
        }
    }
    assert_eq!(
        sample(&out.planes[0], 5, 5),
        sample(&reference.planes[0], 5, 5)
    );
}

#[test]
fn an_intra_block_predicts_from_a_skipped_neighbour() {
    let dec = decoder(W, H);
    let reference = frame(&dec, |p, _, _| if p == 0 { 165 } else { 128 });
    let mut b = Bits::default();
    kind(&mut b, SKIP);
    kind(&mut b, INTRA);
    intra_bits(&mut b, &[0; 16]);
    let out = dec
        .decode(1, &payload(&b), false, Some(&reference))
        .unwrap();
    // The skipped neighbour measures 165 * 8, so the intra block continues the flat area.
    assert_eq!(sample(&out.planes[0], 40, 10), 165);
    assert_eq!(sample(&out.planes[1], 20, 5), 128);
}

#[test]
fn intra_blocks_decode_as_in_a_key_frame() {
    let mut b = Bits::default();
    kind(&mut b, INTRA);
    let mut d = [0; 16];
    d[0] = 5;
    intra_bits(&mut b, &d);
    kind(&mut b, (0b100, 3)); // skip now sits last in the list
    let (reference, out) = run(&b);
    // 1024 + 80 over eight is 138.
    assert_eq!(sample(&out.planes[0], 7, 7), 138);
    assert_eq!(sample(&out.planes[0], 31, 31), 138);
    assert_eq!(sample(&out.planes[1], 3, 3), 128);
    assert_eq!(
        sample(&out.planes[0], 40, 3),
        sample(&reference.planes[0], 40, 3)
    );
}

#[test]
fn inter_frames_need_a_reference_of_the_same_size() {
    let dec = decoder(W, H);
    let mut b = Bits::default();
    kind(&mut b, SKIP);
    assert!(matches!(
        dec.decode(1, &payload(&b), false, None),
        Err(Error::MissingReference(1))
    ));
    let other = decoder(32, 32);
    let wrong = frame(&other, texture);
    assert!(matches!(
        dec.decode(1, &payload(&b), false, Some(&wrong)),
        Err(Error::MissingReference(1))
    ));
}

#[test]
fn the_recency_list_moves_the_chosen_type_up() {
    let mut b = Bits::default();
    // residue, residue again, skip, intra.
    b.put(0b10, 2).put(0b1, 1).put(0b000, 3).put(0b100, 3);
    let data = b.bytes();
    let mut r = BitReader::new(&data);
    let mut lru = TypeLru::new();
    assert_eq!(lru.read(&mut r), BlockKind::Residue);
    assert_eq!(lru.read(&mut r), BlockKind::Residue);
    assert_eq!(lru.read(&mut r), BlockKind::Skip);
    assert_eq!(lru.read(&mut r), BlockKind::Intra);
}

#[test]
fn edge_flag_lists_follow_the_picture_size() {
    // 48x32: three row flags (height to 16) and seven column flags (width to a whole macroblock), each list
    // behind its leading false flag, here spelled out bit by bit.
    let dec = decoder(48, 32);
    let reference = frame(&dec, texture);
    let mut b = Bits::default();
    b.put(0, 1).put(0b101, 3);
    b.put(0, 1).put(0b0110101, 7);
    kind(&mut b, SKIP);
    b.put(0b10, 2);
    let out = dec
        .decode(1, &payload_with_flags(0x12000, &b), false, Some(&reference))
        .unwrap();
    assert_eq!(out.planes[0].data, reference.planes[0].data);
}

#[test]
fn a_slice_that_ends_early_is_rejected() {
    let dec = decoder(W, H);
    let reference = frame(&dec, texture);
    let mut b = Bits::default();
    kind(&mut b, SKIP);
    b.put(0b10, 2);
    b.put(0, 32).put(0, 32); // two more words the slice does not account for
    assert!(matches!(
        dec.decode(1, &payload(&b), false, Some(&reference)),
        Err(Error::Frame { .. })
    ));
}
