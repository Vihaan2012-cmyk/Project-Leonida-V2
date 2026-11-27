//! Package manifests and the checks on them.

use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageType {
    Assets,
    Shaders,
    Systems,
    Mixed,
    Patch,
}

impl PackageType {
    pub fn name(self) -> &'static str {
        match self {
            PackageType::Assets => "assets",
            PackageType::Shaders => "shaders",
            PackageType::Systems => "systems",
            PackageType::Mixed => "mixed",
            PackageType::Patch => "patch",
        }
    }
}

/// Dotted numbers such as `1.0.2.1`. Missing trailing parts count as zero when comparing.
#[derive(Debug, Clone, Eq)]
pub struct Version(Vec<u32>);

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let parts: Option<Vec<u32>> = text.trim().split('.').map(|p| p.parse().ok()).collect();
        parts.filter(|p| !p.is_empty()).map(Version)
    }

    /// True when `self` is `other` or `other` with more parts, so `1.0` covers `1.0.2.1`.
    pub fn covers(&self, other: &Version) -> bool {
        self.0.len() <= other.0.len() && other.0[..self.0.len()] == self.0[..]
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.0.len().max(other.0.len());
        for i in 0..len {
            let a = self.0.get(i).copied().unwrap_or(0);
            let b = other.0.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        Ordering::Equal
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(u32::to_string).collect();
        f.write_str(&parts.join("."))
    }
}

/// What a package asks of another: its folder name or title, and optionally `>= 1.2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub minimum: Option<Version>,
}

