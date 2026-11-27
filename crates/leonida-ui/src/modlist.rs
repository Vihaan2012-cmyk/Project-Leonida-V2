//! The packages in `Mods/`, which of them load and in what order, and the files they contest.
//!
//! `mods.toml` lists the packages that load, lowest priority first; a package it does not name is off. The list
//! and its operations here do not touch the disk; `inspect` reads the packages.

use std::path::Path;

use vfs::{Conflict, ModStatus, ModsConfig, Vfs, fold};

/// A package found in `Mods/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub id: String,
    pub title: String,
    pub version: String,
    pub kind: String,
    pub author: String,
    /// Why it cannot load, when it cannot.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Overview {
    pub installed: Vec<Installed>,
    pub conflicts: Vec<Conflict>,
    pub notes: Vec<String>,
}

/// Reads the packages in `root/Mods` with `load_order` as the order and everything else off. With `None`, every
/// package that can load is on, which is how new packages are found.
pub fn inspect(
    root: &Path,
    game_version: &str,
    load_order: Option<&[String]>,
) -> Result<Overview, String> {
    let mut config = ModsConfig::default();
    let mut builder = Vfs::builder(game_version)?.mods_dir(root.join("Mods"));
    let fixes = root.join("Fixes");
    if fixes.is_dir() {
        builder = builder.fixes(fixes);
    }
    if let Some(order) = load_order {
        let everything = Vfs::builder(game_version)?
            .mods_dir(root.join("Mods"))
            .build()
            .map_err(|error| error.to_string())?
            .1;
        config.order = order.to_vec();
        config.disabled = everything
            .mods
            .active
            .iter()
            .chain(&everything.mods.inactive)
            .map(|entry| entry.id.clone())
            .filter(|id| !order.iter().any(|name| fold(name) == fold(id)))
            .collect();
    }
    let (vfs, report) = builder
        .mods_settings(config)
        .build()
        .map_err(|error| error.to_string())?;
    let describe = |entry: &vfs::ModEntry, problem: Option<String>| Installed {
        id: entry.id.clone(),
        title: entry
            .manifest
            .as_ref()
            .map_or_else(|| entry.id.clone(), |m| m.title.clone()),
        version: entry
            .manifest
            .as_ref()
            .map(|m| m.version.to_string())
            .unwrap_or_default(),
        kind: entry
            .manifest
            .as_ref()
            .map(|m| m.kind.name().to_string())
            .unwrap_or_default(),
        author: entry
            .manifest
            .as_ref()
            .map(|m| m.author.clone())
            .unwrap_or_default(),
        problem,
    };
    let mut installed: Vec<Installed> = report
        .mods
        .active
        .iter()
        .map(|entry| describe(entry, None))
        .collect();
    for entry in &report.mods.inactive {
        match &entry.status {
            ModStatus::Error(error) => installed.push(describe(entry, Some(error.to_string()))),
            ModStatus::Disabled => installed.push(describe(entry, None)),
            ModStatus::Active => {}
        }
    }
    installed.sort_by_key(|item| fold(&item.id));
    let mut notes = report.mods.notes.clone();
    notes.extend(report.notes.iter().cloned());
    Ok(Overview {
        installed,
        conflicts: vfs.conflicts(false),
        notes,
    })
}

