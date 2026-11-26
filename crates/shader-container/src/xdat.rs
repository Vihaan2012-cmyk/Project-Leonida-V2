use flate2::{Decompress, FlushDecompress, Status};

use crate::ContainerError;
use crate::dxbc::{find_part, parts, u32_at};

/// XDAT tag of an XBCH payload (the compressed FSSM program).
pub const XBCH_TAG: u32 = 0x0D;

/// Defensive per-blob ceiling (not a format limit), as upstream.
pub const MAX_FSSM_BYTES: usize = 64 << 20;

/// Entries of the XDAT directory inside a DXIL part, in directory order, as (tag, payload).
pub fn xdat_entries(dxil: &[u8]) -> Result<Vec<(u32, &[u8])>, ContainerError> {
    let Some(x) = memchr::memmem::find(dxil, b"XDAT") else {
        return Ok(Vec::new());
    };
    if x + 12 > dxil.len() {
        return Err(ContainerError::Truncated("XDAT header"));
    }
    let count = u32_at(dxil, x + 4) as usize;
    let table_end = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(x + 12))
        .filter(|&end| end <= dxil.len())
        .ok_or(ContainerError::Truncated("XDAT entry table"))?;
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let at = x + 12 + 12 * k;
        let tag = u32_at(dxil, at);
        let off = u32_at(dxil, at + 4) as usize;
        let size = u32_at(dxil, at + 8) as usize;
        let end = off
            .checked_add(size)
            .filter(|&end| end <= dxil.len())
            .ok_or_else(|| {
                ContainerError::Invalid(format!("XDAT entry {k} extends past the DXIL part"))
            })?;
        if size > 0 && off < table_end && end > x {
            return Err(ContainerError::Invalid(format!(
                "XDAT entry {k} overlaps its directory"
            )));
        }
        out.push((tag, &dxil[off..end]));
    }
    Ok(out)
}

/// Decompress an XBCH payload ("XBCH", u32, u32 stored at 8, u32, u32 unpacked at 16, u32, zlib data at 24) to
/// its FSSM blob.
pub fn inflate_xbch(blob: &[u8]) -> Result<Vec<u8>, ContainerError> {
    if blob.len() < 24 || &blob[..4] != b"XBCH" {
        return Err(ContainerError::Invalid(
            "a tag-0x0D XDAT entry is not an XBCH payload".into(),
        ));
    }
    let stored = u32_at(blob, 8) as usize;
    let unpacked = u32_at(blob, 16) as usize;
    if unpacked > MAX_FSSM_BYTES {
        return Err(ContainerError::Invalid(format!(
            "XBCH declared size {unpacked} exceeds limit {MAX_FSSM_BYTES}"
        )));
    }
    let compressed = &blob[24..];
    if stored != compressed.len() {
        return Err(ContainerError::Invalid(format!(
            "XBCH stored size {stored} does not match payload size {}",
            compressed.len()
        )));
    }
    let data = inflate(compressed, unpacked, true, true)?;
    if data.len() < 6 {
        return Err(ContainerError::Truncated("FSSM magic or version"));
    }
    if &data[..4] != b"FSSM" {
        return Err(ContainerError::Invalid("invalid FSSM magic".into()));
    }
    Ok(data)
}

/// Every tag-0x0D payload in the container, decompressed to its FSSM blob, in directory order.
pub fn unpack_all(dxbc: &[u8]) -> Result<Vec<Vec<u8>>, ContainerError> {
    let parts = parts(dxbc)?;
    let Some(dxil) = find_part(&parts, b"DXIL") else {
        return Ok(Vec::new());
    };
    xdat_entries(dxil)?
        .into_iter()
        .filter(|&(tag, _)| tag == XBCH_TAG)
        .map(|(_, blob)| inflate_xbch(blob))
        .collect()
}

/// Inflate `input` (zlib-wrapped when `zlib`, raw deflate otherwise) to exactly `expected` bytes. With
/// `require_all_input`, bytes after the end of the stream are an error.
pub(crate) fn inflate(
    input: &[u8],
    expected: usize,
    zlib: bool,
    require_all_input: bool,
) -> Result<Vec<u8>, ContainerError> {
    let mut decoder = Decompress::new(zlib);
    // One spare byte detects a stream that expands past the declared size without inflating all of it.
    let mut out = Vec::with_capacity(expected + 1);
    let status = decoder
        .decompress_vec(input, &mut out, FlushDecompress::Finish)
        .map_err(|error| ContainerError::Invalid(format!("corrupt deflate stream: {error}")))?;
    if out.len() > expected {
        return Err(ContainerError::Invalid(format!(
            "inflated data exceeds the declared size {expected}"
        )));
    }
    if status != Status::StreamEnd {
        return Err(ContainerError::Truncated("deflate stream"));
    }
    if require_all_input && decoder.total_in() as usize != input.len() {
        return Err(ContainerError::Invalid(
            "trailing data after the deflate stream".into(),
        ));
    }
    if out.len() != expected {
        return Err(ContainerError::Invalid(format!(
            "inflated size {} does not match the declared size {expected}",
            out.len()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{container, dxil, xbch};

    fn fake_fssm(fill: u8) -> Vec<u8> {
        let mut blob = vec![fill; 0x40];
        blob[..4].copy_from_slice(b"FSSM");
        blob
    }

    #[test]
    fn entries_in_directory_order() {
        let d = dxil(5, &[(0x12, vec![1; 16]), (XBCH_TAG, vec![2; 3])]);
        let entries = xdat_entries(&d).unwrap();
        assert_eq!(
            entries,
            vec![(0x12, &[1u8; 16][..]), (XBCH_TAG, &[2u8; 3][..])]
        );
    }

    #[test]
    fn no_xdat_means_no_entries() {
        assert!(
            xdat_entries(b"\x66\x00\x05\x00nothing here")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn xbch_inflates_to_fssm() {
        assert_eq!(inflate_xbch(&xbch(&fake_fssm(7))).unwrap(), fake_fssm(7));
    }

    #[test]
    fn xbch_size_errors_are_caught() {
        let mut blob = xbch(&fake_fssm(7));
        blob[16..20].copy_from_slice(&0x30u32.to_le_bytes()); // declared smaller than the real 0x40
        assert!(matches!(
            inflate_xbch(&blob),
            Err(ContainerError::Invalid(_))
        ));
        let mut blob = xbch(&fake_fssm(7));
        blob[8..12].copy_from_slice(&1u32.to_le_bytes()); // stored size disagrees with payload
        assert!(matches!(
            inflate_xbch(&blob),
            Err(ContainerError::Invalid(_))
        ));
        assert!(matches!(
            inflate_xbch(b"NOPE and more bytes here!"),
            Err(ContainerError::Invalid(_))
        ));
    }

    #[test]
    fn unpack_all_returns_every_payload_in_order() {
        let c = container(0, &[fake_fssm(1), fake_fssm(2)]);
        assert_eq!(unpack_all(&c).unwrap(), vec![fake_fssm(1), fake_fssm(2)]);
    }

    #[test]
    fn overlapping_directory_is_rejected() {
        let mut d = dxil(5, &[(XBCH_TAG, vec![9; 8])]);
        let x = d.windows(4).position(|w| w == b"XDAT").unwrap();
        d[x + 16..x + 20].copy_from_slice(&(x as u32).to_le_bytes()); // entry offset points at the directory
        assert!(matches!(xdat_entries(&d), Err(ContainerError::Invalid(_))));
    }
}
