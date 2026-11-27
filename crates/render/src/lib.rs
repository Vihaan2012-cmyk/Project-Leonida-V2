#![allow(unsafe_code)]
//! The Vulkan renderer core: long-lived objects the console D3D12 layer and the scene harness build on.

mod error;
mod format;
mod device;
mod sampler;
mod buffer;

pub use error::{RenderError, Result};
pub use format::{Format, level_extent, mip_count};
pub use device::{
    DescriptorLimits, Device, DeviceOptions, Features, Queue, SignalSemaphore, Submission,
    WaitSemaphore, validation_errors,
};
pub use ash::vk;
pub use gpu::DeviceInfo;
pub use sampler::{Sampler, SamplerDesc};
pub use buffer::{Buffer, BufferDesc, MemoryLocation};

/// SPIR-V compiled from the WGSL files in `shaders/` by `naga` at build time.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}
