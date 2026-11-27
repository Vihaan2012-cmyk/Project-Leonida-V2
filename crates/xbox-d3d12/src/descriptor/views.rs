//! Shader-resource, unordered-access, constant-buffer and render-target views as the descriptor words a shader
//! loads from a heap slot (`Create*View` writes these).
//!
//! A slot is 32 bytes. A texture view fills all of it with a T#; a buffer view puts a V# in the first 16 bytes.
//! A render-target or depth-stencil view is also written as the T# of the view it names: no shader reads it, but
//! the slot then survives a raw descriptor copy and finds its image the way an SRV does.

use super::format::{DEFAULT_MAPPING, Format, dxgi, format};
use super::gfx10::{Buffer, Image, ImageType, OutOfBounds, encode_buffer, encode_image};
use crate::resource::desc::{Dimension, ResourceDesc};

pub type Slot = [u32; 8];

/// `D3D12` uses `UINT_MAX` for "all of them" in mip and slice counts.
pub const ALL: u32 = u32::MAX;

/// What a view needs to know of its resource: the GPU address (buffer address, or the key a texture's
/// descriptors carry) and the description.
#[derive(Debug, Clone, Copy)]
pub struct ViewResource {
    pub address: u64,
    pub desc: ResourceDesc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    #[error("DXGI format {0} cannot be viewed this way")]
    Format(u32),
    #[error("a {view} view needs a {need} resource")]
    Dimension {
        view: &'static str,
        need: &'static str,
    },
    #[error("format {0} has no code a buffer descriptor can carry")]
    BufferFormat(u32),
    #[error("mips {first}+{count} are outside a resource of {have} mips")]
    Mips { first: u32, count: u32, have: u32 },
    #[error("slices {first}+{count} are outside a resource of {have} slices")]
    Slices { first: u32, count: u32, have: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SrvKind {
    Buffer {
        first_element: u64,
        num_elements: u32,
        stride: u32,
        raw: bool,
    },
    Texture1D {
        most_detailed_mip: u32,
        mip_levels: u32,
        min_lod_clamp: f32,
    },
    Texture1DArray {
        most_detailed_mip: u32,
        mip_levels: u32,
        first_slice: u32,
        array_size: u32,
        min_lod_clamp: f32,
    },
    Texture2D {
        most_detailed_mip: u32,
        mip_levels: u32,
        plane_slice: u32,
        min_lod_clamp: f32,
    },
    Texture2DArray {
        most_detailed_mip: u32,
        mip_levels: u32,
        first_slice: u32,
        array_size: u32,
        plane_slice: u32,
        min_lod_clamp: f32,
    },
    Texture2DMs,
    Texture2DMsArray {
        first_slice: u32,
        array_size: u32,
    },
    Texture3D {
        most_detailed_mip: u32,
        mip_levels: u32,
        min_lod_clamp: f32,
    },
    TextureCube {
        most_detailed_mip: u32,
        mip_levels: u32,
        min_lod_clamp: f32,
    },
    TextureCubeArray {
        most_detailed_mip: u32,
        mip_levels: u32,
        first_cube: u32,
        num_cubes: u32,
        min_lod_clamp: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SrvDesc {
    /// `DXGI_FORMAT_UNKNOWN` takes the resource's own format.
    pub format: u32,
    pub mapping: [u32; 4],
    pub kind: SrvKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UavKind {
    Buffer {
        first_element: u64,
        num_elements: u32,
        stride: u32,
        counter_offset: u64,
        raw: bool,
    },
    Texture1D {
        mip_slice: u32,
    },
    Texture1DArray {
        mip_slice: u32,
        first_slice: u32,
        array_size: u32,
    },
    Texture2D {
        mip_slice: u32,
        plane_slice: u32,
    },
    Texture2DArray {
        mip_slice: u32,
        first_slice: u32,
        array_size: u32,
        plane_slice: u32,
    },
    Texture3D {
        mip_slice: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UavDesc {
    pub format: u32,
    pub kind: UavKind,
}

/// A render-target or depth-stencil view: one mip, and for arrays a slice range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetDesc {
    pub format: u32,
    pub mip_slice: u32,
    pub first_slice: u32,
    pub slice_count: u32,
    pub multisampled: bool,
    pub array: bool,
}

fn mip_range(desc: &ResourceDesc, first: u32, count: u32) -> Result<(u32, u32), ViewError> {
    let have = desc.mips();
    let count = if count == ALL {
        have.saturating_sub(first)
    } else {
        count
    };
    if first >= have || count == 0 || first + count > have {
        return Err(ViewError::Mips { first, count, have });
    }
    Ok((first, count))
}

fn slice_range(desc: &ResourceDesc, first: u32, count: u32) -> Result<(u32, u32), ViewError> {
    let have = desc.array_size();
    let count = if count == ALL {
        have.saturating_sub(first)
    } else {
        count
    };
    if first >= have || count == 0 || first + count > have {
        return Err(ViewError::Slices { first, count, have });
    }
    Ok((first, count))
}

fn format_of(res: &ViewResource, requested: u32) -> Result<&'static Format, ViewError> {
    let code = if requested == dxgi::UNKNOWN {
        res.desc.format
    } else {
        requested
    };
    format(code).ok_or(ViewError::Format(code))
}

struct Range {
    ty: ImageType,
    mips: (u32, u32),
    layers: (u32, u32),
    min_lod: f32,
}

fn image_slot(res: &ViewResource, info: &Format, mapping: [u32; 4], r: Range) -> Slot {
    let desc = &res.desc;
    let multisampled = r.ty.multisampled();
    encode_image(&Image {
        address: res.address,
        format: info.gfx10,
        ty: r.ty,
        width: desc.width as u32,
        height: if desc.dimension == Dimension::Texture1D {
            1
        } else {
            desc.height
        },
        depth: desc.depth(),
        first_layer: r.layers.0,
        layer_count: r.layers.1,
        first_mip: r.mips.0,
        mip_count: r.mips.1,
        resource_mips: desc.mips(),
        samples: if multisampled { desc.sample_count } else { 1 },
        swizzle: info.view_swizzle(mapping),
        min_lod: r.min_lod,
    })
}

fn need(view: &'static str, want: &'static str, ok: bool) -> Result<(), ViewError> {
    if ok {
        Ok(())
    } else {
        Err(ViewError::Dimension { view, need: want })
    }
}

fn v_slot(buffer: Buffer) -> Slot {
    let mut slot = [0; 8];
    slot[..4].copy_from_slice(&encode_buffer(&buffer));
    slot
}

/// `CreateShaderResourceView`.
pub fn srv(res: &ViewResource, d: &SrvDesc) -> Result<Slot, ViewError> {
    let dim = res.desc.dimension;
    let is = |ds: &[Dimension]| ds.contains(&dim);
    let info_of = || format_of(res, d.format);
    match d.kind {
        SrvKind::Buffer {
            first_element,
            num_elements,
            stride,
            raw,
        } => {
            need("buffer", "buffer", dim == Dimension::Buffer)?;
            Ok(v_slot(buffer_view(
                res,
                d.format,
                d.mapping,
                (first_element, num_elements, stride),
                raw,
            )?))
        }
        SrvKind::Texture1D {
            most_detailed_mip,
            mip_levels,
            min_lod_clamp,
        } => {
            need("1D texture", "1D texture", is(&[Dimension::Texture1D]))?;
            let mips = mip_range(&res.desc, most_detailed_mip, mip_levels)?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::D1,
                    mips,
                    layers: (0, 1),
                    min_lod: min_lod_clamp,
                },
            ))
        }
        SrvKind::Texture1DArray {
            most_detailed_mip,
            mip_levels,
            first_slice,
            array_size,
            min_lod_clamp,
        } => {
            need("1D array", "1D texture", is(&[Dimension::Texture1D]))?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::D1Array,
                    mips: mip_range(&res.desc, most_detailed_mip, mip_levels)?,
                    layers: slice_range(&res.desc, first_slice, array_size)?,
                    min_lod: min_lod_clamp,
                },
            ))
        }
        SrvKind::Texture2D {
            most_detailed_mip,
            mip_levels,
            plane_slice: _,
            min_lod_clamp,
        } => {
            need("2D texture", "2D texture", is(&[Dimension::Texture2D]))?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::D2,
                    mips: mip_range(&res.desc, most_detailed_mip, mip_levels)?,
                    layers: (0, 1),
                    min_lod: min_lod_clamp,
                },
            ))
        }
        SrvKind::Texture2DArray {
            most_detailed_mip,
            mip_levels,
            first_slice,
            array_size,
            plane_slice: _,
            min_lod_clamp,
        } => {
            need("2D array", "2D texture", is(&[Dimension::Texture2D]))?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::D2Array,
                    mips: mip_range(&res.desc, most_detailed_mip, mip_levels)?,
                    layers: slice_range(&res.desc, first_slice, array_size)?,
                    min_lod: min_lod_clamp,
                },
            ))
        }
        SrvKind::Texture2DMs => {
            need("multisampled", "2D texture", is(&[Dimension::Texture2D]))?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::D2Msaa,
                    mips: (0, 1),
                    layers: (0, 1),
                    min_lod: 0.0,
                },
            ))
        }
        SrvKind::Texture2DMsArray {
            first_slice,
            array_size,
        } => {
            need(
                "multisampled array",
                "2D texture",
                is(&[Dimension::Texture2D]),
            )?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::D2MsaaArray,
                    mips: (0, 1),
                    layers: slice_range(&res.desc, first_slice, array_size)?,
                    min_lod: 0.0,
                },
            ))
        }
        SrvKind::Texture3D {
            most_detailed_mip,
            mip_levels,
            min_lod_clamp,
        } => {
            need("3D texture", "3D texture", is(&[Dimension::Texture3D]))?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::D3,
                    mips: mip_range(&res.desc, most_detailed_mip, mip_levels)?,
                    layers: (0, 1),
                    min_lod: min_lod_clamp,
                },
            ))
        }
        SrvKind::TextureCube {
            most_detailed_mip,
            mip_levels,
            min_lod_clamp,
        } => {
            need("cube", "2D array of six", is(&[Dimension::Texture2D]))?;
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::Cube,
                    mips: mip_range(&res.desc, most_detailed_mip, mip_levels)?,
                    layers: slice_range(&res.desc, 0, 6)?,
                    min_lod: min_lod_clamp,
                },
            ))
        }
        SrvKind::TextureCubeArray {
            most_detailed_mip,
            mip_levels,
            first_cube,
            num_cubes,
            min_lod_clamp,
        } => {
            need("cube array", "2D array of six", is(&[Dimension::Texture2D]))?;
            let count = if num_cubes == ALL { ALL } else { num_cubes * 6 };
            Ok(image_slot(
                res,
                info_of()?,
                d.mapping,
                Range {
                    ty: ImageType::Cube,
                    mips: mip_range(&res.desc, most_detailed_mip, mip_levels)?,
                    layers: slice_range(&res.desc, first_cube * 6, count)?,
                    min_lod: min_lod_clamp,
                },
            ))
        }
    }
}

