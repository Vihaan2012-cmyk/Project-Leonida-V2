//! Game-wide BCPack chunk census, gated on `LEONIDA_GAME_DIR`. Only each pool's chunk table is read.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(super) fn pool_paths() -> Vec<PathBuf> {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        return Vec::new();
    };
    let root = PathBuf::from(dir).join("data/ttdb");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "ttpf") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub(super) struct Entry {
    pub codec: u8,
    pub size: u64,
    pub src: u64,
    pub packed: u64,
}

/// Every chunk of a pool, from its entry table alone.
pub(super) fn chunk_entries(path: &Path) -> Option<Vec<Entry>> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 24];
    f.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RDS5" {
        return None;
    }
    let count = u32::from_le_bytes(head[4..8].try_into().ok()?) as usize;
    f.seek(SeekFrom::Start(24)).ok()?;
    let mut raw = vec![0u8; (count - 1) * 8];
    f.read_exact(&mut raw).ok()?;
    let entry = |i: usize| u64::from_le_bytes(raw[i * 8..i * 8 + 8].try_into().unwrap());
    Some(
        (0..count - 2)
            .map(|i| {
                let (v, n) = (entry(i), entry(i + 1));
                let size = (((n >> 30) & 0x3F_FFFF) - ((v >> 30) & 0x3F_FFFF)) * 4096;
                let (src, end) = ((v & 0x3FF_FFFF) * 256, (n & 0x3FF_FFFF) * 256);
                Entry {
                    codec: ((v >> 26) & 15) as u8,
                    size,
                    src,
                    packed: (end - src).saturating_sub(v >> 52),
                }
            })
            .collect(),
    )
}

#[test]
fn bcpack_chunks_per_codec_across_the_game() {
    let mut total: BTreeMap<u8, (usize, u64)> = BTreeMap::new();
    for path in pool_paths() {
        let Some(chunks) = chunk_entries(&path) else {
            continue;
        };
        let mut here: BTreeMap<u8, usize> = BTreeMap::new();
        for Entry { codec, size, .. } in chunks {
            let t = total.entry(codec).or_default();
            t.0 += 1;
            t.1 += size;
            *here.entry(codec).or_default() += 1;
        }
        let name = path.file_name().unwrap().to_string_lossy();
        if here.contains_key(&2) || here.contains_key(&6) {
            eprintln!(
                "{name}: BC2 {} BC6H {}",
                here.get(&2).unwrap_or(&0),
                here.get(&6).unwrap_or(&0)
            );
        }
    }
    for (codec, (n, bytes)) in &total {
        eprintln!(
            "codec {codec}: {n} chunks, {:.1} MB decoded",
            *bytes as f64 / 1e6
        );
    }
}

/// The packed bytes of one chunk.
pub(super) fn read_chunk(path: &Path, e: &Entry) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(e.src)).ok()?;
    let mut raw = vec![0u8; e.packed as usize];
    f.read_exact(&mut raw).ok()?;
    Some(raw)
}
