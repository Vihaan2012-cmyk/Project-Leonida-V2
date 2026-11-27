//! The virtual tree: layers merged into one case-insensitive view.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::filter::{ContentFilter, Verdict};
use crate::order::{ConfigError, ModSet, ModStatus, ModsConfig, Provided};
use crate::package::{Package, PackageError, Version};
use crate::path::{PathError, VPath};
use crate::source::{LooseSource, ReadSeek, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    Base,
    Fixes,
    Mod,
}

struct Layer {
    name: String,
    kind: LayerKind,
    source: Box<dyn Source>,
    files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerInfo {
    pub name: String,
    pub kind: LayerKind,
    pub files: usize,
}

#[derive(Debug, Clone)]
struct Provider {
    layer: usize,
    path: String,
    size: u64,
}

struct Node {
    providers: Vec<Provider>,
}

impl Node {
    fn winner(&self) -> &Provider {
        self.providers.last().expect("a node has a provider")
    }
}

#[derive(Debug, Clone)]
struct Child {
    name: String,
    is_dir: bool,
}

/// One file as the tree serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// The virtual path, in the spelling of the layer that wins.
    pub path: String,
    pub size: u64,
    pub layer: usize,
    pub layer_name: String,
    /// The name inside the winning layer's source, which `Vfs::read_source` and `Vfs::host_path` take.
    pub source_path: String,
    /// True when a filter rule swapped the content for another path's.
    pub replaced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// Zero for folders.
    pub size: u64,
}

/// A path more than one layer provides. `layers` runs lowest first, so the last one is the winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    pub layers: Vec<String>,
}

impl Conflict {
    pub fn winner(&self) -> &str {
        self.layers.last().map_or("", String::as_str)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("`{0}` is not in the tree")]
    NotFound(String),
    #[error("reading `{path}` from {layer}: {source}")]
    Read {
        path: String,
        layer: String,
        source: io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("could not scan the packages folder: {0}")]
    ScanMods(io::Error),
    #[error("the Leonida fixes package ({path}) cannot be used: {error}")]
    Fixes { path: PathBuf, error: PackageError },
    #[error("could not read the files of {layer}: {source}")]
    Scan { layer: String, source: io::Error },
    #[error("package {package} cannot be loaded: {error}")]
    Package {
        package: String,
        error: PackageError,
    },
}

/// What loading found, for printing.
#[derive(Debug, Clone, Default)]
pub struct LoadReport {
    pub mods: ModSet,
    pub notes: Vec<String>,
}

impl LoadReport {
    /// One line per package that did not load, naming the package and why.
    pub fn problems(&self) -> Vec<String> {
        self.mods
            .inactive
            .iter()
            .filter_map(|e| match &e.status {
                ModStatus::Error(err) => Some(format!("{}: {err}", e.id)),
                _ => None,
            })
            .collect()
    }
}

pub struct VfsBuilder {
    game_version: Version,
    base: Option<Box<dyn Source>>,
    base_name: String,
    fixes: Option<PathBuf>,
    mods_dir: Option<PathBuf>,
    mods_config: Option<PathBuf>,
    config: Option<ModsConfig>,
    extra: Vec<(String, LayerKind, Box<dyn Source>)>,
    filter: ContentFilter,
    strict: bool,
}

impl VfsBuilder {
    /// The base game's files: a loose folder now, an `rpf9` index later.
    pub fn base(mut self, source: impl Source + 'static) -> Self {
        self.base = Some(Box::new(source));
        self
    }

    /// Leonida's own fixes: a package folder that loads above the base and below every mod.
    pub fn fixes(mut self, folder: impl Into<PathBuf>) -> Self {
        self.fixes = Some(folder.into());
        self
    }

    pub fn mods_dir(mut self, folder: impl Into<PathBuf>) -> Self {
        self.mods_dir = Some(folder.into());
        self
    }

    /// `Config/mods.toml`; a missing file means the default order.
    pub fn mods_config(mut self, file: impl Into<PathBuf>) -> Self {
        self.mods_config = Some(file.into());
        self
    }

