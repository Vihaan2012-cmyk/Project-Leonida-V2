use gpu::{BindingKind, CacheLoad, Check, Gpu};

const TINY: &str = "
@compute @workgroup_size(1)
fn main() {}
";

fn spirv() -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(TINY).unwrap();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    let pipeline = naga::back::spv::PipelineOptions {
        shader_stage: naga::ShaderStage::Compute,
        entry_point: "main".into(),
    };
    let options = naga::back::spv::Options {
        lang_version: (1, 3),
        ..Default::default()
    };
    naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).unwrap()
}

fn build(gpu: &Gpu) {
    let bindings: [(u32, BindingKind, u32); 0] = [];
    gpu.check(
        Check::Compute {
            spirv: &spirv(),
            subgroup_size: None,
        },
        &bindings,
        16,
    )
    .unwrap();
}

#[test]
fn a_saved_pipeline_cache_loads_into_a_new_device() {
    let Ok(first) = Gpu::new() else {
        eprintln!("skipping: no Vulkan GPU");
        return;
    };
    let folder = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let path = folder.path().join("Output").join("pipelines.cache");
    assert_eq!(
        first.load_pipeline_cache(&path).unwrap(),
        CacheLoad::Missing
    );
    build(&first);
    let size = first.save_pipeline_cache(&path).unwrap();
    assert!(size >= 32, "{size}");
    assert!(path.is_file());
    assert!(!path.with_extension("tmp").exists());
    drop(first);

    let second = Gpu::new().unwrap();
    assert_eq!(
        second.load_pipeline_cache(&path).unwrap(),
        CacheLoad::Loaded(size)
    );
    build(&second);
    assert!(second.pipeline_cache_data().unwrap().len() >= 32);
}

#[test]
fn a_damaged_or_foreign_cache_file_is_rejected() {
    let Ok(gpu) = Gpu::new() else {
        eprintln!("skipping: no Vulkan GPU");
        return;
    };
    build(&gpu);
    let folder = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let path = folder.path().join("pipelines.cache");
    gpu.save_pipeline_cache(&path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        gpu.load_pipeline_cache(&path).unwrap(),
        CacheLoad::Rejected(_)
    ));

    std::fs::write(
        &path,
        b"nothing like a cache, but long enough to hold a header of its own",
    )
    .unwrap();
    assert!(matches!(
        gpu.load_pipeline_cache(&path).unwrap(),
        CacheLoad::Rejected(_)
    ));
}
