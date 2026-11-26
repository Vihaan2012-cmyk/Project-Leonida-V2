use crate::dxbc::{Part, find_part};
use crate::error::ContainerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureElement {
    pub stream: u32,
    pub semantic: String,
    pub semantic_index: u32,
    pub system_value: u32,
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
    pub rw_mask: u8,
    pub min_precision: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Interface {
    pub inputs: Vec<SignatureElement>,
    pub outputs: Vec<SignatureElement>,
    pub pipeline: Option<PipelineInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelineInfo {
    pub stage: Option<u8>,
    pub wave_lanes: (u32, u32),
    pub threads: Option<[u32; 3]>,
    pub mesh: Option<MeshInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshInfo {
    pub shared_bytes: u32,
    pub payload_bytes: u32,
    pub max_vertices: u16,
    pub max_primitives: u16,
    pub topology: u8,
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, ContainerError> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
        .ok_or(ContainerError::Truncated("signature or pipeline part"))
}

fn u16_at(data: &[u8], at: usize) -> Result<u16, ContainerError> {
    data.get(at..at + 2)
        .map(|b| u16::from_le_bytes(b.try_into().expect("2 bytes")))
        .ok_or(ContainerError::Truncated("signature or pipeline part"))
}

fn name_at(data: &[u8], at: usize) -> Result<String, ContainerError> {
    let rest = data
        .get(at..)
        .ok_or(ContainerError::Truncated("signature or pipeline part"))?;
    let end = rest
        .iter()
        .position(|b| *b == 0)
        .ok_or(ContainerError::Truncated("signature or pipeline part"))?;
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Elements of an ISG1/OSG1/PSG1 part (32 bytes each) or an older ISGN/OSGN part (24 bytes each).
pub fn signature(part: &[u8], extended: bool) -> Result<Vec<SignatureElement>, ContainerError> {
    let count = u32_at(part, 0)? as usize;
    let first = u32_at(part, 4)? as usize;
    let size = if extended { 32 } else { 24 };
    let mut elements = Vec::with_capacity(count.min(256));
    for index in 0..count {
        let at = first + index * size;
        let base = if extended { at + 4 } else { at };
        let fields = |i: usize| u32_at(part, base + 4 * i);
        let mask_at = base + 20;
        let masks = part
            .get(mask_at..mask_at + 2)
            .ok_or(ContainerError::Truncated("signature or pipeline part"))?;
        elements.push(SignatureElement {
            stream: if extended { u32_at(part, at)? } else { 0 },
            semantic: name_at(part, fields(0)? as usize)?,
            semantic_index: fields(1)?,
            system_value: fields(2)?,
            component_type: fields(3)?,
            register: fields(4)?,
            mask: masks[0],
            rw_mask: masks[1],
            min_precision: if extended { u32_at(part, at + 28)? } else { 0 },
        });
    }
    Ok(elements)
}

/// The fixed-size runtime information at the start of a PSV0 part.
pub fn pipeline_info(part: &[u8], dxil_kind: Option<u16>) -> Result<PipelineInfo, ContainerError> {
    let size = u32_at(part, 0)? as usize;
    let info = part
        .get(4..4 + size)
        .ok_or(ContainerError::Truncated("signature or pipeline part"))?;
    let mut result = PipelineInfo {
        wave_lanes: (u32_at(info, 16)?, u32_at(info, 20)?),
        ..PipelineInfo::default()
    };
    if size >= 36 {
        result.stage = Some(info[24]);
    }
    let kind = result.stage.map(u16::from).or(dxil_kind);
    if kind == Some(13) {
        result.mesh = Some(MeshInfo {
            shared_bytes: u32_at(info, 0)?,
            payload_bytes: u32_at(info, 8)?,
            max_vertices: u16_at(info, 12)?,
            max_primitives: u16_at(info, 14)?,
            topology: if size >= 36 { info[27] } else { 0 },
        });
    }
    if size >= 48 {
        result.threads = Some([u32_at(info, 36)?, u32_at(info, 40)?, u32_at(info, 44)?]);
    }
    Ok(result)
}

/// The shader's interface from its DXBC parts; missing parts give empty lists.
pub fn interface(parts: &[Part<'_>], dxil_kind: Option<u16>) -> Result<Interface, ContainerError> {
    let read =
        |extended: &[u8; 4], plain: &[u8; 4]| -> Result<Vec<SignatureElement>, ContainerError> {
            if let Some(part) = find_part(parts, extended) {
                return signature(part, true);
            }
            match find_part(parts, plain) {
                Some(part) => signature(part, false),
                None => Ok(Vec::new()),
            }
        };
    Ok(Interface {
        inputs: read(b"ISG1", b"ISGN")?,
        outputs: read(b"OSG1", b"OSGN")?,
        pipeline: find_part(parts, b"PSV0")
            .map(|part| pipeline_info(part, dxil_kind))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn element_part(elements: &[(&str, u32, u32, u8)]) -> Vec<u8> {
        let mut part = Vec::new();
        part.extend((elements.len() as u32).to_le_bytes());
        part.extend(8u32.to_le_bytes());
        let names_at = 8 + 32 * elements.len();
        let mut names = Vec::new();
        for (name, index, register, mask) in elements {
            let offset = names_at + names.len();
            for word in [0, offset as u32, *index, 0, 3, *register] {
                part.extend(word.to_le_bytes());
            }
            part.extend([*mask, *mask, 0, 0]);
            part.extend(0u32.to_le_bytes());
            names.extend(name.as_bytes());
            names.push(0);
        }
        part.extend(names);
        part
    }

    #[test]
    fn extended_signatures_read_names_registers_and_masks() {
        let part = element_part(&[("POSITION", 0, 0, 0x7), ("TEXCOORD", 3, 2, 0x3)]);
        let elements = signature(&part, true).unwrap();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].semantic, "POSITION");
        assert_eq!(
            (elements[1].semantic.as_str(), elements[1].semantic_index),
            ("TEXCOORD", 3)
        );
        assert_eq!(
            (
                elements[1].register,
                elements[1].mask,
                elements[1].component_type
            ),
            (2, 0x3, 3)
        );
    }

    #[test]
    fn mesh_pipeline_info_reads_outputs_and_threads() {
        let mut info = Vec::new();
        for word in [4096u32, 0, 16] {
            info.extend(word.to_le_bytes());
        }
        info.extend(128u16.to_le_bytes());
        info.extend(64u16.to_le_bytes());
        info.extend(64u32.to_le_bytes());
        info.extend(64u32.to_le_bytes());
        info.extend([13, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]);
        for word in [128u32, 1, 1] {
            info.extend(word.to_le_bytes());
        }
        let mut part = (info.len() as u32).to_le_bytes().to_vec();
        part.extend(info);
        let pipeline = pipeline_info(&part, None).unwrap();
        assert_eq!(pipeline.stage, Some(13));
        assert_eq!(pipeline.threads, Some([128, 1, 1]));
        let mesh = pipeline.mesh.unwrap();
        assert_eq!(
            (mesh.max_vertices, mesh.max_primitives, mesh.topology),
            (128, 64, 2)
        );
        assert_eq!((mesh.shared_bytes, mesh.payload_bytes), (4096, 16));
    }

    #[test]
    fn truncated_parts_are_errors() {
        assert!(signature(&[1, 0, 0, 0, 8, 0, 0, 0], true).is_err());
        assert!(pipeline_info(&[48, 0, 0, 0], None).is_err());
    }
}
