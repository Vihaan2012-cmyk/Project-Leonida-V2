//! The stand-in title run through the whole runtime, one stage at a time and then as one boot. A failed stage
//! names the step that failed, which names the subsystem.
#![cfg(windows)]

use std::path::PathBuf;

use standin::{Environment, Report, RunOptions, Stage, steps};

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

/// The runtime swaps thread-local storage slots while an image is loaded, so no other test may run Rust code
/// (a panic message, say) meanwhile: a test holds this for its whole length.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_stage(stage: Stage) -> Report {
    run_stage_in(stage, &environment())
}

fn run_stage_in(stage: Stage, environment: &Environment) -> Report {
    let _alone = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let script = steps::stage(stage);
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

#[test]
fn loader() {
    run_stage(Stage::Loader);
}

#[test]
fn memory() {
    run_stage(Stage::Memory);
}

#[test]
fn files() {
    let environment = environment();
    run_stage_in(Stage::Files, &environment);
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
