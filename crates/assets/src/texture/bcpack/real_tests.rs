//! Real-data checks, gated on `LEONIDA_GAME_DIR`.
//!
//! There is no ground truth for BCPack output, so the check is self-consistency: a texture's mip 1 is a
//! downsample of its mip 0, and the two live in different BCPack chunks. Decode both independently, convert the
//! BCn blocks to RGBA, box-filter mip 0 and compare with mip 1. A correct decode gives a small error; a wrong
//! block stage gives noise. Run with `--nocapture` for the per-codec table.

use super::{BcPackCodec, DecodeOptions};
use crate::texture::bcn;
use crate::texture::format::{format_info, mip_bytes};
use crate::texture::pool::{ChunkCache, Pool};
use crate::texture::{Shape, TextureResult};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn pool_file(rel: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("LEONIDA_GAME_DIR")?;
    let p = PathBuf::from(dir).join("data/ttdb").join(rel);
    std::fs::read(p).ok()
}

/// Pixels of a BCn mip as `channels` bytes each, rows tightly packed.
fn to_pixels(dxgi: u32, w: u32, h: u32, blocks: &[u8]) -> Option<Plane> {
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let (block_bytes, channels) = match dxgi {
        71 | 72 => (8, 4),
        77 | 78 => (16, 4),
        80 => (8, 1),
        83 => (16, 2),
        _ => return None,
    };
    let mut px = vec![0u8; bw * 4 * bh * 4 * channels];
    for by in 0..bh {
        for bx in 0..bw {
            let at = (by * bw + bx) * block_bytes;
            let b = blocks.get(at..at + block_bytes)?;
            for p in 0..16 {
                let (x, y) = (bx * 4 + p % 4, by * 4 + p / 4);
                let o = (y * bw * 4 + x) * channels;
                match dxgi {
                    71 | 72 => px[o..o + 4].copy_from_slice(&bcn::bc1_block(b, false)[p]),
                    77 | 78 => px[o..o + 4].copy_from_slice(&bcn::bc3_block(b)[p]),
                    80 => px[o] = bcn::bc4_block(b)[p],
                    _ => px[o..o + 2].copy_from_slice(&bcn::bc5_block(b)[p]),
                }
            }
        }
    }
    Some(Plane {
        px,
        width: bw * 4,
        height: bh * 4,
        channels,
    })
}

/// Pixels of one mip: `channels` bytes each, `width` pixels per row (a multiple of four).
struct Plane {
    px: Vec<u8>,
    width: usize,
    height: usize,
    channels: usize,
}

/// Mean absolute error per channel between the 2x2 box filter of `big` and `small`, with `small` read `shift`
/// pixels to the right (the control uses a non-zero shift).
fn box_error(big: &Plane, small: &Plane, shift: usize) -> f64 {
    let ch = big.channels;
    let (w, h) = (
        small.width.min(big.width / 2),
        small.height.min(big.height / 2),
    );
    let mut sum = 0u64;
    for y in 0..h {
        for x in 0..w {
            for c in 0..ch {
                let at = |xx: usize, yy: usize| u32::from(big.px[(yy * big.width + xx) * ch + c]);
                let avg = (at(2 * x, 2 * y)
                    + at(2 * x + 1, 2 * y)
                    + at(2 * x, 2 * y + 1)
                    + at(2 * x + 1, 2 * y + 1)
                    + 2)
                    / 4;
                let sx = (x + shift) % w;
                sum +=
                    u64::from(avg.abs_diff(u32::from(small.px[(y * small.width + sx) * ch + c])));
            }
        }
    }
    sum as f64 / (w * h * ch) as f64
}

