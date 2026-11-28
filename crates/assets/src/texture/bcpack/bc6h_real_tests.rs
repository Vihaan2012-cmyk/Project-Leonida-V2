//! BC6H on the game's real chunks, gated on `LEONIDA_GAME_DIR`.

#![allow(clippy::needless_range_loop)]

use super::bc6h::{Entry as Hist, History, Trace, rebuild};
use super::lanes::decode_slice;
use super::survey_tests::{chunk_entries, pool_paths, read_chunk};
use super::{BcPackCodec, DecodeOptions};
use crate::texture::bcn;
use std::collections::BTreeMap;

fn quantile(v: &mut [f32], q: f32) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(((v.len() as f32 - 1.0) * q) as usize)
        .copied()
        .unwrap_or(f32::NAN)
}

/// Which history entries each BC6H chunk of the game reaches.
#[test]
fn bc6h_history_use_on_real_chunks() {
    let widths = BcPackCodec::Bc6h.lane_widths().unwrap();
    let mut neutral = History::empty();
    for e in &mut neutral.endpoints {
        *e = Hist::Provisional([[0x8000, 0x200, 0x20, 0x20]; 3]);
    }
    for e in &mut neutral.indices {
        *e = Hist::Provisional([8; 16]);
    }
    let opts = DecodeOptions {
        allow_provisional: true,
    };
    let (mut chunks, mut lanes_ok, mut selfc) = (0, 0, 0);
    let mut entries: BTreeMap<i16, usize> = BTreeMap::new();
    for path in pool_paths() {
        let Some(table) = chunk_entries(&path) else {
            continue;
        };
        for e in table.iter().filter(|e| e.codec == 6) {
            chunks += 1;
            let raw = read_chunk(&path, e).unwrap();
            let Ok(lanes) = decode_slice(&raw, widths, e.size as usize / 8) else {
                eprintln!("lanes failed: {:?}", path.file_name());
                continue;
            };
            lanes_ok += 1;
            let mut out = vec![0u8; e.size as usize];
            let mut trace = Trace::default();
            rebuild(&lanes, &mut out, &neutral, &opts, Some(&mut trace)).unwrap();
            let n = out.len() / 16;
            let ep = trace.endpoint_root.iter().filter(|r| **r >= 0).count();
            let ix = trace.index_root.iter().filter(|r| **r >= 0).count();
            if ep == 0 && ix == 0 {
                selfc += 1;
            }
            for r in trace.endpoint_root.iter().filter(|r| **r >= 0) {
                *entries.entry(*r).or_default() += 1;
            }
            eprintln!(
                "{:?} chunk {} blocks: {ep} on history endpoints, {ix} on history indices, {} anchor faults",
                path.file_name().unwrap(),
                n,
                trace.faulty
            );
        }
    }
    eprintln!("{chunks} BC6H chunks, {lanes_ok} lane arrays decode, {selfc} without history");
    eprintln!("endpoint roots: {entries:?}");
}

/// The BC6H textures of a pool and the codecs holding their mips.
#[test]
fn bc6h_textures_in_small_pools() {
    use crate::texture::pool::Pool;
    for rel in std::env::var("LEONIDA_BC6H_POOLS")
        .unwrap_or("lighting/lighting.ttpf,vfx/ttdb_vfx.ttpf".into())
        .split(',')
    {
        let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
            return;
        };
        let file =
            std::fs::read(std::path::PathBuf::from(dir).join("data/ttdb").join(rel)).unwrap();
        let pool = Pool::parse(&file).unwrap();
        for i in 0..pool.texture_count() {
            let Some(d) = pool.descriptor(i) else {
                continue;
            };
            if !matches!(d.format, 94..=96) || d.is_unmapped() {
                continue;
            }
            let size = pool.span(i, d.base).unwrap_or(0);
            let mut codecs: BTreeMap<u8, usize> = BTreeMap::new();
            if let Ok(cs) = pool.chunks_for(d.base, size) {
                for c in cs {
                    *codecs.entry(c.codec).or_default() += 1;
                }
            }
            if codecs.contains_key(&6) {
                eprintln!(
                    "{rel} #{i}: {}x{} fmt {} dim {} depth {} mips {} (packed {}) mode {} span {size}: {codecs:?}",
                    d.width,
                    d.height,
                    d.format,
                    d.dimension,
                    d.depth,
                    d.mips,
                    d.packed_mips,
                    d.mode
                );
            }
        }
    }
}

/// Mode mix of the chunks and how often a predictor block has the same mode.
#[test]
fn bc6h_mode_mix_on_real_chunks() {
    let widths = BcPackCodec::Bc6h.lane_widths().unwrap();
    let mut modes = [0usize; 14];
    let (mut refs, mut same, mut hist_refs, mut zero_refs) = (0usize, 0usize, 0usize, 0usize);
    let mut first_hist_pos = [0usize; 4];
    for path in pool_paths() {
        let Some(table) = chunk_entries(&path) else {
            continue;
        };
        for e in table.iter().filter(|e| e.codec == 6).take(30) {
            let raw = read_chunk(&path, e).unwrap();
            let lanes = decode_slice(&raw, widths, e.size as usize / 8).unwrap();
            let n = e.size as usize / 16;
            let (l0, l1, l15) = (lanes.lane(0), lanes.lane(1), lanes.lane(15));
            let mut last_hist = 0;
            for i in 0..n {
                let m = usize::from(l15[2 * i]);
                modes[m] += 1;
                let di = usize::from(super::blocks::interleave4(l1[2 * i], l1[2 * i + 1]));
                let de = (di as i32
                    + super::blocks::unzig(super::blocks::interleave4(l0[2 * i], l0[2 * i + 1])))
                    as usize
                    & 255;
                if de == 0 {
                    zero_refs += 1;
                } else if de > i {
                    hist_refs += 1;
                    last_hist = i;
                } else {
                    refs += 1;
                    if l15[2 * (i - de)] == l15[2 * i] {
                        same += 1;
                    }
                }
            }
            first_hist_pos[(last_hist * 4 / n.max(1)).min(3)] += 1;
        }
    }
    eprintln!("modes {modes:?}");
    eprintln!(
        "endpoint refs: {refs} to earlier blocks ({same} same mode), {hist_refs} to history, {zero_refs} none"
    );
    eprintln!("last history reference by quarter of chunk: {first_hist_pos:?}");
}

