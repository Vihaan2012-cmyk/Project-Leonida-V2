//! Where a path the game names really lives.
//!
//! The console's drives: `GAME:` and `G:` are the package folder, `T:` is scratch space. A path with no drive is
//! package-relative. The package is read-only: anything the game writes under it goes to an overlay folder in
//! `Output/`, and reads see the overlay first. Other host paths keep their place for reads and have their writes
//! redirected the same way.

use std::path::{Path, PathBuf};

use super::policy;

/// Why a path cannot be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Network(offline::Reason),
    Escapes,
    Device,
    /// A path the sandbox does not allow: not in the game folder, `Output/` or a mods or fixes folder.
    Outside,
    /// A link or junction inside an allowed folder that leads out of it.
    Link,
    /// A named pipe, which the sandbox keeps the game from reaching.
    Pipe,
    /// A name Windows would read as something other than a plain file name: a stream, a wildcard, a trailing dot.
    Invalid,
}

impl Refusal {
    /// The Windows error code to report for a call that changes files.
    pub fn code(self) -> u32 {
        match self {
            Refusal::Network(_) => 53,
            Refusal::Escapes | Refusal::Invalid => 123,
            Refusal::Device | Refusal::Outside | Refusal::Link => 5,
            Refusal::Pipe => 2,
        }
    }

    /// The code for a call that only looks: a path outside the sandbox is answered as a path that is not there.
    pub fn code_for(self, writes: bool) -> u32 {
        match self {
            Refusal::Outside | Refusal::Link | Refusal::Device if !writes => 3,
            other => other.code(),
        }
    }
}

/// How a mapped path behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Reads see `overlay` and then `base`; writes go to `overlay`.
    Layered { base: PathBuf, overlay: PathBuf },
    /// Plain host path that may be written as it is.
    Direct { path: PathBuf, make_parents: bool },
    /// A local pipe name, left alone.
    Passthrough,
}

/// The two folders everything is mapped against.
#[derive(Debug, Clone)]
pub struct Roots {
    package: PathBuf,
    output: PathBuf,
    package_key: (char, Vec<String>),
    output_key: (char, Vec<String>),
    sandbox: Option<Vec<(char, Vec<String>)>>,
    reads: Vec<PathBuf>,
}

enum Anchor {
    Game,
    Temp,
    Drive(char),
    Rooted,
    Relative,
}

enum Parsed {
    Path(Anchor, Vec<String>),
    Pipe,
}

fn normalise(rest: &str) -> Result<Vec<String>, Refusal> {
    let mut parts: Vec<String> = Vec::new();
    for part in rest.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or(Refusal::Escapes)?;
            }
            part => parts.push(part.to_string()),
        }
    }
    Ok(parts)
}

fn parse(path: &str, strict: bool) -> Result<Parsed, Refusal> {
    if let Some(reason) = offline::path(path) {
        return Err(Refusal::Network(reason));
    }
    let unified = path.trim_start().replace('/', "\\");
    let lower = unified.to_ascii_lowercase();
    let mut rest: &str = &unified;
    let mut prefixed = false;
    for prefix in ["\\\\?\\", "\\??\\"] {
        if lower.starts_with(prefix) {
            if strict {
                return Err(Refusal::Device);
            }
            rest = &unified[prefix.len()..];
            prefixed = true;
            let tail = rest.to_ascii_lowercase();
            if tail.starts_with("volume{") || tail.starts_with("globalroot") {
                return Err(Refusal::Device);
            }
        }
    }
    if lower.starts_with("\\\\.\\") {
        return if lower.starts_with("\\\\.\\pipe\\") {
            if strict {
                Err(Refusal::Pipe)
            } else {
                Ok(Parsed::Pipe)
            }
        } else {
            Err(Refusal::Device)
        };
    }
    if lower.starts_with("\\\\") && !prefixed {
        return Err(Refusal::Device);
    }
    let bytes = rest.as_bytes();
    let (anchor, body) = if bytes.len() >= 5 && bytes[..5].eq_ignore_ascii_case(b"game:") {
        (Anchor::Game, &rest[5..])
    } else if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let anchor = match bytes[0].to_ascii_lowercase() {
            b'g' => Anchor::Game,
            b't' => Anchor::Temp,
            other => Anchor::Drive(other as char),
        };
        (anchor, &rest[2..])
    } else if rest.starts_with('\\') {
        (Anchor::Rooted, rest)
    } else {
        (Anchor::Relative, rest)
    };
    let parts = normalise(body)?;
    if strict {
        for part in &parts {
            policy::check_name(part)?;
        }
    }
    Ok(Parsed::Path(anchor, parts))
}

