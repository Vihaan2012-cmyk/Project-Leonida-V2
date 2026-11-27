//! Texture dictionaries and pools: parsing, detiling, DDS export and previews.
//!
//! Decoded textures are kept as the game stores them: exact BCn blocks (or uncompressed texels)
//! in DDS order, every slice with its mips tightly packed. Textures that need the BCPack codec
//! come back as [`TextureResult::Partial`] or [`TextureResult::Placeholder`] with a typed
//! [`Unsupported`] reason; nothing is guessed.

pub mod bcn;
pub mod bcpack;
pub mod dds;
pub mod format;
pub mod layout;
pub mod tiling;

pub use bcpack::BcPackCodec;
pub use format::{FormatInfo, format_info};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Texture2D,
    Array2D,
    /// Six faces.
    Cube,
    Volume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureInfo {
    pub width: u32,
    pub height: u32,
    /// Array slices, 6 for cubemaps, volume depth, otherwise 1.
    pub depth: u32,
    pub mips: u32,
    /// DXGI format id.
    pub format: u32,
    pub shape: Shape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub info: TextureInfo,
    /// For each slice (array element or cube face), its mips from the largest down, each with
    /// tightly packed rows of blocks or pixels. Volumes hold one mip of Z-major slices.
    pub data: Vec<u8>,
}

