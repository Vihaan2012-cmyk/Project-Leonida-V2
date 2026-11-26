use std::path::PathBuf;
use std::process::Command;

use gpu::Gpu;

pub fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(gpu)
            if gpu.info.subgroup_size == 32 && gpu.info.int64 && gpu.info.ballot_and_shuffle =>
        {
            Some(gpu)
        }
        Ok(gpu) => {
            eprintln!(
                "skipping: {} lacks 32-wide subgroups, int64 or ballot",
                gpu.info.name
            );
            None
        }
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

pub fn spirv_val(words: &[u32], name: &str) {
    let Ok(output) = Command::new("spirv-val").arg("--version").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.spv"));
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(&path, bytes).unwrap();
    let result = Command::new("spirv-val")
        .arg("--target-env")
        .arg("vulkan1.3")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "spirv-val: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
