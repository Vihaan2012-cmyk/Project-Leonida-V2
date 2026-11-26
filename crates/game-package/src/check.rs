use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::{FileEntry, Manifest, package_path};

/// The result of checking a package folder against a manifest.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub checked: usize,
    pub missing: Vec<String>,
    pub wrong_size: Vec<String>,
    pub hash_mismatch: Vec<String>,
    pub unreadable: Vec<String>,
    pub exe_present: bool,
}

impl CheckReport {
    pub fn problem_count(&self) -> usize {
        self.missing.len()
            + self.wrong_size.len()
            + self.hash_mismatch.len()
            + self.unreadable.len()
    }

    pub fn is_ok(&self) -> bool {
        self.problem_count() == 0
    }
}

/// Check every manifest file for presence and size; with `full`, also compare SHA-256 (reads every byte, in
/// parallel). Lists are in manifest (sorted path) order.
pub fn check_files(game: &Path, manifest: &Manifest, full: bool) -> CheckReport {
    let mut report = CheckReport {
        checked: manifest.files.len(),
        ..CheckReport::default()
    };
    let mut present: Vec<(&String, &FileEntry)> = Vec::new();
    for (rel, entry) in &manifest.files {
        match std::fs::metadata(package_path(game, rel)) {
            Ok(meta) if meta.is_file() && meta.len() == entry.size => present.push((rel, entry)),
            Ok(meta) if meta.is_file() => report.wrong_size.push(rel.clone()),
            _ => report.missing.push(rel.clone()),
        }
    }
    if full {
        let hashed: Vec<(&String, &FileEntry, io::Result<String>)> = present
            .par_iter()
            .map(|&(rel, entry)| (rel, entry, sha256_file(&package_path(game, rel))))
            .collect();
        for (rel, entry, result) in hashed {
            match result {
                Ok(hash) if hash.eq_ignore_ascii_case(&entry.sha256) => {}
                Ok(_) => report.hash_mismatch.push(rel.clone()),
                Err(_) => report.unreadable.push(rel.clone()),
            }
        }
    }
    report.exe_present = package_path(game, &manifest.expected_exe).is_file();
    report
}

/// SHA-256 of a file's contents, as lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Manifest;
    use std::fs;
    use std::path::Path;

    // sha256("hello"), sha256("abc"), sha256("")
    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn manifest() -> Manifest {
        Manifest::from_json(&format!(
            r#"{{"title":"Test package","version":"9.9","expected_exe":"game.exe","files":{{
                "MicrosoftGame.Config":{{"size":5,"sha256":"{HELLO}"}},
                "Catalog000.bin":{{"size":3,"sha256":"{ABC}"}},
                "appdata0.rpf":{{"size":0,"sha256":"{EMPTY}"}},
                "data/other.rpf":{{"size":3,"sha256":"{ABC}"}}}}}}"#
        ))
        .unwrap()
    }

    fn put(dir: &Path, rel: &str, content: &[u8]) {
        let path = crate::package_path(dir, rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn complete_package() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "MicrosoftGame.Config", b"hello");
        put(dir.path(), "Catalog000.bin", b"abc");
        put(dir.path(), "appdata0.rpf", b"");
        put(dir.path(), "data/other.rpf", b"abc");
        dir
    }

    #[test]
    fn sha256_of_known_content() {
        let dir = complete_package();
        assert_eq!(
            sha256_file(&dir.path().join("Catalog000.bin")).unwrap(),
            ABC
        );
    }

    #[test]
    fn complete_package_passes_full_check() {
        let dir = complete_package();
        let report = check_files(dir.path(), &manifest(), true);
        assert!(report.is_ok(), "{report:?}");
        assert_eq!(report.checked, 4);
        assert!(!report.exe_present);
    }

    #[test]
    fn size_check_finds_missing_and_wrong_size() {
        let dir = complete_package();
        fs::remove_file(dir.path().join("Catalog000.bin")).unwrap();
        put(dir.path(), "data/other.rpf", b"abcd");
        let report = check_files(dir.path(), &manifest(), false);
        assert_eq!(report.missing, vec!["Catalog000.bin".to_string()]);
        assert_eq!(report.wrong_size, vec!["data/other.rpf".to_string()]);
        assert_eq!(report.problem_count(), 2);
        assert!(!report.is_ok());
    }

    #[test]
    fn full_check_finds_same_size_changes() {
        let dir = complete_package();
        put(dir.path(), "data/other.rpf", b"abd");
        assert!(check_files(dir.path(), &manifest(), false).is_ok());
        let report = check_files(dir.path(), &manifest(), true);
        assert_eq!(report.hash_mismatch, vec!["data/other.rpf".to_string()]);
    }

    #[test]
    fn executable_presence_is_reported() {
        let dir = complete_package();
        put(dir.path(), "game.exe", b"stand-in");
        assert!(check_files(dir.path(), &manifest(), false).exe_present);
    }
}
