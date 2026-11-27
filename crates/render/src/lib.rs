#![allow(unsafe_code)]
//! The Vulkan renderer core: long-lived objects the console D3D12 layer and the scene harness build on.
//!
//! - [`Device`] owns the instance, device, queues (each with a timeline semaphore), the `gpu-allocator` memory
//!   allocator and the pipeline cache. It reports what the GPU can do as a [`gpu::DeviceInfo`].
//! - [`Buffer`] and [`Image`] own their memory; [`Buffer::from_host_memory`] aliases host memory through
//!   `VK_EXT_external_memory_host`. [`Format`] lists every texture and target format the translator's image arrays
//!   and the console use. [`Sampler`] wraps `VkSampler`.
//! - [`BindlessSet`] is the translator's descriptor set (`shader_translate::images`): the sampled, storage and
//!   atomic image arrays, the sampler arrays, the texture and sampler lookup tables, and the fallback at index 0.
//! - [`CommandStream`] records [`CommandBuffer`]s that keep the resources they name alive until the GPU is done,
//!   and submits them to a [`Queue`], giving a [`Submission`] to wait on or chain from another queue.
//! - [`StagingRing`] is the upload and readback ring.
//! - [`Swapchain`] presents to a window: recreation on resize, selectable [`PresentMode`], an HDR query. Each
//!   acquired [`Frame`] carries the semaphores that order drawing and presenting. [`RenderWindow`] pairs it with a
//!   winit window.
//!
//! Headless use needs no window: `Device::new(&DeviceOptions::default())`.

mod bindless;
mod buffer;
mod command;
mod device;
mod error;
mod format;
mod image;
mod pipeline;
mod sampler;
mod staging;
mod swapchain;
mod window;

pub use ash::vk;
pub use gpu::DeviceInfo;
pub use winit;

pub use bindless::{BindlessSet, BindlessSizes, BindlessView, FlushReport};
pub use buffer::{Buffer, BufferDesc, MemoryLocation};
pub use command::{CommandBuffer, CommandStream, ImageBarrier};
pub use device::{
    DescriptorLimits, Device, DeviceOptions, Features, Queue, SignalSemaphore, Submission,
    WaitSemaphore, validation_errors,
};
pub use error::{RenderError, Result};
pub use format::{Format, level_extent, mip_count};
pub use image::{Image, ImageDesc, ImageKind, ImageView, ViewDesc};
pub use pipeline::{ComputePipeline, DescriptorSet, PipelineLayout, SetLayout};
pub use sampler::{Sampler, SamplerDesc};
pub use staging::{StagingRing, StagingSlice};
pub use swapchain::{
    ColorSpace, Frame, HdrSupport, PresentMode, PresentOutcome, Swapchain, SwapchainDesc,
    WindowTarget, choose_extent, hdr_support, pick_format,
};
pub use window::{RenderWindow, WindowDesc};

/// SPIR-V compiled from the WGSL files in `shaders/` by `naga` at build time.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}