impl Dependency {
    pub fn parse(text: &str) -> Result<Dependency, String> {
        match text.split_once(">=") {
            Some((name, version)) => {
                let minimum = Version::parse(version)
                    .ok_or_else(|| format!("`{}` is not a version", version.trim()))?;
                Ok(Dependency {
                    name: name.trim().to_string(),
                    minimum: Some(minimum),
                })
            }
            None => Ok(Dependency {
                name: text.trim().to_string(),
                minimum: None,
            }),
        }
    }
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.minimum {
            Some(v) => write!(f, "{} >= {v}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawManifest {
    title: String,
    author: String,
    version: String,
    #[serde(rename = "type")]
    kind: PackageType,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    game_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub title: String,
    pub author: String,
    pub version: Version,
    pub kind: PackageType,
    pub dependencies: Vec<Dependency>,
    /// Absent means the package does not say, and is accepted for any game version.
    pub game_version: Option<Version>,
}

impl Manifest {
    pub fn from_json(text: &str) -> Result<Manifest, PackageError> {
        let raw: RawManifest =
            serde_json::from_str(text).map_err(|e| PackageError::ManifestInvalid(e.to_string()))?;
        if raw.title.trim().is_empty() {
            return Err(PackageError::ManifestInvalid("`title` is empty".into()));
        }
        let version = Version::parse(&raw.version).ok_or_else(|| {
            PackageError::ManifestInvalid(format!(
                "`version` \"{}\" is not dotted numbers",
                raw.version
            ))
        })?;
        let game_version = match raw.game_version.as_deref() {
            None | Some("") => None,
            Some(text) => Some(Version::parse(text).ok_or_else(|| {
                PackageError::ManifestInvalid(format!(
                    "`game_version` \"{text}\" is not dotted numbers"
                ))
            })?),
        };
        let dependencies = raw
            .dependencies
            .iter()
            .map(|d| Dependency::parse(d).map_err(PackageError::ManifestInvalid))
            .collect::<Result<Vec<_>, _>>()?;
        if dependencies.iter().any(|d| d.name.is_empty()) {
            return Err(PackageError::ManifestInvalid(
                "a dependency has no name".into(),
            ));
        }
        Ok(Manifest {
            title: raw.title.trim().to_string(),
            author: raw.author,
            version,
            kind: raw.kind,
            dependencies,
            game_version,
        })
    }

    /// Fails when the package names a game version that does not cover `game`.
    pub fn check_game_version(&self, game: &Version) -> Result<(), PackageError> {
        match &self.game_version {
            Some(wanted) if !wanted.covers(game) => Err(PackageError::GameVersion {
                wanted: wanted.to_string(),
                game: game.to_string(),
            }),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    #[error("no manifest.json in the folder")]
    ManifestMissing,
    #[error("manifest.json is not valid: {0}")]
    ManifestInvalid(String),
    #[error("made for game version {wanted}, but the game is {game}")]
    GameVersion { wanted: String, game: String },
    #[error("needs `{dependency}`, which is not installed")]
    MissingDependency { dependency: String },
    #[error("needs `{dependency}`, which is turned off in Config/mods.toml")]
    DisabledDependency { dependency: String },
    #[error("needs `{dependency}` version {wanted} or newer, but {found} is installed")]
    DependencyVersion {
        dependency: String,
        wanted: String,
        found: String,
    },
    #[error("needs `{dependency}`, which cannot be loaded (see its own message)")]
    BrokenDependency { dependency: String },
    #[error("is part of a dependency cycle: {0}")]
    Cycle(String),
    #[error("could not be read: {0}")]
    Unreadable(String),
}

/// A package folder with its manifest read.
#[derive(Debug, Clone)]
pub struct Package {
    /// The folder name, which is how `mods.toml` and dependencies refer to it.
    pub id: String,
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Package {
    pub fn load(root: &Path) -> Result<Package, PackageError> {
        let id = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let manifest_path = root.join(MANIFEST_NAME);
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PackageError::ManifestMissing);
            }
            Err(e) => return Err(PackageError::Unreadable(e.to_string())),
        };
        let manifest = Manifest::from_json(text.trim_start_matches('\u{feff}'))?;
        Ok(Package {
            id,
            root: root.to_path_buf(),
            manifest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"title":"Lucia Pack","author":"me","version":"1.2","type":"assets",
        "dependencies":["Core Fixes >= 1.0.1","other"],"game_version":"1.0.2.1"}"#;

    #[test]
    fn reads_a_manifest() {
        let m = Manifest::from_json(GOOD).unwrap();
        assert_eq!(m.title, "Lucia Pack");
        assert_eq!(m.kind, PackageType::Assets);
        assert_eq!(m.dependencies[0].name, "Core Fixes");
        assert_eq!(m.dependencies[0].minimum, Version::parse("1.0.1"));
        assert_eq!(m.dependencies[1].minimum, None);
        assert!(
            m.check_game_version(&Version::parse("1.0.2.1").unwrap())
                .is_ok()
        );
        let err = m
            .check_game_version(&Version::parse("1.0.3.0").unwrap())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "made for game version 1.0.2.1, but the game is 1.0.3.0"
        );
    }

    #[test]
    fn short_game_version_covers_a_family() {
        let m = Manifest::from_json(
            r#"{"title":"t","author":"a","version":"1","type":"patch","game_version":"1.0"}"#,
        )
        .unwrap();
        assert!(
            m.check_game_version(&Version::parse("1.0.2.1").unwrap())
                .is_ok()
        );
        assert!(
            m.check_game_version(&Version::parse("1.1").unwrap())
                .is_err()
        );
    }

    #[test]
    fn rejects_bad_manifests_with_reasons() {
        for (text, needle) in [
            ("{", "manifest.json is not valid"),
            (
                r#"{"title":"t","author":"a","version":"x","type":"assets"}"#,
                "version",
            ),
            (
                r#"{"title":"t","author":"a","version":"1","type":"weird"}"#,
                "weird",
            ),
            (
                r#"{"title":" ","author":"a","version":"1","type":"assets"}"#,
                "title",
            ),
            (
                r#"{"title":"t","author":"a","version":"1","type":"assets","dependencies":["x >= q"]}"#,
                "not a version",
            ),
            (r#"{"author":"a","version":"1","type":"assets"}"#, "title"),
        ] {
            let err = Manifest::from_json(text).unwrap_err().to_string();
            assert!(err.contains(needle), "{err} should mention {needle}");
        }
    }

    #[test]
    fn versions_compare_with_zero_padding() {
        let v = |t| Version::parse(t).unwrap();
        assert!(v("1.0") == v("1.0.0.0"));
        assert!(v("1.10") > v("1.9"));
        assert!(v("1.0.2.1") > v("1.0.2"));
    }
}
