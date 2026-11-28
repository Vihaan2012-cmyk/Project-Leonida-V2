//! Where the game's files are: the package folder, `Output/`, and the mods and fixes that replace package files.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use game_package::Manifest;
use shims::files::Redirect;
use vfs::{ModsConfig, Vfs, fold};

use super::plan::RuntimePlan;

/// Builds the view of the mods and fixes the plan names. `None` when there is nothing to replace.
pub fn load_overlay(plan: &RuntimePlan, version: &str) -> Result<(Option<Vfs>, Vec<String>)> {
    let mut builder = Vfs::builder(version)
        .map_err(|e| anyhow!(e))?
        .mods_dir(&plan.mods_dir)
        .mods_settings(mods_settings(plan));
    if plan.fixes_dir.is_dir() {
        builder = builder.fixes(&plan.fixes_dir);
    }
    let (vfs, report) = builder.build().context("loading the mods")?;
    let mut notes = report.problems();
    notes.extend(report.mods.notes.iter().cloned());
    notes.extend(report.notes.iter().cloned());
    let any = !vfs.files().is_empty();
    Ok((any.then_some(vfs), notes))
}

/// `mods.toml` names the mods to load, lowest priority first, and only those load: a folder in `Mods/` that the
/// list does not name is switched off.
fn mods_settings(plan: &RuntimePlan) -> ModsConfig {
    let listed = |name: &str| {
        plan.mod_load_order
            .iter()
            .any(|chosen| fold(chosen) == fold(name))
    };
    let disabled = std::fs::read_dir(&plan.mods_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !listed(name))
        .collect();
    ModsConfig {
        order: plan.mod_load_order.clone(),
        disabled,
    }
}

/// The package file a mod or a fix replaces, for the file shims to read instead.
pub fn redirect(vfs: Vfs) -> Redirect {
    Box::new(move |relative: &Path| {
        let name = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let info = vfs.stat(&name).ok().flatten()?;
        vfs.host_path(&info)
    })
}

/// The newest game version Leonida knows, which mods and fixes are checked against.
fn game_version() -> Result<String> {
    Manifest::builtin()
        .last()
        .map(|manifest| manifest.version.clone())
        .ok_or_else(|| anyhow!("no game version is known"))
}

/// Tells the loader and file shims where the package and `Output/` are, and puts the mods over the package.
/// Returns what the mods loader had to say, for printing.
pub fn configure(plan: &RuntimePlan) -> Result<Vec<String>> {
    std::fs::create_dir_all(&plan.output_dir)
        .with_context(|| format!("creating {}", plan.output_dir.display()))?;
    shims::loader::configure(Some(plan.game_dir.clone()), plan.output_dir.clone());
    let configured = if plan.sandbox_files {
        shims::files::configure_sandboxed(
            plan.game_dir.clone(),
            plan.output_dir.clone(),
            vec![plan.mods_dir.clone(), plan.fixes_dir.clone()],
        )
    } else {
        shims::files::configure(plan.game_dir.clone(), plan.output_dir.clone())
    };
    if !configured {
        bail!(
            "the game folder {}, the output folder {}, the mods folder {} and the fixes folder {} must all be on a drive with a letter",
            plan.game_dir.display(),
            plan.output_dir.display(),
            plan.mods_dir.display(),
            plan.fixes_dir.display()
        );
    }
    let (overlay, notes) = load_overlay(plan, &game_version()?)?;
    if let Some(vfs) = overlay {
        shims::files::set_redirect(Some(redirect(vfs)));
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::cli::RunArgs;
    use crate::runtime::plan::Settings;

    fn plan_in(home: &Path, order: &[&str]) -> RuntimePlan {
        let args = RunArgs {
            game: None,
            file: None,
            dry_run: false,
            start_the_game: true,
            all: false,
            trace: false,
            output: Some(home.join("Output")),
            config: home.join("Config"),
            frame_limit: None,
        };
        let mut settings = Settings::default();
        settings.mods.load_order = order.iter().map(|name| name.to_string()).collect();
        RuntimePlan::from(&args, &settings, &home.join("Game")).unwrap()
    }

    fn make_mod(home: &Path, id: &str, files: &[(&str, &str)]) {
        let root = home.join("Mods").join(id);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("manifest.json"),
            r#"{"title":"T","author":"A","version":"1.0","type":"assets"}"#,
        )
        .unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    #[test]
    fn no_mods_folder_means_nothing_to_replace() {
        let home = tempfile::tempdir().unwrap();
        let plan = plan_in(home.path(), &[]);
        let (overlay, notes) = load_overlay(&plan, "1.0.2.1").unwrap();
        assert!(overlay.is_none());
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_listed_mod_replaces_package_files_and_an_unlisted_one_does_not() {
        let home = tempfile::tempdir().unwrap();
        make_mod(home.path(), "roads", &[("data/roads.xml", "roads")]);
        make_mod(home.path(), "radio", &[("data/radio.xml", "radio")]);
        let plan = plan_in(home.path(), &["roads"]);
        let (overlay, _) = load_overlay(&plan, "1.0.2.1").unwrap();
        let redirect = redirect(overlay.expect("a mod loaded"));
        let found = redirect(Path::new("data").join("roads.xml").as_path()).expect("replaced");
        assert_eq!(std::fs::read_to_string(found).unwrap(), "roads");
        assert!(redirect(Path::new("data/radio.xml")).is_none());
        assert!(redirect(Path::new("data/other.xml")).is_none());
        assert!(redirect(Path::new("DATA/ROADS.XML")).is_some());
    }

    #[test]
    fn a_later_mod_wins_where_two_replace_the_same_file() {
        let home = tempfile::tempdir().unwrap();
        make_mod(home.path(), "a", &[("x.bin", "from a")]);
        make_mod(home.path(), "b", &[("x.bin", "from b")]);
        let first = plan_in(home.path(), &["a", "b"]);
        let (overlay, _) = load_overlay(&first, "1.0.2.1").unwrap();
        let found = redirect(overlay.unwrap())(Path::new("x.bin")).unwrap();
        assert_eq!(std::fs::read_to_string(found).unwrap(), "from b");
        let second = plan_in(home.path(), &["b", "a"]);
        let (overlay, _) = load_overlay(&second, "1.0.2.1").unwrap();
        let found = redirect(overlay.unwrap())(Path::new("x.bin")).unwrap();
        assert_eq!(std::fs::read_to_string(found).unwrap(), "from a");
    }

    #[test]
    fn a_game_version_that_is_not_dotted_numbers_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let plan = plan_in(home.path(), &[]);
        assert!(load_overlay(&plan, "latest").is_err());
    }
}
