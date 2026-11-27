//! Everything `leonida run` sets up before the game's entry point runs, one file per concern.
//!
//! The order is fixed: settings become a [`RuntimePlan`], [`configure`] prepares every subsystem, the executable is
//! loaded, and [`patch`] changes its code while no game thread exists. A step that fails stops the run with its
//! name in the error.

mod audio;
mod content;
mod input;
mod patches;
mod paths;
mod plan;
mod upscaler;

use std::path::Path;

use anyhow::{Context, Result};
use config::{Config, Section};

pub use plan::{RuntimePlan, Settings};

/// Reads the settings files. Missing files are written with their documented defaults only when
/// `write_missing` is set, so a dry run leaves the folder as it was.
pub fn load_settings(dir: &Path, write_missing: bool) -> Result<Settings> {
    if write_missing {
        return Config::load(dir)
            .with_context(|| format!("reading the settings in {}", dir.display()));
    }
    fn one<T: Section>(dir: &Path) -> Result<T> {
        match std::fs::read_to_string(dir.join(T::FILE)) {
            Ok(text) => Ok(T::parse(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", T::FILE)),
        }
    }
    let read = || -> Result<Settings> {
        Ok(Config {
            general: one(dir)?,
            graphics: one(dir)?,
            input: one(dir)?,
            mods: one(dir)?,
            display: one(dir)?,
            widescreen: one(dir)?,
            upscaling: one(dir)?,
        })
    };
    read().with_context(|| format!("reading the settings in {}", dir.display()))
}

/// Prints the plan.
pub fn print_plan(plan: &RuntimePlan) {
    for line in plan.summary() {
        println!("{line}");
    }
}

/// Prepares every subsystem the game needs before its code is loaded.
pub fn configure(plan: &RuntimePlan) -> Result<()> {
    let notes = paths::configure(plan).context("setting up the game's files")?;
    for note in notes {
        println!("mods: {note}");
    }
    shims::presentation::set(shims::presentation::Presentation {
        frame_limit: plan.frame_limit,
        graphics: plan.graphics.clone(),
    });
    audio::configure(plan);
    input::configure(&plan.input);
    for skipped in &plan.input_skipped {
        println!("input: not carried out: {skipped}");
    }
    if let Some(note) = upscaler::configure(plan).context("setting up the upscaler")? {
        println!("upscaler: {note}");
    }
    let line = content::configure(plan).context("setting up the nudity filter")?;
    println!("{line}");
    Ok(())
}

/// Called with the loaded image: tells the loader shims which module is the game and applies the code patches.
/// A patch that fails applies none and the game runs as it is.
pub fn patch(plan: &RuntimePlan, bytes: &[u8], module: &pe_loader::Module) {
    shims::loader::set_main_module(module.base());
    let outcome = patches::apply(plan, bytes, module);
    for name in &outcome.applied {
        println!("patched {name}");
    }
    if let Some(warning) = outcome.warning {
        eprintln!("warning: no code patches were applied, the game runs unpatched: {warning}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dry_run_reads_defaults_without_writing_them() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_settings(dir.path(), false).unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let written = load_settings(&dir.path().join("Config"), true).unwrap();
        assert_eq!(written, Settings::default());
        assert_eq!(
            std::fs::read_dir(dir.path().join("Config"))
                .unwrap()
                .count(),
            4
        );
    }

    #[test]
    fn present_files_are_read_and_a_wrong_one_names_itself() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("graphics.toml"), "upscaler = \"fsr1\"\n").unwrap();
        let settings = load_settings(dir.path(), false).unwrap();
        assert_eq!(settings.graphics.upscaler, config::Upscaler::Fsr1);
        std::fs::write(dir.path().join("input.toml"), "mouse_sensitivity = 0\n").unwrap();
        let error = format!("{:#}", load_settings(dir.path(), false).unwrap_err());
        assert!(error.contains("input.toml"), "{error}");
    }
}
