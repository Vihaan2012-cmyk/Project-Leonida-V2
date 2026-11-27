//! Virtual paths: forward-slash, relative to the tree root, compared without regard to case like Windows.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("`{path}` is refused: it would reach {}", .reason.describe())]
    Refused {
        path: String,
        reason: offline::Reason,
    },
    #[error("`{0}` climbs above the root of the tree")]
    Escapes(String),
    #[error("`{path}` is not a valid path: {why}")]
    Invalid { path: String, why: &'static str },
}

/// A cleaned virtual path: `display` keeps the spelling it was given, `key` is the case-folded form used to
/// compare. The root is the empty path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VPath {
    display: String,
    key: String,
}

impl VPath {
    /// Cleans `raw`: either slash is a separator, `.` and empty parts vanish, `..` steps up (never above the
    /// root), trailing dots and spaces on a part are dropped as Windows does, and `:`, control characters and
    /// device names (`CON`, `NUL`, ...) are rejected. Paths `offline::path` names are refused first.
    pub fn parse(raw: &str) -> Result<VPath, PathError> {
        if let Some(reason) = offline::path(raw) {
            return Err(PathError::Refused {
                path: raw.to_string(),
                reason,
            });
        }
        let mut parts: Vec<&str> = Vec::new();
        for part in raw.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    if parts.pop().is_none() {
                        return Err(PathError::Escapes(raw.to_string()));
                    }
                }
                _ => {
                    let part = part.trim_end_matches(['.', ' ']);
                    if part.is_empty() {
                        return Err(invalid(raw, "a part is only dots or spaces"));
                    }
                    if part.contains(':') {
                        return Err(invalid(raw, "a part contains `:`"));
                    }
                    if part
                        .chars()
                        .any(|c| c.is_control() || "<>\"|?*".contains(c))
                    {
                        return Err(invalid(raw, "a part contains a character Windows forbids"));
                    }
                    if is_device_name(part) {
                        return Err(invalid(raw, "a part is a reserved device name"));
                    }
                    parts.push(part);
                }
            }
        }
        let display = parts.join("/");
        let key = fold(&display);
        Ok(VPath { display, key })
    }

    pub fn root() -> VPath {
        VPath {
            display: String::new(),
            key: String::new(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.display
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn is_root(&self) -> bool {
        self.display.is_empty()
    }

    /// The last part, in the spelling given.
    pub fn file_name(&self) -> &str {
        self.display.rsplit('/').next().unwrap_or("")
    }

    /// Everything above the last part; the root for a one-part path.
    pub fn parent(&self) -> VPath {
        match self.display.rsplit_once('/') {
            Some((head, _)) => VPath {
                display: head.to_string(),
                key: fold(head),
            },
            None => VPath::root(),
        }
    }

    pub fn join(&self, name: &str) -> Result<VPath, PathError> {
        VPath::parse(&format!("{}/{}", self.display, name))
    }

    /// The part list, in the spelling given.
    pub fn parts(&self) -> impl Iterator<Item = &str> {
        self.display.split('/').filter(|p| !p.is_empty())
    }
}

impl fmt::Display for VPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display)
    }
}

fn invalid(path: &str, why: &'static str) -> PathError {
    PathError::Invalid {
        path: path.to_string(),
        why,
    }
}

fn is_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit()
        && stem.as_bytes()[3] != b'0')
}

/// The comparison form of a path or name.
pub fn fold(text: &str) -> String {
    text.to_lowercase()
}

pub use name_hash::joaat;

/// Matches a folded, `/`-separated pattern against a folded path. `*` is any run inside one part, `?` one
/// character, and a part that is exactly `**` any number of whole parts, including none.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    match_parts(&pattern, &path)
}

fn match_parts(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| match_parts(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((part, tail)) => match_part(first, part) && match_parts(rest, tail),
            None => false,
        },
    }
}

fn match_part(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) && pattern[p] != '*' {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_separators_dots_and_case() {
        let path = VPath::parse("\\Characters//Lucia/./Lucia.JSON").unwrap();
        assert_eq!(path.as_str(), "Characters/Lucia/Lucia.JSON");
        assert_eq!(path.key(), "characters/lucia/lucia.json");
        assert_eq!(path.parent().as_str(), "Characters/Lucia");
        assert_eq!(path.file_name(), "Lucia.JSON");
        assert_eq!(VPath::parse("a/b/../c").unwrap().as_str(), "a/c");
        assert!(VPath::parse("").unwrap().is_root());
        assert_eq!(VPath::parse("x.txt. ").unwrap().as_str(), "x.txt");
    }

    #[test]
    fn rejects_escapes_and_windows_traps() {
        assert!(matches!(VPath::parse("../x"), Err(PathError::Escapes(_))));
        assert!(matches!(
            VPath::parse("a/../../x"),
            Err(PathError::Escapes(_))
        ));
        for bad in [
            "a/b:stream",
            "C:/x",
            "con",
            "dir/NUL.txt",
            "lpt1",
            "a/...",
            "a/b|c",
            "a\0b",
        ] {
            assert!(
                matches!(VPath::parse(bad), Err(PathError::Invalid { .. })),
                "{bad}"
            );
        }
        assert!(VPath::parse("com0").is_ok());
        assert!(VPath::parse("console").is_ok());
    }

    #[test]
    fn refuses_what_offline_names() {
        for bad in [
            "tcpip:host/file",
            "\\\\server\\share\\x",
            "//server/share/x",
            "https://example.invalid/x",
        ] {
            assert!(
                matches!(VPath::parse(bad), Err(PathError::Refused { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn glob_rules() {
        assert!(glob_match(
            "characters/**/*.dds",
            "characters/lucia/tex/a.dds"
        ));
        assert!(glob_match("characters/**/*.dds", "characters/a.dds"));
        assert!(!glob_match("characters/*.dds", "characters/lucia/a.dds"));
        assert!(glob_match("a/b?/c*", "a/b1/cde"));
        assert!(!glob_match("a/b?/c*", "a/b/cde"));
        assert!(glob_match("**", "any/thing"));
        assert!(glob_match("*nude*", "lucia_nude_01"));
    }

    #[test]
    fn joaat_known_values() {
        assert_eq!(joaat(b""), 0);
        assert_eq!(joaat(b"a"), 0xCA2E_9442);
        assert_eq!(
            joaat(b"The quick brown fox jumps over the lazy dog"),
            0x519E_91F5
        );
    }
}
