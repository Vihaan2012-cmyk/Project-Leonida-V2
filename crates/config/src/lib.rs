//! Leonida's settings: the TOML files in Config (leonida, graphics, input, mods, display, widescreen, upscaling).
//!
//! Every file is TOML. A missing file is written with its documented defaults; a present file is parsed strictly
//! (unknown names are errors) and checked, and every problem is reported with the file and field it is in.

pub mod display;
pub mod general;
pub mod graphics;
pub mod input;
pub mod mods;
mod sampler;
pub mod save;
pub mod upscaling;
pub mod widescreen;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

pub use display::DisplayConfig;
pub use general::{Audio, AudioOutput, Content, General, Nudity};
pub use graphics::{
    AspectMode, FrameRate, FrameRateMode, Graphics, PipelineNotReady, Upscaler, WindowMode,
    adjust_sampler,
};
pub use input::Input;
pub use mods::Mods;
pub use upscaling::Upscaling;
pub use widescreen::Widescreen;

/// The folder the settings live in when nothing else is said, next to where Leonida is started.
pub const DEFAULT_DIR: &str = "Config";

/// One thing wrong in a settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub field: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{file}: {message}")]
    Parse { file: String, message: String },
    #[error("{file} has {} problem(s):{}", problems.len(), list(problems))]
    Invalid {
        file: String,
        problems: Vec<Problem>,
    },
}

fn list(problems: &[Problem]) -> String {
    problems.iter().map(|p| format!("\n  {p}")).collect()
}

/// Collects problems while a section checks itself.
#[derive(Debug, Default)]
pub struct Problems(Vec<Problem>);

impl Problems {
    pub fn add(&mut self, field: &str, message: impl Into<String>) {
        self.0.push(Problem {
            field: field.to_string(),
            message: message.into(),
        });
    }

    pub fn range_f32(&mut self, field: &str, value: f32, low: f32, high: f32) {
        if !(low..=high).contains(&value) {
            self.add(
                field,
                format!("must be between {low} and {high}, got {value}"),
            );
        }
    }

    pub fn range_u32(&mut self, field: &str, value: u32, low: u32, high: u32) {
        if !(low..=high).contains(&value) {
            self.add(
                field,
                format!("must be between {low} and {high}, got {value}"),
            );
        }
    }

    pub fn into_vec(self) -> Vec<Problem> {
        self.0
    }
}

/// A settings file: its name, its documented default text and its checks.
pub trait Section: Serialize + DeserializeOwned + Default + PartialEq {
    const FILE: &'static str;
    fn default_text() -> String;

    fn check(&self, problems: &mut Problems);

    /// Parses `text` as this file and checks it.
    fn parse(text: &str) -> Result<Self, ConfigError> {
        let value: Self = toml::from_str(text).map_err(|error| ConfigError::Parse {
            file: Self::FILE.to_string(),
            message: error.to_string().trim_end().to_string(),
        })?;
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let mut problems = Problems::default();
        self.check(&mut problems);
        match problems.into_vec() {
            list if list.is_empty() => Ok(()),
            list => Err(ConfigError::Invalid {
                file: Self::FILE.to_string(),
                problems: list,
            }),
        }
    }

    /// Reads `dir/FILE`, writing the documented defaults first when it is missing.
    fn load(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join(Self::FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::write_default(dir)?;
                Ok(Self::default())
            }
            Err(source) => Err(ConfigError::Io { path, source }),
        }
    }

    /// Writes the documented defaults to `dir/FILE`, replacing what is there.
    fn write_default(dir: &Path) -> Result<(), ConfigError> {
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| ConfigError::Io { path, source }
        };
        std::fs::create_dir_all(dir).map_err(io(dir))?;
        let path = dir.join(Self::FILE);
        std::fs::write(&path, Self::default_text()).map_err(io(&path))
    }

    /// The settings as TOML, without the documentation.
    fn to_text(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Writes the settings to `dir/FILE`, changing only the values that differ and keeping the file's comments.
    fn save(&self, dir: &Path) -> Result<(), ConfigError>
    where
        Self: Sized,
    {
        save::save(self, dir)
    }
}

/// Every settings file, read together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    pub general: General,
    pub graphics: Graphics,
    pub input: Input,
    pub mods: Mods,
    pub display: DisplayConfig,
    pub widescreen: Widescreen,
    pub upscaling: Upscaling,
}

/// The files, by the name used on the command line.
pub const FILES: [&str; 7] = [
    General::FILE,
    Graphics::FILE,
    Input::FILE,
    Mods::FILE,
    DisplayConfig::FILE,
    Widescreen::FILE,
    Upscaling::FILE,
];

