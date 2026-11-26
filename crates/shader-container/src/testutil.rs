//! Builders for synthetic shader containers. Test data only: nothing here comes from the game.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// A DXBC container holding `parts` in order.
pub fn dxbc(parts: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let table_end = 32 + 4 * parts.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"DXBC");
    out.extend_from_slice(&[0u8; 16]); // hash
    out.extend_from_slice(&1u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // total size, patched below
    out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    let mut offset = table_end;
    for (_, data) in parts {
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += 8 + data.len();
    }
    for (fourcc, data) in parts {
        out.extend_from_slice(&fourcc[..]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    let total = out.len() as u32;
    out[24..28].copy_from_slice(&total.to_le_bytes());
    out
}

/// A DXIL part: a stub program header (shader model 6.6, `kind`) followed by an XDAT directory of `entries`.
/// XDAT layout: "XDAT", u32 count, u32 header size, then count x {u32 tag, u32 offset, u32 size}; offsets are
/// relative to the DXIL part.
pub fn dxil(kind: u16, entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0x66u16.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0u8; 12]);
    let table_end = out.len() + 12 + 12 * entries.len();
    out.extend_from_slice(b"XDAT");
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&12u32.to_le_bytes());
    let mut offset = table_end;
    for (tag, payload) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        offset += payload.len();
    }
    for (_, payload) in entries {
        out.extend_from_slice(payload);
    }
    out
}

/// An XBCH payload: "XBCH", u32 0, u32 stored, u32 0, u32 unpacked, u32 0, then zlib(fssm).
pub fn xbch(fssm: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(fssm).expect("in-memory write");
    let compressed = encoder.finish().expect("in-memory write");
    let mut out = Vec::new();
    out.extend_from_slice(b"XBCH");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(fssm.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&compressed);
    out
}

/// A complete shader container: a DXIL part with one XBCH payload per FSSM blob, then a PRIV part.
pub fn container(kind: u16, fssm_blobs: &[Vec<u8>]) -> Vec<u8> {
    let entries: Vec<(u32, Vec<u8>)> = fssm_blobs
        .iter()
        .map(|blob| (crate::XBCH_TAG, xbch(blob)))
        .collect();
    let dxil = dxil(kind, &entries);
    dxbc(&[(b"DXIL", &dxil[..]), (b"PRIV", &b"private"[..])])
}

/// A minimal FSSM blob: magic, version 1, code offset 0x100 at 0xC0 (so code starts at 0x128), the wave flag at
/// 0xD4, then `code` and a closing s_endpgm.
pub fn fssm(wave32: bool, code: &[u32]) -> Vec<u8> {
    const CODE_OFFSET: usize = 0x100;
    let mut out = vec![0u8; CODE_OFFSET + 0x28];
    out[..4].copy_from_slice(b"FSSM");
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[0xC0..0xC4].copy_from_slice(&(CODE_OFFSET as u32).to_le_bytes());
    if wave32 {
        out[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
    }
    for word in code {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(&crate::ENDPGM);
    out
}
