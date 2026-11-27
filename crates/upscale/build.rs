use std::fmt::Write as _;
use std::path::PathBuf;

const SHADERS: &[(&str, &[&str])] = &[
    ("EASU", &["common.wgsl", "easu.wgsl"]),
    ("RCAS", &["common.wgsl", "rcas.wgsl"]),
    ("TEMPORAL", &["common.wgsl", "temporal.wgsl"]),
    ("FG_KEY", &["fg_common.wgsl", "fg_key.wgsl"]),
    ("FG_FIELD", &["fg_common.wgsl", "fg_field.wgsl"]),
    ("FG_INTERP", &["fg_common.wgsl", "fg_interp.wgsl"]),
];

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("shaders");
    println!("cargo:rerun-if-changed=shaders");
    let mut out = String::new();
    for (name, files) in SHADERS {
        let mut source = String::new();
        for file in *files {
            source.push_str(&std::fs::read_to_string(dir.join(file)).unwrap());
            source.push('\n');
        }
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let pipeline = naga::back::spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Compute,
            entry_point: "main".into(),
        };
        let options = naga::back::spv::Options {
            lang_version: (1, 3),
            ..Default::default()
        };
        let words = naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        write!(out, "pub static {name}: &[u32] = &[").unwrap();
        for word in words {
            write!(out, "{word:#x},").unwrap();
        }
        out.push_str("];\n");
    }
    let target = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("shaders.rs");
    std::fs::write(target, out).unwrap();
}
