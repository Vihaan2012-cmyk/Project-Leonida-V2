use std::process::Command;

use shader_container::testutil::{container, fssm, rpf_entry};

const NOP: u32 = 0xBF80_0000;

#[test]
fn scan_reports_counts() {
    let dir = tempfile::tempdir().unwrap();
    let mut archive = vec![0u8; 8];
    archive.extend(rpf_entry(
        "ps_one",
        3,
        1,
        &container(0, &[fssm(false, &[NOP])]),
    ));
    archive.push(0);
    let two_payloads = container(5, &[fssm(true, &[NOP]), fssm(true, &[NOP, NOP])]);
    archive.extend(rpf_entry("cs_two", 3, 2, &two_payloads));
    std::fs::write(dir.path().join("shaders.rpf"), &archive).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_leonida"))
        .args(["shaders", "scan", "--game"])
        .arg(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("shaders.rpf: 2 containers carved, 0 candidates rejected"),
        "{text}"
    );
    assert!(text.contains("stages: cs 1, ps 1"), "{text}");
    assert!(text.contains("container parts: DXIL 2, PRIV 2"), "{text}");
    assert!(
        text.contains("FSSM payloads: 3 (1 container has more than one; the last is used below)"),
        "{text}"
    );
    assert!(
        text.contains("wave size by stage: cs wave32 1, ps wave64 1"),
        "{text}"
    );
    assert!(text.contains("wave size: wave32 1, wave64 1"), "{text}");
    assert!(text.contains("code range: header 2"), "{text}");
    assert!(text.contains("problems: none"), "{text}");
}

#[test]
fn missing_archive_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_leonida"))
        .args(["shaders", "scan", "--game"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("shaders.rpf"));
}
