use std::process::Command;

fn leonida() -> Command {
    Command::new(env!("CARGO_BIN_EXE_leonida"))
}

#[test]
fn prints_version() {
    let out = leonida().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "leonida 0.1.0");
}
