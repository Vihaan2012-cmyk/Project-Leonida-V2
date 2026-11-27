//! Where every mip of a console surface sits in tiled storage, and where its linear copy goes.
//!
//! Mips are stored smallest first behind one shared tail tile (the last few mips pack into it at fixed in-tile
//! positions); arrays are mip-major; cube faces are slice-major `SW_4KB_S`; volumes are a single 3D mip
//! (docs/textures.md, docs/textures_arrays.md).

use crate::LayoutError;
use crate::swizzle::{Equation, Job, Swizzle};

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

struct Mip {
    blocks: [u32; 2],
    padded: [u32; 2],
    offset: u64,
    origin: [u32; 2],
    in_tail: bool,
}

struct Chain {
    size: u64,
    first_tail: u32,
    mips: Vec<Mip>,
}

fn tail_entry_origin(entry: u32, slot: [u32; 2]) -> [u32; 2] {
    let offset = if entry > 6 {
        16u32 << entry
    } else {
        entry << 8
    };
    let (mut tx, mut ty) = (0, 0);
    for bit in 0..6 {
        tx |= ((offset >> (9 + 2 * bit)) & 1) << bit;
        ty |= ((offset >> (8 + 2 * bit)) & 1) << bit;
    }
    [tx * slot[0], ty * slot[1]]
}

fn compact_chain(desc: &SurfaceDesc, eq: &Equation) -> Result<Chain, LayoutError> {
    let Format { elem_bytes, block } = desc.format;
    if !matches!(block, 1 | 4) {
        return Err(LayoutError::Block(block));
    }
    if desc.width == 0 || desc.height == 0 {
        return Err(LayoutError::Empty);
    }
    let levels = 32 - desc.width.max(desc.height).leading_zeros();
    if desc.mips < 1 || desc.mips > levels {
        return Err(LayoutError::MipCount(desc.mips));
    }
    let tile = desc.swizzle.tile_bytes() as u64;
    let max_tail = if tile == 4096 { 8 } else { 12 };
    let [bw, bh, _] = eq.tile_extent();
    let slot = match elem_bytes {
        1 => [16, 16],
        2 => [16, 8],
        4 => [8, 8],
        8 => [8, 4],
        _ => [4, 4],
    };
    let ew = desc.width.div_ceil(block);
    let eh = desc.height.div_ceil(block);
    let size_of = |m: &Mip| m.padded[0] as u64 * m.padded[1] as u64 * elem_bytes as u64;

    let mut mips = Vec::new();
    let mut first_tail = desc.mips;
    for i in 0..desc.mips {
        let blocks = [ew.div_ceil(1 << i).max(1), eh.div_ceil(1 << i).max(1)];
        if desc.mips > 1 && blocks[0] <= bw / 2 && blocks[1] <= bh && desc.mips - i <= max_tail {
            first_tail = i;
            break;
        }
        mips.push(Mip {
            blocks,
            padded: [blocks[0].div_ceil(bw) * bw, blocks[1].div_ceil(bh) * bh],
            offset: 0,
            origin: [0, 0],
            in_tail: false,
        });
    }
    let mut offset = if first_tail < desc.mips { tile } else { 0 };
    for m in mips.iter_mut().rev() {
        m.offset = offset;
        offset += size_of(m);
    }
    for i in first_tail..desc.mips {
        let entry = max_tail - 1 - (i - first_tail);
        mips.push(Mip {
            blocks: [ew.div_ceil(1 << i).max(1), eh.div_ceil(1 << i).max(1)],
            padded: [bw, bh],
            offset: 0,
            origin: tail_entry_origin(entry, slot),
            in_tail: true,
        });
    }
    Ok(Chain {
        size: offset,
        first_tail,
        mips,
    })
}

