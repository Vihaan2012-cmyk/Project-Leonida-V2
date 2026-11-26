use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub game: Option<PathBuf>,
    pub find_on_start: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            game: None,
            find_on_start: true,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        settings_path()
            .map(|path| Settings::load_from(&path))
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        match settings_path() {
            Some(path) => self.save_to(&path),
            None => Ok(()),
        }
    }

    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(path, text)
    }
}

fn settings_path() -> Option<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(appdata).join("Leonida").join("settings.json"));
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("leonida")
            .join("settings.json")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let settings = Settings {
            game: Some(PathBuf::from("E:/Games/GTA")),
            find_on_start: false,
        };
        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), settings);
    }

    #[test]
    fn missing_or_broken_files_give_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert!(Settings::default().find_on_start);
    }
}
