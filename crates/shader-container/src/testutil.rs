//! Builders for synthetic shader containers. Test data only: nothing here comes from the game.

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
