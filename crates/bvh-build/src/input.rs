use crate::half;
use crate::{BuildError, Result};

/// D3D12 geometry flags kept in a leaf's low byte.
pub const GEOMETRY_OPAQUE: u32 = 1;

const NAN_VERTEX: [f32; 3] = [f32::NAN; 3];

/// A triangle's corners after the geometry's transform; a NaN corner makes it inactive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    pub vertices: [[f32; 3]; 3],
}

/// The triangles of one D3D12 triangle geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub triangles: Vec<Triangle>,
    pub flags: u32,
}

/// A TLAS instance as D3D12 describes it: a row-major 3x4 object-to-world transform, a 24-bit ID and 8-bit
/// mask, a 24-bit hit group contribution with 8 flag bits, and the address of the BLAS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    pub transform: [f32; 12],
    pub id: u32,
    pub mask: u32,
    pub contribution: u32,
    pub flags: u32,
    pub blas: u64,
}

/// What a TLAS build needs to know about the BLAS an instance names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlasInfo {
    pub root: u32,
    pub bounds: [f32; 6],
}

pub const DXGI_R32G32B32A32_FLOAT: u32 = 2;
pub const DXGI_R32G32B32_FLOAT: u32 = 6;
pub const DXGI_R16G16B16A16_FLOAT: u32 = 10;
pub const DXGI_R16G16B16A16_SNORM: u32 = 13;
pub const DXGI_R32G32_FLOAT: u32 = 16;
pub const DXGI_R16G16_FLOAT: u32 = 34;
pub const DXGI_R16G16_SNORM: u32 = 37;
pub const DXGI_R32_UINT: u32 = 42;
pub const DXGI_R16_UINT: u32 = 57;

/// Bytes a vertex of a DXGI format occupies, for the formats D3D12 allows for triangle vertices.
pub fn vertex_bytes(format: u32) -> Option<usize> {
    Some(match format {
        DXGI_R32G32B32_FLOAT => 12,
        DXGI_R32G32B32A32_FLOAT => 16,
        DXGI_R32G32_FLOAT | DXGI_R16G16B16A16_FLOAT | DXGI_R16G16B16A16_SNORM => 8,
        DXGI_R16G16_FLOAT | DXGI_R16G16_SNORM => 4,
        _ => return None,
    })
}

pub fn index_bytes(format: u32) -> Option<usize> {
    match format {
        0 => Some(0),
        DXGI_R16_UINT => Some(2),
        DXGI_R32_UINT => Some(4),
        _ => None,
    }
}

/// One geometry's vertex and index data, as the caller's memory holds it.
pub struct TriangleSource<'a> {
    pub vertex_format: u32,
    pub vertex_stride: usize,
    pub vertex_count: usize,
    pub vertices: &'a [u8],
    pub index_format: u32,
    pub index_count: usize,
    pub indices: &'a [u8],
    pub transform: Option<[f32; 12]>,
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

fn snorm(bits: u16) -> f32 {
    (f32::from(bits as i16) / 32767.0).max(-1.0)
}

fn vertex(format: u32, bytes: &[u8]) -> [f32; 3] {
    let float = |i: usize| f32::from_bits(le32(bytes, 4 * i));
    let half = |i: usize| half::to_f32(le16(bytes, 2 * i));
    match format {
        DXGI_R32G32B32_FLOAT | DXGI_R32G32B32A32_FLOAT => [float(0), float(1), float(2)],
        DXGI_R32G32_FLOAT => [float(0), float(1), 0.0],
        DXGI_R16G16_FLOAT => [half(0), half(1), 0.0],
        DXGI_R16G16B16A16_FLOAT => [half(0), half(1), half(2)],
        DXGI_R16G16_SNORM => [snorm(le16(bytes, 0)), snorm(le16(bytes, 2)), 0.0],
        _ => [
            snorm(le16(bytes, 0)),
            snorm(le16(bytes, 2)),
            snorm(le16(bytes, 4)),
        ],
    }
}

pub fn transform_point(m: &[f32; 12], p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|row| {
        m[4 * row] * p[0] + m[4 * row + 1] * p[1] + m[4 * row + 2] * p[2] + m[4 * row + 3]
    })
}

/// The triangles of a geometry. An index past the vertices, or the all-ones index, makes its triangle inactive.
pub fn read_triangles(source: &TriangleSource<'_>) -> Result<Vec<Triangle>> {
    let size = vertex_bytes(source.vertex_format).ok_or(BuildError::Unsupported(
        "a vertex format other than the DXR ones",
    ))?;
    let index_size = index_bytes(source.index_format).ok_or(BuildError::Unsupported(
        "an index format other than 16 or 32-bit",
    ))?;
    let stride = source.vertex_stride;
    if source.vertex_count > 0 && source.vertices.len() < stride * (source.vertex_count - 1) + size
    {
        return Err(BuildError::Malformed("vertex data shorter than its count"));
    }
    let count = if index_size == 0 {
        source.vertex_count
    } else {
        source.index_count
    };
    if index_size != 0 && source.indices.len() < count * index_size {
        return Err(BuildError::Malformed("index data shorter than its count"));
    }
    let fetch = |slot: usize| -> [f32; 3] {
        let index = match index_size {
            0 => Some(slot),
            2 => {
                let value = le16(source.indices, 2 * slot);
                (value != u16::MAX).then_some(usize::from(value))
            }
            _ => {
                let value = le32(source.indices, 4 * slot);
                (value != u32::MAX).then_some(value as usize)
            }
        };
        match index {
            Some(i) if i < source.vertex_count => {
                let at = stride * i;
                let v = vertex(source.vertex_format, &source.vertices[at..at + size]);
                match &source.transform {
                    Some(m) => transform_point(m, v),
                    None => v,
                }
            }
            _ => NAN_VERTEX,
        }
    };
    Ok((0..count / 3)
        .map(|t| Triangle {
            vertices: [fetch(3 * t), fetch(3 * t + 1), fetch(3 * t + 2)],
        })
        .collect())
}

/// How many triangles a geometry holds: its index count over three, or its vertex count over three.
pub fn triangle_count(index_format: u32, index_count: usize, vertex_count: usize) -> usize {
    if index_format == 0 {
        vertex_count / 3
    } else {
        index_count / 3
    }
}
