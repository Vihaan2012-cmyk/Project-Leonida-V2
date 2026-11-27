use std::fmt::Write;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("shaders");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("shaders.rs");
    println!("cargo:rerun-if-changed=shaders");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wgsl"))
        .collect();
    files.sort();
    let mut code = String::new();
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().to_uppercase();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}: {}", path.display(), e.emit_to_string(&source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        let words =
            naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None)
                .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        let _ = writeln!(code, "pub const {name}: &[u32] = &[");
        for word in words {
            let _ = write!(code, "{word:#x},");
        }
        let _ = writeln!(code, "];");
        println!("cargo:rerun-if-changed={}", path.display());
    }
    std::fs::write(out, code).unwrap();
}
