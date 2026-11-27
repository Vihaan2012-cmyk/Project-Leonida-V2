//! GPU passes for console textures: detiling (and tiling) of every measured Gfx10 swizzle mode, and the BCPack
//! BC1 block rebuild. WGSL kernels are compiled to SPIR-V by `naga` in `build.rs`; the wrappers record them on
//! an `ash` command buffer given raw buffers. `swizzle` and `layout` are the CPU reference the kernels repeat.
#![allow(unsafe_code)]

pub mod swizzle;

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("element size {0} is not 1, 2, 4, 8 or 16 bytes")]
    ElementSize(u32),
    #[error("descriptor mode {0} has no measured tile equation")]
    UnmeasuredTileMode(u32),
    #[error("dimension {0} with depth {1} is not a 2D, array, cube or volume texture")]
    Dimension(u32, u32),
    #[error("block size {0} is not 1 or 4")]
    Block(u32),
    #[error("empty surface")]
    Empty,
    #[error("mip count {0} does not fit the surface")]
    MipCount(u32),
    #[error("only single-mip volumes are measured, got {0} mips")]
    VolumeMips(u32),
    #[error("volumes need the 3D swizzle, an uncompressed format and a depth")]
    VolumeFormat,
    #[error("the job's mip does not fit its tile grid or the 32-bit offsets of the kernels")]
    JobOutOfRange,
}

#[derive(Debug, thiserror::Error)]
pub enum PassError {
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] ash::vk::Result),
    #[error(transparent)]
    Layout(#[from] LayoutError),
    #[error(
        "the descriptor pool is exhausted; call `Passes::reset` once earlier work has finished"
    )]
    PoolExhausted,
    #[error("the dispatch is larger than the device's group limit")]
    TooLarge,
    #[error("BCPack lanes differ in length")]
    LaneLengths,
    #[error("block {block} refers to history entry {entry}, which is not recovered")]
    UnknownHistory { block: usize, entry: usize },
}
