//! The player's game package: which version it is, whether it is complete, and where it is.

mod check;
mod find;
mod manifest;

use std::path::{Path, PathBuf};

pub use check::{CheckReport, check_files, sha256_file};
pub use find::{MARKER, default_roots, find_packages};
pub use manifest::{
    FileEntry, Manifest, ManifestError, Mismatch, PROBES, match_version, probe_mismatches,
};

/// Path of a manifest entry (always `/`-separated) inside the package folder.
pub fn package_path(game: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(game.to_path_buf(), |path, part| path.join(part))
}

/// Map a file read-only. Every archive reader uses this instead of reading multi-gigabyte files into memory.
#[allow(unsafe_code)]
pub fn map_file(file: &std::fs::File) -> std::io::Result<memmap2::Mmap> {
    // SAFETY: Leonida never writes to the game package and only reads through this mapping. If another program
    // truncated the file while it is mapped, reads could fault; that is accepted for a read-only game package.
    unsafe { memmap2::Mmap::map(file) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn map_file_reads_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"mapped bytes").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let map = super::map_file(&file).unwrap();
        assert_eq!(&map[..], b"mapped bytes");
    }
}
