use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for name in ["surface", "bc1"] {
        let path = format!("shaders/{name}.wgsl");
        println!("cargo:rerun-if-changed={path}");
        let source = fs::read_to_string(&path).unwrap();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|error| panic!("{path}: {}", error.emit_to_string(&source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{path}: {error:?}"));
        let pipeline = naga::back::spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Compute,
            entry_point: "main".into(),
        };
        let options = naga::back::spv::Options {
            lang_version: (1, 3),
            ..Default::default()
        };
        let words = naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline))
            .unwrap_or_else(|error| panic!("{path}: {error:?}"));
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        fs::write(out.join(format!("{name}.spv")), bytes).unwrap();
    }
    println!("cargo:rerun-if-changed=build.rs");
}