/// The V# of a buffer view: `first`, `count` and `stride` are the description's first element, element count and
/// structure stride. Raw views have stride 0 and count bytes; structured views (no format) the structure stride;
/// typed views the format's element size, and their own bounds mode and channel order.
fn buffer_view(
    res: &ViewResource,
    requested: u32,
    mapping: [u32; 4],
    (first, count, stride): (u64, u32, u32),
    raw: bool,
) -> Result<Buffer, ViewError> {
    if raw {
        return Ok(Buffer {
            address: res.address + first * 4,
            stride: 0,
            num_records: count.saturating_mul(4),
            out_of_bounds: OutOfBounds::Raw,
            ..Buffer::default()
        });
    }
    if requested == dxgi::UNKNOWN {
        return Ok(Buffer {
            address: res.address + first * u64::from(stride),
            stride,
            num_records: count,
            out_of_bounds: OutOfBounds::StructuredWithOffset,
            ..Buffer::default()
        });
    }
    let info = format(requested).ok_or(ViewError::Format(requested))?;
    if info.gfx10 >= 128 || info.compressed() {
        return Err(ViewError::BufferFormat(requested));
    }
    Ok(Buffer {
        address: res.address + first * u64::from(info.bytes),
        stride: info.bytes,
        num_records: count,
        format: info.gfx10,
        swizzle: info.view_swizzle(mapping),
        out_of_bounds: OutOfBounds::Structured,
    })
}

