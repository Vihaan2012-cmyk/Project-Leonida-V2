//! The command lines the launcher starts. They are built as data and only the window's buttons run them, so what
//! each one would do can be checked without starting anything.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use config::{General, Section};

use crate::model::{Limit, parse_limit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Launch {
    fn new(program: &Path, args: &[&str]) -> Self {
        Self {
            program: program.to_path_buf(),
            args: args.iter().map(OsString::from).collect(),
        }
    }

    fn with(mut self, flag: &str, value: &Path) -> Self {
        self.args.push(flag.into());
        self.args.push(value.into());
        self
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }

    /// The command as one line, for the log.
    pub fn describe(&self) -> String {
        let name = self
            .program
            .file_name()
            .map_or_else(|| "leonida".into(), |name| name.to_string_lossy());
        let mut line = name.into_owned();
        for arg in &self.args {
            let arg = arg.to_string_lossy();
            line.push(' ');
            if arg.contains(' ') {
                line.push_str(&format!("\"{arg}\""));
            } else {
                line.push_str(&arg);
            }
        }
        line
    }

    #[cfg(test)]
    pub fn has_flag(&self, flag: &str) -> bool {
        self.args.iter().any(|arg| arg == flag)
    }

    #[cfg(test)]
    pub fn value_of(&self, flag: &str) -> Option<PathBuf> {
        let at = self.args.iter().position(|arg| arg == flag)?;
        self.args.get(at + 1).map(PathBuf::from)
    }
}

/// Where everything is, as full paths, the way `leonida run` works them out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
    pub config_dir: PathBuf,
    pub output_dir: PathBuf,
    pub game_dir: Option<PathBuf>,
}