/// Linear RGB floats of a BC6H mip of `blocks` (row-major blocks).
fn bc6h_pixels(w: u32, h: u32, blocks: &[u8]) -> (Vec<[f32; 3]>, usize) {
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let stride = bw * 4;
    let mut px = vec![[0f32; 3]; stride * bh * 4];
    for by in 0..bh {
        for bx in 0..bw {
            for (p, v) in bcn::bc6h_block(&blocks[(by * bw + bx) * 16..][..16], false)
                .into_iter()
                .enumerate()
            {
                px[(by * 4 + p / 4) * stride + bx * 4 + p % 4] = v;
            }
        }
    }
    (px, stride)
}

/// Mean log error between the 2x2 box filter of mip 0 and mip 1, and the same with mip 1 shifted sideways.
fn mip_error(w: u32, h: u32, m0: &[u8], m1: &[u8], shift: usize) -> f32 {
    let (p0, s0) = bc6h_pixels(w, h, m0);
    let (w1, h1) = ((w / 2).max(1), (h / 2).max(1));
    let (p1, s1) = bc6h_pixels(w1, h1, m1);
    let (mut sum, mut n) = (0f32, 0f32);
    for y in 0..h1 as usize {
        for x in 0..w1 as usize {
            let at = |xx: usize, yy: usize| p0[yy * s0 + xx];
            for c in 0..3 {
                let avg = (at(2 * x, 2 * y)[c]
                    + at(2 * x + 1, 2 * y)[c]
                    + at(2 * x, 2 * y + 1)[c]
                    + at(2 * x + 1, 2 * y + 1)[c])
                    / 4.0;
                let other = p1[y * s1 + (x + shift) % w1 as usize][c];
                sum += ((avg + 1e-3).ln() - (other + 1e-3).ln()).abs().min(4.0);
                n += 1.0;
            }
        }
    }
    sum / n
}

/// BC6H textures of the small pools through the whole pool pipeline: how many decode completely and how well mip
/// 1 matches mip 0 (against a shifted control).
#[test]
fn bc6h_textures_decode_through_the_pool() {
    use crate::texture::format::{format_info, mip_bytes};
    use crate::texture::pool::{ChunkCache, Pool};
    use crate::texture::{Shape, TextureResult};
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        return;
    };
    let list = std::env::var("LEONIDA_BC6H_DECODE_POOLS")
        .unwrap_or("lighting/lighting.ttpf,vfx/ttdb_vfx.ttpf".into());
    for rel in list.split(',') {
        let file =
            std::fs::read(std::path::PathBuf::from(&dir).join("data/ttdb").join(rel)).unwrap();
        let pool = Pool::parse(&file).unwrap();
        let (mut seen, mut complete, mut partial, mut placeholder) = (0, 0, 0, 0);
        let (mut errs, mut ctls) = (Vec::new(), Vec::new());
        let mut cache = ChunkCache::new();
        for i in 0..pool.texture_count() {
            let Some(d) = pool.descriptor(i) else {
                continue;
            };
            if !matches!(d.format, 95 | 96)
                || d.dimension != 1
                || d.depth != 1
                || d.mips < 2
                || d.is_unmapped()
            {
                continue;
            }
            let size = pool.span(i, d.base).unwrap_or(0);
            let Ok(set) = pool.codecs(d.base, size) else {
                continue;
            };
            if !set.contains(6) {
                continue;
            }
            seen += 1;
            cache.clear();
            match pool.decode_texture(i, &mut cache) {
                Ok(TextureResult::Complete(t)) => {
                    complete += 1;
                    assert_eq!(t.info.shape, Shape::Texture2D);
                    let fi = format_info(d.format).unwrap();
                    let m0 = mip_bytes(&fi, d.width, d.height) as usize;
                    let (w1, h1) = ((d.width / 2).max(1), (d.height / 2).max(1));
                    let m1 = mip_bytes(&fi, w1, h1) as usize;
                    let e = mip_error(d.width, d.height, &t.data[..m0], &t.data[m0..m0 + m1], 0);
                    if std::env::var_os("LEONIDA_BC6H_VERBOSE").is_some() {
                        eprintln!(
                            "  #{i} {}x{} mips {} codecs {:#x}: error {e:.3}",
                            d.width, d.height, d.mips, set.0
                        );
                    }
                    errs.push(e);
                    ctls.push(mip_error(
                        d.width,
                        d.height,
                        &t.data[..m0],
                        &t.data[m0..m0 + m1],
                        (w1 as usize / 3).max(2),
                    ));
                }
                Ok(TextureResult::Partial { .. }) => partial += 1,
                _ => placeholder += 1,
            }
        }
        eprintln!(
            "{rel}: {seen} BC6H textures with BCPack: {complete} complete, {partial} partial, {placeholder} placeholder; mip 0 vs mip 1 log error median {:.3}, shifted control median {:.3}",
            quantile(&mut errs, 0.5),
            quantile(&mut ctls, 0.5)
        );
    }
}
