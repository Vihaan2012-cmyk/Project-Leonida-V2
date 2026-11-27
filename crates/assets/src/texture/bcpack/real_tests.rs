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
fn to_pixels(dxgi: u32, w: u32, h: u32, blocks: &[u8]) -> Option<(Vec<u8>, usize)> {
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
    Some((px, channels))
}

/// Mean absolute error per channel between the 2x2 box filter of `big` and `small`, with `small` optionally
/// shifted by `shift` pixels (the control).
fn box_error(
    big: &[u8],
    bw: usize,
    bh: usize,
    small: &[u8],
    sw: usize,
    sh: usize,
    ch: usize,
    shift: usize,
) -> f64 {
    let (w, h) = (sw.min(bw / 2), sh.min(bh / 2));
    let mut sum = 0u64;
    for y in 0..h {
        for x in 0..w {
            for c in 0..ch {
                let at = |xx: usize, yy: usize| u32::from(big[(yy * bw + xx) * ch + c]);
                let avg = (at(2 * x, 2 * y)
                    + at(2 * x + 1, 2 * y)
                    + at(2 * x, 2 * y + 1)
                    + at(2 * x + 1, 2 * y + 1)
                    + 2)
                    / 4;
                let sx = (x + shift) % w;
                sum += u64::from(avg.abs_diff(u32::from(small[(y * sw + sx) * ch + c])));
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
fn survey(file: &[u8], opts: DecodeOptions, per_pool: usize) -> BTreeMap<&'static str, Stats> {
    let pool = Pool::parse(file).unwrap().with_bcpack_options(opts);
    let mut stats: BTreeMap<&'static str, Stats> = BTreeMap::new();
    let mut cache = ChunkCache::new();
    let mut taken = 0;
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
        if taken >= per_pool {
            break;
        }
        taken += 1;
        cache.clear();
        let st = stats.entry(name).or_default();
        st.seen += 1;
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
        let (Some((p0, ch)), Some((p1, _))) = (
            to_pixels(d.format, w0, h0, &t.data[..m0]),
            to_pixels(d.format, w1, h1, &t.data[m0..m0 + m1]),
        ) else {
            continue;
        };
        let (bw0, bh0) = (w0.div_ceil(4) as usize * 4, h0.div_ceil(4) as usize * 4);
        let (bw1, bh1) = (w1.div_ceil(4) as usize * 4, h1.div_ceil(4) as usize * 4);
        let (cw, chh) = (
            (w1 as usize).min(w0 as usize / 2),
            (h1 as usize).min(h0 as usize / 2),
        );
        let _ = (bh1, chh);
        let err = box_error(&p0, bw0, bh0, &p1, bw1, bh1, ch, 0);
        let control = box_error(&p0, bw0, bh0, &p1, bw1, bh1, ch, (cw / 3).max(2));
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

#[test]
fn mip_zero_downsamples_to_mip_one_on_real_pools() {
    let mut any = false;
    for (rel, per_pool) in [
        ("graphics/ttdb_graphics.ttpf", 400),
        ("lighting/lighting.ttpf", 300),
        ("peds/peds_loose.ttpf", 300),
    ] {
        let Some(file) = pool_file(rel) else { continue };
        any = true;
        for (label, allow) in [("strict", false), ("provisional", true)] {
            let mut stats = survey(
                &file,
                DecodeOptions {
                    allow_provisional: allow,
                },
                per_pool,
            );
            report(&format!("{rel} [{label}]"), &mut stats);
            if !allow {
                if let Some(bc1) = stats.get_mut("BC1") {
                    if bc1.errors.len() >= 10 {
                        let (e, c) = (median(&mut bc1.errors), median(&mut bc1.controls));
                        assert!(e < 12.0, "BC1 median error {e}");
                        assert!(c > e * 1.2, "control {c} not worse than {e}");
                    }
                }
            }
        }
    }
    let _ = (any, BcPackCodec::Bc1);
}
