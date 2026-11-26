use crate::ContainerError;

/// One part of a DXBC container: its four-character code and payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part<'a> {
    pub fourcc: [u8; 4],
    pub data: &'a [u8],
}

/// The parts of a DXBC container, in table order. Layout: "DXBC", 16-byte hash, u32 version, u32 total size,
/// u32 part count at 28, then one u32 offset per part; each part is fourcc, u32 size, payload.
pub fn parts(dxbc: &[u8]) -> Result<Vec<Part<'_>>, ContainerError> {
    if dxbc.len() < 32 {
        return Err(ContainerError::Truncated("DXBC header"));
    }
    if &dxbc[..4] != b"DXBC" {
        return Err(ContainerError::Invalid("not a DXBC container".into()));
    }
    let count = u32_at(dxbc, 28) as usize;
    let table_end = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(32))
        .filter(|&end| end <= dxbc.len())
        .ok_or(ContainerError::Truncated("DXBC part table"))?;
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let off = u32_at(dxbc, 32 + 4 * k) as usize;
        if off < table_end || off.checked_add(8).is_none_or(|end| end > dxbc.len()) {
            return Err(ContainerError::Invalid(format!(
                "DXBC part {k} header offset {off} is outside the part data"
            )));
        }
        let size = u32_at(dxbc, off + 4) as usize;
        let start = off + 8;
        let end = start
            .checked_add(size)
            .filter(|&end| end <= dxbc.len())
            .ok_or(ContainerError::Truncated("DXBC part payload"))?;
        out.push(Part {
            fourcc: dxbc[off..off + 4].try_into().expect("4 bytes"),
            data: &dxbc[start..end],
        });
    }
    Ok(out)
}

/// The payload of the last part with this fourcc (later parts win).
pub fn find_part<'a>(parts: &[Part<'a>], fourcc: &[u8; 4]) -> Option<&'a [u8]> {
    parts
        .iter()
        .rev()
        .find(|part| &part.fourcc == fourcc)
        .map(|part| part.data)
}

/// Pipeline stage, from the DXIL program header's kind field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Pixel,
    Vertex,
    Geometry,
    Hull,
    Domain,
    Compute,
    Library,
    Mesh,
    Amplification,
    Other(u16),
}

impl Stage {
    pub fn from_dxil_kind(kind: u16) -> Stage {
        match kind {
            0 => Stage::Pixel,
            1 => Stage::Vertex,
            2 => Stage::Geometry,
            3 => Stage::Hull,
            4 => Stage::Domain,
            5 => Stage::Compute,
            6 => Stage::Library,
            13 => Stage::Mesh,
            14 => Stage::Amplification,
            other => Stage::Other(other),
        }
    }

    pub fn short_name(self) -> String {
        match self {
            Stage::Pixel => "ps".into(),
            Stage::Vertex => "vs".into(),
            Stage::Geometry => "gs".into(),
            Stage::Hull => "hs".into(),
            Stage::Domain => "ds".into(),
            Stage::Compute => "cs".into(),
            Stage::Library => "lib".into(),
            Stage::Mesh => "ms".into(),
            Stage::Amplification => "as".into(),
            Stage::Other(kind) => format!("kind{kind}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderModel {
    pub major: u8,
    pub minor: u8,
}

/// Stage and shader model from the DXIL part's first four bytes (u16 version, u16 kind), or None when the
/// container has no DXIL part.
pub fn shader_model(parts: &[Part<'_>]) -> Result<Option<(Stage, ShaderModel)>, ContainerError> {
    let Some(dxil) = find_part(parts, b"DXIL") else {
        return Ok(None);
    };
    if dxil.len() < 4 {
        return Err(ContainerError::Truncated("DXIL version and stage"));
    }
    let version = u16_at(dxil, 0);
    let kind = u16_at(dxil, 2);
    let model = ShaderModel {
        major: (version >> 4) as u8,
        minor: (version & 0xF) as u8,
    };
    Ok(Some((Stage::from_dxil_kind(kind), model)))
}

/// Little-endian u16 at `off`. Panics if out of range: callers check bounds first.
pub(crate) fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().expect("2 bytes"))
}

/// Little-endian u32 at `off`. Panics if out of range: callers check bounds first.
pub(crate) fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().expect("4 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::dxbc;

    #[test]
    fn reads_parts_in_order() {
        let c = dxbc(&[
            (b"DXIL", &b"\x66\x00\x01\x00rest"[..]),
            (b"PRIV", &b"private"[..]),
        ]);
        let p = parts(&c).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(&p[0].fourcc, b"DXIL");
        assert_eq!(p[1].data, b"private");
        assert_eq!(find_part(&p, b"PRIV"), Some(&b"private"[..]));
        assert_eq!(find_part(&p, b"STAT"), None);
    }

    #[test]
    fn later_part_with_same_fourcc_wins() {
        let c = dxbc(&[(b"PRIV", &b"one"[..]), (b"PRIV", &b"two"[..])]);
        assert_eq!(find_part(&parts(&c).unwrap(), b"PRIV"), Some(&b"two"[..]));
    }

    #[test]
    fn stage_and_shader_model_from_dxil_header() {
        // version 0x66 = shader model 6.6; kind 1 = vertex
        let c = dxbc(&[(b"DXIL", &b"\x66\x00\x01\x00"[..])]);
        let model = shader_model(&parts(&c).unwrap()).unwrap();
        assert_eq!(
            model,
            Some((Stage::Vertex, ShaderModel { major: 6, minor: 6 }))
        );
        assert_eq!(Stage::from_dxil_kind(13).short_name(), "ms");
        assert_eq!(Stage::from_dxil_kind(99).short_name(), "kind99");
    }

    #[test]
    fn no_dxil_part_means_no_stage() {
        let c = dxbc(&[(b"PRIV", &b"x"[..])]);
        assert_eq!(shader_model(&parts(&c).unwrap()).unwrap(), None);
    }

    #[test]
    fn rejects_damaged_containers() {
        assert_eq!(
            parts(b"DXBC"),
            Err(ContainerError::Truncated("DXBC header"))
        );
        let mut c = dxbc(&[(b"PRIV", &b"private"[..])]);
        assert!(matches!(
            parts(&c[..c.len() - 1]),
            Err(ContainerError::Truncated(_))
        ));
        c[0] = b'X';
        assert!(matches!(parts(&c), Err(ContainerError::Invalid(_))));
        let mut bad_offset = dxbc(&[(b"PRIV", &b"private"[..])]);
        bad_offset[32..36].copy_from_slice(&4u32.to_le_bytes()); // points into the header
        assert!(matches!(
            parts(&bad_offset),
            Err(ContainerError::Invalid(_))
        ));
        let truncated_dxil = dxbc(&[(b"DXIL", &b"\x66\x00"[..])]);
        assert_eq!(
            shader_model(&parts(&truncated_dxil).unwrap()),
            Err(ContainerError::Truncated("DXIL version and stage"))
        );
    }
}
