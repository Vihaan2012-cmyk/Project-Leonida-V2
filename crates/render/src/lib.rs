#![allow(unsafe_code)]
//! The Vulkan renderer core: long-lived objects the console D3D12 layer and the scene harness build on.

mod error;
mod format;
mod device;
mod sampler;
mod buffer;
mod image;
mod pipeline;
mod command;
mod staging;
mod bindless;
mod swapchain;

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
pub use image::{Image, ImageDesc, ImageKind, ImageView, ViewDesc};
pub use pipeline::{ComputePipeline, DescriptorSet, PipelineLayout, SetLayout};
pub use command::{CommandBuffer, CommandStream, ImageBarrier};
pub use staging::{StagingRing, StagingSlice};
pub use bindless::{BindlessSet, BindlessSizes, BindlessView, FlushReport};
pub use swapchain::{
    ColorSpace, Frame, HdrSupport, PresentMode, PresentOutcome, Swapchain, SwapchainDesc,
    WindowTarget, choose_extent, hdr_support, pick_format,
};

/// SPIR-V compiled from the WGSL files in `shaders/` by `naga` at build time.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}
