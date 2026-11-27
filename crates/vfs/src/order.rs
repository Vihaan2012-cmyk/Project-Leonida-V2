//! Finding the packages in `Mods/` and putting them in load order.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::package::{Manifest, Package, PackageError, PackageType, Version};
use crate::path::fold;

/// `Config/mods.toml`:
///
/// ```toml
/// order = ["first-mod", "second-mod"]  # folder names, lowest priority first
/// disabled = ["old-mod"]
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModsConfig {
    #[serde(default, alias = "load_order")]
    pub order: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid: {message}")]
    Parse { path: PathBuf, message: String },
}

impl ModsConfig {
    pub fn from_toml(text: &str) -> Result<ModsConfig, String> {
        toml::from_str(text).map_err(|e| e.message().to_string())
    }

    /// A missing file is the default configuration: everything on, in default order.
    pub fn load(path: &Path) -> Result<ModsConfig, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ModsConfig::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        ModsConfig::from_toml(&text).map_err(|message| ConfigError::Parse {
            path: path.to_path_buf(),
            message,
        })
    }

    fn is_disabled(&self, id: &str) -> bool {
        self.disabled.iter().any(|d| fold(d) == fold(id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModStatus {
    Active,
    Disabled,
    Error(PackageError),
}

#[derive(Debug, Clone)]
pub struct ModEntry {
    pub id: String,
    pub root: PathBuf,
    /// Absent when the manifest could not be read.
    pub manifest: Option<Manifest>,
    pub status: ModStatus,
}

/// A package that is already present below `Mods/`, such as Leonida's own fixes, which others may depend on.
#[derive(Debug, Clone)]
pub struct Provided {
    pub id: String,
    pub title: String,
    pub version: Version,
}

#[derive(Debug, Clone, Default)]
pub struct ModSet {
    /// Packages that will load, lowest priority first.
    pub active: Vec<ModEntry>,
    /// Disabled packages and packages with a problem, by name.
    pub inactive: Vec<ModEntry>,
    pub notes: Vec<String>,
}

fn rank(kind: PackageType) -> u8 {
    match kind {
        PackageType::Assets => 0,
        PackageType::Shaders => 1,
        PackageType::Systems => 2,
        PackageType::Mixed => 3,
        PackageType::Patch => 4,
    }
}

impl ModSet {
    /// Reads every folder in `mods_dir` (missing means none), checks game version and dependencies, and orders
    /// the rest: packages not named in `config.order` first, by type (assets, shaders, systems, mixed, patch)
    /// then name, then the named ones in the order given, so an explicit choice always wins over the default.
    /// A package never loads below one it depends on.
    pub fn discover(
        mods_dir: &Path,
        config: &ModsConfig,
        game: &Version,
        provided: &[Provided],
    ) -> std::io::Result<ModSet> {
        let mut set = ModSet::default();
        let mut folders: Vec<PathBuf> = Vec::new();
        match std::fs::read_dir(mods_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let kind = entry.file_type()?;
                    if kind.is_dir() && !kind.is_symlink() && !name.starts_with('.') {
                        folders.push(entry.path());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        folders.sort_by_key(|p| fold(&p.file_name().unwrap_or_default().to_string_lossy()));

        let mut candidates: Vec<Package> = Vec::new();
        for root in folders {
            match Package::load(&root) {
                Ok(pkg) => {
                    if config.is_disabled(&pkg.id) {
                        set.inactive.push(entry_of(&pkg, ModStatus::Disabled));
                    } else if let Err(e) = pkg.manifest.check_game_version(game) {
                        set.inactive.push(entry_of(&pkg, ModStatus::Error(e)));
                    } else {
                        candidates.push(pkg);
                    }
                }
                Err(e) => set.inactive.push(ModEntry {
                    id: root
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    root,
                    manifest: None,
                    status: ModStatus::Error(e),
                }),
            }
        }

        let mut order = default_then_listed(candidates, config, &mut set.notes);
        resolve_dependencies(&mut order, &mut set, provided);
        let order = dependencies_first(order, provided, &mut set.notes);
        set.active = order
            .iter()
            .map(|pkg| entry_of(pkg, ModStatus::Active))
            .collect();
        set.inactive.sort_by_key(|e| fold(&e.id));
        Ok(set)
    }

    pub fn find(&self, id: &str) -> Option<&ModEntry> {
        self.active
            .iter()
            .chain(&self.inactive)
            .find(|e| fold(&e.id) == fold(id))
    }
}

fn entry_of(pkg: &Package, status: ModStatus) -> ModEntry {
    ModEntry {
        id: pkg.id.clone(),
        root: pkg.root.clone(),
        manifest: Some(pkg.manifest.clone()),
        status,
    }
}

fn default_then_listed(
    mut candidates: Vec<Package>,
    config: &ModsConfig,
    notes: &mut Vec<String>,
) -> Vec<Package> {
    candidates.sort_by_key(|p| (rank(p.manifest.kind), fold(&p.id)));
    let mut listed: Vec<Package> = Vec::new();
    for name in &config.order {
        if listed.iter().any(|p| fold(&p.id) == fold(name)) {
            notes.push(format!(
                "mods.toml lists `{name}` more than once; the first place counts"
            ));
        } else if let Some(at) = candidates.iter().position(|p| fold(&p.id) == fold(name)) {
            listed.push(candidates.remove(at));
        } else if !config.is_disabled(name) {
            notes.push(format!(
                "mods.toml lists `{name}`, which is not installed or cannot load"
            ));
        }
    }
    candidates.extend(listed);
    candidates
}

enum Target<'a> {
    Loaded(&'a Version),
    Gone(PackageError),
}

fn resolve_dependencies(order: &mut Vec<Package>, set: &mut ModSet, provided: &[Provided]) {
    loop {
        let mut failed: Vec<(usize, PackageError)> = Vec::new();
        for (index, pkg) in order.iter().enumerate() {
            for dep in &pkg.manifest.dependencies {
                let key = fold(&dep.name);
                let target = order
                    .iter()
                    .find(|p| fold(&p.id) == key || fold(&p.manifest.title) == key)
                    .map(|p| Target::Loaded(&p.manifest.version))
                    .or_else(|| {
                        provided
                            .iter()
                            .find(|p| fold(&p.id) == key || fold(&p.title) == key)
                            .map(|p| Target::Loaded(&p.version))
                    })
                    .unwrap_or_else(|| {
                        Target::Gone(match set.find(&dep.name) {
                            Some(e) if e.status == ModStatus::Disabled => {
                                PackageError::DisabledDependency {
                                    dependency: dep.name.clone(),
                                }
                            }
                            Some(_) => PackageError::BrokenDependency {
                                dependency: dep.name.clone(),
                            },
                            None => PackageError::MissingDependency {
                                dependency: dep.name.clone(),
                            },
                        })
                    });
                let problem = match target {
                    Target::Gone(e) => Some(e),
                    Target::Loaded(found) => {
                        dep.minimum
                            .as_ref()
                            .filter(|min| *found < **min)
                            .map(|min| PackageError::DependencyVersion {
                                dependency: dep.name.clone(),
                                wanted: min.to_string(),
                                found: found.to_string(),
                            })
                    }
                };
                if let Some(error) = problem {
                    failed.push((index, error));
                    break;
                }
            }
        }
        if failed.is_empty() {
            failed = find_cycles(order);
        }
        if failed.is_empty() {
            return;
        }
        for (index, error) in failed.into_iter().rev() {
            let pkg = order.remove(index);
            set.inactive.push(entry_of(&pkg, ModStatus::Error(error)));
        }
    }
}

fn find_cycles(order: &[Package]) -> Vec<(usize, PackageError)> {
    let index_of = |name: &str| {
        let key = fold(name);
        order
            .iter()
            .position(|p| fold(&p.id) == key || fold(&p.manifest.title) == key)
    };
    let edges: Vec<Vec<usize>> = order
        .iter()
        .map(|p| {
            p.manifest
                .dependencies
                .iter()
                .filter_map(|d| index_of(&d.name))
                .collect()
        })
        .collect();
    let mut found = Vec::new();
    for start in 0..order.len() {
        let mut stack = vec![(start, 0usize)];
        let mut path = vec![start];
        while let Some(&mut (node, ref mut next)) = stack.last_mut() {
            if *next < edges[node].len() {
                let to = edges[node][*next];
                *next += 1;
                if to == start {
                    let names: Vec<&str> = path.iter().map(|&i| order[i].id.as_str()).collect();
                    let text = format!("{} -> {}", names.join(" -> "), order[start].id);
                    found.push((start, PackageError::Cycle(text)));
                    break;
                }
                if !path.contains(&to) {
                    path.push(to);
                    stack.push((to, 0));
                }
            } else {
                stack.pop();
                path.pop();
            }
        }
    }
    found
}

fn dependencies_first(
    order: Vec<Package>,
    provided: &[Provided],
    notes: &mut Vec<String>,
) -> Vec<Package> {
    let mut remaining = order;
    let mut placed: Vec<Package> = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining.iter().position(|pkg| {
            pkg.manifest.dependencies.iter().all(|dep| {
                let key = fold(&dep.name);
                provided
                    .iter()
                    .any(|p| fold(&p.id) == key || fold(&p.title) == key)
                    || placed
                        .iter()
                        .any(|p| fold(&p.id) == key || fold(&p.manifest.title) == key)
            })
        });
        let at = ready.unwrap_or(0);
        let pkg = remaining.remove(at);
        if at != 0 {
            notes.push(format!(
                "`{}` loads before `{}` so that what depends on it comes after it",
                pkg.id, remaining[0].id
            ));
        }
        placed.push(pkg);
    }
    placed
}
