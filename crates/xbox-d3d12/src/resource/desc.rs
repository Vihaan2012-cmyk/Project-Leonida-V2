//! `D3D12_RESOURCE_DESC` as the game fills it, and what it takes to hold one.

use crate::descriptor::format::{self, Format};

use super::layout::{self, ArrayLayout, CompactLayout, LayoutError};
use super::tiling::TileMode;

pub const KIB: u64 = 1024;
pub const BUFFER_ALIGNMENT: u64 = 64 * KIB;
pub const MSAA_ALIGNMENT: u64 = 4 * 1024 * KIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Buffer,
    Texture1D,
    Texture2D,
    Texture3D,
}

impl Dimension {
    pub fn from_raw(raw: u32) -> Option<Dimension> {
        Some(match raw {
            1 => Dimension::Buffer,
            2 => Dimension::Texture1D,
            3 => Dimension::Texture2D,
            4 => Dimension::Texture3D,
            _ => return None,
        })
    }
}

/// `D3D12_TEXTURE_LAYOUT` and the console's tiled value, which the game passes in the same field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Left to the implementation: a buffer is linear, a texture takes the console's tiling.
    Unknown,
    RowMajor,
    Undefined64K,
    Standard64K,
    /// `0x101`, the console's tiled layout: GFX10 `SW_64KB_D`.
    ConsoleTiled,
}

impl Layout {
    pub fn from_raw(raw: u32) -> Option<Layout> {
        Some(match raw {
            0 => Layout::Unknown,
            1 => Layout::RowMajor,
            2 => Layout::Undefined64K,
            3 => Layout::Standard64K,
            0x101 => Layout::ConsoleTiled,
            _ => return None,
        })
    }
}

