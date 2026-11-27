//! Decodes the models of a small game archive when `LEONIDA_GAME_DIR` is set. Resources are carved
//! without the archive's table of contents: a page-aligned zlib stream at page offset 16 behind 16
//! zero bytes, followed by the resource's other page streams.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use flate2::read::ZlibDecoder;

use super::*;

const PAGE: u64 = 4096;
const WINDOW: usize = 4 << 20;

fn archive() -> Option<File> {
    let dir = PathBuf::from(std::env::var_os("LEONIDA_GAME_DIR")?);
    let path = std::env::var("LEONIDA_MODEL_ARCHIVE")
        .unwrap_or_else(|_| "pack/story000_restofworld/story000_restofworld.rpf".into());
    File::open(dir.join(path)).ok()
}

fn inflate_at(file: &mut File, at: u64) -> Option<(Vec<u8>, usize)> {
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut window = Vec::new();
    file.take(WINDOW as u64).read_to_end(&mut window).ok()?;
    let mut decoder = ZlibDecoder::new(&window[..]);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).ok()?;
    Some((out, decoder.total_in() as usize))
}

fn carve(file: &mut File, at: u64) -> Option<(Vec<u8>, Vec<u8>)> {
    let (first, used) = inflate_at(file, at)?;
    if first.len() < 192 || first[..8] != [0; 8] {
        return None;
    }
    let directory =
        u64::from_le_bytes(first[8..16].try_into().ok()?).checked_sub(0x5000_0000)? as usize;
    let (system_pages, graphics_pages) = (
        *first.get(directory + 8)? as usize,
        *first.get(directory + 9)? as usize,
    );
    if system_pages == 0 || system_pages > 32 || graphics_pages > 32 {
        return None;
    }
    let mut streams = vec![first];
    let mut next = at + used as u64;
    for _ in 1..system_pages + graphics_pages {
        next = (next + 15) & !15;
        let mut probe = [0u8; 16];
        file.seek(SeekFrom::Start(next)).ok()?;
        file.read_exact(&mut probe).ok()?;
        next += probe.iter().position(|b| *b != 0)? as u64;
        let (data, used) = inflate_at(file, next)?;
        next += used as u64;
        streams.push(data);
    }
    let graphics = streams.split_off(system_pages).concat();
    Some((streams.concat(), graphics))
}

#[test]
fn small_archive_models_decode_and_export() {
    let Some(mut file) = archive() else {
        eprintln!("LEONIDA_GAME_DIR not set or archive missing: skipped");
        return;
    };
    let length = file.metadata().unwrap().len();
    let limit = std::env::var("LEONIDA_MODEL_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let (mut decoded, mut rejected, mut named, mut failed) = (0, 0, 0, Vec::new());
    let mut page = 0;
    while page + PAGE <= length && decoded < limit {
        let at = page + 16;
        page += PAGE;
        let mut head = [0u8; 18];
        file.seek(SeekFrom::Start(at - 16)).unwrap();
        if file.read_exact(&mut head).is_err() || head[..16] != [0; 16] || head[16] != 0x78 {
            continue;
        }
        let Some((system, graphics)) = carve(&mut file, at) else {
            continue;
        };
        match decode_pages(&system, &graphics, None) {
            Ok(set) => {
                let exported = export(&set, &ExportOptions::default()).unwrap();
                assert!(!exported.is_empty());
                for e in &exported {
                    let glb = tests::parse_glb(&e.glb);
                    glb.validate();
                    check_rest_pose(&glb);
                    serde_json::from_str::<serde_json::Value>(&e.sidecar).unwrap();
                }
                named += set
                    .drawables
                    .iter()
                    .flat_map(|d| &d.materials)
                    .flat_map(|m| &m.textures)
                    .filter(|t| t.param_hash.and_then(parameter_name).is_some())
                    .count();
                decoded += 1;
            }
            Err(ModelError::NotAModel) => rejected += 1,
            Err(e) => failed.push(e.to_string()),
        }
    }
    eprintln!(
        "decoded {decoded}, not models {rejected}, named textures {named}, failed {failed:?}"
    );
    assert!(decoded > 0, "no model decoded");
    assert!(
        named > 0,
        "no texture parameter hash resolved to a known name"
    );
    assert!(failed.is_empty(), "decode failures: {failed:?}");
}

/// With every joint at its default transform, joint world matrix times inverse bind is identity.
fn check_rest_pose(glb: &tests::Glb) {
    let Some(skin) = glb.doc["skins"].get(0) else {
        return;
    };
    let world = tests::world_matrices(glb);
    let ibm = glb.floats(skin["inverseBindMatrices"].as_u64().unwrap() as usize);
    for (j, node) in skin["joints"].as_array().unwrap().iter().enumerate() {
        let m: tests::Mat = ibm[16 * j..16 * j + 16].try_into().unwrap();
        let rest = tests::mul(&world[node.as_u64().unwrap() as usize], &m);
        for (i, v) in rest.iter().enumerate() {
            let want = if i % 5 == 0 { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 2e-3, "joint {j} entry {i}: {v}");
        }
    }
}