impl Config {
    /// Reads every file from `dir`, writing any that is missing. Stops at the first file that is wrong.
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        Ok(Self {
            general: General::load(dir)?,
            graphics: Graphics::load(dir)?,
            input: Input::load(dir)?,
            mods: Mods::load(dir)?,
            display: DisplayConfig::load(dir)?,
            widescreen: Widescreen::load(dir)?,
            upscaling: Upscaling::load(dir)?,
        })
    }

    /// Writes every file into `dir`, keeping the comments in the ones that exist.
    pub fn save(&self, dir: &Path) -> Result<(), ConfigError> {
        self.general.save(dir)?;
        self.graphics.save(dir)?;
        self.input.save(dir)?;
        self.mods.save(dir)?;
        self.display.save(dir)?;
        self.widescreen.save(dir)?;
        self.upscaling.save(dir)
    }

    /// Replaces the named files (all of them when `only` is empty) with the documented defaults.
    pub fn reset(dir: &Path, only: &[&str]) -> Result<Vec<&'static str>, ConfigError> {
        let mut written = Vec::new();
        for file in FILES {
            if !only.is_empty() && !only.contains(&file) {
                continue;
            }
            match file {
                General::FILE => General::write_default(dir)?,
                Graphics::FILE => Graphics::write_default(dir)?,
                Input::FILE => Input::write_default(dir)?,
                Mods::FILE => Mods::write_default(dir)?,
                DisplayConfig::FILE => DisplayConfig::write_default(dir)?,
                Widescreen::FILE => Widescreen::write_default(dir)?,
                _ => Upscaling::write_default(dir)?,
            }
            written.push(file);
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Section + fmt::Debug>() {
        assert_eq!(T::parse(&T::default_text()).unwrap(), T::default());
        assert_eq!(T::parse(&T::default().to_text()).unwrap(), T::default());
        T::default().validate().unwrap();
    }

    #[test]
    fn defaults_round_trip() {
        round_trip::<General>();
        round_trip::<Graphics>();
        round_trip::<Input>();
        round_trip::<Mods>();
        round_trip::<DisplayConfig>();
        round_trip::<Widescreen>();
        round_trip::<Upscaling>();
    }

    #[test]
    fn missing_files_are_written_then_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("Config");
        let config = Config::load(&config_dir).unwrap();
        assert_eq!(config, Config::default());
        for file in FILES {
            let text = std::fs::read_to_string(config_dir.join(file)).unwrap();
            assert!(text.contains('#'), "{file} should be documented");
        }
        assert_eq!(Config::load(&config_dir).unwrap(), config);
    }

    #[test]
    fn edited_values_are_read() {
        let dir = tempfile::tempdir().unwrap();
        Config::load(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("graphics.toml"),
            "upscaler = \"fsr1\"\nresolution_scale = 0.67\nframe_rate_mode = \"cap\"\nframe_rate_cap = 90\n",
        )
        .unwrap();
        let config = Config::load(dir.path()).unwrap();
        assert_eq!(config.graphics.upscaler, Upscaler::Fsr1);
        assert_eq!(config.graphics.frame_rate(), FrameRate::Cap(90));
        assert_eq!(config.graphics.anisotropic_filtering, 16);
    }

    #[test]
    fn reset_replaces_only_the_named_file() {
        let dir = tempfile::tempdir().unwrap();
        Config::load(dir.path()).unwrap();
        std::fs::write(dir.path().join("graphics.toml"), "hdr = true\n").unwrap();
        std::fs::write(dir.path().join("mods.toml"), "load_order = [\"a\"]\n").unwrap();
        let written = Config::reset(dir.path(), &["graphics.toml"]).unwrap();
        assert_eq!(written, ["graphics.toml"]);
        let config = Config::load(dir.path()).unwrap();
        assert!(!config.graphics.hdr);
        assert_eq!(config.mods.load_order, ["a"]);
    }

    #[test]
    fn errors_name_the_file_and_the_field() {
        let error =
            Graphics::parse("resolution_scale = 9.0\nanisotropic_filtering = 3\n").unwrap_err();
        let text = error.to_string();
        assert!(
            text.starts_with("graphics.toml has 2 problem(s):"),
            "{text}"
        );
        assert!(
            text.contains("resolution_scale: must be between 0.25 and 2, got 9"),
            "{text}"
        );
        assert!(
            text.contains("anisotropic_filtering: must be 1, 2, 4, 8 or 16, got 3"),
            "{text}"
        );
    }

    #[test]
    fn unknown_names_and_bad_types_are_parse_errors() {
        let error = Graphics::parse("resolution_scal = 1.0\n").unwrap_err();
        assert!(matches!(&error, ConfigError::Parse { file, .. } if file == "graphics.toml"));
        assert!(error.to_string().contains("resolution_scal"));
        assert!(Graphics::parse("upscaler = \"magic\"\n").is_err());
        assert!(Graphics::parse("hdr = \"yes\"\n").is_err());
    }
}
