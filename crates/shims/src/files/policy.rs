//! What the file sandbox checks beyond where a path maps: that every name is a plain file name, and that the
//! file, once links are followed, still lies inside the folder it was mapped into.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::map::{Refusal, Roots, Target};

const CACHE_LIMIT: usize = 1 << 14;

static FOLDERS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

const DEVICES: &[&str] = &["con", "prn", "aux", "nul", "conin$", "conout$"];

/// Rejects a path component Windows would not treat as a plain file name.
pub(super) fn check_name(name: &str) -> Result<(), Refusal> {
    if name.chars().any(|c| c < ' ' || "<>\"|*?:".contains(c)) {
        return Err(Refusal::Invalid);
    }
    if name.ends_with(['.', ' ']) {
        return Err(Refusal::Invalid);
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let lower = stem.to_lowercase();
    let numbered = |prefix: &str| {
        lower.strip_prefix(prefix).is_some_and(|rest| {
            matches!(rest.chars().collect::<Vec<_>>().as_slice(), [d] if ('1'..='9').contains(d) || "\u{b9}\u{b2}\u{b3}".contains(*d))
        })
    };
    if DEVICES.contains(&lower.as_str()) || numbered("com") || numbered("lpt") {
        return Err(Refusal::Device);
    }
    Ok(())
}

/// Forgets which folders were found to be plain, for a new set of roots.
pub(super) fn forget() {
    if let Ok(mut folders) = FOLDERS.lock() {
        *folders = None;
    }
}

/// Lower case, backslashes, no `\\?\` prefix and no trailing separator.
fn key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let text = match text.strip_prefix("\\\\?\\unc\\") {
        Some(rest) => format!("\\\\{rest}"),
        None => text.strip_prefix("\\\\?\\").unwrap_or(&text).to_string(),
    };
    text.trim_end_matches('\\').to_string()
}

/// Where `folder` really is: the deepest part that exists with its links followed, and the rest as named.
fn real_folder(folder: &Path) -> Option<String> {
    let name = key(folder);
    if let Some(found) = FOLDERS
        .lock()
        .ok()
        .and_then(|f| f.as_ref().and_then(|f| f.get(&name).cloned()))
    {
        return Some(found);
    }
    let mut tail: Vec<String> = Vec::new();
    let mut here = folder.to_path_buf();
    let mut real = loop {
        match std::fs::canonicalize(&here) {
            Ok(found) => break key(&found),
            Err(_) => {
                tail.push(key(Path::new(here.file_name()?)));
                if !here.pop() {
                    return None;
                }
            }
        }
    };
    for part in tail.iter().rev() {
        real.push('\\');
        real.push_str(part);
    }
    if tail.is_empty()
        && let Ok(mut folders) = FOLDERS.lock()
    {
        let folders = folders.get_or_insert_with(HashMap::new);
        if folders.len() >= CACHE_LIMIT {
            folders.clear();
        }
        folders.insert(name, real.clone());
    }
    Some(real)
}

/// Where `path` really is, with every link in it followed.
fn real_path(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let name = key(Path::new(path.file_name()?));
    let mut real = format!("{}\\{name}", real_folder(parent)?);
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        real = key(&std::fs::canonicalize(path).ok()?);
    }
    Some(real)
}

fn inside(real: &str, root: &str) -> bool {
    real == root || real.strip_prefix(root).is_some_and(|r| r.starts_with('\\'))
}

/// Whether `path`, with links followed, lies in one of `roots`.
fn contained<'a>(path: &Path, mut roots: impl Iterator<Item = &'a Path>) -> bool {
    let Some(real) = real_path(path) else {
        return false;
    };
    roots.any(|root| real_folder(root).is_some_and(|root| inside(&real, &root)))
}

impl Roots {
    /// Checks a mapped path against the sandbox: what the game reads lies in the package or a read-only folder,
    /// what it writes or lists lies in `Output/`, and no link in between leads out. Does nothing unsandboxed.
    pub fn verify(&self, target: &Target) -> Result<(), Refusal> {
        if !self.is_sandboxed() {
            return Ok(());
        }
        let output = || std::iter::once(self.output());
        match target {
            Target::Layered { base, overlay } => {
                if !contained(base, self.read_roots()) || !contained(overlay, output()) {
                    return Err(Refusal::Link);
                }
            }
            Target::Direct { path, .. } => {
                if !contained(path, output()) {
                    return Err(Refusal::Link);
                }
            }
            Target::Passthrough => return Err(Refusal::Pipe),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_pass() {
        for name in [
            "a.bin", "Data", "x y", "a.b.c", ".hidden", "console", "com10", "nul_x",
        ] {
            assert_eq!(check_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn streams_wildcards_and_dots_are_invalid() {
        for name in [
            "a.txt:stream",
            "a.txt::$DATA",
            "a*",
            "a?.bin",
            "a<b",
            "a|b",
            "\"q\"",
            "...",
            "a.",
            "a ",
            "tab\there",
        ] {
            assert_eq!(check_name(name), Err(Refusal::Invalid), "{name:?}");
        }
    }

    #[test]
    fn device_names_are_refused_with_any_extension_and_case() {
        for name in [
            "NUL",
            "nul.txt",
            "Con",
            "AUX.bin",
            "prn",
            "COM1",
            "com9.x",
            "LPT3",
            "lpt1.log",
            "CONIN$",
            "CONOUT$",
            "COM\u{b9}",
            "nul ",
        ] {
            let wanted = if name.ends_with(' ') {
                Refusal::Invalid
            } else {
                Refusal::Device
            };
            assert_eq!(check_name(name), Err(wanted), "{name:?}");
        }
    }

    #[test]
    fn roots_are_compared_by_whole_names() {
        assert!(inside("e:\\games\\pkg", "e:\\games\\pkg"));
        assert!(inside("e:\\games\\pkg\\a", "e:\\games\\pkg"));
        assert!(!inside("e:\\games\\pkg2\\a", "e:\\games\\pkg"));
        assert!(!inside("e:\\games", "e:\\games\\pkg"));
    }

    #[test]
    fn keys_ignore_case_prefix_and_separators() {
        assert_eq!(key(Path::new("\\\\?\\E:\\A\\B\\")), "e:\\a\\b");
        assert_eq!(key(Path::new("E:/A/b")), "e:\\a\\b");
        assert_eq!(key(Path::new("\\\\?\\UNC\\srv\\s")), "\\\\srv\\s");
    }
}