fn host_path(drive: char, parts: &[String]) -> PathBuf {
    let mut path = PathBuf::from(format!("{}:\\", drive.to_ascii_uppercase()));
    path.extend(parts);
    path
}

/// Drive letter and lower-cased folder names of an absolute host path.
fn key(path: &Path) -> Option<(char, Vec<String>)> {
    let text = path.to_string_lossy();
    let text = text.trim_start_matches("\\\\?\\");
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let parts = normalise(&text[2..]).ok()?;
    Some((
        bytes[0].to_ascii_lowercase() as char,
        parts.into_iter().map(|p| p.to_lowercase()).collect(),
    ))
}

impl Roots {
    /// Both folders are absolute host paths with drive letters.
    pub fn new(package: PathBuf, output: PathBuf) -> Option<Roots> {
        Some(Roots {
            package_key: key(&package)?,
            output_key: key(&output)?,
            package,
            output,
            sandbox: None,
            reads: Vec::new(),
        })
    }

    /// Closes every path but the package (read only), `Output/` and the `reads` folders (read only). `None` when a
    /// folder in `reads` has no drive letter.
    pub fn sandboxed(mut self, reads: Vec<PathBuf>) -> Option<Roots> {
        let keys = reads.iter().map(|p| key(p)).collect::<Option<Vec<_>>>()?;
        policy::forget();
        self.sandbox = Some(keys);
        self.reads = reads;
        Some(self)
    }

    pub fn is_sandboxed(&self) -> bool {
        self.sandbox.is_some()
    }

