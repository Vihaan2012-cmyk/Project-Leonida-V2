use std::collections::BTreeMap;
use std::fmt;
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

use crate::package_path;

/// Files checked by size alone to recognise a version without reading anything large.
pub const PROBES: [&str; 3] = ["MicrosoftGame.Config", "Catalog000.bin", "appdata0.rpf"];

const BUILTIN: &[&str] = &[include_str!("../manifests/xbox-1.0.2.1.json")];

/// One supported package version: every file's size and SHA-256.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub title: String,
    pub version: String,
    pub expected_exe: String,
    pub files: BTreeMap<String, FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileEntry {
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid version manifest: {0}")]
pub struct ManifestError(#[from] serde_json::Error);

impl Manifest {
    pub fn from_json(text: &str) -> Result<Self, ManifestError> {
        Ok(serde_json::from_str(text)?)
    }

    /// The manifests of every supported version, built into the binary.
    pub fn builtin() -> Vec<Manifest> {
        BUILTIN
            .iter()
            .map(|text| Manifest::from_json(text).expect("built-in manifests are valid"))
            .collect()
    }

    /// For messages, e.g. "Xbox Series package 1.0.2.1".
    pub fn label(&self) -> String {
        format!("{} {}", self.title, self.version)
    }
}

/// Why a folder is not a given version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    NoProbes,
    Missing(String),
    NotAFile(String),
    WrongSize {
        name: String,
        actual: u64,
        expected: u64,
    },
    Unreadable {
        name: String,
        error: String,
    },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mismatch::NoProbes => write!(f, "no version probe files are listed in the manifest"),
            Mismatch::Missing(name) => write!(f, "missing {name}"),
            Mismatch::NotAFile(name) => write!(f, "{name} is not a regular file"),
            Mismatch::WrongSize {
                name,
                actual,
                expected,
            } => {
                write!(
                    f,
                    "wrong size for {name} ({actual} bytes, expected {expected})"
                )
            }
            Mismatch::Unreadable { name, error } => {
                write!(f, "cannot read file metadata for {name} ({error})")
            }
        }
    }
}

/// Why `game` is not `manifest`'s version, from file names and sizes only. Empty means it matches.
pub fn probe_mismatches(game: &Path, manifest: &Manifest) -> Vec<Mismatch> {
    let probes: Vec<(&str, &FileEntry)> = PROBES
        .iter()
        .filter_map(|&name| manifest.files.get(name).map(|entry| (name, entry)))
        .collect();
    if probes.is_empty() {
        return vec![Mismatch::NoProbes];
    }
    let mut problems = Vec::new();
    for (name, entry) in probes {
        match std::fs::metadata(package_path(game, name)) {
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
            {
                problems.push(Mismatch::Missing(name.to_string()));
            }
            Err(error) => problems.push(Mismatch::Unreadable {
                name: name.to_string(),
                error: error.to_string(),
            }),
            Ok(meta) if !meta.is_file() => problems.push(Mismatch::NotAFile(name.to_string())),
            Ok(meta) if meta.len() != entry.size => problems.push(Mismatch::WrongSize {
                name: name.to_string(),
                actual: meta.len(),
                expected: entry.size,
            }),
            Ok(_) => {}
        }
    }
    problems
}

/// The supported version `game` is, recognised from probe file sizes, or None.
pub fn match_version<'a>(game: &Path, manifests: &'a [Manifest]) -> Option<&'a Manifest> {
    manifests
        .iter()
        .find(|manifest| probe_mismatches(game, manifest).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_path;
    use std::fs;
    use std::path::Path;

    const TEST_MANIFEST: &str = r#"{
        "title": "Test package", "version": "9.9", "expected_exe": "game.exe",
        "files": {
            "MicrosoftGame.Config": {"size": 5, "sha256": "x"},
            "Catalog000.bin": {"size": 3, "sha256": "x"},
            "appdata0.rpf": {"size": 4, "sha256": "x"},
            "data/other.rpf": {"size": 9, "sha256": "x"}
        }
    }"#;

    fn make_file(dir: &Path, rel: &str, len: u64) {
        let path = package_path(dir, rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path).unwrap().set_len(len).unwrap();
    }

    #[test]
    fn builtin_manifest_is_version_1_0_2_1() {
        let all = Manifest::builtin();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "1.0.2.1");
        assert_eq!(all[0].files.len(), 108);
        assert_eq!(all[0].expected_exe, "game_scarlett_master.exe");
        assert_eq!(all[0].label(), "Xbox Series package 1.0.2.1");
    }

    #[test]
    fn probe_sizes_identify_the_version() {
        let dir = tempfile::tempdir().unwrap();
        make_file(dir.path(), "MicrosoftGame.Config", 5);
        make_file(dir.path(), "Catalog000.bin", 3);
        make_file(dir.path(), "appdata0.rpf", 4);
        let manifests = vec![Manifest::from_json(TEST_MANIFEST).unwrap()];
        assert!(probe_mismatches(dir.path(), &manifests[0]).is_empty());
        assert_eq!(
            match_version(dir.path(), &manifests).map(|m| m.version.as_str()),
            Some("9.9")
        );
    }

    #[test]
    fn wrong_size_and_missing_probes_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        make_file(dir.path(), "MicrosoftGame.Config", 6);
        make_file(dir.path(), "Catalog000.bin", 3);
        let manifest = Manifest::from_json(TEST_MANIFEST).unwrap();
        let problems = probe_mismatches(dir.path(), &manifest);
        assert_eq!(
            problems,
            vec![
                Mismatch::WrongSize {
                    name: "MicrosoftGame.Config".into(),
                    actual: 6,
                    expected: 5
                },
                Mismatch::Missing("appdata0.rpf".into()),
            ]
        );
        assert_eq!(
            problems[0].to_string(),
            "wrong size for MicrosoftGame.Config (6 bytes, expected 5)"
        );
        assert!(match_version(dir.path(), std::slice::from_ref(&manifest)).is_none());
    }

    #[test]
    fn a_folder_where_a_probe_should_be_is_not_a_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("MicrosoftGame.Config")).unwrap();
        let manifest = Manifest::from_json(TEST_MANIFEST).unwrap();
        assert!(
            probe_mismatches(dir.path(), &manifest)
                .contains(&Mismatch::NotAFile("MicrosoftGame.Config".into()))
        );
    }

    #[test]
    fn manifest_without_probes_never_matches() {
        let manifest =
            Manifest::from_json(r#"{"title":"t","version":"v","expected_exe":"e","files":{}}"#)
                .unwrap();
        assert_eq!(
            probe_mismatches(Path::new("."), &manifest),
            vec![Mismatch::NoProbes]
        );
    }

    #[test]
    fn package_path_splits_on_slashes() {
        let p = package_path(Path::new("game"), "data/ttdb/x.ttpf");
        assert_eq!(
            p,
            Path::new("game").join("data").join("ttdb").join("x.ttpf")
        );
    }
}
