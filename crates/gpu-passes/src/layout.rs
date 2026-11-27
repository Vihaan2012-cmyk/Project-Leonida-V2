//! Where every mip of a console surface sits in tiled storage, and where its linear copy goes.
//!
//! Mips are stored smallest first behind one shared tail tile (the last few mips pack into it at fixed in-tile
//! positions); arrays are mip-major; cube faces are slice-major `SW_4KB_S`; volumes are a single 3D mip
//! (docs/textures.md, docs/textures_arrays.md).

use ash::vk;

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

impl SurfaceDesc {
    pub fn layout(&self) -> Result<Layout, LayoutError> {
        let eq = Equation::new(self.swizzle, self.format.elem_bytes)?;
        let mut subs = Vec::new();
        let mut linear = 0u64;
        let mut push = |mip: u32, slice: u32, texels: [u32; 3], mut job: Job, tiled: u64| {
            job.tiled_offset = tiled;
            job.linear_offset = linear;
            linear = (linear + job.linear_bytes()).next_multiple_of(16);
            subs.push(Subresource {
                mip,
                slice,
                texels,
                job,
            });
        };
        let block = self.format.block;
        let texels_of = |mip: u32| [(self.width >> mip).max(1), (self.height >> mip).max(1)];
        let job_of = |m: &Mip, depth: u32| Job {
            swizzle: self.swizzle,
            elem_bytes: self.format.elem_bytes,
            extent: [m.blocks[0], m.blocks[1], depth],
            origin: m.origin,
            tiles: [
                m.padded[0] / eq.tile_extent()[0],
                m.padded[1] / eq.tile_extent()[1],
            ],
            tiled_offset: 0,
            linear_offset: 0,
        };
        let tiled_size;
        match self.kind {
            Kind::Volume { depth } => {
                if self.mips != 1 {
                    return Err(LayoutError::VolumeMips(self.mips));
                }
                if self.swizzle != Swizzle::Sw4kbS3d || block != 1 || depth == 0 {
                    return Err(LayoutError::VolumeFormat);
                }
                let [tw, th, td] = eq.tile_extent();
                let tiles = [self.width.div_ceil(tw), self.height.div_ceil(th)];
                let job = Job {
                    swizzle: self.swizzle,
                    elem_bytes: self.format.elem_bytes,
                    extent: [self.width, self.height, depth],
                    origin: [0, 0],
                    tiles,
                    tiled_offset: 0,
                    linear_offset: 0,
                };
                tiled_size = job.tiled_bytes()?;
                debug_assert_eq!(
                    tiled_size,
                    tiles[0] as u64 * tiles[1] as u64 * depth.div_ceil(td) as u64 * 4096
                );
                push(0, 0, [self.width, self.height, depth], job, 0);
            }
            kind => {
                if self.swizzle == Swizzle::Sw4kbS3d {
                    return Err(LayoutError::VolumeFormat);
                }
                let chain = compact_chain(self, &eq)?;
                let tile = self.swizzle.tile_bytes() as u64;
                let (slices, base, face_size) = match kind {
                    Kind::Texture2d => (1, 0, 0),
                    Kind::Array { layers } => (layers, 0, 0),
                    Kind::Cube { data_offset } => (6, data_offset, chain.size),
                    Kind::Volume { .. } => unreachable!(),
                };
                let array = matches!(kind, Kind::Array { .. });
                let mip_base = if array && chain.first_tail < self.mips {
                    slices as u64 * tile
                } else {
                    0
                };
                // Standalone mips of an array follow the tail slab, each one a slab of `slices` regions.
                let mut standalone_offset = mip_base;
                let mut array_offsets = vec![0u64; chain.mips.len()];
                if array {
                    for (i, m) in chain
                        .mips
                        .iter()
                        .enumerate()
                        .take(chain.first_tail as usize)
                        .rev()
                    {
                        array_offsets[i] = standalone_offset;
                        standalone_offset += slices as u64
                            * m.padded[0] as u64
                            * m.padded[1] as u64
                            * self.format.elem_bytes as u64;
                    }
                }
                for (i, m) in chain.mips.iter().enumerate() {
                    let [tx, ty] = texels_of(i as u32);
                    for slice in 0..slices {
                        let tiled = if array {
                            let stride = if m.in_tail {
                                tile
                            } else {
                                m.padded[0] as u64
                                    * m.padded[1] as u64
                                    * self.format.elem_bytes as u64
                            };
                            array_offsets[i] + slice as u64 * stride
                        } else {
                            base + slice as u64 * face_size + m.offset
                        };
                        push(i as u32, slice, [tx, ty, 1], job_of(m, 1), tiled);
                    }
                }
                tiled_size = if array {
                    standalone_offset
                } else {
                    base + slices as u64
                        * if face_size == 0 {
                            chain.size
                        } else {
                            face_size
                        }
                };
            }
        }
        Ok(Layout {
            desc: *self,
            tiled_size,
            linear_size: linear,
            subresources: subs,
        })
    }
}

impl Layout {
    /// The buffer-to-image copies that move the linear output into a Vulkan image of the matching BCn or
    /// uncompressed format. Cube faces and array layers become array layers.
    pub fn buffer_image_copies(&self, aspect: vk::ImageAspectFlags) -> Vec<vk::BufferImageCopy> {
        let block = self.desc.format.block;
        self.subresources
            .iter()
            .map(|s| {
                vk::BufferImageCopy::default()
                    .buffer_offset(s.job.linear_offset)
                    .buffer_row_length(s.job.extent[0] * block)
                    .buffer_image_height(s.job.extent[1] * block)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(aspect)
                            .mip_level(s.mip)
                            .base_array_layer(s.slice)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: s.texels[0],
                        height: s.texels[1],
                        depth: s.texels[2],
                    })
            })
            .collect()
    }
}
