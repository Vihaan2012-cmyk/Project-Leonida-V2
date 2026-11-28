//! Real-game checks, gated on `LEONIDA_GAME_DIR`: every frame of a few movies decodes, and the inter chain stays
//! stable. The decoder itself rejects a slice that does not end where the next one starts, so a clean decode also
//! means the bit stream stayed in step with the encoder for every macroblock of every frame.

mod common;

use bink::{Container, Frame, Movie};
use common::Movies;

fn luma_psnr(a: &Frame, b: &Frame) -> f64 {
    let (pa, pb) = (&a.planes[0], &b.planes[0]);
    let mut sum = 0u64;
    for y in 0..a.height as usize {
        let (ra, rb) = (&pa.data[y * pa.stride..], &pb.data[y * pb.stride..]);
        for x in 0..a.width as usize {
            let d = i64::from(ra[x]) - i64::from(rb[x]);
            sum += (d * d) as u64;
        }
    }
    if sum == 0 {
        return 99.0;
    }
    let n = f64::from(a.width) * f64::from(a.height);
    10.0 * (255.0 * 255.0 * n / sum as f64).log10()
}

struct Chain {
    frames: u32,
    keys: Vec<u32>,
    mean: f64,
    min: f64,
    /// For each later keyframe: psnr to the frame before it, and the mean over the 20 pairs before that.
    at_keys: Vec<(u32, f64, f64)>,
}

fn decode_all(movies: &Movies, index: usize) -> Chain {
    let data = movies.arc(index);
    let container = Container::parse(data.clone()).unwrap();
    let mut movie = Movie::open(data).unwrap();
    let mut prev: Option<Frame> = None;
    let mut pairs: Vec<f64> = Vec::new();
    let mut at_keys = Vec::new();
    let mut keys = Vec::new();
    for f in 0..container.header.frames {
        let frame = movie
            .decode_next()
            .unwrap_or_else(|e| panic!("stream {index} frame {f}: {e}"))
            .clone();
        if container.is_key(f) {
            keys.push(f);
        }
        if let Some(p) = &prev {
            let v = luma_psnr(p, &frame);
            if container.is_key(f) {
                let tail = &pairs[pairs.len().saturating_sub(20)..];
                at_keys.push((f, v, tail.iter().sum::<f64>() / tail.len() as f64));
            }
            pairs.push(v);
        }
        prev = Some(frame);
    }
    Chain {
        frames: container.header.frames,
        keys,
        mean: pairs.iter().sum::<f64>() / pairs.len().max(1) as f64,
        min: pairs.iter().copied().fold(99.0, f64::min),
        at_keys,
    }
}

#[test]
fn every_frame_of_small_movies_decodes_and_neighbours_stay_close() {
    let Some(movies) = Movies::open() else { return };
    // A 15-frame KB2j movie, a 150-frame 512x512 KB2n movie, a 198-frame mostly still 1080p one and a 150-frame
    // 720x1280 one (whose width is not a whole number of macroblocks).
    for index in [29, 24, 4, 156] {
        let c = decode_all(&movies, index);
        println!(
            "stream {index}: {} frames, keys {:?}, mean adjacent psnr {:.2}, minimum {:.2}",
            c.frames, c.keys, c.mean, c.min
        );
        assert!(c.mean > 25.0, "stream {index} drifts: mean {:.2}", c.mean);
    }
}

/// Decodes whole 2560x1440 movies, so run it with `--release`. Streams 17 and 18 end in a cut to black, so their
/// last keyframe is a different picture from the frame before it and only shows that the chain is stable up to
/// there; stream 22 cuts between scenes at its keyframes.
#[test]
#[ignore = "decodes about 3300 frames of 2560x1440; run with --release"]
fn a_keyframe_after_a_long_inter_chain_matches_the_chain_before_it() {
    let Some(movies) = Movies::open() else { return };
    for index in [17, 18, 22] {
        let c = decode_all(&movies, index);
        println!(
            "stream {index}: {} frames, mean adjacent psnr {:.2}",
            c.frames, c.mean
        );
        for &(key, v, before) in &c.at_keys {
            println!(
                "  keyframe {key} is {v:.2} dB from the frame before, against {before:.2} dB for the 20 pairs earlier"
            );
            if index != 22 {
                assert!(
                    v > 28.0,
                    "stream {index}: the keyframe at {key} is only {v:.2} dB from the chain"
                );
            }
        }
    }
}

/// Cutscene movies with a keyframe every 150 frames: the keyframe is coded without the chain, so its distance to
/// the inter frame before it measures how far the decoded chain has wandered from the encoder's, and it has to stay
/// within a few dB of what neighbouring inter frames agree on. The block filters and the exact motion compensation
/// arithmetic are what keep it there.
#[test]
#[ignore = "decodes about 1500 frames of 1280x720; run with --release"]
fn regular_keyframes_stay_close_to_the_chain_before_them() {
    let Some(movies) = Movies::open() else { return };
    for (index, floor) in [(99, 40.0), (98, 27.0), (211, 24.0)] {
        let c = decode_all(&movies, index);
        println!(
            "stream {index}: {} frames, mean adjacent psnr {:.2}",
            c.frames, c.mean
        );
        for &(key, v, before) in &c.at_keys {
            println!(
                "  keyframe {key} is {v:.2} dB from the frame before, against {before:.2} dB for the pairs before"
            );
            assert!(v > floor, "stream {index}: keyframe {key} only {v:.2} dB");
        }
    }
}
