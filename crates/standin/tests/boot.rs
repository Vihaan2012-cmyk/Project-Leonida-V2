//! The stand-in title run through the whole runtime, one stage at a time and then as one boot. A failed stage
//! names the step that failed, which names the subsystem.
#![cfg(windows)]

use std::path::PathBuf;

use standin::{Environment, Report, RunOptions, Stage, steps};

fn environment() -> Environment {
    let under = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("standin");
    Environment::new(&under).expect("folders for the stand-in")
}

fn run_stage(stage: Stage) -> Report {
    let script = steps::stage(stage);
    let built = script.build().expect("the stand-in assembles");
    let environment = environment();
    let report =
        standin::run(&built, &environment, &RunOptions::default()).expect("the stand-in runs");
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
