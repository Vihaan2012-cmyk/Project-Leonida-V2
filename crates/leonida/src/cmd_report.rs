//! `leonida report`: gathers what a boot attempt left behind into one folder to send for diagnosis.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};

use crate::cli::BootReportArgs;

pub fn report_command(args: &BootReportArgs) -> Result<ExitCode> {
    let folder = gather(&args.output, &args.config)?;
    println!("report written to {}", folder.display());
    println!("send that folder; it holds the boot trace and your settings, and no game files");
    Ok(ExitCode::SUCCESS)
}

/// Copies `<output>/boot` and the settings files into `<output>/report`, replacing an earlier report, and writes
/// `report.txt` listing what was gathered. Game files are never copied: only the trace and settings folders are read.
pub fn gather(output: &Path, config: &Path) -> Result<PathBuf> {
    let folder = output.join("report");
    if folder.exists() {
        fs::remove_dir_all(&folder).with_context(|| format!("clearing {}", folder.display()))?;
    }
    fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    let mut listing = vec![format!("leonida {}", env!("CARGO_PKG_VERSION"))];
    let boot = output.join("boot");
    let copied = copy_files(&boot, &folder.join("boot"))?;
    if copied.is_empty() {
        listing.push("no boot trace: run `leonida run --start-the-game --trace` first".into());
    }
    listing.extend(copied.iter().map(|name| format!("boot/{name}")));
    let settings = copy_files(config, &folder.join("settings"))?
        .into_iter()
        .filter(|name| name.ends_with(".toml"))
        .collect::<Vec<_>>();
    listing.extend(settings.iter().map(|name| format!("settings/{name}")));
    fs::write(folder.join("report.txt"), listing.join("\n") + "\n")
        .with_context(|| format!("writing {}", folder.join("report.txt").display()))?;
    Ok(folder)
}

/// Copies the plain files directly inside `from` (not sub-folders) into `to`; a missing `from` copies nothing.
fn copy_files(from: &Path, to: &Path) -> Result<Vec<String>> {
    let Ok(entries) = fs::read_dir(from) else {
        return Ok(Vec::new());
    };
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        fs::copy(&path, to.join(&name)).with_context(|| format!("copying {}", path.display()))?;
        names.push(name);
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trace_and_settings_are_gathered_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("Output");
        let config = dir.path().join("Config");
        fs::create_dir_all(output.join("boot")).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(
            output.join("boot").join("summary.txt"),
            "stopped at stage 3",
        )
        .unwrap();
        fs::write(output.join("boot").join("crash.txt"), "int3").unwrap();
        fs::write(
            config.join("graphics.toml"),
            "frame_rate_mode = \"uncapped\"",
        )
        .unwrap();
        fs::write(config.join("notes.bak"), "old").unwrap();

        let folder = gather(&output, &config).unwrap();

        assert_eq!(
            fs::read_to_string(folder.join("boot").join("summary.txt")).unwrap(),
            "stopped at stage 3"
        );
        let listing = fs::read_to_string(folder.join("report.txt")).unwrap();
        assert!(listing.contains("boot/crash.txt"));
        assert!(listing.contains("boot/summary.txt"));
        assert!(listing.contains("settings/graphics.toml"));
        assert!(!listing.contains("notes.bak"));
    }

    #[test]
    fn a_missing_trace_says_how_to_make_one_and_a_second_report_replaces_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("Output");
        let config = dir.path().join("Config");
        let first = gather(&output, &config).unwrap();
        fs::write(first.join("stale.txt"), "old").unwrap();
        let second = gather(&output, &config).unwrap();
        assert!(!second.join("stale.txt").exists());
        let listing = fs::read_to_string(second.join("report.txt")).unwrap();
        assert!(listing.contains("no boot trace"));
    }
}
