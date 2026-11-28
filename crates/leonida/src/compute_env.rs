//! A headless copy of the game's pipeline environment, so a prebuilt compute pipeline is exactly the pipeline
//! the game's device asks for: the same device, bindless layout, push range and translation, in the cache file
//! the game's device opens.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use shader_container::Stage;
use shader_translate::FloatMode;
use xbox_d3d12::assembly::bindless::Bindless;
use xbox_d3d12::assembly::gpu::{Gpu, GpuConfig};
use xbox_d3d12::pipeline::{
    Bytecode, CodeStore, ComputePsoDesc, DirectTranslation, Link, PipelineCache, PipelineEnv,
    PipelineError, ShaderSource, TranslatedStage,
};

struct NoArena;

impl CodeStore for NoArena {
    fn upload(&self, _: &[u8]) -> Result<u64, PipelineError> {
        Ok(0x8000)
    }
}

/// Translates each program afresh: a prebuild of thousands of programs must not keep their SPIR-V.
struct Forgetful(FloatMode);

impl ShaderSource for Forgetful {
    fn translate(
        &self,
        bytecode: &[u8],
        stage: Stage,
        link: &Link,
    ) -> Result<Arc<TranslatedStage>, PipelineError> {
        DirectTranslation::new(self.0).translate(bytecode, stage, link)
    }
}

pub struct ComputeEnv {
    // Declared first so they drop before the device they were made on.
    env: PipelineEnv,
    _bindless: Bindless,
    gpu: Gpu,
}

impl ComputeEnv {
    /// Opens the device and starts from the cache file at `cache` when it is this GPU's.
    pub fn open(cache: &Path) -> Result<ComputeEnv> {
        let gpu = Gpu::create(&GpuConfig::from_env(false)).context("opening the game's device")?;
        let bindless = Bindless::new(&gpu).context("creating the bindless layout")?;
        let cache = PipelineCache::open(&gpu.ctx, Some(cache))?;
        let env = PipelineEnv::new(
            gpu.ctx.clone(),
            bindless.layout,
            cache,
            Arc::new(Forgetful(FloatMode::default())),
            Arc::new(NoArena),
        )?;
        Ok(ComputeEnv {
            env,
            _bindless: bindless,
            gpu,
        })
    }

    /// Builds the pipeline of a compute container and drops it again, leaving it in the cache.
    pub fn build(&self, container: &[u8]) -> Result<(), PipelineError> {
        self.env
            .create_compute(&ComputePsoDesc {
                root: None,
                cs: Bytecode(container.to_vec()),
            })
            .map(drop)
    }

    /// Whether the cache file held data for this device.
    pub fn loaded(&self) -> bool {
        self.env.cache.loaded
    }

    pub fn cache_size(&self) -> Result<usize, PipelineError> {
        Ok(self.env.cache.data()?.len())
    }

    pub fn save(&self) -> Result<(), PipelineError> {
        self.env.cache.save()
    }

    /// Names this GPU and driver build the way `gpu::Gpu::pipeline_cache_id` does.
    pub fn cache_id(&self) -> u64 {
        let p = &self.gpu.ctx.properties;
        gpu::cache_id(p.vendor_id, p.device_id, &p.pipeline_cache_uuid)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    const FILE: &str = "LEONIDA_CACHE_TEST_FILE";

    /// A compute program of `length` adds, different for each `seed`, as a bare FSSM blob.
    fn program(seed: u32, length: usize) -> Vec<u8> {
        let mut source = String::new();
        for n in 0..length {
            source += &format!("v_add_nc_u32_e32 v1, {}, v1\n", 1 + (seed + n as u32) % 60);
        }
        source += "s_endpgm";
        let code = rdna::assemble(&source, true).unwrap();
        let mut blob = vec![0u8; 0x228];
        blob[..4].copy_from_slice(b"FSSM");
        blob[4..6].copy_from_slice(&1u16.to_le_bytes());
        blob[0xC0..0xC4].copy_from_slice(&0x200u32.to_le_bytes());
        blob[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
        for (at, value) in [(0x138, 4u32 << 1), (0x13C, 64), (0x140, 1), (0x144, 1)] {
            blob[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        blob.extend_from_slice(&code);
        blob
    }

    /// The first process of the test below: builds one program into the cache file and saves it.
    #[test]
    fn child_prebuilds_into_the_cache_file() {
        let Some(path) = std::env::var_os(FILE).map(PathBuf::from) else {
            return;
        };
        let env = ComputeEnv::open(&path).unwrap();
        env.build(&program(1, 800)).unwrap();
        env.save().unwrap();
    }

    #[test]
    fn a_pipeline_prebuilt_by_one_process_is_a_cache_hit_in_the_next() {
        if Gpu::create(&GpuConfig::from_env(false)).is_err() {
            eprintln!("skipping: no Vulkan GPU");
            return;
        }
        let folder = std::env::current_exe()
            .unwrap()
            .with_file_name(format!("cache-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("game-pipelines.cache");
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "compute_env::tests::child_prebuilds_into_the_cache_file",
            ])
            .env(FILE, &path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(path.is_file());

        let game = ComputeEnv::open(&path).unwrap();
        assert!(game.loaded(), "the game's device does not accept the file");
        let before = game.cache_size().unwrap();
        game.build(&program(1, 800)).unwrap();
        assert_eq!(game.cache_size().unwrap(), before, "built again, not found");
        game.build(&program(2, 800)).unwrap();
        assert!(
            game.cache_size().unwrap() > before,
            "a program nobody built should have added to the cache"
        );
        drop(game);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
