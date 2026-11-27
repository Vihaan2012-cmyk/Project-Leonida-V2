//! Where every mip of a console surface sits in tiled storage, and where its linear copy goes.
//!
//! Mips are stored smallest first behind one shared tail tile (the last few mips pack into it at fixed in-tile
//! positions); arrays are mip-major; cube faces are slice-major `SW_4KB_S`; volumes are a single 3D mip
//! (docs/textures.md, docs/textures_arrays.md).

use crate::LayoutError;
use crate::swizzle::{Job, Swizzle};

/// The element of a format: a pixel (`block` 1) or a 4x4 block of a BCn format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub elem_bytes: u32,
    pub block: u32,
}

impl Format {
    pub fn pixel(elem_bytes: u32) -> Format {
        Format {
            elem_bytes,
            block: 1,
        }
    }

    pub fn block(elem_bytes: u32) -> Format {
        Format {
            elem_bytes,
            block: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Texture2d,
    /// Mip-major `SW_64KB_D` array.
    Array {
        layers: u32,
    },
    /// Six faces, face `f` at `data_offset + f * face_size`. `data_offset` is the size of the earlier streaming
    /// snapshots in front of the full-resolution one (0 for descriptor modes 18 and for a lone snapshot).
    Cube {
        data_offset: u64,
    },
    Volume {
        depth: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceDesc {
    pub kind: Kind,
    pub swizzle: Swizzle,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
}

impl SurfaceDesc {
    /// From the fields of a TTPF descriptor: `mode` (`b >> 23`), `dimension` (1 2D or array, 2 cube, 3 volume)
    /// and `depth` (slices, faces or volume depth).
    pub fn from_ttpf(
        mode: u32,
        dimension: u32,
        depth: u32,
        width: u32,
        height: u32,
        mips: u32,
        format: Format,
    ) -> Result<SurfaceDesc, LayoutError> {
        let kind = match (dimension, depth) {
            (1, 0 | 1) => Kind::Texture2d,
            (1, layers) => Kind::Array { layers },
            (2, 6) => Kind::Cube { data_offset: 0 },
            (3, depth) => Kind::Volume { depth },
            _ => return Err(LayoutError::Dimension(dimension, depth)),
        };
        Ok(SurfaceDesc {
            kind,
            swizzle: Swizzle::from_descriptor_mode(mode, dimension == 3)?,
            format,
            width,
            height,
            mips,
        })
    }
}

/// One mip of one slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subresource {
    pub mip: u32,
    /// Array layer or cube face (0 for a plain 2D texture and for volumes).
    pub slice: u32,
    /// Size in texels (depth for a volume).
    pub texels: [u32; 3],
    pub job: Job,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub desc: SurfaceDesc,
    /// Bytes of tiled storage the surface occupies from offset 0.
    pub tiled_size: u64,
    /// Bytes of linear storage, mips in order, slices within a mip, each start 16-byte aligned.
    pub linear_size: u64,
    pub subresources: Vec<Subresource>,
}
