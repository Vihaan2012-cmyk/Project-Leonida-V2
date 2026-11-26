use gpu::Gpu;

const DOUBLE: &str = "
@group(0) @binding(0) var<storage, read_write> data: array<u32>;
@group(0) @binding(1) var<storage, read_write> sizes: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>, @builtin(subgroup_size) size: u32) {
    data[id.x] = data[id.x] * 2u + id.x;
    sizes[id.x] = size;
}
";

fn spirv(source: &str) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(source).unwrap();
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

#[test]
fn a_wgsl_kernel_runs_and_reads_back() {
    let gpu = match Gpu::new() {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("skipping: {error}");
            return;
        }
    };
    let mut data: Vec<u32> = (0..128).map(|i| i * 10).collect();
    let mut sizes = vec![0u32; 128];
    gpu.dispatch(
        &spirv(DOUBLE),
        &mut [&mut data, &mut sizes],
        [2, 1, 1],
        None,
    )
    .unwrap();
    for (i, value) in data.iter().enumerate() {
        assert_eq!(*value, i as u32 * 20 + i as u32);
    }
    assert!(
        sizes.iter().all(|s| *s == gpu.info.subgroup_size),
        "{sizes:?}"
    );
    assert_eq!(gpu::validation_errors(), 0);
    eprintln!("{:?}", gpu.info);
}