/// One line for a contested file: who wins and over whom.
pub fn conflict_line(conflict: &Conflict) -> String {
    let beaten: Vec<&str> = conflict.layers[..conflict.layers.len().saturating_sub(1)]
        .iter()
        .rev()
        .map(String::as_str)
        .collect();
    format!(
        "{}: {} wins over {}",
        conflict.path,
        conflict.winner(),
        beaten.join(", ")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModItem {
    pub id: String,
    pub title: String,
    pub version: String,
    pub kind: String,
    pub enabled: bool,
    pub problem: Option<String>,
}

/// The packages with the ones that load first, lowest priority first, then the ones that are off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModList {
    items: Vec<ModItem>,
}

impl ModList {
    pub fn new(installed: &[Installed], load_order: &[String]) -> Self {
        let mut items: Vec<ModItem> = Vec::new();
        for name in load_order {
            if items.iter().any(|item| fold(&item.id) == fold(name)) {
                continue;
            }
            items.push(
                match installed.iter().find(|pkg| fold(&pkg.id) == fold(name)) {
                    Some(pkg) => item_of(pkg, true),
                    None => ModItem {
                        id: name.clone(),
                        title: name.clone(),
                        version: String::new(),
                        kind: String::new(),
                        enabled: true,
                        problem: Some("not found in Mods".to_string()),
                    },
                },
            );
        }
        for pkg in installed {
            if !items.iter().any(|item| fold(&item.id) == fold(&pkg.id)) {
                items.push(item_of(pkg, false));
            }
        }
        Self { items }
    }

    pub fn items(&self) -> &[ModItem] {
        &self.items
    }

    fn enabled_count(&self) -> usize {
        self.items.iter().take_while(|item| item.enabled).count()
    }

    /// The names for `mods.toml`: the packages that load, lowest priority first.
    pub fn load_order(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.enabled)
            .map(|item| item.id.clone())
            .collect()
    }

    /// Turns a package on, as the highest priority, or off. A package that cannot load stays off. Returns where it
    /// is now.
    pub fn set_enabled(&mut self, index: usize, enabled: bool) -> Option<usize> {
        let item = self.items.get(index)?;
        if item.enabled == enabled || (enabled && item.problem.is_some()) {
            return Some(index);
        }
        let mut item = self.items.remove(index);
        item.enabled = enabled;
        if enabled {
            let at = self.enabled_count();
            self.items.insert(at, item);
            Some(at)
        } else if item.problem.as_deref() == Some("not found in Mods") {
            None
        } else {
            let at = self.enabled_count();
            self.items.insert(at, item);
            Some(at)
        }
    }

    /// Moves a package that loads one place lower in priority. Returns where it is now.
    pub fn move_up(&mut self, index: usize) -> Option<usize> {
        let item = self.items.get(index)?;
        if !item.enabled || index == 0 {
            return Some(index);
        }
        self.items.swap(index, index - 1);
        Some(index - 1)
    }

    /// Moves a package that loads one place higher in priority.
    pub fn move_down(&mut self, index: usize) -> Option<usize> {
        let item = self.items.get(index)?;
        if !item.enabled || index + 1 >= self.enabled_count() {
            return Some(index);
        }
        self.items.swap(index, index + 1);
        Some(index + 1)
    }
}

