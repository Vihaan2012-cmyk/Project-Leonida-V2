//! Checks that look at the PC rather than at the settings alone: does the game folder hold a supported copy, does
//! the DLSS file exist, is that screen connected.

use std::path::Path;

use config::{Config, Upscaler};
use game_package::Manifest;

use crate::game::GameState;
use crate::model::FieldError;
use crate::play::Paths;

pub struct World<'a> {
    pub manifests: &'a [Manifest],
    pub paths: &'a Paths,
    /// How many screens are connected.
    pub monitors: usize,
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
    match &config.upscaling.dlss_path {
        Some(path) if config.graphics.upscaler == Upscaler::Dlss && !path.is_file() => {
            found.push(FieldError::error(
                "upscaling.dlss_path",
                "there is no file at this path",
            ));
        }
        None if config.graphics.upscaler == Upscaler::Dlss => found.push(FieldError::note(
            "upscaling.dlss_path",
            "without it the temporal upscaler runs instead",
        )),
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
        let paths = Paths::of(root, "Config", &config.general);
        check(
            config,
            &World {
                manifests: &manifests(),
                paths: &paths,
                monitors,
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
        let found = run(&config, root.path(), 1);
        assert_eq!(found[0].severity, Severity::Note);
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
