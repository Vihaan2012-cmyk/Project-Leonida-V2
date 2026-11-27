use std::path::PathBuf;

use upscale::dlss::{DlssConfig, DlssStatus, probe};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_missing_or_foreign_file_is_reported_not_loaded() {
    let dir = scratch("dlss-probe");
    let text = dir.join("not_a_library.dll");
    std::fs::write(&text, b"plain text").unwrap();
    let config = |path: PathBuf| DlssConfig {
        dlss_dll: Some(path),
        ngx_runtime: Some(dir.join("absent_nvngx.dll")),
    };
    assert!(matches!(
        probe(&config(text)),
        DlssStatus::Unavailable(r) if r.contains("not a Windows library")
    ));
    assert!(matches!(
        probe(&config(dir.join("absent.dll"))),
        DlssStatus::Unavailable(_)
    ));
}

#[test]
fn a_missing_ngx_runtime_makes_dlss_unavailable() {
    let dir = scratch("dlss-runtime");
    let library = dir.join("nvngx_dlss.dll");
    std::fs::write(&library, b"MZ").unwrap();
    let status = probe(&DlssConfig {
        dlss_dll: Some(library),
        ngx_runtime: Some(dir.join("absent_nvngx.dll")),
    });
    assert!(matches!(status, DlssStatus::Unavailable(_)), "{status:?}");
}