/// `CreateUnorderedAccessView`. A counter takes the second V# of the slot (raw, four bytes at the counter's
/// address): where the console keeps it is a hypothesis, not a measurement.
pub fn uav(
    res: &ViewResource,
    d: &UavDesc,
    counter: Option<&ViewResource>,
) -> Result<Slot, ViewError> {
    let dim = res.desc.dimension;
    let info_of = || format_of(res, d.format);
    let texture = |ty, mips: (u32, u32), layers: (u32, u32)| -> Result<Slot, ViewError> {
        Ok(image_slot(
            res,
            info_of()?,
            DEFAULT_MAPPING,
            Range {
                ty,
                mips,
                layers,
                min_lod: 0.0,
            },
        ))
    };
    match d.kind {
        UavKind::Buffer {
            first_element,
            num_elements,
            stride,
            counter_offset,
            raw,
        } => {
            need("buffer", "buffer", dim == Dimension::Buffer)?;
            let mut slot = v_slot(buffer_view(
                res,
                d.format,
                DEFAULT_MAPPING,
                (first_element, num_elements, stride),
                raw,
            )?);
            if let Some(c) = counter {
                let tail = encode_buffer(&Buffer {
                    address: c.address + counter_offset,
                    stride: 0,
                    num_records: 4,
                    out_of_bounds: OutOfBounds::Raw,
                    ..Buffer::default()
                });
                slot[4..].copy_from_slice(&tail);
            }
            Ok(slot)
        }
        UavKind::Texture1D { mip_slice } => {
            need("1D texture", "1D texture", dim == Dimension::Texture1D)?;
            texture(ImageType::D1, mip_range(&res.desc, mip_slice, 1)?, (0, 1))
        }
        UavKind::Texture1DArray {
            mip_slice,
            first_slice,
            array_size,
        } => {
            need("1D array", "1D texture", dim == Dimension::Texture1D)?;
            texture(
                ImageType::D1Array,
                mip_range(&res.desc, mip_slice, 1)?,
                slice_range(&res.desc, first_slice, array_size)?,
            )
        }
        UavKind::Texture2D { mip_slice, .. } => {
            need("2D texture", "2D texture", dim == Dimension::Texture2D)?;
            texture(ImageType::D2, mip_range(&res.desc, mip_slice, 1)?, (0, 1))
        }
        UavKind::Texture2DArray {
            mip_slice,
            first_slice,
            array_size,
            ..
        } => {
            need("2D array", "2D texture", dim == Dimension::Texture2D)?;
            texture(
                ImageType::D2Array,
                mip_range(&res.desc, mip_slice, 1)?,
                slice_range(&res.desc, first_slice, array_size)?,
            )
        }
        UavKind::Texture3D { mip_slice } => {
            need("3D texture", "3D texture", dim == Dimension::Texture3D)?;
            texture(ImageType::D3, mip_range(&res.desc, mip_slice, 1)?, (0, 1))
        }
    }
}

