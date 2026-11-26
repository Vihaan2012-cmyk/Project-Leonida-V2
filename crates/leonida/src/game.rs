use std::path::PathBuf;

use anyhow::{Result, bail};
use game_package::{Manifest, default_roots, find_packages, match_version};

/// How many folder levels below each search root a package may be.
const SEARCH_DEPTH: usize = 2;

/// The game folder to use: the one given, or the only supported copy found on this PC.
pub fn resolve_game(given: Option<&PathBuf>, manifests: &[Manifest]) -> Result<PathBuf> {
    if let Some(path) = given {
        if !path.is_dir() {
            bail!("game folder not found: {}", path.display());
        }
        return Ok(path.clone());
    }
    let candidates: Vec<PathBuf> = find_packages(&default_roots(), SEARCH_DEPTH)
        .into_iter()
        .filter(|path| match_version(path, manifests).is_some())
        .collect();
    match candidates.as_slice() {
        [] => bail!(
            "no supported copy of the game was found in Downloads, Desktop, Documents, Games, your home folder or \
             the top of any drive. Pass --game <folder> (the folder that holds MicrosoftGame.Config)."
        ),
        [only] => Ok(only.clone()),
        many => {
            let list: Vec<String> = many
                .iter()
                .map(|path| format!("  {}", path.display()))
                .collect();
            bail!(
                "more than one copy was found; pick one with --game:\n{}",
                list.join("\n")
            )
        }
    }
}
