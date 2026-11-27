//! `[content]` in Config/leonida.toml.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("Config/leonida.toml is not valid: {0}")]
    Parse(String),
}

/// What to do with nudity. Gore, violence and language are never touched by this filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NudityMode {
    #[default]
    Block,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContentSettings {
    pub nudity: NudityMode,
}

#[derive(Deserialize, Default)]
struct File {
    #[serde(default)]
    content: Section,
}

#[derive(Deserialize, Default)]
struct Section {
    #[serde(default)]
    nudity: NudityMode,
}

impl ContentSettings {
    /// Settings from the text of leonida.toml. A missing `[content]` section means the default, `block`.
    pub fn from_toml(text: &str) -> Result<Self, SettingsError> {
        let file: File =
            toml::from_str(text).map_err(|error| SettingsError::Parse(error.to_string()))?;
        Ok(Self {
            nudity: file.content.nudity,
        })
    }

    /// Settings from a file; a file that does not exist means the default.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(SettingsError::Read {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    pub fn blocks_nudity(&self) -> bool {
        self.nudity == NudityMode::Block
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_by_default() {
        assert!(ContentSettings::from_toml("").unwrap().blocks_nudity());
        assert!(
            ContentSettings::from_toml("[graphics]\nfps = 60\n")
                .unwrap()
                .blocks_nudity()
        );
        assert!(ContentSettings::default().blocks_nudity());
    }

    #[test]
    fn reads_allow_and_block() {
        let allow = ContentSettings::from_toml("[content]\nnudity = \"allow\"\n").unwrap();
        assert_eq!(allow.nudity, NudityMode::Allow);
        let block = ContentSettings::from_toml("[content]\nnudity = \"block\"\n").unwrap();
        assert_eq!(block.nudity, NudityMode::Block);
    }

    #[test]
    fn rejects_unknown_values() {
        assert!(ContentSettings::from_toml("[content]\nnudity = \"maybe\"\n").is_err());
    }

    #[test]
    fn missing_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let settings = ContentSettings::load(&dir.path().join("leonida.toml")).unwrap();
        assert!(settings.blocks_nudity());
    }
}