pub const ALLOW_RENDER_TARGET: u32 = 0x1;
pub const ALLOW_DEPTH_STENCIL: u32 = 0x2;
pub const ALLOW_UNORDERED_ACCESS: u32 = 0x4;
pub const DENY_SHADER_RESOURCE: u32 = 0x8;
pub const ALLOW_SIMULTANEOUS_ACCESS: u32 = 0x20;
/// The bits the public enum defines; the console adds more above them.
pub const PC_FLAGS: u32 = 0x1FF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceDesc {
    pub dimension: Dimension,
    pub alignment: u64,
    pub width: u64,
    pub height: u32,
    pub depth_or_array_size: u32,
    /// 0 asks for the full chain.
    pub mip_levels: u32,
    pub format: u32,
    pub sample_count: u32,
    pub sample_quality: u32,
    pub layout: Layout,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DescError {
    #[error("DXGI format {0} is not a format of this layer")]
    Format(u32),
    #[error("a buffer has format UNKNOWN, one mip, one sample and no height")]
    Buffer,
    #[error("a {0}x{1} texture is empty")]
    Empty(u64, u32),
    #[error("{0}")]
    Layout(#[from] LayoutError),
    #[error("{0} is larger than the layer supports")]
    TooLarge(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationInfo {
    pub size: u64,
    pub alignment: u64,
}

impl ResourceDesc {
    pub fn buffer(size: u64) -> ResourceDesc {
        ResourceDesc {
            dimension: Dimension::Buffer,
            alignment: 0,
            width: size,
            height: 1,
            depth_or_array_size: 1,
            mip_levels: 1,
            format: format::dxgi::UNKNOWN,
            sample_count: 1,
            sample_quality: 0,
            layout: Layout::RowMajor,
            flags: 0,
        }
    }

    pub fn texture_2d(format: u32, width: u32, height: u32, mips: u32) -> ResourceDesc {
        ResourceDesc {
            dimension: Dimension::Texture2D,
            alignment: 0,
            width: u64::from(width),
            height,
            depth_or_array_size: 1,
            mip_levels: mips,
            format,
            sample_count: 1,
            sample_quality: 0,
            layout: Layout::ConsoleTiled,
            flags: 0,
        }
    }

    pub fn is_buffer(&self) -> bool {
        self.dimension == Dimension::Buffer
    }

    pub fn format_info(&self) -> Result<&'static Format, DescError> {
        format::format(self.format).ok_or(DescError::Format(self.format))
    }

    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    pub fn depth(&self) -> u32 {
        if self.dimension == Dimension::Texture3D {
            self.depth_or_array_size.max(1)
        } else {
            1
        }
    }

    pub fn array_size(&self) -> u32 {
        if self.dimension == Dimension::Texture3D {
            1
        } else {
            self.depth_or_array_size.max(1)
        }
    }

    /// The mips the resource has: the full chain when the description left it open.
    pub fn mips(&self) -> u32 {
        if self.is_buffer() {
            return 1;
        }
        let largest = (self.width as u32).max(self.height).max(self.depth());
        let full = 32 - largest.max(1).leading_zeros();
        if self.mip_levels == 0 {
            full
        } else {
            self.mip_levels.min(full)
        }
    }

    pub fn subresources(&self) -> u32 {
        self.mips() * self.array_size()
    }

    /// Checked once at creation, so the rest can rely on the fields.
    pub fn validate(&self) -> Result<(), DescError> {
        if self.is_buffer() {
            if self.format != format::dxgi::UNKNOWN
                || self.sample_count > 1
                || self.height > 1
                || self.depth_or_array_size > 1
            {
                return Err(DescError::Buffer);
            }
            return Ok(());
        }
        let info = self.format_info()?;
        if self.width == 0 || self.height == 0 {
            return Err(DescError::Empty(self.width, self.height));
        }
        if self.width > 16384 || self.height > 16384 {
            return Err(DescError::TooLarge("a texture"));
        }
        if info.kind == format::Kind::StencilPlane || info.kind == format::Kind::DepthPlane {
            return Err(DescError::Format(self.format));
        }
        Ok(())
    }

    /// The mode a console tiled texture is stored in.
    pub fn tile_mode(&self) -> TileMode {
        TileMode::Sw64kD
    }

    /// Whether the texture is kept in the console's tiled layout.
    pub fn is_tiled(&self) -> bool {
        !self.is_buffer()
            && self.dimension != Dimension::Texture3D
            && self.sample_count <= 1
            && matches!(
                self.layout,
                Layout::ConsoleTiled | Layout::Unknown | Layout::Undefined64K
            )
    }

    /// The compact mip chain of a tiled 2D texture (one slice).
    pub fn compact_layout(&self) -> Result<CompactLayout, DescError> {
        let info = self.format_info()?;
        Ok(layout::compact_layout(
            self.tile_mode(),
            self.width as u32,
            self.height,
            self.mips(),
            info.bytes,
            info.block[0],
        )?)
    }

    pub fn array_layout(&self) -> Result<ArrayLayout, DescError> {
        let info = self.format_info()?;
        Ok(layout::array_layout(
            self.tile_mode(),
            self.width as u32,
            self.height,
            self.array_size(),
            self.mips(),
            info.bytes,
            info.block[0],
        )?)
    }

    /// Bytes and alignment of the memory it needs.
    pub fn allocation_info(&self) -> Result<AllocationInfo, DescError> {
        self.validate()?;
        if self.is_buffer() {
            return Ok(AllocationInfo {
                size: self.width.next_multiple_of(BUFFER_ALIGNMENT),
                alignment: BUFFER_ALIGNMENT,
            });
        }
        let info = self.format_info()?;
        let alignment = if self.sample_count > 1 {
            MSAA_ALIGNMENT
        } else {
            self.alignment.max(BUFFER_ALIGNMENT)
        };
        let bytes = if self.is_tiled()
            && info.block[0] == info.block[1]
            && info.bytes.is_power_of_two()
            && info.bytes <= 16
        {
            if self.array_size() > 1 {
                self.array_layout()?.size
            } else {
                self.compact_layout()?.size
            }
        } else {
            let (_, total) = layout::row_major_footprints(
                self.width as u32,
                self.height,
                self.depth(),
                self.mips(),
                self.array_size(),
                info.block,
                info.bytes,
            );
            total
        };
        let samples = u64::from(self.sample_count.max(1));
        Ok(AllocationInfo {
            size: (bytes * samples).next_multiple_of(alignment),
            alignment,
        })
    }
}
