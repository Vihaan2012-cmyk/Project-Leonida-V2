//! How long a pipeline create call takes with and without the service, on a real GPU. Skips without a
//! Vulkan 1.3 GPU. Run with `--nocapture` to see the times.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::time::Instant;

use ash::vk;
use shader_translate::FloatMode;
use xbox_d3d12::compile_queue::{PipelineService, ServiceConfig};
use xbox_d3d12::execute::VkContext;
use xbox_d3d12::pipeline::{
    Bytecode, CodeStore, ComputePsoDesc, DirectTranslation, PipelineCache, PipelineEnv,
    PipelineError, bindless_bindings,
};

struct NoArena;

impl CodeStore for NoArena {
    fn upload(&self, _: &[u8]) -> Result<u64, PipelineError> {
        Ok(0x8000)
    }
}

/// A compute shader of `length` adds, different for each `seed`.
fn shader(seed: u32, length: usize) -> ComputePsoDesc {
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
    ComputePsoDesc {
        root: None,
        cs: Bytecode(blob),
    }
}

fn environment(
    ctx: &Arc<VkContext>,
    bindless: vk::DescriptorSetLayout,
) -> Result<PipelineEnv, PipelineError> {
    let cache = PipelineCache::open(ctx, None)?;
    PipelineEnv::new(
        ctx.clone(),
        bindless,
        cache,
        Arc::new(DirectTranslation::new(FloatMode::default())),
        Arc::new(NoArena),
    )
}

#[test]
fn the_create_call_returns_before_the_build() {
    let ctx = match VkContext::standalone() {
        Ok(ctx) => Arc::new(ctx),
        Err(error) => {
            eprintln!("skipping: {error}");
            return;
        }
    };
    if ctx.properties.api_version < vk::API_VERSION_1_3 {
        return;
    }
    // SAFETY: a plain create call on a live device; destroyed at the end after every environment is dropped.
    let bindless = unsafe {
        let bindings = bindless_bindings(4);
        ctx.device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
            .unwrap()
    };
    const LENGTH: usize = 1500;

    let env = environment(&ctx, bindless).unwrap();
    let before = shader(1, LENGTH);
    let started = Instant::now();
    env.create_compute(&before).unwrap();
    let synchronous = started.elapsed();
    drop(env);

    let loader_ctx = ctx.clone();
    let started = Instant::now();
    let service = PipelineService::start(
        ServiceConfig {
            workers: 2,
            ..ServiceConfig::default()
        },
        move || environment(&loader_ctx, bindless),
    );
    let started_service = started.elapsed();
    let after = shader(2, LENGTH);
    let started = Instant::now();
    let handle = service.create_compute(after);
    let asynchronous = started.elapsed();
    let started = Instant::now();
    assert!(handle.resolve().ready().is_some());
    let until_ready = started.elapsed();
    eprintln!(
        "create call: {synchronous:?} before (in the game's thread), {asynchronous:?} after; \
         service start {started_service:?}; ready {until_ready:?} later"
    );
    assert!(asynchronous < synchronous);
    drop(handle);
    drop(service);
    // SAFETY: every pipeline and environment built on the layout is gone.
    unsafe { ctx.device.destroy_descriptor_set_layout(bindless, None) };
}
