//! Replaying recorded command lists on Vulkan: draws, dispatches, copies, barriers.

pub mod barrier;
mod command;
mod context;
mod replay;

use ash::vk;

pub use command::*;
pub use context::{Features, VkContext};
pub use replay::{Arena, ReplayEnv, ReplayReport, Replayer, SpillAllocator, index_type, run};

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("no usable Vulkan device: {0}")]
    NoDevice(String),
    #[error("Vulkan: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("root arguments: {0}")]
    Root(#[from] crate::root::RootError),
    #[error("a {0} draw or dispatch without a {0} pipeline set")]
    NoPipeline(&'static str),
    #[error("an indexed draw without an index buffer")]
    NoIndexBuffer,
    #[error("a root signature spills but the replay has nowhere to put the spill array")]
    NoSpillSpace,
}
