//! The persistent `VkPipelineCache`: loaded from and saved to one file.
//!
//! The file is the driver's own cache data, as the game's device (`xbox-d3d12`'s `PipelineCache`) keeps it, so
//! one file serves a prebuild and the game. Files in this crate's earlier wrapped form (magic and checksum before
//! the driver data) still load.

use std::path::Path;

use ash::vk;

use crate::{Gpu, GpuError};

/// The magic and checksum an earlier version of this file put before the driver's data.
const LEGACY_MAGIC: &[u8; 8] = b"LVPCACH1";
const LEGACY_HEADER: usize = 16;
/// The driver's own header: size, version, vendor, device and cache UUID.
const DRIVER_HEADER: usize = 32;

/// What loading a cache file did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLoad {
    Missing,
    /// This many bytes of driver data were merged into the cache.
    Loaded(usize),
    /// The file is not usable on this GPU and driver, for this reason; the cache stays as it was.
    Rejected(&'static str),
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Names a GPU and driver build as `Gpu::pipeline_cache_id` does, from the device's vendor and device ids and its
/// pipeline cache UUID.
pub fn cache_id(vendor_id: u32, device_id: u32, uuid: &[u8; 16]) -> u64 {
    let mut identity = [0u8; 24];
    identity[..4].copy_from_slice(&vendor_id.to_le_bytes());
    identity[4..8].copy_from_slice(&device_id.to_le_bytes());
    identity[8..].copy_from_slice(uuid);
    checksum(&identity)
}

impl Gpu {
    /// Names this GPU and driver build: what a cache file must match to be loaded.
    pub fn pipeline_cache_id(&self) -> u64 {
        checksum(&self.cache_identity)
    }

    /// The driver's serialized cache.
    pub fn pipeline_cache_data(&self) -> Result<Vec<u8>, GpuError> {
        // SAFETY: `self.cache` was created from `self.device` and lives until the Gpu is dropped.
        Ok(unsafe { self.device.get_pipeline_cache_data(self.cache)? })
    }

    /// Merges the cache file at `path` into the cache every pipeline build uses. A missing, damaged or foreign
    /// file is reported, not an error.
    pub fn load_pipeline_cache(&self, path: &Path) -> Result<CacheLoad, GpuError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLoad::Missing);
            }
            Err(error) => return Err(error.into()),
        };
        let data = match bytes.strip_prefix(LEGACY_MAGIC) {
            Some(rest) if rest.len() >= 8 => {
                let stored = u64::from_le_bytes(rest[..8].try_into().expect("8 bytes"));
                let data = &bytes[LEGACY_HEADER..];
                if checksum(data) != stored {
                    return Ok(CacheLoad::Rejected("the file is damaged"));
                }
                data
            }
            _ => &bytes[..],
        };
        if data.len() < DRIVER_HEADER {
            return Ok(CacheLoad::Rejected("not a pipeline cache"));
        }
        let version = u32::from_le_bytes(data[4..8].try_into().expect("4 bytes"));
        if version != vk::PipelineCacheHeaderVersion::ONE.as_raw() as u32
            || data[8..DRIVER_HEADER] != self.cache_identity
        {
            return Ok(CacheLoad::Rejected("made by another GPU or driver"));
        }
        // SAFETY: the temporary cache is created and destroyed here, and merged into `self.cache`, which
        // is a valid cache of the same device.
        unsafe {
            let loaded = self.device.create_pipeline_cache(
                &vk::PipelineCacheCreateInfo::default().initial_data(data),
                None,
            )?;
            let merged = self.device.merge_pipeline_caches(self.cache, &[loaded]);
            self.device.destroy_pipeline_cache(loaded, None);
            merged?;
        }
        Ok(CacheLoad::Loaded(data.len()))
    }

    /// Writes the cache to `path` through a temporary file, so a crash leaves the old file whole. Returns the
    /// size of the driver's data.
    pub fn save_pipeline_cache(&self, path: &Path) -> Result<usize, GpuError> {
        let data = self.pipeline_cache_data()?;
        if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder)?;
        }
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, &data)?;
        std::fs::rename(&temporary, path)?;
        Ok(data.len())
    }
}
