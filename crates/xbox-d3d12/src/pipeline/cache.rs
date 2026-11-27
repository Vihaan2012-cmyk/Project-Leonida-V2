//! The persistent pipeline cache: a `VkPipelineCache` kept in a file between runs, and the blob
//! `ID3D12PipelineState::GetCachedBlob` hands out for a description.

use std::path::{Path, PathBuf};

use ash::vk;
use sha2::{Digest, Sha256};

use crate::execute::VkContext;

use super::PipelineError;

/// The header Vulkan puts at the start of cache data, version one: length, version, vendor, device, UUID.
const HEADER: usize = 32;

/// Whether `data` is a cache written by this device and driver.
pub fn header_matches(data: &[u8], properties: &vk::PhysicalDeviceProperties) -> bool {
    let word = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().expect("4 bytes"));
    data.len() >= HEADER
        && word(0) as usize >= HEADER
        && word(4) == vk::PipelineCacheHeaderVersion::ONE.as_raw() as u32
        && word(8) == properties.vendor_id
        && word(12) == properties.device_id
        && data[16..32] == properties.pipeline_cache_uuid
}

pub struct PipelineCache {
    device: ash::Device,
    handle: vk::PipelineCache,
    path: Option<PathBuf>,
    properties: vk::PhysicalDeviceProperties,
    /// Whether the file's contents were used.
    pub loaded: bool,
}

impl PipelineCache {
    /// Opens the cache, starting from the file at `path` when it holds data for this device and driver.
    pub fn open(ctx: &VkContext, path: Option<&Path>) -> Result<PipelineCache, PipelineError> {
        let data = path
            .and_then(|p| std::fs::read(p).ok())
            .filter(|d| header_matches(d, &ctx.properties))
            .unwrap_or_default();
        let info = vk::PipelineCacheCreateInfo::default().initial_data(&data);
        // SAFETY: the create info borrows `data` for the call.
        let handle = unsafe { ctx.device.create_pipeline_cache(&info, None)? };
        Ok(PipelineCache {
            device: ctx.device.clone(),
            handle,
            path: path.map(Path::to_path_buf),
            properties: ctx.properties,
            loaded: !data.is_empty(),
        })
    }

    pub fn handle(&self) -> vk::PipelineCache {
        self.handle
    }

    /// The cache's current data, as Vulkan serializes it.
    pub fn data(&self) -> Result<Vec<u8>, PipelineError> {
        // SAFETY: `handle` is a live cache of `device`.
        Ok(unsafe { self.device.get_pipeline_cache_data(self.handle)? })
    }

    /// Writes the cache to its file, through a temporary file so a crash leaves the old one whole.
    pub fn save(&self) -> Result<(), PipelineError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let data = self.data()?;
        let temporary = path.with_extension("tmp");
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        std::fs::write(&temporary, data)
            .and_then(|()| std::fs::rename(&temporary, path))
            .map_err(|e| PipelineError::Cache(e.to_string()))
    }

    /// A blob that names this device, driver and the description `key`, sealed by a checksum.
    pub fn cached_blob(&self, key: &[u8; 32]) -> Vec<u8> {
        let mut blob = Vec::with_capacity(0x60);
        blob.extend_from_slice(b"LPSO");
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&self.properties.vendor_id.to_le_bytes());
        blob.extend_from_slice(&self.properties.device_id.to_le_bytes());
        blob.extend_from_slice(&self.properties.driver_version.to_le_bytes());
        blob.extend_from_slice(&self.properties.pipeline_cache_uuid);
        blob.extend_from_slice(key);
        let seal = Sha256::digest(&blob);
        blob.extend_from_slice(&seal[..8]);
        blob
    }

    /// Whether `blob` came from `cached_blob` on this device and driver for the description `key`.
    pub fn accepts_blob(&self, blob: &[u8], key: &[u8; 32]) -> bool {
        blob == self.cached_blob(key).as_slice()
    }
}

impl Drop for PipelineCache {
    fn drop(&mut self) {
        let _ = self.save();
        // SAFETY: the cache is no longer used once its owner drops.
        unsafe { self.device.destroy_pipeline_cache(self.handle, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties() -> vk::PhysicalDeviceProperties {
        vk::PhysicalDeviceProperties {
            vendor_id: 0x10DE,
            device_id: 0x2504,
            pipeline_cache_uuid: [7; 16],
            ..Default::default()
        }
    }

    fn header(properties: &vk::PhysicalDeviceProperties) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&32u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&properties.vendor_id.to_le_bytes());
        data.extend_from_slice(&properties.device_id.to_le_bytes());
        data.extend_from_slice(&properties.pipeline_cache_uuid);
        data.extend_from_slice(&[1, 2, 3, 4]);
        data
    }

    #[test]
    fn a_cache_header_must_name_this_device() {
        let properties = properties();
        let data = header(&properties);
        assert!(header_matches(&data, &properties));
        let mut other = properties;
        other.pipeline_cache_uuid = [8; 16];
        assert!(!header_matches(&data, &other));
        assert!(!header_matches(&data[..20], &properties));
        let mut other = properties;
        other.device_id = 1;
        assert!(!header_matches(&data, &other));
    }
}
