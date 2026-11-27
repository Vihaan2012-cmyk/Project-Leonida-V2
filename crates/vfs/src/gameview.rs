//! The running game's view of the tree: how its file calls become tree paths, and where its writes go.
//!
//! The file shims call [`GameView::resolve`] with the path string the game passed and get back where to read from
//! and where a write must land. The shims keep the Win32 error codes; this module keeps the rules.
//!
//! # What is measured and what is assumed
//!
//! The game executable has never been run or loaded in this work, so no path the game really passes has been
//! observed. Everything here comes from the upstream notes (`runtime_imports.md`, "Console paths and the write
//! overlay", which itself cites the Xbox GDK documentation) and from the boot notes (`boot.md`, `saves.md`):
//!
//! * `GAME:` (any case, either slash) is the game folder: documented for GDK titles. Used as is.
//! * `G:` is the same folder and `T:` a scratch folder that `GAME:` does not see: ASSUMED from console convention
//!   (upstream says only the executable can tell which spelling the title uses). [`Assumption::GameDrive`] and
//!   [`Assumption::TempDrive`] mark these so a shim can trace them. `D:` is NOT mapped: on a PC it is an ordinary
//!   host drive, and the notes name no console meaning for it. [`DriveMap::with`] can add it if a trace shows it.
//! * An absolute host path inside the game folder, the overlay, the temp or persistent folders or `Output/` is the
//!   same place as the console name for it: ASSUMED, as the title may build such paths from its own module path.
//!   Inside the game folder it goes through the tree, so mods apply.
//! * A path with no drive is relative to the game folder: ASSUMED (the process's working directory is not known).
//! * `common:/`, `platform:/` and `pack:/` are the engine's device names for archive-internal files (they appear in
//!   its data files: measured in `gamedata.md`). The OS never sees them, but the file layer under it may ask; they
//!   map to `common/`, `platform/` and `pack/` in the tree: ASSUMED, the `rpf9` source decides the real names.
//!   `$(common)` is the same as `common:/`; `$(target)` and other tokens are measured in the data but their
//!   expansion is not, so they fail with [`ResolveError::UnmeasuredToken`].
//! * Save data is not reached through file paths: it goes through `XGameSave*` calls (`saves.md`). The layout
//!   under `Output/saves` is in [`WritePolicy::save_blob`], and the container and blob names are not measured.
//! * Anything else on the PC is read as it is and written to `Output/runtime/host/<drive>/...` instead.
//! * `tcpip:`, shared folders and web addresses are refused through `offline::path`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::path::{PathError, VPath, fold};
use crate::tree::{DirEntry, FileInfo, Vfs};
use crate::write::{WriteError, WritePolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// The game folder: the tree, with the game's own writes on top.
    Game,
    Temp,
    Persistent,
    /// A path already inside `Output/`.
    Output,
    /// Some other place on the PC.
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Missing,
    File,
    Dir,
}

/// Which unmeasured rule produced a mapping. Shims should trace these once each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assumption {
    GameDrive,
    TempDrive,
    RelativeToGameFolder,
    DeviceName,
    HostPathInsideKnownFolder,
    HostRedirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFrom {
    /// A real file: the game's own earlier write, or a loose file of a layer.
    File(PathBuf),
    /// A file only the tree can read, such as an archive member: use [`GameView::read`] or [`GameView::open`].
    Tree(FileInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub area: Area,
    /// The path inside the area, cleaned.
    pub path: VPath,
    pub kind: EntryKind,
    /// None when nothing is there, or the thing there is a folder.
    pub read_from: Option<ReadFrom>,
    /// Where a write to this name lands. Always inside `Output/`.
    pub write_to: PathBuf,
    /// True when `read_from` is a file the game itself wrote.
    pub in_overlay: bool,
    pub assumed: Option<Assumption>,
    pub original: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("`{path}` is refused: it would reach {}", .reason.describe())]
    Refused {
        path: String,
        reason: offline::Reason,
    },
    #[error(transparent)]
    Path(PathError),
    #[error("`{0}` climbs above the game folder")]
    OutsideRoot(String),
    #[error("`{0}` names a device Leonida does not know")]
    UnknownDevice(String),
    #[error("`{0}` is a path token whose meaning has not been measured")]
    UnmeasuredToken(String),
}

