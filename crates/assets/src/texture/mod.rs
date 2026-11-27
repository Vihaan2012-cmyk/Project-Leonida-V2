//! Texture dictionaries and pools: parsing, detiling, DDS export and previews.
//!
//! Decoded textures are kept as the game stores them: exact BCn blocks (or uncompressed texels)
//! in DDS order, every slice with its mips tightly packed. Textures that need the BCPack codec
//! come back as [`TextureResult::Partial`] or [`TextureResult::Placeholder`] with a typed
//! [`Unsupported`] reason; nothing is guessed.

pub mod bcn;
pub mod bcpack;
pub mod format;
pub mod layout;
pub mod tiling;

pub use bcpack::BcPackCodec;
pub use format::{FormatInfo, format_info};