#[derive(Default)]
struct Stats {
    seen: usize,
    refused: usize,
    errors: Vec<f64>,
    controls: Vec<f64>,
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

/// Self-consistency over the 2D BC1/BC3/BC4/BC5 textures of a pool.
fn survey(file: &[u8], opts: DecodeOptions, per_codec: usize) -> BTreeMap<&'static str, Stats> {
    let pool = Pool::parse(file).unwrap().with_bcpack_options(opts);
    let mut stats: BTreeMap<&'static str, Stats> = BTreeMap::new();
    let mut cache = ChunkCache::new();
    for index in 0..pool.texture_count() {
        let Some(d) = pool.descriptor(index) else {
            continue;
        };
        if d.dimension != 1
            || d.depth != 1
            || d.mips < 2
            || d.width < 16
            || d.height < 16
            || d.is_unmapped()
        {
            continue;
        }
        let name = match d.format {
            71 | 72 => "BC1",
            77 | 78 => "BC3",
            80 => "BC4",
            83 => "BC5",
            _ => continue,
        };
        let (start, size) = (d.base, pool.span(index, d.base).unwrap_or(0));
        let Ok(set) = pool.codecs(start, size) else {
            continue;
        };
        if !set.has_bcpack() {
            continue;
        }
        let st = stats.entry(name).or_default();
        if st.seen >= per_codec {
            continue;
        }
        st.seen += 1;
        cache.clear();
        let Ok(result) = pool.decode_texture(index, &mut cache) else {
            st.refused += 1;
            continue;
        };
        let TextureResult::Complete(t) = result else {
            st.refused += 1;
            continue;
        };
        assert_eq!(t.info.shape, Shape::Texture2D);
        let fi = format_info(d.format).unwrap();
        let (w0, h0) = (d.width, d.height);
        let (w1, h1) = ((w0 / 2).max(1), (h0 / 2).max(1));
        let m0 = mip_bytes(&fi, w0, h0) as usize;
        let m1 = mip_bytes(&fi, w1, h1) as usize;
        let (Some(p0), Some(p1)) = (
            to_pixels(d.format, w0, h0, &t.data[..m0]),
            to_pixels(d.format, w1, h1, &t.data[m0..m0 + m1]),
        ) else {
            continue;
        };
        let err = box_error(&p0, &p1, 0);
        let control = box_error(&p0, &p1, (w1 as usize / 3).max(2));
        st.errors.push(err);
        st.controls.push(control);
    }
    stats
}

fn report(label: &str, stats: &mut BTreeMap<&'static str, Stats>) {
    for (codec, st) in stats.iter_mut() {
        let n = st.errors.len();
        let med = median(&mut st.errors);
        let p90 = st.errors.get(n * 9 / 10).copied().unwrap_or(f64::NAN);
        let ctl = median(&mut st.controls);
        eprintln!(
            "{label} {codec}: {} sampled, {} decoded, {} refused; mean abs error median {med:.2}, p90 {p90:.2}; shifted-control median {ctl:.2}",
            st.seen, n, st.refused
        );
    }
}

/// Pools to survey: `LEONIDA_BCPACK_POOLS` (comma separated, relative to `data/ttdb`) or a small default set.
fn survey_pools() -> Vec<String> {
    match std::env::var("LEONIDA_BCPACK_POOLS") {
        Ok(v) => v.split(',').map(str::to_string).collect(),
        Err(_) => [
            "graphics/ttdb_graphics.ttpf",
            "peds/peds_loose.ttpf",
            "lighting/lighting.ttpf",
        ]
        .map(str::to_string)
        .to_vec(),
    }
}

#[test]
fn mip_zero_downsamples_to_mip_one_on_real_pools() {
    for rel in survey_pools() {
        let Some(file) = pool_file(&rel) else {
            continue;
        };
        for (label, allow) in [("strict", false), ("provisional", true)] {
            let opts = DecodeOptions {
                allow_provisional: allow,
            };
            let mut stats = survey(&file, opts, 400);
            report(&format!("{rel} [{label}]"), &mut stats);
            for (codec, st) in stats.iter_mut() {
                if st.errors.len() >= 10 {
                    let (e, c) = (median(&mut st.errors), median(&mut st.controls));
                    assert!(e < 12.0, "{rel} {codec} median error {e}");
                    assert!(
                        c > e * 1.2,
                        "{rel} {codec}: control {c} is not worse than {e}"
                    );
                }
            }
        }
    }
}

/// Throughput of the whole chunk decode over every BC1/BC3/BC5 chunk of a pool, in decoded MB/s.
#[test]
fn decode_speed_on_real_chunks() {
    use super::decode_with;
    use std::time::Instant;
    for rel in ["peds/peds_loose.ttpf", "graphics/ttdb_graphics.ttpf"] {
        let Some(file) = pool_file(rel) else { continue };
        let pool = Pool::parse(&file).unwrap();
        let opts = DecodeOptions {
            allow_provisional: true,
        };
        let mut per: BTreeMap<u8, (usize, usize, f64)> = BTreeMap::new();
        for c in pool.chunks().iter().filter(|c| (1..=5).contains(&c.codec)) {
            let codec = BcPackCodec::from_chunk_codec(c.codec).unwrap();
            let raw = &file[c.src as usize..(c.src + c.packed) as usize];
            let t = Instant::now();
            let ok = decode_with(codec, raw, c.size as usize, &opts).is_ok();
            let e = per.entry(c.codec).or_default();
            if ok {
                e.0 += 1;
                e.1 += c.size as usize;
                e.2 += t.elapsed().as_secs_f64();
            }
        }
        for (codec, (n, bytes, secs)) in per {
            eprintln!(
                "{rel} codec {codec}: {n} chunks, {:.1} MB decoded in {secs:.2} s = {:.1} MB/s",
                bytes as f64 / 1e6,
                bytes as f64 / 1e6 / secs
            );
        }
    }
}