    pub fn mods_settings(mut self, config: ModsConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// A layer above the mods that is not a package, such as generated content.
    pub fn extra_layer(mut self, name: &str, source: impl Source + 'static) -> Self {
        self.extra
            .push((name.to_string(), LayerKind::Mod, Box::new(source)));
        self
    }

    pub fn filter(mut self, filter: ContentFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Fail the build on the first package that cannot load instead of skipping it.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    pub fn build(self) -> Result<(Vfs, LoadReport), BuildError> {
        let mut report = LoadReport::default();
        let mut layers: Vec<Layer> = Vec::new();
        if let Some(source) = self.base {
            layers.push(Layer {
                name: self.base_name,
                kind: LayerKind::Base,
                source,
                files: 0,
            });
        }
        let mut provided = Vec::new();
        if let Some(folder) = &self.fixes {
            let pkg = Package::load(folder).map_err(|error| BuildError::Fixes {
                path: folder.clone(),
                error,
            })?;
            pkg.manifest
                .check_game_version(&self.game_version)
                .map_err(|error| BuildError::Fixes {
                    path: folder.clone(),
                    error,
                })?;
            provided.push(Provided {
                id: pkg.id.clone(),
                title: pkg.manifest.title.clone(),
                version: pkg.manifest.version.clone(),
            });
            layers.push(Layer {
                name: format!("fixes ({})", pkg.id),
                kind: LayerKind::Fixes,
                source: Box::new(LooseSource::package(&pkg.root)),
                files: 0,
            });
        }
        if let Some(dir) = &self.mods_dir {
            let config = match (&self.config, &self.mods_config) {
                (Some(config), _) => config.clone(),
                (None, Some(path)) => ModsConfig::load(path)?,
                (None, None) => ModsConfig::default(),
            };
            let set = ModSet::discover(dir, &config, &self.game_version, &provided)
                .map_err(BuildError::ScanMods)?;
            if self.strict
                && let Some(bad) = set
                    .inactive
                    .iter()
                    .find(|e| matches!(e.status, ModStatus::Error(_)))
                && let ModStatus::Error(error) = &bad.status
            {
                return Err(BuildError::Package {
                    package: bad.id.clone(),
                    error: error.clone(),
                });
            }
            for entry in &set.active {
                layers.push(Layer {
                    name: entry.id.clone(),
                    kind: LayerKind::Mod,
                    source: Box::new(LooseSource::package(&entry.root)),
                    files: 0,
                });
            }
            report.mods = set;
        }
        for (name, kind, source) in self.extra {
            layers.push(Layer {
                name,
                kind,
                source,
                files: 0,
            });
        }
        let vfs = Vfs::from_layers(layers, self.filter, &mut report.notes)?;
        Ok((vfs, report))
    }
}

type HashKey = (usize, String);

pub struct Vfs {
    layers: Vec<Layer>,
    files: HashMap<String, Node>,
    dirs: HashMap<String, BTreeMap<String, Child>>,
    filter: ContentFilter,
    hashes: Mutex<HashMap<HashKey, Option<[u8; 32]>>>,
}

impl Vfs {
    pub fn builder(game_version: &str) -> Result<VfsBuilder, String> {
        let game_version = Version::parse(game_version)
            .ok_or_else(|| format!("`{game_version}` is not a game version"))?;
        Ok(VfsBuilder {
            game_version,
            base: None,
            base_name: "base game".to_string(),
            fixes: None,
            mods_dir: None,
            mods_config: None,
            config: None,
            extra: Vec::new(),
            filter: ContentFilter::new(),
            strict: false,
        })
    }

    fn from_layers(
        mut layers: Vec<Layer>,
        filter: ContentFilter,
        notes: &mut Vec<String>,
    ) -> Result<Vfs, BuildError> {
        let mut files: HashMap<String, Node> = HashMap::new();
        for (index, layer) in layers.iter_mut().enumerate() {
            let listed = layer.source.scan().map_err(|source| BuildError::Scan {
                layer: layer.name.clone(),
                source,
            })?;
            for file in listed {
                match VPath::parse(&file.path) {
                    Ok(path) if !path.is_root() => {
                        layer.files += 1;
                        files
                            .entry(path.key().to_string())
                            .or_insert_with(|| Node {
                                providers: Vec::new(),
                            })
                            .providers
                            .push(Provider {
                                layer: index,
                                path: file.path,
                                size: file.size,
                            });
                    }
                    Ok(_) => {}
                    Err(error) => notes.push(format!("{}: skipped a file: {error}", layer.name)),
                }
            }
        }
        let mut dirs: HashMap<String, BTreeMap<String, Child>> = HashMap::new();
        let mut sorted: Vec<(&String, &Node)> = files.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (_, node) in sorted {
            let winner = node.winner();
            let clean = VPath::parse(&winner.path).expect("checked while scanning");
            let display: Vec<&str> = clean.parts().collect();
            let mut key = String::new();
            for (depth, part) in display.iter().enumerate() {
                let child_key = crate::path::fold(part);
                let is_dir = depth + 1 < display.len();
                dirs.entry(key.clone())
                    .or_default()
                    .entry(child_key.clone())
                    .or_insert_with(|| Child {
                        name: (*part).to_string(),
                        is_dir,
                    });
                if !key.is_empty() {
                    key.push('/');
                }
                key.push_str(&child_key);
            }
        }
        Ok(Vfs {
            layers,
            files,
            dirs,
            filter,
            hashes: Mutex::new(HashMap::new()),
        })
    }

    /// Layers lowest priority first.
    pub fn layers(&self) -> Vec<LayerInfo> {
        self.layers
            .iter()
            .map(|l| LayerInfo {
                name: l.name.clone(),
                kind: l.kind,
                files: l.files,
            })
            .collect()
    }

    pub fn filter(&self) -> &ContentFilter {
        &self.filter
    }

    pub fn stat(&self, path: &str) -> Result<Option<FileInfo>, PathError> {
        Ok(self.stat_path(&VPath::parse(path)?))
    }

    pub fn stat_path(&self, path: &VPath) -> Option<FileInfo> {
        let node = self.files.get(path.key())?;
        let winner = node.winner();
        if self.filter.is_empty() {
            return Some(self.info(path, winner, false));
        }
        let mut hash = || self.hash_of(winner);
        match self.filter.verdict(path, winner.size, &mut hash) {
            Verdict::Keep => Some(self.info(path, winner, false)),
            Verdict::Hide(_) => None,
            Verdict::Replace(_, with) => {
                let node = self.files.get(with.key())?;
                Some(self.info(path, node.winner(), true))
            }
        }
    }

    fn info(&self, path: &VPath, provider: &Provider, replaced: bool) -> FileInfo {
        let own = self.files.get(path.key()).map(|n| n.winner().path.clone());
        FileInfo {
            path: own.unwrap_or_else(|| path.as_str().to_string()),
            size: provider.size,
            layer: provider.layer,
            layer_name: self.layers[provider.layer].name.clone(),
            source_path: provider.path.clone(),
            replaced,
        }
    }

    fn hash_of(&self, provider: &Provider) -> Option<[u8; 32]> {
        let key = (provider.layer, provider.path.clone());
        if let Some(found) = self.hashes.lock().ok()?.get(&key) {
            return *found;
        }
        let mut reader = self.layers[provider.layer].source.open(&provider.path).ok();
        let digest = reader.as_mut().and_then(|r| {
            let mut hasher = Sha256::new();
            io::copy(r, &mut hasher).ok()?;
            Some(<[u8; 32]>::from(hasher.finalize()))
        });
        if let Ok(mut cache) = self.hashes.lock() {
            cache.insert(key, digest);
        }
        digest
    }

    pub fn exists(&self, path: &str) -> Result<bool, PathError> {
        let path = VPath::parse(path)?;
        Ok(self.stat_path(&path).is_some() || self.is_dir_path(&path))
    }

    pub fn is_dir(&self, path: &str) -> Result<bool, PathError> {
        Ok(self.is_dir_path(&VPath::parse(path)?))
    }

    pub fn is_dir_path(&self, path: &VPath) -> bool {
        if !self.dirs.contains_key(path.key()) {
            return false;
        }
        if self.filter.is_empty() {
            return true;
        }
        self.has_visible_file(path)
    }

    fn has_visible_file(&self, dir: &VPath) -> bool {
        let Some(children) = self.dirs.get(dir.key()) else {
            return false;
        };
        children.values().any(|child| match dir.join(&child.name) {
            Ok(path) if child.is_dir => self.has_visible_file(&path),
            Ok(path) => self.stat_path(&path).is_some(),
            Err(_) => false,
        })
    }

    /// The entries directly in a folder, sorted by name, with filtered files left out.
    pub fn list(&self, dir: &str) -> Result<Vec<DirEntry>, PathError> {
        self.list_path(&VPath::parse(dir)?)
    }

    pub fn list_path(&self, dir: &VPath) -> Result<Vec<DirEntry>, PathError> {
        let Some(children) = self.dirs.get(dir.key()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for child in children.values() {
            let path = dir.join(&child.name)?;
            if child.is_dir {
                if self.is_dir_path(&path) {
                    out.push(DirEntry {
                        name: child.name.clone(),
                        is_dir: true,
                        size: 0,
                    });
                }
            } else if let Some(info) = self.stat_path(&path) {
                out.push(DirEntry {
                    name: child.name.clone(),
                    is_dir: false,
                    size: info.size,
                });
            }
        }
        Ok(out)
    }

    /// Every visible file, sorted by path.
    pub fn files(&self) -> Vec<FileInfo> {
        let mut keys: Vec<&String> = self.files.keys().collect();
        keys.sort();
        keys.into_iter()
            .filter_map(|key| {
                let path = VPath::parse(&self.files[key].winner().path).ok()?;
                self.stat_path(&path)
            })
            .collect()
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let info = self
            .stat(path)?
            .ok_or_else(|| VfsError::NotFound(path.to_string()))?;
        self.read_source(&info)
    }

    pub fn read_source(&self, info: &FileInfo) -> Result<Vec<u8>, VfsError> {
        self.layers[info.layer]
            .source
            .read(&info.source_path)
            .map_err(|source| VfsError::Read {
                path: info.path.clone(),
                layer: info.layer_name.clone(),
                source,
            })
    }

    pub fn open(&self, info: &FileInfo) -> io::Result<Box<dyn ReadSeek>> {
        self.layers[info.layer].source.open(&info.source_path)
    }

    /// The real file behind `info` when its layer is a folder on disk.
    pub fn host_path(&self, info: &FileInfo) -> Option<PathBuf> {
        self.layers[info.layer].source.host_path(&info.source_path)
    }

    /// Every path two or more layers provide, sorted. With `include_base` false the base game is not counted as a
    /// contender, which lists only contests between Leonida's fixes and mods.
    pub fn conflicts(&self, include_base: bool) -> Vec<Conflict> {
        let mut out: Vec<Conflict> = self
            .files
            .values()
            .filter_map(|node| {
                let layers: Vec<String> = node
                    .providers
                    .iter()
                    .filter(|p| include_base || self.layers[p.layer].kind != LayerKind::Base)
                    .map(|p| self.layers[p.layer].name.clone())
                    .collect();
                (layers.len() > 1).then(|| Conflict {
                    path: node.winner().path.clone(),
                    layers,
                })
            })
            .collect();
        out.sort_by_key(|c| crate::path::fold(&c.path));
        out
    }

    /// Where the winning copy of `path` lives, for messages.
    pub fn winner_of(&self, path: &str) -> Result<Option<String>, PathError> {
        Ok(self.stat(path)?.map(|i| i.layer_name))
    }
}