/// `CreateConstantBufferView`: a raw V# over `size` bytes at the buffer address.
pub fn cbv(address: u64, size: u32) -> Slot {
    v_slot(Buffer {
        address,
        stride: 0,
        num_records: size,
        out_of_bounds: OutOfBounds::Raw,
        ..Buffer::default()
    })
}

fn target(res: &ViewResource, d: &TargetDesc, depth: bool) -> Result<Slot, ViewError> {
    let dim = res.desc.dimension;
    let info = format_of(res, d.format)?;
    if depth != info.has_depth() && depth {
        return Err(ViewError::Format(d.format));
    }
    let ty = match (dim, d.array, d.multisampled) {
        (Dimension::Texture1D, false, _) => ImageType::D1,
        (Dimension::Texture1D, true, _) => ImageType::D1Array,
        (Dimension::Texture3D, ..) => ImageType::D3,
        (Dimension::Texture2D, false, false) => ImageType::D2,
        (Dimension::Texture2D, true, false) => ImageType::D2Array,
        (Dimension::Texture2D, false, true) => ImageType::D2Msaa,
        (Dimension::Texture2D, true, true) => ImageType::D2MsaaArray,
        (Dimension::Buffer, ..) => {
            return Err(ViewError::Dimension {
                view: "target",
                need: "texture",
            });
        }
    };
    let layers = if d.array {
        slice_range(&res.desc, d.first_slice, d.slice_count)?
    } else {
        (0, 1)
    };
    let mips = if d.multisampled {
        (0, 1)
    } else {
        mip_range(&res.desc, d.mip_slice, 1)?
    };
    Ok(image_slot(
        res,
        info,
        DEFAULT_MAPPING,
        Range {
            ty,
            mips,
            layers,
            min_lod: 0.0,
        },
    ))
}

/// `CreateRenderTargetView`.
pub fn rtv(res: &ViewResource, d: &TargetDesc) -> Result<Slot, ViewError> {
    target(res, d, false)
}

/// `CreateDepthStencilView`.
pub fn dsv(res: &ViewResource, d: &TargetDesc) -> Result<Slot, ViewError> {
    target(res, d, true)
}

/// A view of nothing: all zero, which no shader resolves to an image.
pub const NULL: Slot = [0; 8];
