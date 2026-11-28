//! Everything `leonida run` sets up before the game's entry point runs, one file per concern.
//!
//! The order is fixed: settings become a [`RuntimePlan`], [`configure`] prepares every subsystem, the executable is
//! loaded, and [`patch`] changes its code while no game thread exists. A step that fails stops the run with its
//! name in the error.

mod audio;
mod content;
mod framegen;
mod input;
mod mouse;
mod patches;
mod paths;
mod plan;
#[cfg(test)]
mod real_tests;
#[cfg(test)]
mod synthetic;
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

/// Closes the ways a library could be picked up from the game folder or the current folder: from here on a library
/// load that names no folder searches Windows's system folder and Leonida's own folder only.
#[cfg(windows)]
pub fn protect_library_search() {
    if let Err(code) = shims::hostlib::confine_search_path() {
        eprintln!(
            "warning: could not limit where libraries are searched for (error {code}); Windows libraries are still loaded from the system folder by full path"
        );
    }
}

/// Makes sure every Windows library the imports name is in the Windows system folder and loads from there, never
/// from the game folder. Gives how many there are, or the message to show when one is missing.
#[cfg(windows)]
pub fn check_system_libraries<'a>(
    dlls: impl IntoIterator<Item = &'a str>,
) -> Result<usize, shims::hostlib::SystemLibraryError> {
    let needed = shims::system_libraries(dlls);
    shims::hostlib::require(needed.iter().map(String::as_str))?;
    Ok(needed.len())
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
    if plan.custom_limit {
        shims::threads::raise_timer_resolution();
    }
    shims::loader::set_language(&plan.language);
    audio::configure(plan);
    input::configure(&plan.input);
    for skipped in &plan.input_skipped {
        println!("input: not carried out: {skipped}");
    }
    if let Some(note) = upscaler::configure(plan).context("setting up the upscaler")? {
        println!("upscaler: {note}");
    }
    if let Some(note) = framegen::configure(plan) {
        println!("frame generation: {note}");
    }
    let line = content::configure(plan).context("setting up the nudity filter")?;
    println!("{line}");
    // DirectStorage requests name a BCPack mode in bits 1-3 of their options, so mode m arrives as format m << 1.
    #[cfg(windows)]
    for mode in 1..=7u8 {
        shims::dstorage::set_bcpack_format(mode << 1, mode);
    }
    #[cfg(windows)]
    shims::dstorage::set_page_writer(Some(shims::xmem::write_physical_page));
    // The title reserves its GPU-visible memory before it creates the device, so the device's address space and
    // memory hooks must exist first.
    shims::d3d12::prepare()
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("setting up the graphics device")?;
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
    for note in &outcome.notes {
        eprintln!("warning: {note}");
    }
    if outcome.mouse_camera {
        println!("the mouse turns the camera directly");
    }
    if let Some(warning) = outcome.warning {
        eprintln!("warning: no code patches were applied, the game runs unpatched: {warning}");
    }
}

/// Called when the entry point has returned: the mouse goes back to moving the stick, and the cell it filled may go.
pub fn detach_mouse() {
    shims::gameinput::direct::detach();
}

/// What `--dry-run` says about the mouse camera patch for the executable `bytes`: built against it, nothing written.
pub fn describe_mouse_patch(plan: &RuntimePlan, bytes: &[u8]) -> String {
    let Some(settings) = &plan.mouse_camera else {
        return "mouse camera patch: off, the mouse acts as the right stick".to_string();
    };
    match mouse::build(settings, bytes, &enhancements::camera_mouse::GAME, &[]) {
        Ok(built) => format!(
            "mouse camera patch: will be applied ({} patches, cell at {:#x})",
            built.patches.len(),
            built.cell
        ),
        Err(reason) => format!("mouse camera patch: would be left out, {reason}"),
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
            7
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_libraries_are_checked_in_the_system_folder_and_never_the_games() {
        assert_eq!(
            check_system_libraries(["KERNEL32.dll", "kernel32.dll", "ucrtbase.dll", "XMem.dll"])
                .unwrap(),
            2,
            "repeats and console libraries are not counted"
        );
        let needed = shims::system_libraries([
            "MSVCP140.dll",
            "VCRUNTIME140_1.dll",
            "concrt140.dll",
            "XCurl.dll",
            "libHttpClient.GDK.dll",
            "WS2_32.dll",
            "ADVAPI32.dll",
        ]);
        assert_eq!(
            needed,
            ["msvcp140.dll", "vcruntime140_1.dll", "advapi32.dll"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_missing_c_runtime_stops_with_the_redistributable_message() {
        let error = check_system_libraries(["MSVCP140_leonida_absent.dll"]).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("msvcp140_leonida_absent.dll"), "{text}");
        assert!(
            text.contains("Microsoft Visual C++ Redistributable"),
            "{text}"
        );
        assert!(
            text.contains("never loads the copy in the game folder"),
            "{text}"
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
