use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A package folder is one that holds this file.
pub const MARKER: &str = "MicrosoftGame.Config";

/// Folders under `roots` (the roots included) that hold `MARKER`, searched `depth` levels deep. A package's own
/// subfolders are not searched. Unreadable folders are skipped silently.
pub fn find_packages(roots: &[PathBuf], depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in roots {
        visit(root, depth, &mut found);
    }
    found.sort();
    found.dedup();
    found
}

fn visit(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if dir.join(MARKER).is_file() {
        found.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir && !is_skipped(&entry.file_name()) {
            visit(&entry.path(), depth - 1, found);
        }
    }
}

fn is_skipped(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.')
        || name.starts_with('$')
        || matches!(
            name.as_ref(),
            "Windows" | "System Volume Information" | "node_modules"
        )
}

/// Where a player's copy usually is: Downloads, Desktop, Documents, Games, the home folder, and the top of every
/// drive.
pub fn default_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        for sub in ["Downloads", "Desktop", "Documents", "Games"] {
            roots.push(home.join(sub));
        }
        roots.push(home);
    }
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            if drive.is_dir() {
                roots.push(drive);
            }
        }
    } else {
        roots.extend(["/", "/mnt", "/media"].map(PathBuf::from));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn package_at(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join(MARKER), b"<Game/>").unwrap();
    }

    #[test]
    fn finds_packages_within_depth() {
        let root = tempfile::tempdir().unwrap();
        package_at(&root.path().join("a").join("game"));
        package_at(&root.path().join("b"));
        package_at(&root.path().join("deep").join("x").join("y"));
        let found = find_packages(&[root.path().to_path_buf()], 2);
        assert_eq!(
            found,
            vec![root.path().join("a").join("game"), root.path().join("b")]
        );
    }

    #[test]
    fn root_itself_can_be_the_package_and_duplicates_collapse() {
        let root = tempfile::tempdir().unwrap();
        package_at(root.path());
        let roots = vec![root.path().to_path_buf(), root.path().to_path_buf()];
        assert_eq!(find_packages(&roots, 2), vec![root.path().to_path_buf()]);
    }

    #[test]
    fn hidden_and_missing_folders_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        package_at(&root.path().join(".hidden"));
        let roots = vec![
            root.path().to_path_buf(),
            root.path().join("does-not-exist"),
        ];
        assert!(find_packages(&roots, 2).is_empty());
    }

    #[test]
    fn default_roots_are_not_empty() {
        assert!(!default_roots().is_empty());
    }
}
