//! The one real device: the parts of this crate assembled over one Vulkan device.
//!
//! Every part was written against an interface; this module puts real implementations behind them.
//!
//! - `gpu`: one Vulkan 1.3 instance and device with what every part needs (`LEONIDA_GPU` picks the GPU,
//!   `LEONIDA_VALIDATION` turns the validation layer on), and from it the handles of replay and pipelines
//!   (`VkContext`), presentation (`VulkanContext`) and memory (`Handles`).
//! - `state`: `Assembly`, which holds the address space and its resources, the descriptor registry, the bindless
//!   image set, the pipeline environment and the presenter, and builds the `Integration` of `create_device`.
//! - `executor`, `bridge`: closed command lists become replay commands and run on the queue.
//! - `timeline`, `arena`: fences as timeline semaphores; upload, code and spill memory in the GPU's address space.
//! - `devmethods`, `objects`, `rawdesc`, `resapi`: the device methods (resources, descriptors, root signatures,
//!   pipelines) and the COM objects they make.
//! - `frames`: the frame events, `PresentX`, `SuspendX` and `ResumeX`, with the frame limit policy.
//! - `hooks`: where input events, the content filter and the Vulkan device are handed over.
//!
//! The `XMem*` shims report the game's memory to the address space through `memory::hooks`, which `init` installs
//! (call it before the game allocates). Descriptor heaps are placed by `heap::ConsolePlacement`; the title's own
//! allocation hook comes in through `set_heap_allocator`.

pub mod access;
pub mod arena;
pub mod bindless;
pub mod bridge;
mod devmethods;
mod executor;
pub mod frames;
pub mod gpu;
pub mod hooks;
pub mod indirect;
pub mod objects;
pub mod rawdesc;
mod resapi;
pub mod state;
pub mod timeline;
pub mod views;

use std::ffi::c_void;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use crate::com::{self, Kind};
use crate::device::{create_device, device_core};
use crate::execute::VkContext;
use crate::heap::console::{HeapAllocator, NoAllocator};
use crate::memory::AddressSpace;
use crate::pacing::FrameLimit;
use crate::present::VulkanContext;

pub use hooks::{InputEvent, MouseButton, set_content_filter, set_input_sink, set_motion_sink};
pub use resapi::Placement;
pub use state::{
    Assembly, AssemblyConfig, AssemblyError, Output, WindowOptionsConfig, current, register,
    set_current,
};

pub use crate::pacing::FrameLimit as Limit;

const DXGI_ERROR_UNSUPPORTED: u32 = 0x887A_0004;

static LIMIT: Mutex<FrameLimit> = Mutex::new(FrameLimit::Console);
static ALLOCATOR: RwLock<Option<Arc<dyn HeapAllocator>>> = RwLock::new(None);

/// The frame rate policy. Call it before the device exists; a device that exists applies it at once.
pub fn set_frame_limit(limit: FrameLimit) {
    *LIMIT.lock().unwrap_or_else(PoisonError::into_inner) = limit;
    if let Some(asm) = current() {
        asm.pacer.set_limit(limit);
    }
}

pub fn frame_limit() -> FrameLimit {
    *LIMIT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The title's allocation hook as the heap step takes it (`shims::xmem::title_heap_allocator()`): descriptor heaps
/// are placed where the hook says. Without one they are placed in low memory.
pub fn set_heap_allocator(allocator: Option<Arc<dyn HeapAllocator>>) {
    *ALLOCATOR.write().unwrap_or_else(PoisonError::into_inner) = allocator;
}

/// Builds the assembly if it does not exist yet (and installs the memory hooks, so call it before the game
/// allocates), and makes it the current one. The same assembly is returned after the first call.
pub fn init(config: AssemblyConfig) -> Result<Arc<Assembly>, AssemblyError> {
    if let Some(asm) = current() {
        return Ok(asm);
    }
    let asm = Assembly::build(&config)?;
    set_current(Some(asm.clone()));
    Ok(asm)
}

/// `D3D12XboxCreateDevice`'s body on the assembled device: the current assembly, or one built with a window.
///
/// # Safety
/// `params` is null or 24 readable words; `riid` and `out` as COM passes them.
pub unsafe fn create_assembled_device(
    params: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let asm = match init(AssemblyConfig::windowed()) {
        Ok(asm) => asm,
        Err(e) => {
            com::note(format!("the device could not be assembled: {e}"));
            return DXGI_ERROR_UNSUPPORTED;
        }
    };
    // SAFETY: as the caller promises.
    unsafe { create_assembled_device_on(&asm, params, riid, out) }
}

/// As `create_assembled_device`, on a given assembly, which becomes the current one.
///
/// # Safety
/// As `create_assembled_device`.
pub unsafe fn create_assembled_device_on(
    asm: &Arc<Assembly>,
    params: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    set_current(Some(asm.clone()));
    let allocator = ALLOCATOR
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| Arc::new(NoAllocator));
    let integration = asm.integration_with(allocator);
    // SAFETY: as the caller promises.
    let result = unsafe { create_device(params, riid, out, integration) };
    // SAFETY: on success `*out` is a device object of this layer.
    unsafe {
        if result == 0
            && !out.is_null()
            && !(*out).is_null()
            && com::kind_of(*out) == Some(Kind::Device)
        {
            register(device_core(*out), asm);
        }
    }
    result
}

/// The Vulkan device an upscaler can share, once the assembly exists.
pub fn vulkan_context() -> Option<VulkanContext> {
    current().map(|a| a.gpu.vulkan.clone())
}

/// The replay and pipeline view of the same device.
pub fn vk_context() -> Option<Arc<VkContext>> {
    current().map(|a| a.gpu.ctx.clone())
}

/// The address space of the game's GPU-visible memory, shared with the `XMem*` shims through `memory::hooks`.
pub fn address_space() -> Option<Arc<Mutex<AddressSpace>>> {
    current().map(|a| a.resources().shared_space().clone())
}

/// `D3D12SerializeVersionedRootSignature(desc, blob, error)`: the serialized form in an `ID3DBlob`.
///
/// # Safety
/// `desc` is a readable `D3D12_VERSIONED_ROOT_SIGNATURE_DESC` with readable arrays; `blob` is writable and
/// `error` is writable or null.
pub unsafe fn serialize_versioned_root_signature(
    desc: *const u8,
    blob: *mut *mut c_void,
    error: *mut *mut c_void,
) -> u32 {
    if desc.is_null() || blob.is_null() {
        return com::E_POINTER;
    }
    // SAFETY: as the caller promises.
    unsafe {
        match rawdesc::read_versioned_root_signature(desc) {
            Some(root) => {
                *blob = objects::new_blob(root.serialize());
                if !error.is_null() {
                    *error = std::ptr::null_mut();
                }
                com::S_OK
            }
            None => {
                if !error.is_null() {
                    *error = objects::new_blob(
                        b"unknown root signature version or parameter\0".to_vec(),
                    );
                }
                com::E_INVALIDARG
            }
        }
    }
}

#[cfg(test)]
mod bridge_tests;
#[cfg(test)]
mod rawdesc_tests;
