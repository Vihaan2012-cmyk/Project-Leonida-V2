//! Texture dictionaries and pools: parsing, detiling, DDS export and previews.
//!
//! Decoded textures are kept as the game stores them: exact BCn blocks (or uncompressed texels)
//! in DDS order, every slice with its mips tightly packed. Textures that need the BCPack codec
//! come back as [`TextureResult::Partial`] or [`TextureResult::Placeholder`] with a typed
//! [`Unsupported`] reason; nothing is guessed.

pub mod bcn;
pub mod bcpack;
pub mod dds;
mod extract;
pub mod format;
pub mod layout;
pub mod pool;
pub mod tiling;

pub use bcpack::BcPackCodec;
pub use extract::TextureError;
pub use format::{FormatInfo, format_info};
pub use pool::{ChunkCache, Pool, PoolError, TextureDescriptor};

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

/// Why a texture could not be decoded exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// A mip lives in a BCPack chunk and the codec's tables are not available.
    BcPack(BcPackCodec),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureResult {
    Complete(Texture),
    /// Only the mips from `first_mip` on were decodable; `texture.info` describes that chain.
    Partial {
        texture: Texture,
        first_mip: u32,
        missing: Unsupported,
    },
    /// Nothing usable: `texture` is a small stand-in of the same shape, `original` the real
    /// description.
    Placeholder {
        texture: Texture,
        original: TextureInfo,
        reason: Unsupported,
    },
}

impl TextureResult {
    pub fn texture(&self) -> &Texture {
        match self {
            TextureResult::Complete(t) => t,
            TextureResult::Partial { texture, .. } | TextureResult::Placeholder { texture, .. } => {
                texture
            }
        }
    }

    /// True when the texture holds exactly what the game stores.
    pub fn is_complete(&self) -> bool {
        matches!(self, TextureResult::Complete(_))
    }
}

/// A magenta and dark checker, 4x4 RGBA8, with the slice count of `info` (volumes become 2D).
pub fn placeholder(info: &TextureInfo) -> Texture {
    let depth = match info.shape {
        Shape::Texture2D | Shape::Volume => 1,
        Shape::Array2D | Shape::Cube => info.depth.max(1),
    };
    let shape = if info.shape == Shape::Volume {
        Shape::Texture2D
    } else {
        info.shape
    };
    let mut slice = Vec::with_capacity(64);
    for y in 0..4 {
        for x in 0..4 {
            let lit = (x / 2 + y / 2) % 2 == 0;
            slice.extend_from_slice(if lit {
                &[255, 0, 255, 255]
            } else {
                &[32, 0, 32, 255]
            });
        }
    }
    Texture {
        info: TextureInfo {
            width: 4,
            height: 4,
            depth,
            mips: 1,
            format: 28,
            shape,
        },
        data: slice.repeat(depth as usize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_matches_the_shape_and_is_a_valid_dds_payload() {
        let cube = TextureInfo {
            width: 256,
            height: 256,
            depth: 6,
            mips: 9,
            format: 98,
            shape: Shape::Cube,
        };
        let p = placeholder(&cube);
        assert_eq!(p.info.shape, Shape::Cube);
        assert_eq!(p.info.depth, 6);
        assert_eq!(p.data.len() as u64, dds::payload_size(&p.info).unwrap());
        let vol = TextureInfo {
            shape: Shape::Volume,
            depth: 8,
            format: 10,
            mips: 1,
            ..cube
        };
        let p = placeholder(&vol);
        assert_eq!(p.info.shape, Shape::Texture2D);
        assert_eq!(p.data.len(), 64);
    }
}
