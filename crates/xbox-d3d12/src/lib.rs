//! The console D3D12 interface on top of the Vulkan renderer.
#![allow(unsafe_code)]

pub mod assembly;
pub mod com;
pub mod compile_queue;
pub mod descriptor;
pub mod device;
pub mod display;
pub mod execute;
pub mod heap;
pub mod list;
pub mod memory;
pub mod pacing;
pub mod pipeline;
pub mod present;
pub mod queue;
pub mod resource;
pub mod root;
pub mod stream;
mod thunks;

pub use device::{Integration, create_device};

/// `D3D12XboxCreateDevice`'s body with nothing plugged in beyond the defaults of `Integration`.
///
/// # Safety
/// `params` is null or 24 readable words; `riid` and `out` as COM passes them.
pub unsafe fn create_default_device(
    params: *const u32,
    riid: *const u8,
    out: *mut *mut std::ffi::c_void,
) -> u32 {
    // SAFETY: as the caller promises.
    unsafe { create_device(params, riid, out, Integration::default()) }
}