impl From<PathError> for ResolveError {
    fn from(error: PathError) -> Self {
        match error {
            PathError::Refused { path, reason } => ResolveError::Refused { path, reason },
            PathError::Escapes(path) => ResolveError::OutsideRoot(path),
            other => ResolveError::Path(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// The file starts empty.
    Truncate,
    /// The file keeps its bytes, so a package file is copied into the overlay first.
    KeepContents,
}

#[derive(Debug, Clone)]
pub struct DriveMap {
    drives: Vec<(String, Area, Assumption)>,
}

impl Default for DriveMap {
    fn default() -> Self {
        let mut map = DriveMap { drives: Vec::new() };
        map.drives
            .push(("g".into(), Area::Game, Assumption::GameDrive));
        map.drives
            .push(("t".into(), Area::Temp, Assumption::TempDrive));
        map
    }
}

impl DriveMap {
    /// Adds a drive letter (without the colon) that the game uses for one of the areas.
    pub fn with(mut self, letter: char, area: Area) -> Self {
        let name = fold(&letter.to_string());
        self.drives.retain(|(n, _, _)| *n != name);
        let assumed = match area {
            Area::Temp => Assumption::TempDrive,
            Area::Game => Assumption::GameDrive,
            _ => Assumption::HostRedirect,
        };
        self.drives.push((name, area, assumed));
        self
    }

    fn get(&self, letter: &str) -> Option<(Area, Assumption)> {
        self.drives
            .iter()
            .find(|(n, _, _)| n == letter)
            .map(|(_, area, assumed)| (*area, *assumed))
    }
}

const DEVICES: &[(&str, &str)] = &[
    ("common", "common"),
    ("platform", "platform"),
    ("pack", "pack"),
];

struct Located {
    area: Area,
    rel: String,
    drive: Option<char>,
    assumed: Option<Assumption>,
}

pub struct GameView {
    vfs: Arc<Vfs>,
    policy: WritePolicy,
    game_dir: PathBuf,
    drives: DriveMap,
}

impl GameView {
    /// `game_dir` is the package folder on disk, used to recognise absolute paths into it.
    pub fn new(vfs: Arc<Vfs>, game_dir: impl AsRef<Path>, policy: WritePolicy) -> GameView {
        let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        GameView {
            vfs,
            game_dir: absolute(game_dir.as_ref()),
            policy: WritePolicy::new(absolute(policy.output())),
            drives: DriveMap::default(),
        }
    }

    pub fn with_drives(mut self, drives: DriveMap) -> Self {
        self.drives = drives;
        self
    }

    pub fn policy(&self) -> &WritePolicy {
        &self.policy
    }

    pub fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    /// The folder `XPersistentLocalStorage`-style requests are served from.
    pub fn persistent_dir(&self) -> PathBuf {
        self.policy.persistent_dir()
    }

    fn locate(&self, raw: &str) -> Result<Located, ResolveError> {
        if let Some(reason) = offline::path(raw) {
            return Err(ResolveError::Refused {
                path: raw.to_string(),
                reason,
            });
        }
        let mut text = raw.trim();
        for prefix in ["\\\\?\\", "\\\\.\\", "\\??\\"] {
            if text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix) {
                text = &text[prefix.len()..];
                break;
            }
        }
        if let Some(token) = text.strip_prefix("$(") {
            let (name, rest) = token
                .split_once(')')
                .ok_or_else(|| ResolveError::UnmeasuredToken(raw.to_string()))?;
            return match DEVICES.iter().find(|(d, _)| *d == fold(name)) {
                Some((_, prefix)) if *prefix == "common" => Ok(Located {
                    area: Area::Game,
                    rel: format!("{prefix}/{rest}"),
                    drive: None,
                    assumed: Some(Assumption::DeviceName),
                }),
                _ => Err(ResolveError::UnmeasuredToken(format!("$({name})"))),
            };
        }
        let first_separator = text.find(['/', '\\']).unwrap_or(text.len());
        let Some(colon) = text[..first_separator].find(':') else {
            return Ok(Located {
                area: Area::Game,
                rel: text.to_string(),
                drive: None,
                assumed: Some(Assumption::RelativeToGameFolder),
            });
        };
        let (name, rest) = (&text[..colon], &text[colon + 1..]);
        let lower = fold(name);
        if lower == "game" {
            return Ok(Located {
                area: Area::Game,
                rel: rest.to_string(),
                drive: None,
                assumed: None,
            });
        }
        if name.chars().count() == 1 && name.chars().all(|c| c.is_ascii_alphabetic()) {
            if let Some(found) = self.host_folder(text) {
                return Ok(found);
            }
            if let Some((area, assumed)) = self.drives.get(&lower) {
                return Ok(Located {
                    area,
                    rel: rest.to_string(),
                    drive: None,
                    assumed: Some(assumed),
                });
            }
            return Ok(Located {
                area: Area::Host,
                rel: rest.to_string(),
                drive: lower.chars().next(),
                assumed: Some(Assumption::HostRedirect),
            });
        }
        if let Some((_, prefix)) = DEVICES.iter().find(|(d, _)| *d == lower) {
            return Ok(Located {
                area: Area::Game,
                rel: format!("{prefix}/{rest}"),
                drive: None,
                assumed: Some(Assumption::DeviceName),
            });
        }
        Err(ResolveError::UnknownDevice(name.to_string()))
    }

    fn host_folder(&self, text: &str) -> Option<Located> {
        let known = [
            (self.policy.overlay_dir(), Area::Game),
            (self.policy.temp_dir(), Area::Temp),
            (self.policy.persistent_dir(), Area::Persistent),
            (self.policy.output().to_path_buf(), Area::Output),
            (self.game_dir.clone(), Area::Game),
        ];
        known.iter().find_map(|(dir, area)| {
            strip_dir(text, dir).map(|rel| Located {
                area: *area,
                rel,
                drive: None,
                assumed: Some(Assumption::HostPathInsideKnownFolder),
            })
        })
    }

    /// Turns the path string a file call passed into where to read from and where to write.
    pub fn resolve(&self, game_path: &str) -> Result<Resolved, ResolveError> {
        let located = self.locate(game_path)?;
        let path = VPath::parse(&located.rel)?;
        let mut resolved = Resolved {
            area: located.area,
            path: path.clone(),
            kind: EntryKind::Missing,
            read_from: None,
            write_to: PathBuf::new(),
            in_overlay: false,
            assumed: located.assumed,
            original: game_path.to_string(),
        };
        let write_err = |e: WriteError| {
            ResolveError::Path(PathError::Invalid {
                path: game_path.to_string(),
                why: match e {
                    WriteError::Path(_) => "the path cannot be placed in Output",
                    _ => "the path leaves the Output folder",
                },
            })
        };
        match located.area {
            Area::Game => {
                resolved.write_to = self
                    .policy
                    .under(&self.policy.overlay_dir(), &path)
                    .map_err(write_err)?;
                self.fill_game(&mut resolved)?;
            }
            Area::Temp | Area::Persistent | Area::Output => {
                let base = match located.area {
                    Area::Temp => self.policy.temp_dir(),
                    Area::Persistent => self.policy.persistent_dir(),
                    _ => self.policy.output().to_path_buf(),
                };
                resolved.write_to = self.policy.under(&base, &path).map_err(write_err)?;
                let target = resolved.write_to.clone();
                fill_from_disk(&mut resolved, &target);
            }
            Area::Host => {
                let drive = located.drive.unwrap_or('c').to_string();
                let redirect = self
                    .policy
                    .under(&self.policy.host_dir().join(&drive), &path)
                    .map_err(write_err)?;
                resolved.write_to = redirect.clone();
                fill_from_disk(&mut resolved, &redirect);
                if resolved.kind == EntryKind::Missing {
                    let mut real = PathBuf::from(format!("{drive}:\\"));
                    for part in path.parts() {
                        real.push(part);
                    }
                    fill_from_disk(&mut resolved, &real);
                    resolved.in_overlay = false;
                } else {
                    resolved.in_overlay = true;
                }
            }
        }
        Ok(resolved)
    }

    fn fill_game(&self, r: &mut Resolved) -> Result<(), ResolveError> {
        let overlay = r.write_to.clone();
        match fs::metadata(&overlay) {
            Ok(meta) if meta.is_file() => {
                r.kind = EntryKind::File;
                r.read_from = Some(ReadFrom::File(overlay));
                r.in_overlay = true;
                return Ok(());
            }
            Ok(_) => {
                r.kind = EntryKind::Dir;
                return Ok(());
            }
            Err(_) => {}
        }
        if r.path.is_root() {
            r.kind = EntryKind::Dir;
            return Ok(());
        }
        if self.is_deleted(&r.path) {
            return Ok(());
        }
        if let Some(info) = self.vfs.stat_path(&r.path) {
            r.kind = EntryKind::File;
            r.read_from = Some(match self.vfs.host_path(&info) {
                Some(real) => ReadFrom::File(real),
                None => ReadFrom::Tree(info),
            });
        } else if self.vfs.is_dir_path(&r.path) {
            r.kind = EntryKind::Dir;
        }
        Ok(())
    }

    fn marker(&self, path: &VPath) -> Result<PathBuf, WriteError> {
        let key = VPath::parse(path.key())?;
        self.policy.under(&self.policy.deleted_dir(), &key)
    }

    fn is_deleted(&self, path: &VPath) -> bool {
        self.marker(path).is_ok_and(|m| m.exists())
    }

    fn kind_in_game_view(&self, path: &VPath) -> Result<EntryKind, WriteError> {
        let mut probe = Resolved {
            area: Area::Game,
            path: path.clone(),
            kind: EntryKind::Missing,
            read_from: None,
            write_to: self.policy.under(&self.policy.overlay_dir(), path)?,
            in_overlay: false,
            assumed: None,
            original: path.to_string(),
        };
        self.fill_game(&mut probe).ok();
        Ok(probe.kind)
    }

    pub fn read(&self, resolved: &Resolved) -> std::io::Result<Vec<u8>> {
        match &resolved.read_from {
            Some(ReadFrom::File(path)) => fs::read(path),
            Some(ReadFrom::Tree(info)) => self
                .vfs
                .read_source(info)
                .map_err(|e| std::io::Error::other(e.to_string())),
            None => Err(std::io::ErrorKind::NotFound.into()),
        }
    }

    pub fn open(&self, resolved: &Resolved) -> std::io::Result<Box<dyn crate::source::ReadSeek>> {
        match &resolved.read_from {
            Some(ReadFrom::File(path)) => Ok(Box::new(fs::File::open(path)?)),
            Some(ReadFrom::Tree(info)) => self.vfs.open(info),
            None => Err(std::io::ErrorKind::NotFound.into()),
        }
    }

    /// Makes `write_to` ready to be opened for writing and returns it: the folder above must exist in the game's
    /// view (except under `T:`, which creates its folders), the file must not be a folder, and with
    /// [`WriteMode::KeepContents`] a package file is copied into the overlay first. The package is never written.
    pub fn prepare_write(
        &self,
        resolved: &Resolved,
        mode: WriteMode,
    ) -> Result<PathBuf, WriteError> {
        let now = self
            .resolve(&resolved.original)
            .map_err(|e| WriteError::NotFound(e.to_string()))?;
        if now.kind == EntryKind::Dir {
            return Err(WriteError::IsDirectory(now.original));
        }
        let target = now.write_to.clone();
        self.policy.check(&target)?;
        let parent = target.parent().map(Path::to_path_buf);
        if now.area == Area::Game
            && !now.path.is_root()
            && self.kind_in_game_view(&now.path.parent())? != EntryKind::Dir
        {
            return Err(WriteError::ParentMissing(now.original));
        }
        if let Some(parent) = &parent {
            fs::create_dir_all(parent)?;
        }
        if mode == WriteMode::KeepContents && now.kind == EntryKind::File && !now.in_overlay {
            let temp = target.with_file_name(format!(
                ".{}.copy-{}",
                target.file_name().unwrap_or_default().to_string_lossy(),
                std::process::id()
            ));
            match &now.read_from {
                Some(ReadFrom::File(from)) => {
                    fs::copy(from, &temp)?;
                }
                Some(ReadFrom::Tree(info)) => {
                    fs::write(
                        &temp,
                        self.vfs
                            .read_source(info)
                            .map_err(|e| std::io::Error::other(e.to_string()))?,
                    )?;
                }
                None => {}
            }
            if temp.exists() {
                if target.exists() {
                    fs::remove_file(&temp)?;
                } else {
                    fs::rename(&temp, &target)?;
                }
            }
        }
        if now.area == Area::Game {
            let marker = self.marker(&now.path)?;
            if marker.exists() {
                fs::remove_file(marker)?;
            }
        }
        Ok(target)
    }

    /// Deletes a file as the game sees it: an overlay file is removed, and a package file is hidden by a marker.
    pub fn delete_file(&self, resolved: &Resolved) -> Result<(), WriteError> {
        let now = self
            .resolve(&resolved.original)
            .map_err(|e| WriteError::NotFound(e.to_string()))?;
        match now.kind {
            EntryKind::Missing => return Err(WriteError::NotFound(now.original)),
            EntryKind::Dir => return Err(WriteError::IsDirectory(now.original)),
            EntryKind::File => {}
        }
        if now.in_overlay {
            fs::remove_file(&now.write_to)?;
        }
        if now.area == Area::Game && self.vfs.stat_path(&now.path).is_some() {
            let marker = self.marker(&now.path)?;
            if let Some(parent) = marker.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(marker, b"")?;
        }
        Ok(())
    }

    /// Makes a folder in the overlay. A name the game can already see is `AlreadyExists`.
    pub fn create_dir(&self, resolved: &Resolved) -> Result<PathBuf, WriteError> {
        let now = self
            .resolve(&resolved.original)
            .map_err(|e| WriteError::NotFound(e.to_string()))?;
        if now.kind != EntryKind::Missing {
            return Err(WriteError::AlreadyExists(now.original));
        }
        if now.area == Area::Game
            && !now.path.is_root()
            && self.kind_in_game_view(&now.path.parent())? != EntryKind::Dir
        {
            return Err(WriteError::ParentMissing(now.original));
        }
        self.policy.check(&now.write_to)?;
        fs::create_dir_all(&now.write_to)?;
        if now.area == Area::Game {
            let marker = self.marker(&now.path)?;
            if marker.exists() {
                fs::remove_file(marker)?;
            }
        }
        Ok(now.write_to)
    }

    /// Removes a folder that is empty in the game's view. A folder only the package has stays on disk, hidden.
    pub fn remove_dir(&self, resolved: &Resolved) -> Result<(), WriteError> {
        let now = self
            .resolve(&resolved.original)
            .map_err(|e| WriteError::NotFound(e.to_string()))?;
        if now.kind != EntryKind::Dir || now.path.is_root() {
            return Err(WriteError::NotFound(now.original));
        }
        if !self.list(&now)?.is_empty() {
            return Err(WriteError::NotEmpty(now.original));
        }
        if now.write_to.is_dir() {
            fs::remove_dir(&now.write_to)?;
        }
        if now.area == Area::Game && self.vfs.is_dir_path(&now.path) {
            let marker = self.marker(&now.path)?;
            if let Some(parent) = marker.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(marker, b"")?;
        }
        Ok(())
    }

    /// The entries of a folder as the game sees them: tree and overlay merged, the overlay winning on a name, and
    /// deleted names left out.
    pub fn list(&self, resolved: &Resolved) -> Result<Vec<DirEntry>, WriteError> {
        if resolved.kind != EntryKind::Dir {
            return Err(WriteError::NotFound(resolved.original.clone()));
        }
        let mut merged: BTreeMap<String, DirEntry> = BTreeMap::new();
        let from_disk = |dir: &Path, merged: &mut BTreeMap<String, DirEntry>| {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') && name.contains(".copy-") {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                merged.insert(
                    fold(&name),
                    DirEntry {
                        name,
                        is_dir: meta.is_dir(),
                        size: if meta.is_dir() { 0 } else { meta.len() },
                    },
                );
            }
        };
        match resolved.area {
            Area::Game => {
                for entry in self
                    .vfs
                    .list_path(&resolved.path)
                    .map_err(WriteError::Path)?
                {
                    let child = resolved.path.join(&entry.name)?;
                    if !self.is_deleted(&child) {
                        merged.insert(fold(&entry.name), entry);
                    }
                }
                from_disk(&resolved.write_to, &mut merged);
            }
            Area::Host if resolved.in_overlay => from_disk(&resolved.write_to, &mut merged),
            _ => {
                let dir = match &resolved.read_from {
                    Some(ReadFrom::File(p)) => p.clone(),
                    _ => resolved.write_to.clone(),
                };
                from_disk(&dir, &mut merged);
            }
        }
        Ok(merged.into_values().collect())
    }
}

fn fill_from_disk(resolved: &mut Resolved, path: &Path) {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => {
            resolved.kind = EntryKind::File;
            resolved.read_from = Some(ReadFrom::File(path.to_path_buf()));
            resolved.in_overlay = true;
        }
        Ok(_) => {
            resolved.kind = EntryKind::Dir;
            resolved.read_from = None;
        }
        Err(_) => {}
    }
}

/// The part of `text` below `dir`, if it is below it, comparing parts without regard to case.
fn strip_dir(text: &str, dir: &Path) -> Option<String> {
    let dir_text = dir.to_string_lossy();
    let dir_text = dir_text.strip_prefix("\\\\?\\").unwrap_or(&dir_text);
    let dir_parts: Vec<String> = dir_text
        .split(['/', '\\'])
        .filter(|p| !p.is_empty())
        .map(fold)
        .collect();
    let parts: Vec<&str> = text.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.len() < dir_parts.len() || dir_parts.is_empty() {
        return None;
    }
    parts
        .iter()
        .zip(&dir_parts)
        .all(|(a, b)| fold(a) == *b)
        .then(|| parts[dir_parts.len()..].join("/"))
}
