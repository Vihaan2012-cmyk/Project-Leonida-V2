//! BC6H on the game's real chunks, gated on `LEONIDA_GAME_DIR`.

use super::bc6h::{Entry as Hist, History, Trace, rebuild};
use super::lanes::decode_slice;
use super::survey_tests::{chunk_entries, pool_paths, read_chunk};
use super::{BcPackCodec, DecodeOptions};
use std::collections::BTreeMap;

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
    for rel in ["lighting/lighting.ttpf", "vfx/ttdb_vfx.ttpf"] {
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
