//! The write policy: every file Leonida or the game writes lands under `Output/`, and nothing else is written.

use std::path::{Path, PathBuf};

use crate::path::{PathError, VPath};

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("`{0}` would be written outside the Output folder")]
    OutsideOutput(PathBuf),
    #[error("the folder above `{0}` does not exist")]
    ParentMissing(String),
    #[error("`{0}` is a folder")]
    IsDirectory(String),
    #[error("`{0}` is not there")]
    NotFound(String),
    #[error("`{0}` is already there")]
    AlreadyExists(String),
    #[error("the folder `{0}` is not empty")]
    NotEmpty(String),
    #[error("`{0}` is part of the package and cannot be changed in place")]
    ReadOnly(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The folders under `Output/` that writes go to, by what is being written.
#[derive(Debug, Clone)]
pub struct WritePolicy {
    output: PathBuf,
}

impl WritePolicy {
    pub fn new(output: impl Into<PathBuf>) -> Self {
        Self {
            output: output.into(),
        }
    }

    pub fn output(&self) -> &Path {
        &self.output
    }

    /// What the game writes to files it names inside its own folder, and the changes it makes to the ones it
    /// reads. Reads see this folder before the tree.
    pub fn overlay_dir(&self) -> PathBuf {
        self.output.join("runtime").join("overlay")
    }

    /// Markers for package files the game deleted, one empty file per deleted path, named by its folded path.
    pub fn deleted_dir(&self) -> PathBuf {
        self.output.join("runtime").join("overlay-deleted")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.output.join("runtime").join("temp")
    }

    pub fn persistent_dir(&self) -> PathBuf {
        self.output.join("runtime").join("persistent")
    }

    pub fn saves_dir(&self) -> PathBuf {
        self.output.join("saves")
    }

    /// Where writes to other places on the PC land instead, one folder per drive letter.
    pub fn host_dir(&self) -> PathBuf {
        self.output.join("runtime").join("host")
    }

    /// A file a tool writes, such as `logs/run.txt` or `captures/a.png`, relative to `Output/`.
    pub fn tool(&self, relative: &str) -> Result<PathBuf, WriteError> {
        self.under(&self.output, &VPath::parse(relative)?)
    }

    /// Where a tool's write to a virtual path lands: the overlay the game also reads first.
    pub fn for_virtual(&self, path: &str) -> Result<PathBuf, WriteError> {
        self.under(&self.overlay_dir(), &VPath::parse(path)?)
    }

    /// One save blob. `XGameSave` has containers holding named blobs; both names are %-encoded into one folder
    /// level each. ASSUMED layout, taken from the one the upstream runtime uses (`local-user/<container>/<blob>`);
    /// the real container and blob names come from the executable's calls.
    pub fn save_blob(&self, container: &str, blob: &str) -> Result<PathBuf, WriteError> {
        for name in [container, blob] {
            if name.is_empty() {
                return Err(PathError::Invalid {
                    path: String::new(),
                    why: "a container or blob name is empty",
                }
                .into());
            }
        }
        let root = self.saves_dir().join("local-user");
        let folder = self.under(&root, &VPath::parse(&encode_name(container))?)?;
        self.under(&folder, &VPath::parse(&encode_name(blob))?)
    }

    pub fn under(&self, base: &Path, rel: &VPath) -> Result<PathBuf, WriteError> {
        let mut path = base.to_path_buf();
        for part in rel.parts() {
            path.push(part);
        }
        self.check(&path)?;
        Ok(path)
    }

    /// Refuses a path that is not inside `Output/` once links in its existing part are followed.
    pub fn check(&self, path: &Path) -> Result<(), WriteError> {
        if !path.starts_with(&self.output) {
            return Err(WriteError::OutsideOutput(path.to_path_buf()));
        }
        let Ok(root) = self.output.canonicalize() else {
            return Ok(());
        };
        let mut existing = path;
        while !existing.exists() {
            match existing.parent() {
                Some(parent) => existing = parent,
                None => return Ok(()),
            }
        }
        match existing.canonicalize() {
            Ok(real) if real.starts_with(&root) => Ok(()),
            _ => Err(WriteError::OutsideOutput(path.to_path_buf())),
        }
    }
}

/// Letters, digits, `-` and `_` stay; everything else, and a `.` at either end, becomes `%XX` per UTF-8 byte.
pub fn encode_name(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = String::new();
    for (i, &b) in bytes.iter().enumerate() {
        let edge_dot = b == b'.' && (i == 0 || i + 1 == bytes.len());
        if (b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) && !edge_dot {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_targets_are_inside_output() {
        let dir = tempfile::tempdir().unwrap();
        let policy = WritePolicy::new(dir.path().join("Output"));
        assert!(
            policy
                .tool("logs/run.txt")
                .unwrap()
                .starts_with(policy.output())
        );
        assert!(
            policy
                .for_virtual("Systems/a.xml")
                .unwrap()
                .starts_with(policy.overlay_dir())
        );
        assert!(policy.tool("../Mods/x").is_err());
        assert!(policy.for_virtual("//server/share/x").is_err());
        assert!(policy.check(dir.path()).is_err());
    }

    #[test]
    fn save_names_are_encoded_into_one_part_each() {
        assert_eq!(encode_name("slot 1/../x"), "slot%201%2F..%2Fx");
        assert_eq!(encode_name(".hidden."), "%2Ehidden%2E");
        let dir = tempfile::tempdir().unwrap();
        let policy = WritePolicy::new(dir.path());
        let path = policy.save_blob("../escape", "a:b").unwrap();
        assert!(path.starts_with(policy.saves_dir()));
        assert!(path.ends_with("a%3Ab"));
        assert!(policy.save_blob("", "x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_output_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let output = dir.path().join("Output");
        std::fs::create_dir_all(&output).unwrap();
        std::os::unix::fs::symlink(&outside, output.join("link")).unwrap();
        let policy = WritePolicy::new(&output);
        assert!(policy.tool("link/x.txt").is_err());
    }
}