impl Paths {
    /// Relative folders are taken from `root`, the folder Leonida is started in.
    pub fn of(root: &Path, config_name: &str, general: &General) -> Self {
        let from_root = |path: &Path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            }
        };
        Self {
            root: root.to_path_buf(),
            config_dir: from_root(Path::new(config_name)),
            output_dir: from_root(&general.output_folder),
            game_dir: general.game_folder.as_deref().map(from_root),
        }
    }

    pub fn boot_dir(&self) -> PathBuf {
        self.output_dir.join("boot")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayOptions {
    /// Record a boot trace in `Output/boot`.
    pub trace: bool,
    /// The frame limit as the window shows it; written the way `leonida run --frame-limit` reads it.
    pub frame_limit: Option<String>,
}

fn limit_argument(text: &str) -> Option<String> {
    Some(match parse_limit(text).ok()? {
        Limit::Console => "console".to_string(),
        Limit::Uncapped => "uncapped".to_string(),
        Limit::Vsync => "vsync".to_string(),
        Limit::Cap(fps) => fps.to_string(),
    })
}

/// The command that starts the game with the chosen settings. Only the Play button runs it.
pub fn play(program: &Path, paths: &Paths, options: &PlayOptions) -> Launch {
    let mut launch = Launch::new(program, &["run", "--start-the-game"]);
    if let Some(game) = &paths.game_dir {
        launch = launch.with("--game", game);
    }
    launch = launch
        .with("--config", &paths.config_dir)
        .with("--output", &paths.output_dir);
    if options.trace {
        launch.args.push("--trace".into());
    }
    if let Some(limit) = options.frame_limit.as_deref().and_then(limit_argument) {
        launch.args.push("--frame-limit".into());
        launch.args.push(limit.into());
    }
    launch
}

/// Translates every shader and builds the pipelines into the cache.
pub fn prepare_shaders(program: &Path, paths: &Paths) -> Launch {
    let mut launch = Launch::new(program, &["pipelines"]);
    if let Some(game) = &paths.game_dir {
        launch = launch.with("--game", game);
    }
    launch.with("--output", &paths.output_dir)
}

/// Finds what the nudity filter hides and writes the blocklist.
pub fn scan_content(program: &Path, paths: &Paths) -> Launch {
    let mut launch = Launch::new(program, &["content", "scan"]);
    if let Some(game) = &paths.game_dir {
        launch = launch.with("--game", game);
    }
    launch
        .with("--output", &paths.output_dir)
        .with("--config", &paths.config_dir.join(General::FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(game: Option<&str>) -> Paths {
        let general = General {
            game_folder: game.map(PathBuf::from),
            ..General::default()
        };
        Paths::of(Path::new("C:/Leonida"), "Config", &general)
    }

    fn texts(launch: &Launch) -> Vec<String> {
        launch
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn relative_folders_are_taken_from_where_leonida_is() {
        let full = paths(Some("E:/Games/GTA6"));
        assert_eq!(full.config_dir, Path::new("C:/Leonida/Config"));
        assert_eq!(full.output_dir, Path::new("C:/Leonida/Output"));
        assert_eq!(full.game_dir.as_deref(), Some(Path::new("E:/Games/GTA6")));
        assert_eq!(full.boot_dir(), Path::new("C:/Leonida/Output/boot"));
        let relative = paths(Some("Games/GTA6"));
        assert_eq!(
            relative.game_dir.unwrap(),
            Path::new("C:/Leonida/Games/GTA6")
        );
    }

    #[test]
    fn play_starts_the_game_with_the_chosen_folders() {
        let launch = play(
            Path::new("C:/Leonida/leonida.exe"),
            &paths(Some("E:/Games/GTA6")),
            &PlayOptions::default(),
        );
        let args = texts(&launch);
        assert_eq!(&args[..2], ["run", "--start-the-game"]);
        assert!(!launch.has_flag("--dry-run"));
        assert!(!launch.has_flag("--trace"));
        assert_eq!(
            launch.value_of("--game").unwrap(),
            Path::new("E:/Games/GTA6")
        );
        assert_eq!(
            launch.value_of("--config").unwrap(),
            Path::new("C:/Leonida/Config")
        );
        assert_eq!(
            launch.value_of("--output").unwrap(),
            Path::new("C:/Leonida/Output")
        );
        assert_eq!(launch.program, Path::new("C:/Leonida/leonida.exe"));
    }

    #[test]
    fn play_adds_the_trace_and_the_frame_limit_when_chosen() {
        let options = PlayOptions {
            trace: true,
            frame_limit: Some(" 90 ".into()),
        };
        let launch = play(Path::new("leonida"), &paths(None), &options);
        assert!(launch.has_flag("--trace"));
        assert_eq!(launch.value_of("--frame-limit").unwrap(), Path::new("90"));
        assert!(
            !launch.has_flag("--game"),
            "left out so Leonida looks for it"
        );
        for (said, passed) in [
            ("Default", "console"),
            ("unlimited", "uncapped"),
            ("VSYNC", "vsync"),
        ] {
            let options = PlayOptions {
                trace: false,
                frame_limit: Some(said.into()),
            };
            let launch = play(Path::new("leonida"), &paths(None), &options);
            assert_eq!(launch.value_of("--frame-limit").unwrap(), Path::new(passed));
        }
        let bad = PlayOptions {
            trace: false,
            frame_limit: Some("fast".into()),
        };
        assert!(!play(Path::new("leonida"), &paths(None), &bad).has_flag("--frame-limit"));
    }

    #[test]
    fn the_command_to_run_carries_the_same_arguments() {
        let launch = play(
            Path::new("leonida"),
            &paths(Some("E:/G")),
            &PlayOptions::default(),
        );
        let command = launch.command();
        assert_eq!(command.get_args().count(), launch.args.len());
        assert_eq!(command.get_program(), "leonida");
        assert!(
            launch
                .describe()
                .starts_with("leonida run --start-the-game --game E:/G")
        );
    }

    #[test]
    fn preparing_shaders_runs_the_pipelines_command_on_the_chosen_game() {
        let launch = prepare_shaders(Path::new("leonida"), &paths(Some("E:/Games/GTA6")));
        assert_eq!(texts(&launch)[0], "pipelines");
        assert_eq!(
            launch.value_of("--game").unwrap(),
            Path::new("E:/Games/GTA6")
        );
        assert_eq!(
            launch.value_of("--output").unwrap(),
            Path::new("C:/Leonida/Output")
        );
        assert!(!launch.has_flag("--start-the-game"));
    }

    #[test]
    fn scanning_content_reads_the_chosen_settings_file() {
        let launch = scan_content(Path::new("leonida"), &paths(Some("E:/Games/GTA6")));
        assert_eq!(texts(&launch)[..2], ["content", "scan"]);
        assert_eq!(
            launch.value_of("--config").unwrap(),
            Path::new("C:/Leonida/Config/leonida.toml")
        );
        assert_eq!(
            launch.value_of("--game").unwrap(),
            Path::new("E:/Games/GTA6")
        );
    }

    #[test]
    fn a_path_with_spaces_is_quoted_in_the_log_line() {
        let launch = prepare_shaders(
            Path::new("leonida"),
            &paths(Some("E:/Project Leonida V2/GTA")),
        );
        assert!(
            launch
                .describe()
                .contains("--game \"E:/Project Leonida V2/GTA\"")
        );
    }
}
