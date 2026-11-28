//! `leonida.toml`: where the game is, where Leonida writes, and content and language choices.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Problems, Section};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Nudity {
    /// Nudity is blocked.
    #[default]
    Block,
    Allow,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Content {
    pub nudity: Nudity,
}

/// Whether the game's sound reaches the speakers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioOutput {
    /// Opens the default audio device when the game starts its sound.
    #[default]
    Auto,
    /// The game runs with its sound mixed but never played.
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Audio {
    pub output: AudioOutput,
}

/// What the game may do with files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Files {
    /// Limits the game to its own folder (read only), Leonida's output folder and the mods and fixes folders
    /// (read only). Turn it off only to find out whether a missing file is the reason something fails.
    pub sandbox: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self { sandbox: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// The game folder (the one that holds MicrosoftGame.Config). Searched for when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_folder: Option<PathBuf>,
    /// Where Leonida writes what it makes while running.
    pub output_folder: PathBuf,
    /// Language code for text Leonida shows and for the game's text, such as `en` or `pt-BR`.
    pub language: String,
    pub content: Content,
    pub audio: Audio,
    pub files: Files,
}

impl Default for General {
    fn default() -> Self {
        Self {
            game_folder: None,
            output_folder: PathBuf::from("Output"),
            language: "en".to_string(),
            content: Content::default(),
            audio: Audio::default(),
            files: Files::default(),
        }
    }
}

fn valid_language(code: &str) -> bool {
    let mut parts = code.split('-');
    let primary = parts.next().unwrap_or("");
    (2..=3).contains(&primary.len())
        && primary.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

impl Section for General {
    const FILE: &'static str = "leonida.toml";
    fn default_text() -> String {
        "\
# Leonida settings. Delete this file (or run `leonida config reset`) to get it back with the defaults.

# The game folder, the one that holds MicrosoftGame.Config. When this is not set, Leonida looks in Downloads,
# Desktop, Documents, Games, your home folder and the top of every drive.
# game_folder = \"E:/Games/GTA6\"

# Where Leonida writes what it makes while running (logs, caches, converted files).
output_folder = \"Output\"

# Language code for Leonida's text and the game's text: two or three lowercase letters, optionally followed by
# a region, such as \"en\", \"fr\" or \"pt-BR\".
language = \"en\"

[content]
# \"block\" keeps the game's nudity blocked; \"allow\" lets it through.
nudity = \"block\"

[audio]
# \"auto\" plays the game's sound on the default audio device; \"off\" never opens one.
output = \"auto\"

[files]
# true keeps the game to its own folder (read only), the output folder, and the mods and fixes folders (read
# only); every other path on this PC is closed to it. false lifts that, for troubleshooting only.
sandbox = true
"
        .to_string()
    }

    fn check(&self, problems: &mut Problems) {
        if let Some(folder) = &self.game_folder
            && folder.as_os_str().is_empty()
        {
            problems.add(
                "game_folder",
                "must not be empty; remove the line to search for the game",
            );
        }
        if self.output_folder.as_os_str().is_empty() {
            problems.add("output_folder", "must not be empty");
        }
        if !valid_language(&self.language) {
            problems.add(
                "language",
                format!(
                    "must be a language code such as \"en\" or \"pt-BR\", got \"{}\"",
                    self.language
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nudity_is_blocked_by_default_and_can_be_allowed() {
        assert_eq!(General::default().content.nudity, Nudity::Block);
        let general = General::parse("[content]\nnudity = \"allow\"\n").unwrap();
        assert_eq!(general.content.nudity, Nudity::Allow);
        assert!(General::parse("[content]\nnudity = \"maybe\"\n").is_err());
    }

    #[test]
    fn audio_output_defaults_to_auto_and_can_be_turned_off() {
        assert_eq!(General::default().audio.output, AudioOutput::Auto);
        let general = General::parse(
            "[audio]
output = \"off\"
",
        )
        .unwrap();
        assert_eq!(general.audio.output, AudioOutput::Off);
        assert!(
            General::parse(
                "[audio]
output = \"loud\"
"
            )
            .is_err()
        );
    }

    #[test]
    fn the_file_sandbox_is_on_unless_turned_off() {
        assert!(General::default().files.sandbox);
        assert!(General::parse("").unwrap().files.sandbox);
        let general = General::parse(
            "[files]
sandbox = false
",
        )
        .unwrap();
        assert!(!general.files.sandbox);
        assert!(
            General::parse(
                "[files]
sandbox = \"no\"
"
            )
            .is_err()
        );
        assert!(
            General::parse(&General::default_text())
                .unwrap()
                .files
                .sandbox
        );
    }

    #[test]
    fn language_codes_are_checked() {
        for good in ["en", "fr", "pt-BR", "zh-Hans", "fil"] {
            assert!(valid_language(good), "{good}");
        }
        for bad in ["", "EN", "english", "e", "en-", "en_US"] {
            assert!(!valid_language(bad), "{bad}");
        }
        let error = General::parse("language = \"english\"\n").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("language: must be a language code")
        );
    }

    #[test]
    fn empty_folders_are_refused() {
        let error = General::parse("game_folder = \"\"\noutput_folder = \"\"\n").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("game_folder: must not be empty"), "{text}");
        assert!(text.contains("output_folder: must not be empty"), "{text}");
    }
}
