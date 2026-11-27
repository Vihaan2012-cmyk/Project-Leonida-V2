//! The persistent `VkPipelineCache`: loaded from and saved to one file.

use std::path::Path;

use ash::vk;

use crate::{Gpu, GpuError};

const MAGIC: &[u8; 8] = b"LVPCACH1";
/// The file's magic and the checksum of the driver's data after it.
const FILE_HEADER: usize = 16;
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
        if bytes.len() < FILE_HEADER + DRIVER_HEADER || &bytes[..8] != MAGIC {
            return Ok(CacheLoad::Rejected("not a Leonida pipeline cache"));
        }
        let data = &bytes[FILE_HEADER..];
        let stored = u64::from_le_bytes(bytes[8..FILE_HEADER].try_into().expect("8 bytes"));
        if checksum(data) != stored {
            return Ok(CacheLoad::Rejected("the file is damaged"));
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
        let mut file = Vec::with_capacity(FILE_HEADER + data.len());
        file.extend_from_slice(MAGIC);
        file.extend_from_slice(&checksum(&data).to_le_bytes());
        file.extend_from_slice(&data);
        if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder)?;
        }
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, &file)?;
        std::fs::rename(&temporary, path)?;
        Ok(data.len())
    }
}
