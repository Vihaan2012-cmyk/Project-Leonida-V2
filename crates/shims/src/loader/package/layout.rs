//! What the game folder holds, read from its own files: the chunks of `layout_*.xml` (each a list of files
//! below the folder) and the languages of `MicrosoftGame.Config`. A chunk is installed when every file
//! it lists is present; the folder is a finished install, so nothing is ever pending or installable.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Chunk {
    pub id: u32,
    files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Layout {
    root: PathBuf,
    pub chunks: Vec<Chunk>,
    pub languages: Vec<String>,
}

/// The value of `name="..."` inside one tag's text.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let from = tag.find(&key)? + key.len();
    let to = tag[from..].find('"')? + from;
    Some(tag[from..to].to_string())
}

/// Every `<Name ...>` tag of `xml` in order, as its inner text.
fn tags(xml: &str) -> impl Iterator<Item = &str> {
    xml.split('<')
        .skip(1)
        .filter_map(|part| part.split('>').next())
}

fn matches_glob(pattern: &str, name: &str) -> bool {
    let Some((head, tail)) = pattern.split_once('*') else {
        return pattern.eq_ignore_ascii_case(name);
    };
    name.len() >= head.len() + tail.len()
        && name[..head.len()].eq_ignore_ascii_case(head)
        && name[name.len() - tail.len()..].eq_ignore_ascii_case(tail)
}

impl Chunk {
    /// Whether every file the chunk lists is in the folder. A pattern with `*` needs one match.
    fn present(&self, root: &Path) -> bool {
        self.files.iter().all(|file| {
            let path = root.join(file);
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            if !name.contains('*') {
                return path.is_file();
            }
            path.parent()
                .and_then(|dir| std::fs::read_dir(dir).ok())
                .is_some_and(|mut entries| {
                    entries.any(|e| {
                        e.ok()
                            .and_then(|e| e.file_name().into_string().ok())
                            .is_some_and(|n| matches_glob(name, &n))
                    })
                })
        })
    }

    /// The bytes of the files that are there.
    fn size(&self, root: &Path) -> u64 {
        self.files
            .iter()
            .filter_map(|f| std::fs::metadata(root.join(f)).ok())
            .map(|m| m.len())
            .sum()
    }
}

impl Layout {
    pub fn parse(root: &Path, layout: &str, config: &str) -> Layout {
        let mut chunks: Vec<Chunk> = Vec::new();
        for tag in tags(layout) {
            if tag.starts_with("Chunk ") {
                if let Some(id) = attribute(tag, "Id").and_then(|id| id.parse().ok()) {
                    chunks.push(Chunk {
                        id,
                        files: Vec::new(),
                    });
                }
            } else if tag.starts_with("FileGroup ")
                && let (Some(chunk), Some(include)) = (chunks.last_mut(), attribute(tag, "Include"))
            {
                let destination = attribute(tag, "DestinationPath").unwrap_or_default();
                let relative = format!("{destination}/{include}")
                    .split(['\\', '/'])
                    .filter(|part| !part.is_empty())
                    .collect::<PathBuf>();
                chunk.files.push(relative);
            }
        }
        let languages = tags(config)
            .filter(|tag| tag.starts_with("Resource "))
            .filter_map(|tag| attribute(tag, "Language"))
            .collect();
        Layout {
            root: root.to_path_buf(),
            chunks,
            languages,
        }
    }

    /// The layout of the game folder; empty when it has none.
    pub fn read(root: &Path) -> Layout {
        let layout = std::fs::read_dir(root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                name.starts_with("layout_") && name.ends_with(".xml")
            })
            .map(|e| e.path())
            .min()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        let config = std::fs::read_to_string(root.join("MicrosoftGame.Config")).unwrap_or_default();
        Layout::parse(root, &layout, &config)
    }

    pub fn chunk_installed(&self, id: u32) -> bool {
        self.chunks
            .iter()
            .find(|c| c.id == id)
            .is_some_and(|c| c.present(&self.root))
    }

    pub fn language_installed(&self, tag: &str) -> bool {
        self.languages.iter().any(|l| l.eq_ignore_ascii_case(tag))
    }

    /// The bytes of the installed chunks, or of one chunk.
    pub fn installed_bytes(&self, only: Option<u32>) -> u64 {
        self.chunks
            .iter()
            .filter(|c| only.is_none_or(|id| c.id == id) && c.present(&self.root))
            .map(|c| c.size(&self.root))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: &str = r#"<?xml version="1.0"?>
<Package>
  <Chunk Id="1000">
    <FileGroup SourcePath=".\" DestinationPath="\" Include="a.bin" />
  </Chunk>
  <Chunk Id="3000">
    <FileGroup SourcePath=".\" DestinationPath="\pack\one\data\" Include="b.bin" />
    <FileGroup SourcePath=".\" DestinationPath="\pack\one\" Include="*.rpf" />
  </Chunk>
</Package>"#;
    const CONFIG: &str = r#"<Game><Resources><Resource Language="en-US" /><Resource Language="de-DE" /></Resources></Game>"#;

    fn folder(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn chunks_are_installed_when_all_their_files_are_there() {
        let dir = folder("layout-chunks");
        let layout = Layout::parse(&dir, LAYOUT, CONFIG);
        assert_eq!(
            layout.chunks.iter().map(|c| c.id).collect::<Vec<_>>(),
            [1000, 3000]
        );
        assert!(!layout.chunk_installed(1000));
        std::fs::write(dir.join("a.bin"), [0u8; 10]).unwrap();
        assert!(layout.chunk_installed(1000));
        std::fs::create_dir_all(dir.join("pack/one/data")).unwrap();
        std::fs::write(dir.join("pack/one/data/b.bin"), [0u8; 5]).unwrap();
        assert!(
            !layout.chunk_installed(3000),
            "the pattern has no match yet"
        );
        std::fs::write(dir.join("pack/one/x.RPF"), [0u8; 7]).unwrap();
        assert!(layout.chunk_installed(3000));
        assert!(!layout.chunk_installed(4000), "no such chunk");
        assert_eq!(layout.installed_bytes(Some(1000)), 10);
        assert_eq!(layout.installed_bytes(None), 10 + 5);
    }

    #[test]
    fn languages_come_from_the_config() {
        let layout = Layout::parse(Path::new("."), LAYOUT, CONFIG);
        assert!(layout.language_installed("DE-de"));
        assert!(!layout.language_installed("fr-FR"));
    }
}
