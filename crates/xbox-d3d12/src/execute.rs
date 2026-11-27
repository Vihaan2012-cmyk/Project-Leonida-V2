//! Replaying recorded command lists on Vulkan: draws, dispatches, copies, barriers.

mod context;

use ash::vk;

pub use context::{Features, VkContext};

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("no usable Vulkan device: {0}")]
    NoDevice(String),
    #[error("Vulkan: {0}")]
    Vulkan(#[from] vk::Result),
}
