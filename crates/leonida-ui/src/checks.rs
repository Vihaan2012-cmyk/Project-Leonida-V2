//! Checks that look at the PC rather than at the settings alone: does the game folder hold a supported copy, does
//! the DLSS file exist, is that screen connected.

use std::path::Path;

use config::{Config, Upscaler};
use game_package::Manifest;

use crate::dlss::Line;
use crate::game::GameState;
use crate::model::FieldError;
use crate::play::Paths;

pub struct World<'a> {
    pub manifests: &'a [Manifest],
    pub paths: &'a Paths,
    /// How many screens are connected.
    pub monitors: usize,
    /// What NGX said about DLSS on this PC; `None` while DLSS is not the chosen upscaler.
    pub dlss: Option<&'a Line>,
}

/// What the game folder is, as the Game page shows it.
pub fn game_state(path: &Path, manifests: &[Manifest]) -> Result<GameState, String> {
    if !path.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    let state = GameState::recognise(path.to_path_buf(), manifests);
    if state.is_supported() {
        Ok(state)
    } else {
        Err(format!(
            "{} does not hold a supported copy of the game",
            path.display()
        ))
    }
}

/// The hint beside the DLSS library field: what NGX said, in the words `leonida config dlss` uses.
fn dlss_line(line: &Line) -> FieldError {
    const ID: &str = "upscaling.dlss_path";
    match line {
        Line::Checking => FieldError::info(ID, "Checking whether DLSS can run on this PC..."),
        Line::Available(summary) => FieldError::info(ID, format!("DLSS is {summary}")),
        Line::Unavailable(summary) => FieldError::note(
            ID,
            format!("DLSS is {summary}. The temporal upscaler runs instead."),
        ),
    }
}

