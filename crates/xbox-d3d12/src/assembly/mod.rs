//! The one real device: the parts assembled over one Vulkan device.

pub mod bindless;
pub mod bridge;
pub mod gpu;
pub mod indirect;
pub mod rawdesc;
pub mod timeline;
pub mod views;

#[cfg(test)]
mod bridge_tests;

#[cfg(test)]
mod rawdesc_tests;
