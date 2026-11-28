//! Descriptors in the console format (T#, V#, S# words) and their registration in the translated shaders' lookup
//! tables.
//!
//! The device calls `Descriptors::create_*` for each `Create*View` and `CreateSampler`: the words come back (and
//! `write_slot` puts them in the heap slot at the CPU handle), and an image or sampler descriptor is registered
//! in the lookup tables the shaders search. Descriptors the game builds itself in heap memory are registered by
//! `scan_textures` and `scan_samplers` before the commands that use them run.

pub mod fasthash;
pub mod format;
pub mod gfx10;
pub mod registry;
pub mod sampler;
pub mod views;
pub mod vulkan;

use gfx10::{ImageWords, SamplerWords};
use registry::{Registration, Registry, RegistryError};
use sampler::{BorderTable, SamplerDesc};
use views::{Slot, SrvDesc, TargetDesc, UavDesc, ViewError, ViewResource};

use crate::resource::Resource;

#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    #[error("{0}")]
    View(#[from] ViewError),
    #[error("{0}")]
    Registry(#[from] RegistryError),
}

impl From<&Resource> for ViewResource {
    fn from(r: &Resource) -> ViewResource {
        ViewResource {
            address: r.address,
            desc: r.desc,
        }
    }
}

/// Bytes a descriptor slot takes in a CBV/SRV/UAV heap and in a sampler heap (a sampler fills the first 16).
/// The console's own increments are not measured; shaders take the stride from their own code, which fixes it
/// through the static offsets of their loads at 32 bytes for a T# and 16 for an S#.
pub const SLOT_BYTES: u64 = 32;
pub const SAMPLER_SLOT_BYTES: u64 = 32;

#[derive(Default)]
pub struct Descriptors {
    pub registry: Registry,
    pub borders: BorderTable,
}

impl Descriptors {
    pub fn new() -> Descriptors {
        Descriptors::default()
    }

    /// Registers the slot when it is an image descriptor; a buffer descriptor has nothing to register.
    fn register(&mut self, slot: &Slot) -> Result<Option<Registration>, DescriptorError> {
        if gfx10::is_image(slot) {
            Ok(Some(self.registry.register_texture(slot)?))
        } else {
            Ok(None)
        }
    }

    /// `CreateShaderResourceView`. No resource is a null view: all zero, registered nowhere.
    pub fn create_srv(
        &mut self,
        resource: Option<&ViewResource>,
        desc: &SrvDesc,
    ) -> Result<(Slot, Option<Registration>), DescriptorError> {
        let Some(resource) = resource else {
            return Ok((views::NULL, None));
        };
        let slot = views::srv(resource, desc)?;
        Ok((slot, self.register(&slot)?))
    }

    /// `CreateUnorderedAccessView`, with the counter resource when the description names one.
    pub fn create_uav(
        &mut self,
        resource: Option<&ViewResource>,
        counter: Option<&ViewResource>,
        desc: &UavDesc,
    ) -> Result<(Slot, Option<Registration>), DescriptorError> {
        let Some(resource) = resource else {
            return Ok((views::NULL, None));
        };
        let slot = views::uav(resource, desc, counter)?;
        Ok((slot, self.register(&slot)?))
    }

    /// `CreateConstantBufferView`.
    pub fn create_cbv(&self, address: u64, size: u32) -> Slot {
        views::cbv(address, size)
    }

    /// `CreateRenderTargetView`: the slot holds the descriptor of the view it names, and it is registered so the
    /// device can find the image from the slot.
    pub fn create_rtv(
        &mut self,
        resource: &ViewResource,
        desc: &TargetDesc,
    ) -> Result<(Slot, Option<Registration>), DescriptorError> {
        let slot = views::rtv(resource, desc)?;
        Ok((slot, self.register(&slot)?))
    }

    pub fn create_dsv(
        &mut self,
        resource: &ViewResource,
        desc: &TargetDesc,
    ) -> Result<(Slot, Option<Registration>), DescriptorError> {
        let slot = views::dsv(resource, desc)?;
        Ok((slot, self.register(&slot)?))
    }

    /// `CreateSampler`.
    pub fn create_sampler(&mut self, desc: &SamplerDesc) -> (SamplerWords, Registration) {
        let words = gfx10::encode_sampler(&sampler::sampler(desc, &mut self.borders));
        let registration = self.registry.register_sampler(&words);
        (words, registration)
    }

    /// `CreateSamplerX`: the console's own sampler description.
    pub fn create_sampler_console(
        &mut self,
        desc: &sampler::ConsoleSampler,
    ) -> (SamplerWords, Registration) {
        let words = gfx10::encode_sampler(&sampler::from_console(desc, &mut self.borders));
        let registration = self.registry.register_sampler(&words);
        (words, registration)
    }

    /// Registers the image descriptors the game wrote into heap memory itself (`stride` dwords a slot).
    pub fn scan_textures(&mut self, heap: &[u32], stride: usize) -> Vec<(usize, Registration)> {
        self.registry.scan_textures(heap, stride)
    }

    pub fn scan_samplers(&mut self, heap: &[u32], stride: usize) -> Vec<(usize, Registration)> {
        self.registry.scan_samplers(heap, stride)
    }

    pub fn scan_textures_with(
        &mut self,
        heap: &[u32],
        stride: usize,
        each: impl FnMut(usize, Registration),
    ) {
        self.registry.scan_textures_with(heap, stride, each);
    }

    pub fn scan_samplers_with(
        &mut self,
        heap: &[u32],
        stride: usize,
        each: impl FnMut(usize, Registration),
    ) {
        self.registry.scan_samplers_with(heap, stride, each);
    }

    /// The lookup tables to upload to set 1 bindings 0 (textures) and 1 (samplers).
    pub fn tables(&mut self) -> Result<&registry::Tables, DescriptorError> {
        Ok(self.registry.tables()?)
    }

    /// The slot word a shader gets for these image words (0 when not registered).
    pub fn texture_slot(&self, words: &ImageWords) -> u32 {
        self.registry.texture_slot(words)
    }
}

/// Writes a descriptor into the heap slot at a CPU descriptor handle.
///
/// # Safety
/// `address` must be the address of a descriptor slot, with `SLOT_BYTES` bytes of writable memory that nothing
/// else is using, as a CPU descriptor handle of a heap this layer made is.
pub unsafe fn write_slot(address: u64, slot: &Slot) {
    // SAFETY: as the caller promises.
    unsafe {
        std::ptr::copy_nonoverlapping(slot.as_ptr(), address as usize as *mut u32, slot.len())
    };
}

/// Writes a sampler descriptor into the sampler heap slot at a CPU descriptor handle, zeroing the rest of it.
///
/// # Safety
/// As `write_slot`, with `SAMPLER_SLOT_BYTES` bytes.
pub unsafe fn write_sampler_slot(address: u64, words: &SamplerWords) {
    let mut slot = [0u32; 8];
    slot[..4].copy_from_slice(words);
    // SAFETY: as the caller promises.
    unsafe { write_slot(address, &slot) };
}

/// Reads the descriptor at a CPU descriptor handle.
///
/// # Safety
/// `address` must be a descriptor slot of `SLOT_BYTES` readable bytes.
pub unsafe fn read_slot(address: u64) -> Slot {
    let mut slot = [0; 8];
    // SAFETY: as the caller promises.
    unsafe { std::ptr::copy_nonoverlapping(address as usize as *const u32, slot.as_mut_ptr(), 8) };
    slot
}

#[cfg(test)]
mod descriptors_tests;
#[cfg(test)]
mod gfx10_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod sampler_tests;
#[cfg(test)]
mod views_tests;
#[cfg(test)]
mod vulkan_tests;