pub fn check(config: &Config, world: &World) -> Vec<FieldError> {
    let mut found = Vec::new();
    if let Some(game) = &world.paths.game_dir
        && config.general.game_folder.is_some()
        && let Err(message) = game_state(game, world.manifests)
    {
        found.push(FieldError::error("general.game_folder", message));
    }
    if world.paths.output_dir.exists() && !world.paths.output_dir.is_dir() {
        found.push(FieldError::error(
            "general.output_folder",
            "is a file, not a folder",
        ));
    }
    let dlss_chosen = config.graphics.upscaler == Upscaler::Dlss;
    match &config.upscaling.dlss_path {
        Some(path) if dlss_chosen && !path.is_file() => {
            found.push(FieldError::error(
                "upscaling.dlss_path",
                "there is no file at this path",
            ));
        }
        _ if dlss_chosen => found.extend(world.dlss.map(dlss_line)),
        _ => {}
    }
    let screens = config.display.monitor as usize;
    if screens > world.monitors.max(1) {
        found.push(FieldError::note(
            "display.monitor",
            format!("screen {screens} is not connected; the primary screen is used"),
        ));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Severity;
    use std::path::PathBuf;

    const MANIFEST: &str = r#"{
        "title": "Test package", "version": "9.9", "expected_exe": "game.exe",
        "files": {
            "MicrosoftGame.Config": {"size": 5, "sha256": "x"},
            "Catalog000.bin": {"size": 3, "sha256": "x"}
        }
    }"#;

    fn manifests() -> Vec<Manifest> {
        vec![Manifest::from_json(MANIFEST).unwrap()]
    }

    fn package(dir: &Path) {
        for (name, len) in [("MicrosoftGame.Config", 5), ("Catalog000.bin", 3)] {
            std::fs::File::create(dir.join(name))
                .unwrap()
                .set_len(len)
                .unwrap();
        }
    }

    fn run(config: &Config, root: &Path, monitors: usize) -> Vec<FieldError> {
        run_with(config, root, monitors, None)
    }

    fn run_with(
        config: &Config,
        root: &Path,
        monitors: usize,
        dlss: Option<&Line>,
    ) -> Vec<FieldError> {
        let paths = Paths::of(root, "Config", &config.general);
        check(
            config,
            &World {
                manifests: &manifests(),
                paths: &paths,
                monitors,
                dlss,
            },
        )
    }

    #[test]
    fn a_supported_game_folder_passes_and_others_are_named() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let mut config = Config::default();
        config.general.game_folder = Some(game.clone());
        let found = run(&config, root.path(), 1);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.as_deref(), Some("general.game_folder"));
        assert!(found[0].message.contains("does not hold a supported copy"));
        package(&game);
        assert!(run(&config, root.path(), 1).is_empty());
        config.general.game_folder = Some(root.path().join("elsewhere"));
        assert!(
            run(&config, root.path(), 1)[0]
                .message
                .contains("is not a folder")
        );
        config.general.game_folder = None;
        assert!(
            run(&config, root.path(), 1).is_empty(),
            "left empty means it is searched for"
        );
    }

    #[test]
    fn a_relative_game_folder_is_looked_for_under_the_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("game")).unwrap();
        package(&root.path().join("game"));
        let mut config = Config::default();
        config.general.game_folder = Some(PathBuf::from("game"));
        assert!(run(&config, root.path(), 1).is_empty());
    }

    #[test]
    fn dlss_wants_its_file() {
        let root = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.graphics.upscaler = Upscaler::Dlss;
        assert!(
            run(&config, root.path(), 1).is_empty(),
            "nothing is said until NGX has been asked"
        );
        let dll = root.path().join("nvngx_dlss.dll");
        config.upscaling.dlss_path = Some(dll.clone());
        let found = run(&config, root.path(), 1);
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].id.as_deref(), Some("upscaling.dlss_path"));
        std::fs::write(&dll, b"x").unwrap();
        assert!(run(&config, root.path(), 1).is_empty());
        config.graphics.upscaler = Upscaler::Off;
        std::fs::remove_file(&dll).unwrap();
        assert!(
            run(&config, root.path(), 1).is_empty(),
            "only checked while DLSS is chosen"
        );
    }

    #[test]
    fn the_dlss_report_is_shown_beside_the_library_field() {
        let root = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        let available =
            Line::Available("available on a GPU (nvngx_dlss.dll from NVIDIA's folders)".into());
        let said = |line: &Line, config: &Config| run_with(config, root.path(), 1, Some(line));
        assert!(
            said(&available, &config).is_empty(),
            "only while DLSS is chosen"
        );
        config.graphics.upscaler = Upscaler::Dlss;
        let found = said(&Line::Checking, &config);
        assert_eq!(found[0].severity, Severity::Info);
        let found = said(&available, &config);
        assert_eq!(found[0].severity, Severity::Info);
        assert_eq!(
            found[0].message,
            "DLSS is available on a GPU (nvngx_dlss.dll from NVIDIA's folders)"
        );
        assert_eq!(found[0].id.as_deref(), Some("upscaling.dlss_path"));
        let found = said(
            &Line::Unavailable("unavailable: no NVIDIA GPU".into()),
            &config,
        );
        assert_eq!(found[0].severity, Severity::Note);
        assert!(
            found[0]
                .message
                .starts_with("DLSS is unavailable: no NVIDIA GPU")
        );
        assert!(!found[0].message.contains("without it"));
    }

    #[test]
    fn a_screen_that_is_not_connected_is_a_note() {
        let root = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.display.monitor = 3;
        let found = run(&config, root.path(), 2);
        assert_eq!(found[0].severity, Severity::Note);
        assert!(run(&config, root.path(), 3).is_empty());
        config.display.monitor = 0;
        assert!(run(&config, root.path(), 1).is_empty());
    }

    #[test]
    fn an_output_folder_that_is_a_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Output"), b"x").unwrap();
        let found = run(&Config::default(), root.path(), 1);
        assert_eq!(found[0].id.as_deref(), Some("general.output_folder"));
    }
}
