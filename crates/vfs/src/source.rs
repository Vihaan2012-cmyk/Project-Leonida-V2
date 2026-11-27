//! Where a layer's files come from.

use std::io::{self, Cursor, Read, Seek};
use std::path::{Path, PathBuf};

pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// `/`-separated, relative to the source's root, in the source's own spelling.
    pub path: String,
    pub size: u64,
}

/// A read-only provider of files. The loose-folder source below serves the base game and packages; the `rpf9`
/// index plugs in as another implementation that lists archive members under their readable names.
pub trait Source: Send + Sync {
    /// Every file, listed once. Folders are implied by the paths.
    fn scan(&self) -> io::Result<Vec<SourceFile>>;

    fn read(&self, path: &str) -> io::Result<Vec<u8>>;

    fn open(&self, path: &str) -> io::Result<Box<dyn ReadSeek>> {
        Ok(Box::new(Cursor::new(self.read(path)?)))
    }

    /// The real file for `path` when the source is a folder on disk. Archive members have none; the caller reads
    /// them through `read` or `open`.
    fn host_path(&self, _path: &str) -> Option<PathBuf> {
        None
    }
}

/// A folder on disk. Links are not followed, so a package cannot expose files outside its folder.
#[derive(Debug, Clone)]
pub struct LooseSource {
    root: PathBuf,
    skip_root_files: Vec<String>,
}

impl LooseSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            skip_root_files: Vec::new(),
        }
    }

    /// A package folder: its `manifest.json` is metadata, not content.
    pub fn package(root: impl Into<PathBuf>) -> Self {
        let mut source = Self::new(root);
        source.skip_root_files.push("manifest.json".to_string());
        source
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn walk(&self, dir: &Path, prefix: &str, out: &mut Vec<SourceFile>) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                self.walk(&entry.path(), &path, out)?;
            } else if kind.is_file() {
                if prefix.is_empty()
                    && self
                        .skip_root_files
                        .iter()
                        .any(|skip| skip.eq_ignore_ascii_case(&name))
                {
                    continue;
                }
                out.push(SourceFile {
                    path,
                    size: entry.metadata()?.len(),
                });
            }
        }
        Ok(())
    }
}

impl Source for LooseSource {
    fn scan(&self) -> io::Result<Vec<SourceFile>> {
        let mut files = Vec::new();
        self.walk(&self.root, "", &mut files)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.root.join(path))
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn ReadSeek>> {
        Ok(Box::new(std::fs::File::open(self.root.join(path))?))
    }

    fn host_path(&self, path: &str) -> Option<PathBuf> {
        Some(self.root.join(path))
    }
}

/// Files held in memory, for tests and generated content.
#[derive(Debug, Default, Clone)]
pub struct MemorySource {
    files: Vec<(String, Vec<u8>)>,
}

impl MemorySource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, path: &str, bytes: impl Into<Vec<u8>>) -> Self {
        self.files.push((path.to_string(), bytes.into()));
        self
    }
}

impl Source for MemorySource {
    fn scan(&self) -> io::Result<Vec<SourceFile>> {
        Ok(self
            .files
            .iter()
            .map(|(path, bytes)| SourceFile {
                path: path.clone(),
                size: bytes.len() as u64,
            })
            .collect())
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.files
            .iter()
            .find(|(name, _)| name == path)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_string()))
    }
}
