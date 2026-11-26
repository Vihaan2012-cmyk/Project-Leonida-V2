use std::path::{Path, PathBuf};

use game_package::{Manifest, default_roots, find_packages, match_version, package_path};

const SEARCH_DEPTH: usize = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameState {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub exe_name: Option<String>,
    pub exe_present: bool,
}

impl GameState {
    pub fn recognise(path: PathBuf, manifests: &[Manifest]) -> GameState {
        match match_version(&path, manifests) {
            Some(manifest) => GameState {
                exe_present: package_path(&path, &manifest.expected_exe).is_file(),
                exe_name: Some(manifest.expected_exe.clone()),
                version: Some(manifest.label()),
                path: Some(path),
            },
            None => GameState {
                path: Some(path),
                ..GameState::default()
            },
        }
    }

    pub fn is_supported(&self) -> bool {
        self.version.is_some()
    }
}

pub fn find_games(manifests: &[Manifest]) -> Vec<PathBuf> {
    find_packages(&default_roots(), SEARCH_DEPTH)
        .into_iter()
        .filter(|path| match_version(path, manifests).is_some())
        .collect()
}

pub fn short_path(path: &Path, max: usize) -> String {
    let text = path.display().to_string();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max || max < 8 {
        return text;
    }
    let keep = (max - 1) / 2;
    let head: String = chars[..keep].iter().collect();
    let tail: String = chars[chars.len() - keep..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const TEST_MANIFEST: &str = r#"{
        "title": "Test package", "version": "9.9", "expected_exe": "game.exe",
        "files": {
            "MicrosoftGame.Config": {"size": 5, "sha256": "x"},
            "Catalog000.bin": {"size": 3, "sha256": "x"},
            "appdata0.rpf": {"size": 4, "sha256": "x"}
        }
    }"#;

    fn package(dir: &Path) {
        for (name, len) in [
            ("MicrosoftGame.Config", 5),
            ("Catalog000.bin", 3),
            ("appdata0.rpf", 4),
        ] {
            fs::File::create(dir.join(name))
                .unwrap()
                .set_len(len)
                .unwrap();
        }
    }

    #[test]
    fn recognises_a_supported_copy() {
        let dir = tempfile::tempdir().unwrap();
        package(dir.path());
        let manifests = vec![Manifest::from_json(TEST_MANIFEST).unwrap()];
        let state = GameState::recognise(dir.path().to_path_buf(), &manifests);
        assert_eq!(state.version.as_deref(), Some("Test package 9.9"));
        assert_eq!(state.exe_name.as_deref(), Some("game.exe"));
        assert!(!state.exe_present);
        fs::write(dir.path().join("game.exe"), b"x").unwrap();
        assert!(GameState::recognise(dir.path().to_path_buf(), &manifests).exe_present);
    }

    #[test]
    fn unsupported_folder_keeps_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let manifests = vec![Manifest::from_json(TEST_MANIFEST).unwrap()];
        let state = GameState::recognise(dir.path().to_path_buf(), &manifests);
        assert!(!state.is_supported());
        assert_eq!(state.path.as_deref(), Some(dir.path()));
    }

    #[test]
    fn short_path_keeps_both_ends() {
        let long = Path::new("E:/Project Leonida V2/a very long folder name/GTA6");
        let short = short_path(long, 20);
        assert_eq!(short.chars().count(), 19);
        assert!(short.starts_with("E:/Proje"));
        assert!(short.ends_with("e/GTA6"));
        assert_eq!(short_path(Path::new("C:/x"), 20), "C:/x");
    }
}
