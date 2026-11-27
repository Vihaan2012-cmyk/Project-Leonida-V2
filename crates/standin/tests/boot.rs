//! The stand-in title run through the whole runtime, one stage at a time and then as one boot. A failed stage
//! names the step that failed, which names the subsystem.
//!
//! Each stage runs in a process of its own, as the game does: the runtime starts threads that outlive the entry
//! point and swaps thread-local storage slots, so a process takes one image, and a guest crash fails one test.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::process::Command;

use standin::{Environment, Report, RunOptions, Script, Stage, steps};

fn environment() -> Environment {
    let under = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("standin");
    Environment::new(&under).expect("folders for the stand-in")
}

fn find(folder: &std::path::Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(folder).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

fn run_script(script: &Script, environment: &Environment) -> Report {
    let built = script.build().expect("the stand-in assembles");
    let report =
        standin::run(&built, environment, &RunOptions::default()).expect("the stand-in runs");
    if let Some(failure) = &report.failed {
        panic!(
            "{failure} (exit {:#x}, {} steps passed)",
            report.exit, report.completed
        );
    }
    assert!(report.passed(), "exit {:#x}", report.exit);
    assert_eq!(report.completed as usize, script.len());
    report
}

/// What the child process runs for a stage, with the checks on what the stage left behind.
fn child(name: &str) {
    let environment = environment();
    match name {
        "boot" => {
            let report = run_script(&steps::boot(), &environment);
            for stage in Stage::ALL {
                check_stage(stage, &report, &environment);
            }
        }
        name => {
            let stage = Stage::ALL
                .into_iter()
                .find(|s| s.name() == name)
                .unwrap_or_else(|| panic!("no stage named {name}"));
            let report = run_script(&steps::stage(stage), &environment);
            check_stage(stage, &report, &environment);
        }
    }
}

fn check_stage(stage: Stage, report: &Report, environment: &Environment) {
    match stage {
        Stage::Loader => {
            let base = report.base as u64;
            assert_eq!(
                report.u64("function_base"),
                base,
                "the unwinder found our image"
            );
            assert_ne!(report.u64("function_entry"), 0);
            assert_eq!(
                report.u64("image_pointer"),
                base,
                "the relocation was applied"
            );
            assert_eq!(report.u32("title_id"), steps::TITLE_ID);
        }
        Stage::Files => {
            let package = std::fs::read(environment.package.join("data").join("boot.bin")).unwrap();
            assert_eq!(
                package,
                steps::package_file_bytes(),
                "the package is untouched"
            );
            assert!(
                !environment.package.join("data").join("slot.bin").exists(),
                "nothing is written into the package"
            );
            assert_eq!(
                find(&environment.output, "slot.bin").map(|p| std::fs::metadata(p).unwrap().len()),
                Some(steps::files::SAVE_BYTES),
                "the written file landed under Output"
            );
        }
        _ => {}
    }
}

/// The body of every stage test: runs the stage in a child process and shows what it said when it failed.
fn in_child(name: &str) {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "child_stage",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("STANDIN_STAGE", name)
        .output()
        .expect("the child process starts");
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains(&format!("stage {name} passed")),
        "stage {name} failed ({}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

unsafe extern "system" {
    fn GetCurrentProcess() -> isize;
    fn TerminateProcess(process: isize, code: u32) -> i32;
}

#[test]
#[ignore = "run by the stage tests, in a process of its own"]
fn child_stage() {
    if let Ok(name) = std::env::var("STANDIN_STAGE") {
        child(&name);
        use std::io::Write;
        println!("stage {name} passed");
        let _ = std::io::stdout().flush();
        // The image's thread-local storage is still in place and shim threads still run, so the usual exit would
        // run Rust's thread-local destructors against it; the process ends here, as the game's does.
        // SAFETY: ends this process, which has nothing left to do.
        unsafe { TerminateProcess(GetCurrentProcess(), 0) };
    }
}

#[test]
fn loader() {
    in_child("loader");
}

#[test]
fn memory() {
    in_child("memory");
}

#[test]
fn files() {
    in_child("files");
}

#[test]
fn threads() {
    in_child("threads");
}

#[test]
fn input() {
    in_child("input");
}

#[test]
fn audio() {
    in_child("audio");
}

#[test]
fn device() {
    in_child("device");
}

#[test]
#[ignore = "needs the assembled device"]
fn present() {
    in_child("present");
}

#[test]
#[ignore = "needs the assembled device"]
fn full_boot() {
    in_child("boot");
}
