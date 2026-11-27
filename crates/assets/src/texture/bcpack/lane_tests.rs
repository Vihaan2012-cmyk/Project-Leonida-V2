use super::lanes::{LaneError, decode_slice};
use super::testenc::{Frag, Rng, encode_cut, encode_flush, interleave};

const BC1: [u8; 12] = [8, 8, 5, 6, 5, 5, 6, 5, 8, 8, 8, 8];

fn lane_data(rng: &mut Rng, widths: &[u8], n: usize) -> Vec<Vec<u8>> {
    widths
        .iter()
        .map(|&w| (0..n).map(|_| rng.skewed(u32::from(w))).collect())
        .collect()
}

fn flush_stream(widths: &[u8], data: &[Vec<u8>], chunk: usize, q: u8) -> Vec<u8> {
    let lanes = widths
        .iter()
        .zip(data)
        .map(|(&w, d)| {
            let chunks: Vec<(Frag, &[u8])> = d
                .chunks(chunk)
                .map(|c| (Frag { z: 0, q, count: 0 }, c))
                .collect();
            encode_flush(w, &chunks)
        })
        .collect();
    interleave(widths, lanes, data[0].len())
}

#[test]
fn flush_fragments_round_trip_through_the_shared_stream() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for (n, chunk, q) in [
        (1usize, 1usize, 0u8),
        (7, 7, 3),
        (300, 100, 8),
        (3000, 700, 12),
    ] {
        let data = lane_data(&mut rng, &BC1, n);
        let stream = flush_stream(&BC1, &data, chunk, q);
        let out = decode_slice(&stream, &BC1, n).unwrap();
        for (i, d) in data.iter().enumerate() {
            assert_eq!(out.lane(i), &d[..], "lane {i}, n {n}");
        }
    }
}

#[test]
fn sixteen_lane_streams_round_trip() {
    let widths = [4u8, 4, 8, 8, 8, 8, 8, 7, 7, 5, 6, 8, 8, 8, 8, 5];
    let mut rng = Rng(77);
    let data = lane_data(&mut rng, &widths, 500);
    let stream = flush_stream(&widths, &data, 250, 6);
    let out = decode_slice(&stream, &widths, 500).unwrap();
    for (i, d) in data.iter().enumerate() {
        assert_eq!(out.lane(i), &d[..]);
    }
}

#[test]
fn padding_after_the_lanes_is_accepted_and_other_data_is_not() {
    let mut rng = Rng(5);
    let data = lane_data(&mut rng, &BC1, 40);
    let mut stream = flush_stream(&BC1, &data, 40, 4);
    let used = stream.len();
    stream.extend([0u8; 37]);
    assert!(decode_slice(&stream, &BC1, 40).is_ok());
    stream[used + 5] = 1;
    assert_eq!(
        decode_slice(&stream, &BC1, 40).err(),
        Some(LaneError::TrailingData { consumed: used })
    );
}

#[test]
fn a_truncated_stream_is_an_error_not_a_guess() {
    let mut rng = Rng(9);
    let data = lane_data(&mut rng, &BC1, 200);
    let stream = flush_stream(&BC1, &data, 200, 5);
    for cut in [0usize, 10, 50, stream.len() / 2, stream.len() - 1] {
        assert!(
            decode_slice(&stream[..cut], &BC1, 200).is_err(),
            "cut {cut}"
        );
    }
}

#[test]
fn a_corrupt_stream_never_decodes_to_the_original() {
    let mut rng = Rng(11);
    let data = lane_data(&mut rng, &BC1, 400);
    let mut stream = flush_stream(&BC1, &data, 400, 2);
    let at = stream.len() / 2;
    stream[at] ^= 0x55;
    if let Ok(out) = decode_slice(&stream, &BC1, 400) {
        assert!((0..12).any(|i| out.lane(i) != &data[i][..]));
    }
}

