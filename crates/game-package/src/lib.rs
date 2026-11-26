//! The player's game package: which version it is, whether it is complete, and where it is.

mod check;
mod manifest;

use std::path::{Path, PathBuf};

pub use check::{CheckReport, check_files, sha256_file};
pub use manifest::{
    FileEntry, Manifest, ManifestError, Mismatch, PROBES, match_version, probe_mismatches,
};

/// Path of a manifest entry (always `/`-separated) inside the package folder.
pub fn package_path(game: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(game.to_path_buf(), |path, part| path.join(part))
}