    /// The folders a sandboxed game may read besides `Output/`: the package and the extra folders.
    pub(super) fn read_roots(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.package.as_path()).chain(self.reads.iter().map(PathBuf::as_path))
    }

    pub fn package(&self) -> &Path {
        &self.package
    }

    pub fn output(&self) -> &Path {
        &self.output
    }

    fn overlay(&self, parts: &[String]) -> PathBuf {
        let mut path = self.output.join("overlay");
        path.extend(parts);
        path
    }

    fn layered(&self, parts: &[String]) -> Target {
        let mut base = self.package.clone();
        base.extend(parts);
        Target::Layered {
            base,
            overlay: self.overlay(parts),
        }
    }

    /// How many leading names of `parts` the root `key` accounts for, if it is a prefix.
    fn under(key: &(char, Vec<String>), drive: char, parts: &[String]) -> Option<usize> {
        let lower: Vec<String> = parts.iter().map(|p| p.to_lowercase()).collect();
        (key.0 == drive && lower.starts_with(&key.1)).then_some(key.1.len())
    }

    /// Maps a path the game names.
    pub fn map(&self, path: &str) -> Result<Target, Refusal> {
        let (anchor, parts) = match parse(path, self.sandbox.is_some())? {
            Parsed::Pipe => return Ok(Target::Passthrough),
            Parsed::Path(anchor, parts) => (anchor, parts),
        };
        let trimmed = path.trim_start().trim_start_matches("\\\\?\\");
        let bytes = trimmed.as_bytes();
        let drive = match anchor {
            Anchor::Drive(letter) => Some(letter),
            Anchor::Game | Anchor::Temp if bytes.get(1) == Some(&b':') => {
                Some(bytes[0].to_ascii_lowercase() as char)
            }
            _ => None,
        };
        if let Some(drive) = drive {
            if Self::under(&self.output_key, drive, &parts).is_some() {
                return Ok(Target::Direct {
                    path: host_path(drive, &parts),
                    make_parents: false,
                });
            }
            if let Some(skip) = Self::under(&self.package_key, drive, &parts) {
                return Ok(self.layered(&parts[skip..]));
            }
            if let Some(reads) = &self.sandbox
                && !reads
                    .iter()
                    .any(|root| Self::under(root, drive, &parts).is_some())
                && matches!(anchor, Anchor::Drive(_))
            {
                return Err(Refusal::Outside);
            }
        }
        match anchor {
            Anchor::Game | Anchor::Rooted | Anchor::Relative => Ok(self.layered(&parts)),
            Anchor::Temp => {
                let mut path = self.output.join("temp");
                path.extend(&parts);
                Ok(Target::Direct {
                    path,
                    make_parents: true,
                })
            }
            Anchor::Drive(letter) => {
                let mut overlay = self.output.join("overlay").join("host");
                overlay.push(letter.to_string());
                overlay.extend(&parts);
                Ok(Target::Layered {
                    base: host_path(letter, &parts),
                    overlay,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Roots {
        Roots::new(
            PathBuf::from("E:\\Games\\Pkg"),
            PathBuf::from("E:\\Games\\Leo\\Output"),
        )
        .expect("roots")
    }

    fn layered(base: &str, overlay: &str) -> Target {
        Target::Layered {
            base: PathBuf::from(base),
            overlay: PathBuf::from(overlay),
        }
    }

    #[test]
    fn mapping_table() {
        let roots = roots();
        let package = |rel: &str| {
            layered(
                &format!("E:\\Games\\Pkg\\{rel}"),
                &format!("E:\\Games\\Leo\\Output\\overlay\\{rel}"),
            )
        };
        let table: Vec<(&str, Target)> = vec![
            ("GAME:\\data\\a.bin", package("data\\a.bin")),
            ("game:/data/a.bin", package("data\\a.bin")),
            ("G:\\data\\a.bin", package("data\\a.bin")),
            ("g:data/a.bin", package("data\\a.bin")),
            ("data\\a.bin", package("data\\a.bin")),
            ("\\data\\a.bin", package("data\\a.bin")),
            ("./data/../data/./a.bin", package("data\\a.bin")),
            (
                "GAME:\\",
                layered("E:\\Games\\Pkg", "E:\\Games\\Leo\\Output\\overlay"),
            ),
            ("E:\\Games\\Pkg\\data\\a.bin", package("data\\a.bin")),
            ("e:/games/pkg/DATA/a.bin", package("DATA\\a.bin")),
            ("\\\\?\\E:\\Games\\Pkg\\data\\a.bin", package("data\\a.bin")),
            (
                "T:\\cache\\x.tmp",
                Target::Direct {
                    path: PathBuf::from("E:\\Games\\Leo\\Output\\temp\\cache\\x.tmp"),
                    make_parents: true,
                },
            ),
            (
                "E:\\Games\\Leo\\Output\\saves\\a.sav",
                Target::Direct {
                    path: PathBuf::from("E:\\Games\\Leo\\Output\\saves\\a.sav"),
                    make_parents: false,
                },
            ),
            (
                "C:\\Windows\\Fonts\\a.ttf",
                layered(
                    "C:\\Windows\\Fonts\\a.ttf",
                    "E:\\Games\\Leo\\Output\\overlay\\host\\c\\Windows\\Fonts\\a.ttf",
                ),
            ),
            ("\\\\.\\pipe\\crash", Target::Passthrough),
        ];
        for (input, expected) in table {
            assert_eq!(roots.map(input), Ok(expected), "{input}");
        }
    }

    #[test]
    fn a_package_on_drive_g_or_t_stays_the_package() {
        let roots = Roots::new(PathBuf::from("G:\\Pkg"), PathBuf::from("E:\\Out")).expect("roots");
        assert_eq!(
            roots.map("G:\\Pkg\\a.bin"),
            Ok(layered("G:\\Pkg\\a.bin", "E:\\Out\\overlay\\a.bin"))
        );
    }

    #[test]
    fn refusals() {
        let roots = roots();
        assert_eq!(
            roots.map("tcpip:10.0.0.1/file"),
            Err(Refusal::Network(offline::Reason::RemoteFiles))
        );
        assert_eq!(
            roots.map("\\\\server\\share\\a"),
            Err(Refusal::Network(offline::Reason::NetworkShares))
        );
        assert_eq!(
            roots.map("https://example.com/a"),
            Err(Refusal::Network(offline::Reason::WebRequests))
        );
        assert_eq!(roots.map("GAME:\\..\\x"), Err(Refusal::Escapes));
        assert_eq!(roots.map("a\\..\\..\\x"), Err(Refusal::Escapes));
        assert_eq!(roots.map("\\\\.\\PhysicalDrive0"), Err(Refusal::Device));
        assert_eq!(Refusal::Escapes.code(), 123);
    }

    fn sandboxed() -> Roots {
        roots()
            .sandboxed(vec![PathBuf::from("E:\\Games\\Leo\\Mods")])
            .expect("sandboxed")
    }

    #[test]
    fn the_sandbox_keeps_the_allowed_mappings() {
        let roots = sandboxed();
        let plain = self::roots();
        for path in [
            "GAME:\\data\\a.bin",
            "g:/data/a.bin",
            "data\\a.bin",
            "\\data\\a.bin",
            "./data/../data/./a.bin",
            "E:\\Games\\Pkg\\data\\a.bin",
            "e:/GAMES/pkg/DATA/a.bin",
            "T:\\cache\\x.tmp",
            "E:\\Games\\Leo\\Output\\saves\\a.sav",
            "e:\\games\\leo\\output",
            "E:\\Games\\Leo\\Mods\\fix\\a.bin",
            "e:/games/leo/mods/fix/a.bin",
        ] {
            assert!(roots.map(path).is_ok(), "{path}");
            assert_eq!(roots.map(path), plain.map(path), "{path}");
        }
    }

    #[test]
    fn the_sandbox_closes_every_other_path() {
        let roots = sandboxed();
        for path in [
            "C:\\Windows\\Fonts\\a.ttf",
            "c:/users/someone/documents/a.docx",
            "E:\\",
            "E:\\Games",
            "E:\\Games\\Pkg2\\a.bin",
            "E:\\Games\\Leo\\other.txt",
            "E:\\Games\\Leo\\Outputs\\a",
            "Z:\\x",
            "D:rel\\x",
            "E:\\Games\\Pkg\\..\\Leo\\other",
        ] {
            assert_eq!(roots.map(path), Err(Refusal::Outside), "{path}");
        }
        assert_eq!(Refusal::Outside.code_for(false), 3);
        assert_eq!(Refusal::Outside.code_for(true), 5);
        assert_eq!(Refusal::Link.code_for(false), 3);
        assert_eq!(Refusal::Pipe.code_for(false), 2);
        assert_eq!(Refusal::Invalid.code_for(true), 123);
    }

    #[test]
    fn the_sandbox_refuses_special_names() {
        let roots = sandboxed();
        let cases: &[(&str, Refusal)] = &[
            ("\\\\?\\E:\\Games\\Pkg\\data\\a.bin", Refusal::Device),
            ("\\??\\E:\\Games\\Pkg\\a", Refusal::Device),
            ("\\\\.\\PhysicalDrive0", Refusal::Device),
            ("\\\\.\\C:", Refusal::Device),
            ("\\\\.\\pipe\\crash", Refusal::Pipe),
            ("GAME:\\data\\a.bin:stream", Refusal::Invalid),
            ("GAME:\\data\\a.bin::$DATA", Refusal::Invalid),
            ("GAME:\\data\\a.bin.", Refusal::Invalid),
            ("GAME:\\data\\a.bin ", Refusal::Invalid),
            ("GAME:\\data\\...\\a", Refusal::Invalid),
            ("GAME:\\data\\*.bin", Refusal::Invalid),
            ("GAME:\\data\\a|b", Refusal::Invalid),
            ("GAME:\\data\\NUL", Refusal::Device),
            ("data\\com3.txt", Refusal::Device),
            ("T:\\lpt1", Refusal::Device),
            ("GAME:\\..\\x", Refusal::Escapes),
            ("a\\..\\..\\x", Refusal::Escapes),
        ];
        for (path, refusal) in cases {
            assert_eq!(roots.map(path), Err(*refusal), "{path}");
        }
        assert_eq!(
            roots.map("\\\\server\\share\\a"),
            Err(Refusal::Network(offline::Reason::NetworkShares))
        );
        assert_eq!(
            roots.map("\\\\?\\UNC\\server\\share\\a"),
            Err(Refusal::Network(offline::Reason::NetworkShares))
        );
        assert!(self::roots().map("\\\\.\\pipe\\crash").is_ok());
    }

    #[test]
    fn dot_dot_cannot_climb_out_of_an_allowed_folder() {
        let roots = sandboxed();
        assert!(roots.map("GAME:\\a\\..\\b").is_ok());
        assert!(roots.map("E:\\Games\\Leo\\Output\\..\\Mods\\x").is_ok());
        assert_eq!(
            roots.map("E:\\Games\\Leo\\Output\\..\\..\\x"),
            Err(Refusal::Outside)
        );
        assert_eq!(
            roots.map("E:\\Games\\Leo\\Mods\\..\\Output2"),
            Err(Refusal::Outside)
        );
    }
}