#[test]
fn narrowed_widths_and_model_changes_between_fragments() {
    let mut rng = Rng(21);
    let widths = [8u8, 8, 5, 6];
    let n = 900usize;
    let l0: Vec<u8> = (0..n).map(|i| (i % 19) as u8).collect();
    let l1 = vec![0u8; n];
    let l2: Vec<u8> = (0..n).map(|_| rng.skewed(3)).collect();
    let l3: Vec<u8> = (0..n).map(|_| rng.skewed(6)).collect();
    let frag = |z, q, count| Frag { z, q, count };
    let lanes = vec![
        encode_cut(
            8,
            &[frag(2, 9, 100), frag(0, 3, 100), frag(1, 14, 100)],
            &l0,
        ),
        encode_flush(8, &[(frag(0, 15, 0), &l1)]),
        encode_cut(5, &[frag(2, 5, 300), frag(0, 0, 1)], &l2),
        encode_flush(6, &[(frag(0, 1, 0), &l3)]),
    ];
    let stream = interleave(&widths, lanes, n);
    let out = decode_slice(&stream, &widths, n).unwrap();
    for (i, d) in [l0, l1, l2, l3].iter().enumerate() {
        assert_eq!(out.lane(i), &d[..], "lane {i}");
    }
}

#[test]
fn a_fragment_wider_than_the_lane_is_rejected() {
    let widths = [3u8];
    let lane = encode_flush(
        3,
        &[(
            Frag {
                z: 0,
                q: 0,
                count: 0,
            },
            &[1u8, 2, 3, 4],
        )],
    );
    let mut bad = lane.clone();
    bad[0] |= 3 << 6;
    assert_eq!(
        decode_slice(&bad, &widths, 4).err(),
        Some(LaneError::BadHeader { lane: 0 })
    );
}

#[test]
fn the_first_bytes_follow_the_eight_per_cycle_window() {
    // Twelve lanes of one symbol: lanes 0..8 are served first, lanes 8..12 wait for their turn.
    let widths = [8u8; 12];
    let lanes: Vec<Vec<u8>> = (0..12u8)
        .map(|l| {
            encode_flush(
                8,
                &[(
                    Frag {
                        z: 0,
                        q: l,
                        count: 0,
                    },
                    &[l],
                )],
            )
        })
        .collect();
    let stream = interleave(&widths, lanes, 1);
    let first_headers: Vec<u8> = stream[..8].iter().map(|b| b & 15).collect();
    assert_eq!(first_headers, [0, 1, 2, 3, 4, 5, 6, 7]);
    let second: Vec<u8> = stream[8..16].to_vec();
    assert!(
        second.iter().all(|b| *b == 3),
        "payload length byte of each short fragment"
    );
    assert_eq!(
        stream[8 * 6..8 * 6 + 4]
            .iter()
            .map(|b| b & 15)
            .collect::<Vec<_>>(),
        [8, 9, 10, 11]
    );
}

/// A synthetic BC1 chunk of `blocks` blocks: no predictors (lane 0 and 1 are zero), skewed endpoint and index
/// symbols. The lane array is built with the test encoder and replayed through the clock.
pub fn synthetic_bc1(blocks: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed);
    let widths = super::BcPackCodec::Bc1.lane_widths().unwrap();
    let lanes = widths
        .iter()
        .enumerate()
        .map(|(l, &w)| {
            let data: Vec<u8> = (0..blocks)
                .map(|_| if l < 2 { 0 } else { rng.skewed(u32::from(w)) })
                .collect();
            let chunks: Vec<(Frag, &[u8])> = data
                .chunks(20000)
                .map(|c| {
                    let q = if l < 2 { 15 } else { 5 };
                    (Frag { z: 0, q, count: 0 }, c)
                })
                .collect();
            encode_flush(w, &chunks)
        })
        .collect();
    interleave(widths, lanes, blocks)
}

#[test]
#[ignore = "benchmark: cargo test --release -p assets --lib -- --ignored speed --nocapture"]
fn speed_on_synthetic_bc1() {
    use std::time::Instant;
    let blocks = 262_144;
    let stream = synthetic_bc1(blocks, 99);
    let mb = (blocks * 8) as f64 / 1e6;
    let mut best_lanes = f64::MAX;
    let mut best_full = f64::MAX;
    for _ in 0..7 {
        let t = Instant::now();
        let out = decode_slice(
            &stream,
            &super::BcPackCodec::Bc1.lane_widths().unwrap(),
            blocks,
        )
        .unwrap();
        best_lanes = best_lanes.min(t.elapsed().as_secs_f64());
        std::hint::black_box(&out);
        let t = Instant::now();
        let full = super::decode(super::BcPackCodec::Bc1, &stream, blocks * 8).unwrap();
        best_full = best_full.min(t.elapsed().as_secs_f64());
        std::hint::black_box(&full);
    }
    eprintln!(
        "synthetic BC1, {} KiB packed -> {mb:.1} MB: lane stage {:.1} MB/s, full decode {:.1} MB/s",
        stream.len() / 1024,
        mb / best_lanes,
        mb / best_full
    );
}