fn item_of(pkg: &Installed, enabled: bool) -> ModItem {
    ModItem {
        id: pkg.id.clone(),
        title: pkg.title.clone(),
        version: pkg.version.clone(),
        kind: pkg.kind.clone(),
        enabled,
        problem: pkg.problem.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn installed(id: &str) -> Installed {
        Installed {
            id: id.to_string(),
            title: id.to_uppercase(),
            version: "1.0".into(),
            kind: "assets".into(),
            author: "me".into(),
            problem: None,
        }
    }

    fn names(list: &ModList) -> Vec<(&str, bool)> {
        list.items()
            .iter()
            .map(|item| (item.id.as_str(), item.enabled))
            .collect()
    }

    fn list() -> ModList {
        let all = [
            installed("a"),
            installed("b"),
            installed("c"),
            installed("d"),
        ];
        ModList::new(&all, &["c".to_string(), "a".to_string()])
    }

    #[test]
    fn the_listed_packages_come_first_in_their_order_and_the_rest_are_off() {
        let list = list();
        assert_eq!(
            names(&list),
            [("c", true), ("a", true), ("b", false), ("d", false)]
        );
        assert_eq!(list.load_order(), ["c", "a"]);
    }

    #[test]
    fn names_match_without_regard_to_case_and_repeats_count_once() {
        let all = [installed("Better-Roads")];
        let list = ModList::new(&all, &["better-roads".into(), "BETTER-ROADS".into()]);
        assert_eq!(list.items().len(), 1);
        assert_eq!(list.items()[0].id, "Better-Roads");
        assert!(list.items()[0].enabled);
    }

    #[test]
    fn a_listed_package_that_is_gone_is_kept_until_it_is_turned_off() {
        let mut list = ModList::new(&[installed("a")], &["gone".into(), "a".into()]);
        assert_eq!(
            list.items()[0].problem.as_deref(),
            Some("not found in Mods")
        );
        assert_eq!(list.load_order(), ["gone", "a"]);
        assert_eq!(list.set_enabled(0, false), None);
        assert_eq!(list.load_order(), ["a"]);
        assert_eq!(list.items().len(), 1);
    }

    #[test]
    fn moving_changes_priority_only_among_the_packages_that_load() {
        let mut list = list();
        assert_eq!(list.move_down(0), Some(1));
        assert_eq!(list.load_order(), ["a", "c"]);
        assert_eq!(
            list.move_down(1),
            Some(1),
            "the last one that loads stays put"
        );
        assert_eq!(list.move_up(0), Some(0));
        assert_eq!(list.move_up(1), Some(0));
        assert_eq!(list.load_order(), ["c", "a"]);
        assert_eq!(
            list.move_up(2),
            Some(2),
            "a package that is off does not move"
        );
        assert_eq!(list.move_down(2), Some(2));
        assert_eq!(list.move_down(9), None);
    }

    #[test]
    fn turning_a_package_on_makes_it_the_highest_priority() {
        let mut list = list();
        assert_eq!(list.set_enabled(3, true), Some(2));
        assert_eq!(list.load_order(), ["c", "a", "d"]);
        assert_eq!(names(&list)[3], ("b", false));
    }

    #[test]
    fn turning_a_package_off_removes_it_from_the_order_and_keeps_the_rest() {
        let mut list = list();
        assert_eq!(list.set_enabled(0, false), Some(1));
        assert_eq!(list.load_order(), ["a"]);
        assert_eq!(
            names(&list),
            [("a", true), ("c", false), ("b", false), ("d", false)]
        );
        assert_eq!(list.set_enabled(1, false), Some(1), "already off");
    }

    #[test]
    fn a_package_that_cannot_load_cannot_be_turned_on() {
        let mut broken = installed("x");
        broken.problem = Some("manifest.json is missing".into());
        let mut list = ModList::new(&[installed("a"), broken], &[]);
        assert_eq!(list.set_enabled(1, true), Some(1));
        assert!(list.load_order().is_empty());
    }

    fn package(root: &Path, name: &str, files: &[&str]) {
        let dir = root.join("Mods").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("manifest.json"),
            format!(r#"{{"title":"{name} pack","author":"me","version":"1.0","type":"assets"}}"#),
        )
        .unwrap();
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
        }
    }

    #[test]
    fn packages_on_disk_are_found_with_their_problems() {
        let dir = tempfile::tempdir().unwrap();
        package(dir.path(), "roads", &["Textures/road.dds"]);
        package(dir.path(), "radio", &["Audio/radio.bin"]);
        let broken = dir.path().join("Mods").join("broken");
        fs::create_dir_all(&broken).unwrap();
        let overview = inspect(dir.path(), "1.0.2.1", None).unwrap();
        let ids: Vec<&str> = overview.installed.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["broken", "radio", "roads"]);
        assert!(overview.installed[0].problem.is_some());
        assert_eq!(overview.installed[1].title, "radio pack");
        assert_eq!(overview.installed[1].kind, "assets");
        assert!(overview.conflicts.is_empty());
    }

    #[test]
    fn a_missing_mods_folder_is_no_packages() {
        let dir = tempfile::tempdir().unwrap();
        let overview = inspect(dir.path(), "1.0.2.1", None).unwrap();
        assert!(overview.installed.is_empty());
        assert!(inspect(dir.path(), "not a version", None).is_err());
    }

    #[test]
    fn contested_files_name_the_winner_by_the_order_chosen() {
        let dir = tempfile::tempdir().unwrap();
        package(
            dir.path(),
            "a",
            &["Textures/shared.dds", "Textures/only_a.dds"],
        );
        package(dir.path(), "b", &["Textures/shared.dds"]);
        package(dir.path(), "c", &["Textures/shared.dds"]);
        let order = ["a".to_string(), "b".to_string()];
        let overview = inspect(dir.path(), "1.0.2.1", Some(&order)).unwrap();
        assert_eq!(
            overview.conflicts.len(),
            1,
            "c is off, so it contests nothing"
        );
        assert_eq!(overview.conflicts[0].winner(), "b");
        assert_eq!(
            conflict_line(&overview.conflicts[0]),
            "Textures/shared.dds: b wins over a"
        );
        let flipped = ["b".to_string(), "a".to_string()];
        let overview = inspect(dir.path(), "1.0.2.1", Some(&flipped)).unwrap();
        assert_eq!(overview.conflicts[0].winner(), "a");
        let none = inspect(dir.path(), "1.0.2.1", Some(&[])).unwrap();
        assert!(none.conflicts.is_empty());
    }
}
