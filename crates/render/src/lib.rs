#![allow(unsafe_code)]
//! The Vulkan renderer core: long-lived objects the console D3D12 layer and the scene harness build on.

mod error;
mod format;

pub use error::{RenderError, Result};
pub use format::{Format, level_extent, mip_count};

/// SPIR-V compiled from the WGSL files in `shaders/` by `naga` at build time.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}
