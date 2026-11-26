use std::fs::File;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use game_package::{Manifest, map_file, match_version, package_path};
use shader_container::{ScanResult, ScanSummary, scan::scan};

use crate::jobs::{Outcome, Reporter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Waiting,
    Done,
    Failed,
    NotBuiltYet,
}

pub struct Step {
    pub name: &'static str,
    pub milestone: Option<&'static str>,
}

pub const STEPS: &[Step] = &[
    Step {
        name: "Find and recognise your copy",
        milestone: None,
    },
    Step {
        name: "Check every file",
        milestone: Some("Phase 0, Task 6"),
    },
    Step {
        name: "Read shader containers",
        milestone: None,
    },
    Step {
        name: "Translate shaders to SPIR-V",
        milestone: Some("M1.2–M1.8"),
    },
    Step {
        name: "Index assets",
        milestone: Some("M2.1"),
    },
    Step {
        name: "Precompile pipelines",
        milestone: Some("M3.4"),
    },
    Step {
        name: "Prepare the runtime",
        milestone: Some("M4.0–M4.7"),
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    pub name: &'static str,
    pub state: StepState,
    pub detail: String,
}

pub fn initial_reports() -> Vec<StepReport> {
    STEPS
        .iter()
        .map(|step| match step.milestone {
            Some(milestone) => StepReport {
                name: step.name,
                state: StepState::NotBuiltYet,
                detail: format!("arrives in {milestone}"),
            },
            None => StepReport {
                name: step.name,
                state: StepState::Waiting,
                detail: String::new(),
            },
        })
        .collect()
}

pub fn scan_shaders(
    game: &Path,
    reporter: &Reporter,
    from: f32,
    to: f32,
) -> Result<ScanResult, String> {
    let path = package_path(game, "shaders.rpf");
    let file =
        File::open(&path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let archive =
        map_file(&file).map_err(|error| format!("cannot map {}: {error}", path.display()))?;
    let total = archive.len().max(1) as f32;
    let mut count = 0usize;
    let result = scan(&archive, |offset| {
        count += 1;
        if count.is_multiple_of(64) {
            let fraction = from + (to - from) * (offset as f32 / total);
            reporter.progress(fraction, format!("{count} shader containers read"));
        }
        if reporter.cancelled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    if result.cancelled {
        return Err("cancelled".to_string());
    }
    Ok(result)
}

pub fn run_build(game: Option<PathBuf>, manifests: Vec<Manifest>, reporter: &Reporter) -> Outcome {
    let mut steps = initial_reports();
    let mut scan_result = None;
    let mut failed = false;
    for (index, step) in STEPS.iter().enumerate() {
        if step.milestone.is_some() {
            continue;
        }
        if failed {
            steps[index].detail = "skipped: an earlier step failed".to_string();
            continue;
        }
        let fraction = index as f32 / STEPS.len() as f32;
        reporter.progress(fraction, step.name);
        let result = match index {
            0 => recognise_step(game.as_deref(), &manifests),
            2 => match game.as_deref() {
                Some(path) => {
                    let to = (index + 1) as f32 / STEPS.len() as f32;
                    scan_shaders(path, reporter, fraction, to).map(|result| {
                        let summary = ScanSummary::of(&result);
                        let detail = format!(
                            "{} containers, {} problems",
                            summary.carved,
                            summary.problems.values().sum::<usize>()
                        );
                        scan_result = Some(result);
                        detail
                    })
                }
                None => Err("no game folder".to_string()),
            },
            _ => Ok(String::new()),
        };
        match result {
            Ok(detail) => {
                reporter.log(format!("{}: {detail}", step.name));
                steps[index].state = StepState::Done;
                steps[index].detail = detail;
            }
            Err(error) => {
                reporter.log(format!("{} failed: {error}", step.name));
                steps[index].state = StepState::Failed;
                steps[index].detail = error;
                failed = true;
            }
        }
    }
    reporter.progress(1.0, "Build finished");
    Outcome::Build {
        steps,
        scan: scan_result,
    }
}

fn recognise_step(game: Option<&Path>, manifests: &[Manifest]) -> Result<String, String> {
    let game = game.ok_or("no game folder chosen")?;
    match match_version(game, manifests) {
        Some(manifest) => Ok(manifest.label()),
        None => Err(format!("{} is not a supported version", game.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Jobs, Wake};
    use shader_container::testutil::{container, fssm, rpf_entry};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const TEST_MANIFEST: &str = r#"{
        "title": "Test package", "version": "9.9", "expected_exe": "game.exe",
        "files": {
            "MicrosoftGame.Config": {"size": 5, "sha256": "x"},
            "Catalog000.bin": {"size": 3, "sha256": "x"},
            "appdata0.rpf": {"size": 4, "sha256": "x"}
        }
    }"#;

    fn game_folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, len) in [
            ("MicrosoftGame.Config", 5),
            ("Catalog000.bin", 3),
            ("appdata0.rpf", 4),
        ] {
            std::fs::File::create(dir.path().join(name))
                .unwrap()
                .set_len(len)
                .unwrap();
        }
        let mut archive = vec![0u8; 8];
        archive.extend(rpf_entry(
            "ps_one",
            3,
            1,
            &container(0, &[fssm(false, &[0xBF80_0000])]),
        ));
        std::fs::write(dir.path().join("shaders.rpf"), archive).unwrap();
        dir
    }

    fn build(game: Option<PathBuf>) -> (Vec<StepReport>, Option<ScanResult>) {
        let mut jobs = Jobs::default();
        let manifests = vec![Manifest::from_json(TEST_MANIFEST).unwrap()];
        let wake: Wake = Arc::new(|| {});
        jobs.start("build", wake, move |r| run_build(game, manifests, r));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(Outcome::Build { steps, scan }) = jobs.poll().finished {
                return (steps, scan);
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn existing_steps_run_and_the_rest_are_not_built_yet() {
        let dir = game_folder();
        let (steps, scan) = build(Some(dir.path().to_path_buf()));
        assert_eq!(steps.len(), STEPS.len());
        assert_eq!(steps[0].state, StepState::Done);
        assert_eq!(steps[0].detail, "Test package 9.9");
        assert_eq!(steps[1].state, StepState::NotBuiltYet);
        assert_eq!(steps[2].state, StepState::Done);
        assert_eq!(steps[2].detail, "1 containers, 0 problems");
        assert!(steps[3..].iter().all(|s| s.state == StepState::NotBuiltYet));
        assert_eq!(scan.unwrap().shaders.len(), 1);
    }

    #[test]
    fn without_a_game_the_first_step_fails_and_stops_the_rest() {
        let (steps, scan) = build(None);
        assert_eq!(steps[0].state, StepState::Failed);
        assert_eq!(steps[2].state, StepState::Waiting);
        assert!(steps[2].detail.contains("earlier step failed"));
        assert!(scan.is_none());
    }

    #[test]
    fn initial_reports_mark_unbuilt_steps() {
        let reports = initial_reports();
        assert_eq!(reports[0].state, StepState::Waiting);
        assert_eq!(reports[3].state, StepState::NotBuiltYet);
        assert_eq!(reports[3].detail, "arrives in M1.2–M1.8");
    }
}
