# Phase 0 and shader containers: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust workspace whose `leonida.exe` finds and checks the player's game package (`leonida check`) and reads every shader container out of `shaders.rpf` down to its FSSM program (`leonida shaders scan`), with CI guarding against game data.

**Architecture:** One Cargo workspace. `game-package` knows the package (version manifests, file checks, finding it on disk, read-only mapping). `shader-container` turns `shaders.rpf` bytes into carved DXBC containers, XBCH-unpacked FSSM blobs and parsed FSSM headers. `leonida` is the only binary and wires both into subcommands. `xtask` holds developer commands (the game-data scan used by CI).

**Tech Stack:** Rust 1.97.1 (edition 2024), clap 4, serde/serde_json, sha2, rayon, flate2, memchr, memmap2, thiserror, anyhow, tempfile (tests).

**Spec:** `docs/specs/2026-11-26-leonida-rust-design.md` (sections 2, 4.1, 10, 11 and 12 rows 0–1 are implemented here).

## Global Constraints

- No game data in the repository, ever: no archives, executables, shaders in any form, textures, audio, or logs that quote them. Test inputs are built in code by `shader-container`'s `testutil` module.
- No keys, decryption or protection workarounds. The RPF table of contents is encrypted and is never read; shaders are found by scanning for their plaintext records.
- Offline: nothing contacts the network.
- License: GPL-3.0 (`license = "GPL-3.0-only"` in every crate).
- No global mutable state.
- `unsafe` only in the single read-only `game_package::map_file` helper (this plan) and later the `gpu`, `pe-loader`, `shims`, `xbox-d3d12` crates. The workspace lints set `unsafe_code = "deny"`.
- Library crates return typed errors (`thiserror`); only the `leonida` binary uses `anyhow`.
- Same inputs give the same bytes and the same printed report (use `BTreeMap` for anything printed).
- Before every commit: `cargo fmt --all`, then `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test --workspace --all-features` must pass.
- Commit messages have no `Co-Authored-By` or other attribution trailers.
- `README.md` stays empty.

## Before you start

- Work in `E:\Project Leonida V2\leonida` (the git repo; `main` branch, pushed to
  `github.com/Vihaan2012-cmyk/Project-Leonida-V2`, private). README, LICENSE, spec, plans and `.gitignore` are
  already committed.
- The upstream source is `E:\Project Leonida V2\Leonida-main.zip`. One file is copied from it (Task 3); everything
  else is written fresh.

## File map

```
leonida/
├── Cargo.toml                         workspace, shared dependencies and lints
├── rust-toolchain.toml                pins Rust 1.97.1 + rustfmt + clippy
├── .cargo/config.toml                 `cargo xtask` alias
├── LICENSE                            GPL-3.0 text (from upstream)
├── README.md
├── .github/workflows/ci.yml           fmt, clippy, tests (Windows + Linux), game-data scan
├── xtask/                             developer commands
│   ├── Cargo.toml
│   └── src/main.rs                    `scan-game-data`
└── crates/
    ├── leonida/                       the binary
    │   ├── Cargo.toml
    │   ├── src/main.rs                dispatch
    │   ├── src/cli.rs                 clap definitions
    │   ├── src/game.rs                choose the game folder
    │   ├── src/cmd_check.rs           `leonida check`
    │   ├── src/cmd_shaders.rs         `leonida shaders scan`
    │   ├── tests/cli.rs
    │   ├── tests/check.rs
    │   └── tests/shaders.rs
    ├── game-package/
    │   ├── Cargo.toml
    │   ├── manifests/xbox-1.0.2.1.json   sizes + SHA-256 only (from upstream)
    │   ├── src/lib.rs                 package_path, map_file, re-exports
    │   ├── src/manifest.rs            Manifest, probes, version matching
    │   ├── src/check.rs               full file check, sha256_file
    │   └── src/find.rs                search the PC for packages
    └── shader-container/
        ├── Cargo.toml
        ├── src/lib.rs                 re-exports
        ├── src/error.rs               ContainerError
        ├── src/dxbc.rs                DXBC parts, Stage, ShaderModel, byte readers
        ├── src/xdat.rs                XDAT directory, XBCH inflate, unpack_all, inflate helper
        ├── src/fssm.rs                FSSM header, wave size, code range
        ├── src/carve.rs               find shader records in shaders.rpf
        └── src/testutil.rs            synthetic container builders (tests and `test-support` feature)
```

---

### Task 1: Workspace skeleton and `leonida --version`

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml`
- Create: `crates/leonida/Cargo.toml`, `crates/leonida/src/main.rs`, `crates/leonida/src/cli.rs`
- Test: `crates/leonida/tests/cli.rs`

**Interfaces:**
- Produces: workspace with members `crates/*` and `xtask`; shared `[workspace.dependencies]` used by every later task; `cli::Cli` (clap parser) that later tasks extend with subcommands.

- [ ] **Step 1: Write the workspace files**

`Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["crates/*", "xtask"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "GPL-3.0-only"
rust-version = "1.97"

[workspace.dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
flate2 = "1"
memchr = "2"
memmap2 = "0.9"
rayon = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
tempfile = "3"
thiserror = "2"
game-package = { path = "crates/game-package" }
shader-container = { path = "crates/shader-container" }

[workspace.lints.rust]
unsafe_code = "deny"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.97.1"
components = ["rustfmt", "clippy"]
```

`.cargo/config.toml`:

```toml
[alias]
xtask = "run --quiet --package xtask --"
```

`README.md` (empty) and `LICENSE` already exist; leave them.

- [ ] **Step 2: Write the `leonida` crate**

`crates/leonida/Cargo.toml`:

```toml
[package]
name = "leonida"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
anyhow.workspace = true
clap.workspace = true

[lints]
workspace = true
```

`crates/leonida/src/cli.rs`:

```rust
use clap::Parser;

/// Builds and runs the Leonida PC port from your own copy of the game.
#[derive(Debug, Parser)]
#[command(name = "leonida", version)]
pub struct Cli {}
```

`crates/leonida/src/main.rs`:

```rust
mod cli;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let _cli = cli::Cli::parse();
    ExitCode::SUCCESS
}
```

- [ ] **Step 3: Write the test**

`crates/leonida/tests/cli.rs`:

```rust
use std::process::Command;

fn leonida() -> Command {
    Command::new(env!("CARGO_BIN_EXE_leonida"))
}

#[test]
fn prints_version() {
    let out = leonida().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "leonida 0.1.0");
}
```

The workspace also lists `xtask`, which Task 2 fills in. Create its crate now so the workspace loads:

`xtask/Cargo.toml`:

```toml
[package]
name = "xtask"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish = false

[lints]
workspace = true
```

`xtask/src/main.rs`:

```rust
//! Developer commands, run as `cargo xtask <command>`.

fn main() {}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p leonida`
Expected: `prints_version ... ok`

- [ ] **Step 5: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add Cargo.toml rust-toolchain.toml .cargo crates xtask
git commit -m "Workspace skeleton and leonida --version"
```

---

### Task 2: Game-data scan (`cargo xtask scan-game-data`)

**Files:**
- Modify: `xtask/src/main.rs` (created empty in Task 1)

**Interfaces:**
- Produces: `cargo xtask scan-game-data [paths...]` — exit 0 when no tracked file (or given path) starts like game data, 1 otherwise, 2 on usage or git errors. CI (Task 7) runs it.

- [ ] **Step 1: Write the failing tests**

Add this module to the end of `xtask/src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn recognises_magic_at_start() {
        assert_eq!(classify(b"DXBC\x00\x01\x02\x03"), Some("DXBC container"));
        assert_eq!(classify(b"FSSM\x01\x00"), Some("FSSM shader"));
        assert_eq!(classify(b"\x03\x02\x23\x07rest"), Some("SPIR-V module"));
    }

    #[test]
    fn ignores_text_and_magic_elsewhere() {
        assert_eq!(classify(b"fn main() { /* DXBC */ }"), None);
        assert_eq!(classify(b""), None);
    }

    #[test]
    fn recognises_xvd_at_0x200() {
        let mut head = vec![0u8; 0x208];
        head[0x200..0x208].copy_from_slice(b"msft-xvd");
        assert_eq!(classify(&head), Some("XVD image"));
    }

    #[test]
    fn recognises_pe_executable() {
        let mut head = vec![0u8; 0x80];
        head[..2].copy_from_slice(b"MZ");
        head[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        head[0x40..0x44].copy_from_slice(b"PE\0\0");
        assert_eq!(classify(&head), Some("PE executable"));
        head[0x40] = b'X';
        assert_eq!(classify(&head), None);
    }

    #[test]
    fn recognises_wave_audio() {
        assert_eq!(classify(b"RIFF\x10\x00\x00\x00WAVEfmt "), Some("RIFF WAVE audio"));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p xtask`
Expected: FAIL to compile, "cannot find function `classify`".

- [ ] **Step 3: Implement**

`xtask/src/main.rs` (replace everything above the test module from Step 1):

```rust
//! Developer commands, run as `cargo xtask <command>`.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Bytes read from the start of each file: enough for the XVD magic at 0x200 and typical PE headers.
const HEAD: u64 = 0x1000;

/// (format, offset, magic): matched at that offset from the start of the file only, so the same bytes inside a
/// text file do not count. List from upstream's scripts/check_no_game_data.py.
const MAGICS: &[(&str, usize, &[u8])] = &[
    ("RPF9 archive", 0, b"9FPR"),
    ("RPF9 archive", 0, b"RPF9"),
    ("TTPF texture pool", 0, b"TTPF"),
    ("RAGE resource", 0, b"RSC7"),
    ("RAGE resource", 0, b"RSC8"),
    ("RAGE resource", 0, b"RSCC"),
    ("RAGE page directory", 0, b"0DRP"),
    ("RBF0 tree", 0, b"RBF0"),
    ("XBCH shader container", 0, b"XBCH"),
    ("FSSM shader", 0, b"FSSM"),
    ("DXBC container", 0, b"DXBC"),
    ("SPIR-V module", 0, b"\x03\x02\x23\x07"),
    ("SPIR-V module", 0, b"\x07\x23\x02\x03"),
    ("DDS texture", 0, b"DDS "),
    ("AWC audio bank", 0, b"ADAT"),
    ("AWC audio bank", 0, b"TADA"),
    ("AWC audio bank", 0, b"AWC\x00"),
    ("PSIN metadata container", 0, b"PSIN"),
    ("PAMH heightmap", 0, b"PAMH"),
    ("MHIU heightmap", 0, b"MHIU"),
    ("SPS1 shader-preset table", 0, b"SPS1"),
    ("SGD5 effects database", 0, b"SGD5"),
    ("RDS5 texture container", 0, b"RDS5"),
    ("YSC compiled script", 0, b"YSC\x00"),
    ("MoVE animation container", 0, b"MoVE"),
    ("SKEL_ bone table", 0, b"SKEL_"),
    ("SCRP compiled script", 0, b"SCRP"),
    ("JSAV story save", 0, b"JSAV"),
    ("ASAV save record", 0, b"ASAV"),
    ("SLAYER1 packed save", 0, b"SLAYER1\0"),
    ("LPK1 world staging pack", 0, b"LPK1"),
    ("Ogg stream", 0, b"OggS"),
    ("Bink 2 video", 0, b"KB2"),
    ("Bink video", 0, b"BIK"),
    ("glTF binary", 0, b"glTF"),
    ("XVD image", 0x200, b"msft-xvd"),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("scan-game-data") => scan_game_data(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask scan-game-data [paths...]");
            ExitCode::from(2)
        }
    }
}

/// The format a file starts like, or None. `head` is its first bytes.
fn classify(head: &[u8]) -> Option<&'static str> {
    for &(name, offset, magic) in MAGICS {
        if head.get(offset..offset + magic.len()) == Some(magic) {
            return Some(name);
        }
    }
    if head.starts_with(b"RIFF") && head.get(8..12) == Some(&b"WAVE"[..]) {
        return Some("RIFF WAVE audio");
    }
    if head.starts_with(b"MZ") && head.len() >= 0x40 {
        let e_lfanew = u32::from_le_bytes(head[0x3C..0x40].try_into().expect("4 bytes")) as usize;
        if head.get(e_lfanew..e_lfanew + 4) == Some(&b"PE\0\0"[..]) {
            return Some("PE executable");
        }
    }
    None
}

fn scan_game_data(paths: &[String]) -> ExitCode {
    let files = if paths.is_empty() {
        match tracked_files() {
            Ok(files) => files,
            Err(error) => {
                eprintln!("cannot list tracked files: {error}");
                return ExitCode::from(2);
            }
        }
    } else {
        paths.iter().map(PathBuf::from).collect()
    };
    let mut flagged = 0usize;
    for path in &files {
        match read_head(path) {
            Ok(head) => {
                if let Some(format) = classify(&head) {
                    println!("{}: looks like {format}", path.display());
                    flagged += 1;
                }
            }
            // A tracked file deleted in the working tree has nothing to scan.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                println!("{}: cannot read ({error})", path.display());
                flagged += 1;
            }
        }
    }
    if flagged > 0 {
        eprintln!("{flagged} file(s) look like game data or could not be read. Game data must never be committed.");
        return ExitCode::FAILURE;
    }
    println!("{} file(s) checked, no game data found.", files.len());
    ExitCode::SUCCESS
}

/// Every file git tracks, as paths from the current directory.
fn tracked_files() -> io::Result<Vec<PathBuf>> {
    let top = git(&["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(String::from_utf8_lossy(&top).trim());
    let listing = git(&["-C", &top.to_string_lossy(), "ls-files", "-z"])?;
    Ok(listing
        .split(|&byte| byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| top.join(String::from_utf8_lossy(name).as_ref()))
        .collect())
}

fn git(args: &[&str]) -> io::Result<Vec<u8>> {
    let out = Command::new("git").args(args).output()?;
    if !out.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(out.stdout)
}

fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    File::open(path)?.take(HEAD).read_to_end(&mut head)?;
    Ok(head)
}
```

- [ ] **Step 4: Run the tests and the scan**

Run: `cargo test -p xtask`
Expected: 5 tests pass.

Run: `cargo xtask scan-game-data README.md`
Expected: `1 file(s) checked, no game data found.` and exit code 0.

- [ ] **Step 5: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add xtask
git commit -m "xtask: scan tracked files for game data by magic bytes"
```

---

### Task 3: `game-package` — version manifests and recognising a version

**Files:**
- Create: `crates/game-package/Cargo.toml`, `crates/game-package/src/lib.rs`, `crates/game-package/src/manifest.rs`
- Create: `crates/game-package/manifests/xbox-1.0.2.1.json` (copied from upstream)

**Interfaces:**
- Produces:
  - `pub fn package_path(game: &Path, rel: &str) -> PathBuf` — joins a `/`-separated manifest path onto the game folder.
  - `pub struct Manifest { pub title: String, pub version: String, pub expected_exe: String, pub files: BTreeMap<String, FileEntry> }` with `Manifest::from_json(&str) -> Result<Manifest, ManifestError>`, `Manifest::builtin() -> Vec<Manifest>`, `Manifest::label(&self) -> String`.
  - `pub struct FileEntry { pub size: u64, pub sha256: String }`
  - `pub enum Mismatch { NoProbes, Missing(String), NotAFile(String), WrongSize { name, actual, expected }, Unreadable { name, error } }` (implements `Display`).
  - `pub fn probe_mismatches(game: &Path, manifest: &Manifest) -> Vec<Mismatch>`
  - `pub fn match_version<'a>(game: &Path, manifests: &'a [Manifest]) -> Option<&'a Manifest>`

- [ ] **Step 1: Copy the manifest**

From the repo root:

```powershell
New-Item -ItemType Directory -Force crates\game-package\manifests | Out-Null
tar -xf ..\Leonida-main.zip Leonida-main/game/xbox-1.0.2.1.json
Move-Item Leonida-main\game\xbox-1.0.2.1.json crates\game-package\manifests\xbox-1.0.2.1.json
Remove-Item -Recurse Leonida-main
```

It holds only file names, sizes and SHA-256 hashes (108 files) plus `expected_exe` — no game content.

- [ ] **Step 2: Write the crate with the failing tests**

`crates/game-package/Cargo.toml`:

```toml
[package]
name = "game-package"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true
```

`crates/game-package/src/lib.rs`:

```rust
//! The player's game package: which version it is, whether it is complete, and where it is.

mod manifest;

use std::path::{Path, PathBuf};

pub use manifest::{FileEntry, Manifest, ManifestError, Mismatch, PROBES, match_version, probe_mismatches};

/// Path of a manifest entry (always `/`-separated) inside the package folder.
pub fn package_path(game: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(game.to_path_buf(), |path, part| path.join(part))
}
```

`crates/game-package/src/manifest.rs` — tests first (the rest comes in Step 4):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_path;
    use std::fs;
    use std::path::Path;

    const TEST_MANIFEST: &str = r#"{
        "title": "Test package", "version": "9.9", "expected_exe": "game.exe",
        "files": {
            "MicrosoftGame.Config": {"size": 5, "sha256": "x"},
            "Catalog000.bin": {"size": 3, "sha256": "x"},
            "appdata0.rpf": {"size": 4, "sha256": "x"},
            "data/other.rpf": {"size": 9, "sha256": "x"}
        }
    }"#;

    fn make_file(dir: &Path, rel: &str, len: u64) {
        let path = package_path(dir, rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path).unwrap().set_len(len).unwrap();
    }

    #[test]
    fn builtin_manifest_is_version_1_0_2_1() {
        let all = Manifest::builtin();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "1.0.2.1");
        assert_eq!(all[0].files.len(), 108);
        assert_eq!(all[0].expected_exe, "game_scarlett_master.exe");
        assert_eq!(all[0].label(), "Xbox Series package 1.0.2.1");
    }

    #[test]
    fn probe_sizes_identify_the_version() {
        let dir = tempfile::tempdir().unwrap();
        make_file(dir.path(), "MicrosoftGame.Config", 5);
        make_file(dir.path(), "Catalog000.bin", 3);
        make_file(dir.path(), "appdata0.rpf", 4);
        let manifests = vec![Manifest::from_json(TEST_MANIFEST).unwrap()];
        assert!(probe_mismatches(dir.path(), &manifests[0]).is_empty());
        assert_eq!(match_version(dir.path(), &manifests).map(|m| m.version.as_str()), Some("9.9"));
    }

    #[test]
    fn wrong_size_and_missing_probes_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        make_file(dir.path(), "MicrosoftGame.Config", 6);
        make_file(dir.path(), "Catalog000.bin", 3);
        let manifest = Manifest::from_json(TEST_MANIFEST).unwrap();
        let problems = probe_mismatches(dir.path(), &manifest);
        assert_eq!(
            problems,
            vec![
                Mismatch::WrongSize { name: "MicrosoftGame.Config".into(), actual: 6, expected: 5 },
                Mismatch::Missing("appdata0.rpf".into()),
            ]
        );
        assert_eq!(problems[0].to_string(), "wrong size for MicrosoftGame.Config (6 bytes, expected 5)");
        assert!(match_version(dir.path(), std::slice::from_ref(&manifest)).is_none());
    }

    #[test]
    fn a_folder_where_a_probe_should_be_is_not_a_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("MicrosoftGame.Config")).unwrap();
        let manifest = Manifest::from_json(TEST_MANIFEST).unwrap();
        assert!(probe_mismatches(dir.path(), &manifest).contains(&Mismatch::NotAFile("MicrosoftGame.Config".into())));
    }

    #[test]
    fn manifest_without_probes_never_matches() {
        let manifest = Manifest::from_json(r#"{"title":"t","version":"v","expected_exe":"e","files":{}}"#).unwrap();
        assert_eq!(probe_mismatches(Path::new("."), &manifest), vec![Mismatch::NoProbes]);
    }

    #[test]
    fn package_path_splits_on_slashes() {
        let p = package_path(Path::new("game"), "data/ttdb/x.ttpf");
        assert_eq!(p, Path::new("game").join("data").join("ttdb").join("x.ttpf"));
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p game-package`
Expected: FAIL to compile (`Manifest`, `probe_mismatches` not found).

- [ ] **Step 4: Implement** (above the test module in `manifest.rs`)

```rust
use std::collections::BTreeMap;
use std::fmt;
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

use crate::package_path;

/// Files checked by size alone to recognise a version without reading anything large.
pub const PROBES: [&str; 3] = ["MicrosoftGame.Config", "Catalog000.bin", "appdata0.rpf"];

const BUILTIN: &[&str] = &[include_str!("../manifests/xbox-1.0.2.1.json")];

/// One supported package version: every file's size and SHA-256.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub title: String,
    pub version: String,
    pub expected_exe: String,
    pub files: BTreeMap<String, FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileEntry {
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid version manifest: {0}")]
pub struct ManifestError(#[from] serde_json::Error);

impl Manifest {
    pub fn from_json(text: &str) -> Result<Self, ManifestError> {
        Ok(serde_json::from_str(text)?)
    }

    /// The manifests of every supported version, built into the binary.
    pub fn builtin() -> Vec<Manifest> {
        BUILTIN
            .iter()
            .map(|text| Manifest::from_json(text).expect("built-in manifests are valid"))
            .collect()
    }

    /// For messages, e.g. "Xbox Series package 1.0.2.1".
    pub fn label(&self) -> String {
        format!("{} {}", self.title, self.version)
    }
}

/// Why a folder is not a given version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    NoProbes,
    Missing(String),
    NotAFile(String),
    WrongSize { name: String, actual: u64, expected: u64 },
    Unreadable { name: String, error: String },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mismatch::NoProbes => write!(f, "no version probe files are listed in the manifest"),
            Mismatch::Missing(name) => write!(f, "missing {name}"),
            Mismatch::NotAFile(name) => write!(f, "{name} is not a regular file"),
            Mismatch::WrongSize { name, actual, expected } => {
                write!(f, "wrong size for {name} ({actual} bytes, expected {expected})")
            }
            Mismatch::Unreadable { name, error } => write!(f, "cannot read file metadata for {name} ({error})"),
        }
    }
}

/// Why `game` is not `manifest`'s version, from file names and sizes only. Empty means it matches.
pub fn probe_mismatches(game: &Path, manifest: &Manifest) -> Vec<Mismatch> {
    let probes: Vec<(&str, &FileEntry)> = PROBES
        .iter()
        .filter_map(|&name| manifest.files.get(name).map(|entry| (name, entry)))
        .collect();
    if probes.is_empty() {
        return vec![Mismatch::NoProbes];
    }
    let mut problems = Vec::new();
    for (name, entry) in probes {
        match std::fs::metadata(package_path(game, name)) {
            Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                problems.push(Mismatch::Missing(name.to_string()));
            }
            Err(error) => problems.push(Mismatch::Unreadable { name: name.to_string(), error: error.to_string() }),
            Ok(meta) if !meta.is_file() => problems.push(Mismatch::NotAFile(name.to_string())),
            Ok(meta) if meta.len() != entry.size => problems.push(Mismatch::WrongSize {
                name: name.to_string(),
                actual: meta.len(),
                expected: entry.size,
            }),
            Ok(_) => {}
        }
    }
    problems
}

/// The supported version `game` is, recognised from probe file sizes, or None.
pub fn match_version<'a>(game: &Path, manifests: &'a [Manifest]) -> Option<&'a Manifest> {
    manifests.iter().find(|manifest| probe_mismatches(game, manifest).is_empty())
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p game-package`
Expected: 6 tests pass.

- [ ] **Step 6: Format, lint, scan, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo xtask scan-game-data crates/game-package/manifests/xbox-1.0.2.1.json
git add crates/game-package
git commit -m "game-package: version manifests and recognising a version by probe sizes"
```

---

### Task 4: `game-package` — checking every file

**Files:**
- Create: `crates/game-package/src/check.rs`
- Modify: `crates/game-package/src/lib.rs` (add module and re-exports), `crates/game-package/Cargo.toml` (add `rayon`, `sha2`)

**Interfaces:**
- Consumes: `Manifest`, `FileEntry`, `package_path` (Task 3).
- Produces:
  - `pub struct CheckReport { pub checked: usize, pub missing: Vec<String>, pub wrong_size: Vec<String>, pub hash_mismatch: Vec<String>, pub unreadable: Vec<String>, pub exe_present: bool }` with `is_ok(&self) -> bool` and `problem_count(&self) -> usize`.
  - `pub fn check_files(game: &Path, manifest: &Manifest, full: bool) -> CheckReport`
  - `pub fn sha256_file(path: &Path) -> io::Result<String>` (lowercase hex)

- [ ] **Step 1: Write the failing tests**

`crates/game-package/src/check.rs`:

```rust
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
        assert_eq!(sha256_file(&dir.path().join("Catalog000.bin")).unwrap(), ABC);
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
```

Add to `lib.rs`: `mod check;` and `pub use check::{CheckReport, check_files, sha256_file};`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p game-package`
Expected: FAIL to compile (`check_files` not found).

- [ ] **Step 3: Implement**

Add to `crates/game-package/Cargo.toml` `[dependencies]`:

```toml
rayon.workspace = true
sha2.workspace = true
```

Top of `check.rs` (above the tests):

```rust
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
        self.missing.len() + self.wrong_size.len() + self.hash_mismatch.len() + self.unreadable.len()
    }

    pub fn is_ok(&self) -> bool {
        self.problem_count() == 0
    }
}

/// Check every manifest file for presence and size; with `full`, also compare SHA-256 (reads every byte, in
/// parallel). Lists are in manifest (sorted path) order.
pub fn check_files(game: &Path, manifest: &Manifest, full: bool) -> CheckReport {
    let mut report = CheckReport { checked: manifest.files.len(), ..CheckReport::default() };
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
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p game-package`
Expected: 11 tests pass.

- [ ] **Step 5: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add crates/game-package
git commit -m "game-package: check every file by size, optionally by SHA-256"
```

---

### Task 5: `game-package` — finding packages and mapping files

**Files:**
- Create: `crates/game-package/src/find.rs`
- Modify: `crates/game-package/src/lib.rs` (module, re-exports, `map_file`), `crates/game-package/Cargo.toml` (add `memmap2`)

**Interfaces:**
- Produces:
  - `pub const MARKER: &str = "MicrosoftGame.Config"`
  - `pub fn find_packages(roots: &[PathBuf], depth: usize) -> Vec<PathBuf>` — sorted, deduplicated folders holding `MARKER`, searched `depth` levels below each root (the root itself is level 0).
  - `pub fn default_roots() -> Vec<PathBuf>` — Downloads, Desktop, Documents, Games, home, and every drive root (Windows) or `/`, `/mnt`, `/media` (elsewhere).
  - `pub fn map_file(file: &File) -> io::Result<memmap2::Mmap>` — read-only mapping; the only `unsafe` in this plan.

- [ ] **Step 1: Write the failing tests**

`crates/game-package/src/find.rs`:

```rust
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
        assert_eq!(found, vec![root.path().join("a").join("game"), root.path().join("b")]);
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
        let roots = vec![root.path().to_path_buf(), root.path().join("does-not-exist")];
        assert!(find_packages(&roots, 2).is_empty());
    }

    #[test]
    fn default_roots_are_not_empty() {
        assert!(!default_roots().is_empty());
    }
}
```

Add a test for `map_file` at the end of `lib.rs`:

```rust
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
```

Add to `lib.rs`: `mod find;` and `pub use find::{MARKER, default_roots, find_packages};`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p game-package`
Expected: FAIL to compile (`find_packages`, `map_file` not found).

- [ ] **Step 3: Implement**

Add to `crates/game-package/Cargo.toml` `[dependencies]`: `memmap2.workspace = true`

Top of `find.rs`:

```rust
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
    let Ok(entries) = std::fs::read_dir(dir) else { return };
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
        || matches!(name.as_ref(), "Windows" | "System Volume Information" | "node_modules")
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
```

Add to `lib.rs` (below `package_path`):

```rust
/// Map a file read-only. Every archive reader uses this instead of reading multi-gigabyte files into memory.
#[allow(unsafe_code)]
pub fn map_file(file: &std::fs::File) -> std::io::Result<memmap2::Mmap> {
    // SAFETY: Leonida never writes to the game package and only reads through this mapping. If another program
    // truncated the file while it is mapped, reads could fault; that is accepted for a read-only game package.
    unsafe { memmap2::Mmap::map(file) }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p game-package`
Expected: 16 tests pass.

- [ ] **Step 5: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add crates/game-package
git commit -m "game-package: find packages on disk, map files read-only"
```

---

### Task 6: `leonida check`

**Files:**
- Create: `crates/leonida/src/game.rs`, `crates/leonida/src/cmd_check.rs`
- Modify: `crates/leonida/src/cli.rs`, `crates/leonida/src/main.rs`, `crates/leonida/Cargo.toml`
- Test: `crates/leonida/tests/check.rs`

**Interfaces:**
- Consumes: `game_package::{Manifest, match_version, probe_mismatches, check_files, find_packages, default_roots}`.
- Produces:
  - CLI: `leonida check [--game <folder>] [--full]` (hidden `--manifest <file>` for tests). Exit 0 when the copy is complete, 1 when it is not or is not a supported version, 2 on errors.
  - `game::resolve_game(given: Option<&PathBuf>, manifests: &[Manifest]) -> anyhow::Result<PathBuf>` — used again by Task 12.
  - `cli::Command` enum that Task 12 extends with `Shaders`.

- [ ] **Step 1: Write the failing integration tests**

`crates/leonida/tests/check.rs`:

```rust
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn put(dir: &Path, rel: &str, content: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// (temp dir, game folder, manifest file) for a complete synthetic package.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    put(&game, "MicrosoftGame.Config", b"hello");
    put(&game, "Catalog000.bin", b"abc");
    put(&game, "appdata0.rpf", b"");
    put(&game, "data/other.rpf", b"abc");
    let manifest = dir.path().join("manifest.json");
    fs::write(
        &manifest,
        format!(
            r#"{{"title":"Test package","version":"9.9","expected_exe":"game.exe","files":{{
                "MicrosoftGame.Config":{{"size":5,"sha256":"{HELLO}"}},
                "Catalog000.bin":{{"size":3,"sha256":"{ABC}"}},
                "appdata0.rpf":{{"size":0,"sha256":"{EMPTY}"}},
                "data/other.rpf":{{"size":3,"sha256":"{ABC}"}}}}}}"#
        ),
    )
    .unwrap();
    (dir, game, manifest)
}

fn check(game: &Path, manifest: &Path, extra: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_leonida"))
        .arg("check")
        .arg("--game")
        .arg(game)
        .arg("--manifest")
        .arg(manifest)
        .args(extra)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

#[test]
fn complete_copy_passes() {
    let (_dir, game, manifest) = fixture();
    let (ok, text) = check(&game, &manifest, &[]);
    assert!(ok, "{text}");
    assert!(text.contains("Found Test package 9.9"), "{text}");
    assert!(text.contains("All 4 files present with the right sizes."), "{text}");
    assert!(text.contains("Executable game.exe is not in the folder"), "{text}");
}

#[test]
fn full_check_catches_changed_bytes() {
    let (_dir, game, manifest) = fixture();
    put(&game, "data/other.rpf", b"abd");
    let (ok, text) = check(&game, &manifest, &["--full"]);
    assert!(!ok, "{text}");
    assert!(text.contains("wrong hash   data/other.rpf"), "{text}");
}

#[test]
fn unsupported_version_lists_probe_problems() {
    let (_dir, game, manifest) = fixture();
    put(&game, "Catalog000.bin", b"abcd");
    let (ok, text) = check(&game, &manifest, &[]);
    assert!(!ok, "{text}");
    assert!(text.contains("doesn't look like a supported version"), "{text}");
    assert!(text.contains("wrong size for Catalog000.bin (4 bytes, expected 3)"), "{text}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p leonida --test check`
Expected: FAIL — the binary rejects the `check` subcommand (clap usage error), so assertions fail.

- [ ] **Step 3: Implement**

`crates/leonida/Cargo.toml` — `[dependencies]` add `game-package.workspace = true`; add:

```toml
[dev-dependencies]
tempfile.workspace = true
```

`crates/leonida/src/cli.rs` (replace):

```rust
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Builds and runs the Leonida PC port from your own copy of the game.
#[derive(Debug, Parser)]
#[command(name = "leonida", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Find your copy of the game and check that it is complete and supported.
    Check(CheckArgs),
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// The game folder (holds MicrosoftGame.Config and the .rpf archives). Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// Also compare the SHA-256 of every file (slow: reads the whole package).
    #[arg(long)]
    pub full: bool,
    /// Use this version manifest instead of the built-in ones (for tests).
    #[arg(long, hide = true)]
    pub manifest: Option<PathBuf>,
}
```

`crates/leonida/src/game.rs`:

```rust
use std::path::PathBuf;

use anyhow::{Result, bail};
use game_package::{Manifest, default_roots, find_packages, match_version};

/// How many folder levels below each search root a package may be.
const SEARCH_DEPTH: usize = 2;

/// The game folder to use: the one given, or the only supported copy found on this PC.
pub fn resolve_game(given: Option<&PathBuf>, manifests: &[Manifest]) -> Result<PathBuf> {
    if let Some(path) = given {
        if !path.is_dir() {
            bail!("game folder not found: {}", path.display());
        }
        return Ok(path.clone());
    }
    let candidates: Vec<PathBuf> = find_packages(&default_roots(), SEARCH_DEPTH)
        .into_iter()
        .filter(|path| match_version(path, manifests).is_some())
        .collect();
    match candidates.as_slice() {
        [] => bail!(
            "no supported copy of the game was found in Downloads, Desktop, Documents, Games, your home folder or \
             the top of any drive. Pass --game <folder> (the folder that holds MicrosoftGame.Config)."
        ),
        [only] => Ok(only.clone()),
        many => {
            let list: Vec<String> = many.iter().map(|path| format!("  {}", path.display())).collect();
            bail!("more than one copy was found; pick one with --game:\n{}", list.join("\n"))
        }
    }
}
```

`crates/leonida/src/cmd_check.rs`:

```rust
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use game_package::{Manifest, check_files, match_version, probe_mismatches};

use crate::cli::CheckArgs;
use crate::game::resolve_game;

pub fn run(args: &CheckArgs) -> Result<ExitCode> {
    let manifests = load_manifests(args.manifest.as_deref())?;
    let game = resolve_game(args.game.as_ref(), &manifests)?;
    let Some(manifest) = match_version(&game, &manifests) else {
        eprintln!("{} doesn't look like a supported version of the game:", game.display());
        for candidate in &manifests {
            eprintln!("  {}", candidate.label());
            for problem in probe_mismatches(&game, candidate) {
                eprintln!("    {problem}");
            }
        }
        return Ok(ExitCode::FAILURE);
    };
    println!("Found {} at {}", manifest.label(), game.display());
    if args.full {
        println!("Comparing the SHA-256 of every file (this reads the whole package)...");
    }
    let report = check_files(&game, manifest, args.full);
    for rel in &report.missing {
        println!("  missing      {rel}");
    }
    for rel in &report.wrong_size {
        println!("  wrong size   {rel}");
    }
    for rel in &report.hash_mismatch {
        println!("  wrong hash   {rel}");
    }
    for rel in &report.unreadable {
        println!("  unreadable   {rel}");
    }
    if !report.is_ok() {
        println!(
            "{} of {} files have problems. Use a complete, unmodified copy of version {}.",
            report.problem_count(),
            report.checked,
            manifest.version
        );
        return Ok(ExitCode::FAILURE);
    }
    let how = if args.full { "and their hashes match" } else { "with the right sizes" };
    println!("All {} files present {how}.", report.checked);
    if report.exe_present {
        println!("Executable {} found.", manifest.expected_exe);
    } else {
        println!(
            "Executable {} is not in the folder: everything except running the game can be built.",
            manifest.expected_exe
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The built-in manifests, or only the one in `path`.
pub fn load_manifests(path: Option<&Path>) -> Result<Vec<Manifest>> {
    let Some(path) = path else { return Ok(Manifest::builtin()) };
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(vec![Manifest::from_json(&text)?])
}
```

`crates/leonida/src/main.rs` (replace):

```rust
mod cli;
mod cmd_check;
mod game;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Check(args) => cmd_check::run(args),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error:#}");
        ExitCode::from(2)
    })
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p leonida`
Expected: `prints_version` and the 3 `check` tests pass.

- [ ] **Step 5: Try it on the real package (manual)**

Run: `cargo run -p leonida -- check` (or with `--game "<your package folder>"`).
Expected: `Found Xbox Series package 1.0.2.1 at ...`, `All 108 files present with the right sizes.` (or a list of what is missing), and the executable line. Paste nothing from the package into commits or issues.

- [ ] **Step 6: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add crates/leonida
git commit -m "leonida check: find the package, recognise its version, check its files"
```

---

### Task 7: Continuous integration

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: CI that runs format, lint and tests on Windows and Linux, and the game-data scan, on every push to `main` and every pull request.

- [ ] **Step 1: Write the workflow**

`.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

jobs:
  test:
    strategy:
      fail-fast: false
      matrix:
        os: [windows-latest, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      # rustup installs the toolchain pinned in rust-toolchain.toml on first use.
      - run: cargo --version
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - run: cargo test --workspace --all-features

  no-game-data:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - run: cargo xtask scan-game-data
```

- [ ] **Step 2: Run the same checks locally**

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo xtask scan-game-data
```

Expected: all succeed; the scan prints `N file(s) checked, no game data found.`

- [ ] **Step 3: Commit**

```powershell
git add .github
git commit -m "CI: format, lint, tests on Windows and Linux, game-data scan"
```

Phase 0 is complete here.

---

### Task 8: `shader-container` — DXBC parts, stage and shader model

**Files:**
- Create: `crates/shader-container/Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/dxbc.rs`, `src/testutil.rs`

**Interfaces:**
- Produces:
  - `pub enum ContainerError { Truncated(&'static str), Invalid(String) }` (`thiserror`, `Clone + PartialEq`).
  - `pub struct Part<'a> { pub fourcc: [u8; 4], pub data: &'a [u8] }`
  - `pub fn parts(dxbc: &[u8]) -> Result<Vec<Part<'_>>, ContainerError>`
  - `pub fn find_part<'a>(parts: &[Part<'a>], fourcc: &[u8; 4]) -> Option<&'a [u8]>` — last part with that fourcc.
  - `pub enum Stage { Pixel, Vertex, Geometry, Hull, Domain, Compute, Library, Mesh, Amplification, Other(u16) }` with `from_dxil_kind(u16) -> Stage` and `short_name(self) -> String` (`ps`, `vs`, `gs`, `hs`, `ds`, `cs`, `lib`, `ms`, `as`, `kind<N>`).
  - `pub struct ShaderModel { pub major: u8, pub minor: u8 }`
  - `pub fn shader_model(parts: &[Part<'_>]) -> Result<Option<(Stage, ShaderModel)>, ContainerError>`
  - `pub(crate) fn u16_at(b: &[u8], off: usize) -> u16`, `pub(crate) fn u32_at(b: &[u8], off: usize) -> u32` (callers check bounds first).
  - `testutil::dxbc(parts: &[(&[u8; 4], &[u8])]) -> Vec<u8>` (more builders arrive in Tasks 9–11).

- [ ] **Step 1: Crate files and failing tests**

`crates/shader-container/Cargo.toml`:

```toml
[package]
name = "shader-container"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[features]
# Exposes `testutil` (synthetic container builders) to other crates' tests.
test-support = []

[dependencies]
flate2.workspace = true
memchr.workspace = true
thiserror.workspace = true

[lints]
workspace = true
```

`src/lib.rs`:

```rust
//! Shader containers in `shaders.rpf`: carving records, DXBC parts, XDAT/XBCH unpacking and FSSM headers.

mod dxbc;
mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod testutil;

pub use dxbc::{Part, ShaderModel, Stage, find_part, parts, shader_model};
pub use error::ContainerError;
```

`src/error.rs`:

```rust
/// Why a shader container could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContainerError {
    #[error("truncated {0}")]
    Truncated(&'static str),
    #[error("{0}")]
    Invalid(String),
}
```

`src/testutil.rs`:

```rust
//! Builders for synthetic shader containers. Test data only: nothing here comes from the game.

/// A DXBC container holding `parts` in order.
pub fn dxbc(parts: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let table_end = 32 + 4 * parts.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"DXBC");
    out.extend_from_slice(&[0u8; 16]); // hash
    out.extend_from_slice(&1u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // total size, patched below
    out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
    let mut offset = table_end;
    for (_, data) in parts {
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += 8 + data.len();
    }
    for (fourcc, data) in parts {
        out.extend_from_slice(&fourcc[..]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    let total = out.len() as u32;
    out[24..28].copy_from_slice(&total.to_le_bytes());
    out
}
```

`src/dxbc.rs` — tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::dxbc;

    #[test]
    fn reads_parts_in_order() {
        let c = dxbc(&[(b"DXIL", &b"\x66\x00\x01\x00rest"[..]), (b"PRIV", &b"private"[..])]);
        let p = parts(&c).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(&p[0].fourcc, b"DXIL");
        assert_eq!(p[1].data, b"private");
        assert_eq!(find_part(&p, b"PRIV"), Some(&b"private"[..]));
        assert_eq!(find_part(&p, b"STAT"), None);
    }

    #[test]
    fn later_part_with_same_fourcc_wins() {
        let c = dxbc(&[(b"PRIV", &b"one"[..]), (b"PRIV", &b"two"[..])]);
        assert_eq!(find_part(&parts(&c).unwrap(), b"PRIV"), Some(&b"two"[..]));
    }

    #[test]
    fn stage_and_shader_model_from_dxil_header() {
        // version 0x66 = shader model 6.6; kind 1 = vertex
        let c = dxbc(&[(b"DXIL", &b"\x66\x00\x01\x00"[..])]);
        let model = shader_model(&parts(&c).unwrap()).unwrap();
        assert_eq!(model, Some((Stage::Vertex, ShaderModel { major: 6, minor: 6 })));
        assert_eq!(Stage::from_dxil_kind(13).short_name(), "ms");
        assert_eq!(Stage::from_dxil_kind(99).short_name(), "kind99");
    }

    #[test]
    fn no_dxil_part_means_no_stage() {
        let c = dxbc(&[(b"PRIV", &b"x"[..])]);
        assert_eq!(shader_model(&parts(&c).unwrap()).unwrap(), None);
    }

    #[test]
    fn rejects_damaged_containers() {
        assert_eq!(parts(b"DXBC"), Err(ContainerError::Truncated("DXBC header")));
        let mut c = dxbc(&[(b"PRIV", &b"private"[..])]);
        assert!(matches!(parts(&c[..c.len() - 1]), Err(ContainerError::Truncated(_))));
        c[0] = b'X';
        assert!(matches!(parts(&c), Err(ContainerError::Invalid(_))));
        let mut bad_offset = dxbc(&[(b"PRIV", &b"private"[..])]);
        bad_offset[32..36].copy_from_slice(&4u32.to_le_bytes()); // points into the header
        assert!(matches!(parts(&bad_offset), Err(ContainerError::Invalid(_))));
        let truncated_dxil = dxbc(&[(b"DXIL", &b"\x66\x00"[..])]);
        assert_eq!(
            shader_model(&parts(&truncated_dxil).unwrap()),
            Err(ContainerError::Truncated("DXIL version and stage"))
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shader-container`
Expected: FAIL to compile (`parts`, `Stage` not found).

- [ ] **Step 3: Implement** (top of `dxbc.rs`)

```rust
use crate::ContainerError;

/// One part of a DXBC container: its four-character code and payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part<'a> {
    pub fourcc: [u8; 4],
    pub data: &'a [u8],
}

/// The parts of a DXBC container, in table order. Layout: "DXBC", 16-byte hash, u32 version, u32 total size,
/// u32 part count at 28, then one u32 offset per part; each part is fourcc, u32 size, payload.
pub fn parts(dxbc: &[u8]) -> Result<Vec<Part<'_>>, ContainerError> {
    if dxbc.len() < 32 {
        return Err(ContainerError::Truncated("DXBC header"));
    }
    if &dxbc[..4] != b"DXBC" {
        return Err(ContainerError::Invalid("not a DXBC container".into()));
    }
    let count = u32_at(dxbc, 28) as usize;
    let table_end = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(32))
        .filter(|&end| end <= dxbc.len())
        .ok_or(ContainerError::Truncated("DXBC part table"))?;
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let off = u32_at(dxbc, 32 + 4 * k) as usize;
        if off < table_end || off.checked_add(8).is_none_or(|end| end > dxbc.len()) {
            return Err(ContainerError::Invalid(format!("DXBC part {k} header offset {off} is outside the part data")));
        }
        let size = u32_at(dxbc, off + 4) as usize;
        let start = off + 8;
        let end = start
            .checked_add(size)
            .filter(|&end| end <= dxbc.len())
            .ok_or(ContainerError::Truncated("DXBC part payload"))?;
        out.push(Part { fourcc: dxbc[off..off + 4].try_into().expect("4 bytes"), data: &dxbc[start..end] });
    }
    Ok(out)
}

/// The payload of the last part with this fourcc (later parts win).
pub fn find_part<'a>(parts: &[Part<'a>], fourcc: &[u8; 4]) -> Option<&'a [u8]> {
    parts.iter().rev().find(|part| &part.fourcc == fourcc).map(|part| part.data)
}

/// Pipeline stage, from the DXIL program header's kind field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Pixel,
    Vertex,
    Geometry,
    Hull,
    Domain,
    Compute,
    Library,
    Mesh,
    Amplification,
    Other(u16),
}

impl Stage {
    pub fn from_dxil_kind(kind: u16) -> Stage {
        match kind {
            0 => Stage::Pixel,
            1 => Stage::Vertex,
            2 => Stage::Geometry,
            3 => Stage::Hull,
            4 => Stage::Domain,
            5 => Stage::Compute,
            6 => Stage::Library,
            13 => Stage::Mesh,
            14 => Stage::Amplification,
            other => Stage::Other(other),
        }
    }

    pub fn short_name(self) -> String {
        match self {
            Stage::Pixel => "ps".into(),
            Stage::Vertex => "vs".into(),
            Stage::Geometry => "gs".into(),
            Stage::Hull => "hs".into(),
            Stage::Domain => "ds".into(),
            Stage::Compute => "cs".into(),
            Stage::Library => "lib".into(),
            Stage::Mesh => "ms".into(),
            Stage::Amplification => "as".into(),
            Stage::Other(kind) => format!("kind{kind}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderModel {
    pub major: u8,
    pub minor: u8,
}

/// Stage and shader model from the DXIL part's first four bytes (u16 version, u16 kind), or None when the
/// container has no DXIL part.
pub fn shader_model(parts: &[Part<'_>]) -> Result<Option<(Stage, ShaderModel)>, ContainerError> {
    let Some(dxil) = find_part(parts, b"DXIL") else { return Ok(None) };
    if dxil.len() < 4 {
        return Err(ContainerError::Truncated("DXIL version and stage"));
    }
    let version = u16_at(dxil, 0);
    let kind = u16_at(dxil, 2);
    let model = ShaderModel { major: (version >> 4) as u8, minor: (version & 0xF) as u8 };
    Ok(Some((Stage::from_dxil_kind(kind), model)))
}

/// Little-endian u16 at `off`. Panics if out of range: callers check bounds first.
pub(crate) fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().expect("2 bytes"))
}

/// Little-endian u32 at `off`. Panics if out of range: callers check bounds first.
pub(crate) fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().expect("4 bytes"))
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p shader-container`
Expected: 5 tests pass.

- [ ] **Step 5: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add crates/shader-container
git commit -m "shader-container: DXBC parts, stage and shader model"
```

---

### Task 9: `shader-container` — XDAT directory and XBCH → FSSM

**Files:**
- Create: `crates/shader-container/src/xdat.rs`
- Modify: `crates/shader-container/src/lib.rs`, `crates/shader-container/src/testutil.rs`

**Interfaces:**
- Consumes: `parts`, `find_part`, `u32_at`, `ContainerError` (Task 8).
- Produces:
  - `pub const XBCH_TAG: u32 = 0x0D`, `pub const MAX_FSSM_BYTES: usize = 64 << 20`
  - `pub fn xdat_entries(dxil: &[u8]) -> Result<Vec<(u32, &[u8])>, ContainerError>` — directory order.
  - `pub fn inflate_xbch(blob: &[u8]) -> Result<Vec<u8>, ContainerError>` — the FSSM blob.
  - `pub fn unpack_all(dxbc: &[u8]) -> Result<Vec<Vec<u8>>, ContainerError>` — every tag-0x0D payload's FSSM, directory order. Callers that want one use the **last** (upstream's default).
  - `pub(crate) fn inflate(input: &[u8], expected: usize, zlib: bool, require_all_input: bool) -> Result<Vec<u8>, ContainerError>` — Task 11 uses it with raw deflate.
  - `testutil::dxil(kind: u16, entries: &[(u32, Vec<u8>)]) -> Vec<u8>`, `testutil::xbch(fssm: &[u8]) -> Vec<u8>`, `testutil::container(kind: u16, fssm_blobs: &[Vec<u8>]) -> Vec<u8>`

- [ ] **Step 1: Add the builders**

Append to `src/testutil.rs`:

```rust
use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// A DXIL part: a stub program header (shader model 6.6, `kind`) followed by an XDAT directory of `entries`.
/// XDAT layout: "XDAT", u32 count, u32 header size, then count x {u32 tag, u32 offset, u32 size}; offsets are
/// relative to the DXIL part.
pub fn dxil(kind: u16, entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0x66u16.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0u8; 12]);
    let table_end = out.len() + 12 + 12 * entries.len();
    out.extend_from_slice(b"XDAT");
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&12u32.to_le_bytes());
    let mut offset = table_end;
    for (tag, payload) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        offset += payload.len();
    }
    for (_, payload) in entries {
        out.extend_from_slice(payload);
    }
    out
}

/// An XBCH payload: "XBCH", u32 0, u32 stored, u32 0, u32 unpacked, u32 0, then zlib(fssm).
pub fn xbch(fssm: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(fssm).expect("in-memory write");
    let compressed = encoder.finish().expect("in-memory write");
    let mut out = Vec::new();
    out.extend_from_slice(b"XBCH");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(fssm.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&compressed);
    out
}

/// A complete shader container: a DXIL part with one XBCH payload per FSSM blob, then a PRIV part.
pub fn container(kind: u16, fssm_blobs: &[Vec<u8>]) -> Vec<u8> {
    let entries: Vec<(u32, Vec<u8>)> = fssm_blobs.iter().map(|blob| (crate::XBCH_TAG, xbch(blob))).collect();
    let dxil = dxil(kind, &entries);
    dxbc(&[(b"DXIL", &dxil[..]), (b"PRIV", &b"private"[..])])
}
```

- [ ] **Step 2: Write the failing tests**

`src/xdat.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{container, dxil, xbch};

    fn fake_fssm(fill: u8) -> Vec<u8> {
        let mut blob = vec![fill; 0x40];
        blob[..4].copy_from_slice(b"FSSM");
        blob
    }

    #[test]
    fn entries_in_directory_order() {
        let d = dxil(5, &[(0x12, vec![1; 16]), (XBCH_TAG, vec![2; 3])]);
        let entries = xdat_entries(&d).unwrap();
        assert_eq!(entries, vec![(0x12, &[1u8; 16][..]), (XBCH_TAG, &[2u8; 3][..])]);
    }

    #[test]
    fn no_xdat_means_no_entries() {
        assert!(xdat_entries(b"\x66\x00\x05\x00nothing here").unwrap().is_empty());
    }

    #[test]
    fn xbch_inflates_to_fssm() {
        assert_eq!(inflate_xbch(&xbch(&fake_fssm(7))).unwrap(), fake_fssm(7));
    }

    #[test]
    fn xbch_size_errors_are_caught() {
        let mut blob = xbch(&fake_fssm(7));
        blob[16..20].copy_from_slice(&0x30u32.to_le_bytes()); // declared smaller than the real 0x40
        assert!(matches!(inflate_xbch(&blob), Err(ContainerError::Invalid(_))));
        let mut blob = xbch(&fake_fssm(7));
        blob[8..12].copy_from_slice(&1u32.to_le_bytes()); // stored size disagrees with payload
        assert!(matches!(inflate_xbch(&blob), Err(ContainerError::Invalid(_))));
        assert!(matches!(inflate_xbch(b"NOPE and more bytes here!"), Err(ContainerError::Invalid(_))));
    }

    #[test]
    fn unpack_all_returns_every_payload_in_order() {
        let c = container(0, &[fake_fssm(1), fake_fssm(2)]);
        assert_eq!(unpack_all(&c).unwrap(), vec![fake_fssm(1), fake_fssm(2)]);
    }

    #[test]
    fn overlapping_directory_is_rejected() {
        let mut d = dxil(5, &[(XBCH_TAG, vec![9; 8])]);
        let x = d.windows(4).position(|w| w == b"XDAT").unwrap();
        d[x + 16..x + 20].copy_from_slice(&(x as u32).to_le_bytes()); // entry offset points at the directory
        assert!(matches!(xdat_entries(&d), Err(ContainerError::Invalid(_))));
    }
}
```

Add to `lib.rs`: `mod xdat;` and `pub use xdat::{MAX_FSSM_BYTES, XBCH_TAG, inflate_xbch, unpack_all, xdat_entries};`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p shader-container`
Expected: FAIL to compile (`xdat_entries` not found).

- [ ] **Step 4: Implement** (top of `xdat.rs`)

```rust
use flate2::{Decompress, FlushDecompress, Status};

use crate::ContainerError;
use crate::dxbc::{find_part, parts, u32_at};

/// XDAT tag of an XBCH payload (the compressed FSSM program).
pub const XBCH_TAG: u32 = 0x0D;

/// Defensive per-blob ceiling (not a format limit), as upstream.
pub const MAX_FSSM_BYTES: usize = 64 << 20;

/// Entries of the XDAT directory inside a DXIL part, in directory order, as (tag, payload).
pub fn xdat_entries(dxil: &[u8]) -> Result<Vec<(u32, &[u8])>, ContainerError> {
    let Some(x) = memchr::memmem::find(dxil, b"XDAT") else { return Ok(Vec::new()) };
    if x + 12 > dxil.len() {
        return Err(ContainerError::Truncated("XDAT header"));
    }
    let count = u32_at(dxil, x + 4) as usize;
    let table_end = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(x + 12))
        .filter(|&end| end <= dxil.len())
        .ok_or(ContainerError::Truncated("XDAT entry table"))?;
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let at = x + 12 + 12 * k;
        let tag = u32_at(dxil, at);
        let off = u32_at(dxil, at + 4) as usize;
        let size = u32_at(dxil, at + 8) as usize;
        let end = off
            .checked_add(size)
            .filter(|&end| end <= dxil.len())
            .ok_or_else(|| ContainerError::Invalid(format!("XDAT entry {k} extends past the DXIL part")))?;
        if size > 0 && off < table_end && end > x {
            return Err(ContainerError::Invalid(format!("XDAT entry {k} overlaps its directory")));
        }
        out.push((tag, &dxil[off..end]));
    }
    Ok(out)
}

/// Decompress an XBCH payload ("XBCH", u32, u32 stored at 8, u32, u32 unpacked at 16, u32, zlib data at 24) to
/// its FSSM blob.
pub fn inflate_xbch(blob: &[u8]) -> Result<Vec<u8>, ContainerError> {
    if blob.len() < 24 || &blob[..4] != b"XBCH" {
        return Err(ContainerError::Invalid("a tag-0x0D XDAT entry is not an XBCH payload".into()));
    }
    let stored = u32_at(blob, 8) as usize;
    let unpacked = u32_at(blob, 16) as usize;
    if unpacked > MAX_FSSM_BYTES {
        return Err(ContainerError::Invalid(format!(
            "XBCH declared size {unpacked} exceeds limit {MAX_FSSM_BYTES}"
        )));
    }
    let compressed = &blob[24..];
    if stored != compressed.len() {
        return Err(ContainerError::Invalid(format!(
            "XBCH stored size {stored} does not match payload size {}",
            compressed.len()
        )));
    }
    let data = inflate(compressed, unpacked, true, true)?;
    if data.len() < 6 {
        return Err(ContainerError::Truncated("FSSM magic or version"));
    }
    if &data[..4] != b"FSSM" {
        return Err(ContainerError::Invalid("invalid FSSM magic".into()));
    }
    Ok(data)
}

/// Every tag-0x0D payload in the container, decompressed to its FSSM blob, in directory order.
pub fn unpack_all(dxbc: &[u8]) -> Result<Vec<Vec<u8>>, ContainerError> {
    let parts = parts(dxbc)?;
    let Some(dxil) = find_part(&parts, b"DXIL") else { return Ok(Vec::new()) };
    xdat_entries(dxil)?
        .into_iter()
        .filter(|&(tag, _)| tag == XBCH_TAG)
        .map(|(_, blob)| inflate_xbch(blob))
        .collect()
}

/// Inflate `input` (zlib-wrapped when `zlib`, raw deflate otherwise) to exactly `expected` bytes. With
/// `require_all_input`, bytes after the end of the stream are an error.
pub(crate) fn inflate(
    input: &[u8],
    expected: usize,
    zlib: bool,
    require_all_input: bool,
) -> Result<Vec<u8>, ContainerError> {
    let mut decoder = Decompress::new(zlib);
    // One spare byte detects a stream that expands past the declared size without inflating all of it.
    let mut out = Vec::with_capacity(expected + 1);
    let status = decoder
        .decompress_vec(input, &mut out, FlushDecompress::Finish)
        .map_err(|error| ContainerError::Invalid(format!("corrupt deflate stream: {error}")))?;
    if out.len() > expected {
        return Err(ContainerError::Invalid(format!("inflated data exceeds the declared size {expected}")));
    }
    if status != Status::StreamEnd {
        return Err(ContainerError::Truncated("deflate stream"));
    }
    if require_all_input && decoder.total_in() as usize != input.len() {
        return Err(ContainerError::Invalid("trailing data after the deflate stream".into()));
    }
    if out.len() != expected {
        return Err(ContainerError::Invalid(format!(
            "inflated size {} does not match the declared size {expected}",
            out.len()
        )));
    }
    Ok(out)
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p shader-container`
Expected: 11 tests pass.

- [ ] **Step 6: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add crates/shader-container
git commit -m "shader-container: XDAT directory and XBCH to FSSM unpacking"
```

---

### Task 10: `shader-container` — FSSM header, wave size, code range

**Files:**
- Create: `crates/shader-container/src/fssm.rs`
- Modify: `crates/shader-container/src/lib.rs`, `crates/shader-container/src/testutil.rs`

**Interfaces:**
- Consumes: `u16_at`, `u32_at`, `ContainerError` (Task 8).
- Produces:
  - `pub const ENDPGM: [u8; 4] = [0x00, 0x00, 0x81, 0xBF]` (`s_endpgm`, 0xBF810000 little-endian)
  - `pub enum WaveSize { Wave32, Wave64 }`
  - `pub enum CodeRange { Header { start: usize, end: usize }, Scan { end: usize }, NotFound }`
  - `pub struct Fssm<'a>` with `Fssm::parse(&'a [u8]) -> Result<Fssm<'a>, ContainerError>` (needs "FSSM" and at least 0xD8 bytes), `bytes()`, `version() -> u16`, `wave_size() -> WaveSize`, `code_range() -> CodeRange`.
  - `testutil::fssm(wave32: bool, code: &[u32]) -> Vec<u8>` — header offset 0x100, so code starts at 0x128, followed by `ENDPGM`.

Facts used (measured by upstream over all 26,503 blobs, re-checked in Task 12): wave size is bit 5 (0x20) of the u32 at 0xD4, set for wave32; the code section offset is the u32 at 0xC0 and instructions start 0x28 bytes into it, running to the last 4-byte-aligned `s_endpgm` minus an optional trailing constant-data block.

- [ ] **Step 1: Add the builder**

Append to `src/testutil.rs`:

```rust
/// A minimal FSSM blob: magic, version 1, code offset 0x100 at 0xC0 (so code starts at 0x128), the wave flag at
/// 0xD4, then `code` and a closing s_endpgm.
pub fn fssm(wave32: bool, code: &[u32]) -> Vec<u8> {
    const CODE_OFFSET: usize = 0x100;
    let mut out = vec![0u8; CODE_OFFSET + 0x28];
    out[..4].copy_from_slice(b"FSSM");
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[0xC0..0xC4].copy_from_slice(&(CODE_OFFSET as u32).to_le_bytes());
    if wave32 {
        out[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
    }
    for word in code {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(&crate::ENDPGM);
    out
}
```

- [ ] **Step 2: Write the failing tests**

`src/fssm.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fssm;

    const NOP: u32 = 0xBF80_0000; // s_nop 0
    const END: u32 = 0xBF81_0000; // s_endpgm

    #[test]
    fn wave_size_from_flag() {
        assert_eq!(Fssm::parse(&fssm(true, &[NOP])).unwrap().wave_size(), WaveSize::Wave32);
        assert_eq!(Fssm::parse(&fssm(false, &[NOP])).unwrap().wave_size(), WaveSize::Wave64);
        assert_eq!(Fssm::parse(&fssm(false, &[NOP])).unwrap().version(), 1);
    }

    #[test]
    fn code_range_from_header() {
        // NOP at 0x128, NOP at 0x12C, s_endpgm at 0x130
        let blob = fssm(false, &[NOP, NOP]);
        assert_eq!(Fssm::parse(&blob).unwrap().code_range(), CodeRange::Header { start: 0x128, end: 0x134 });
    }

    #[test]
    fn trailing_constant_data_is_cut() {
        // 0x128 NOP, 0x12C s_endpgm, 0x130 count 2, 0x134 data, 0x138 data, 0x13C s_endpgm
        let blob = fssm(false, &[NOP, END, 2, 0xBEEF, 0xDEAD]);
        assert_eq!(Fssm::parse(&blob).unwrap().code_range(), CodeRange::Header { start: 0x128, end: 0x130 });
    }

    #[test]
    fn falls_back_to_first_aligned_endpgm() {
        let mut blob = fssm(false, &[NOP, NOP]);
        blob[0xC0..0xC4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(Fssm::parse(&blob).unwrap().code_range(), CodeRange::Scan { end: 0x134 });
    }

    #[test]
    fn no_endpgm_anywhere() {
        let mut blob = vec![0u8; 0xD8];
        blob[..4].copy_from_slice(b"FSSM");
        assert_eq!(Fssm::parse(&blob).unwrap().code_range(), CodeRange::NotFound);
    }

    #[test]
    fn rejects_short_or_wrong_blobs() {
        assert_eq!(Fssm::parse(b"FSSM").unwrap_err(), ContainerError::Truncated("FSSM header"));
        let mut blob = fssm(false, &[NOP]);
        blob[0] = b'X';
        assert!(matches!(Fssm::parse(&blob), Err(ContainerError::Invalid(_))));
    }
}
```

Add to `lib.rs`: `mod fssm;` and `pub use fssm::{CodeRange, ENDPGM, Fssm, WaveSize};`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p shader-container`
Expected: FAIL to compile (`Fssm` not found).

- [ ] **Step 4: Implement** (top of `fssm.rs`)

```rust
use crate::ContainerError;
use crate::dxbc::{u16_at, u32_at};

/// `s_endpgm`, 0xBF810000 little-endian.
pub const ENDPGM: [u8; 4] = [0x00, 0x00, 0x81, 0xBF];

const CODE_OFFSET_FIELD: usize = 0xC0;
const CODE_START_BIAS: usize = 0x28;
const WAVE_FLAGS: usize = 0xD4;
const FLAG_WAVE32: u32 = 0x20;
const MIN_LEN: usize = 0xD8;
const MAX_DATA_DWORDS: usize = 1 << 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WaveSize {
    Wave32,
    Wave64,
}

/// Where a blob's instructions are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRange {
    /// From the header's code offset: instructions in `start..end`.
    Header { start: usize, end: usize },
    /// The header offset did not lead to code; only the end of the first aligned s_endpgm is known.
    Scan { end: usize },
    NotFound,
}

/// A precompiled RDNA2 program with its metadata, as unpacked from XBCH.
#[derive(Debug, Clone, Copy)]
pub struct Fssm<'a> {
    bytes: &'a [u8],
}

impl<'a> Fssm<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ContainerError> {
        if bytes.len() < MIN_LEN {
            return Err(ContainerError::Truncated("FSSM header"));
        }
        if &bytes[..4] != b"FSSM" {
            return Err(ContainerError::Invalid("not an FSSM blob".into()));
        }
        Ok(Self { bytes })
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn version(&self) -> u16 {
        u16_at(self.bytes, 4)
    }

    /// Bit 5 of the u32 at 0xD4: set for wave32, clear for wave64.
    pub fn wave_size(&self) -> WaveSize {
        if u32_at(self.bytes, WAVE_FLAGS) & FLAG_WAVE32 != 0 { WaveSize::Wave32 } else { WaveSize::Wave64 }
    }

    pub fn code_range(&self) -> CodeRange {
        code_range(self.bytes)
    }
}

/// The code section offset is the u32 at 0xC0; instructions start 0x28 bytes into it and run to the last
/// s_endpgm aligned to the start, minus a trailing constant-data block. Without a usable offset, only the first
/// 4-byte-aligned s_endpgm is known.
fn code_range(b: &[u8]) -> CodeRange {
    let off = u32_at(b, CODE_OFFSET_FIELD) as usize;
    let start = off + CODE_START_BIAS;
    if off > 0 && start < b.len() {
        let last = b[start..].chunks_exact(4).rposition(|word| word == ENDPGM.as_slice());
        if let Some(index) = last {
            let end = start + 4 * index + 4;
            return CodeRange::Header { start, end: data_block_start(b, start, end) };
        }
    }
    match b.chunks_exact(4).position(|word| word == ENDPGM.as_slice()) {
        Some(index) => CodeRange::Scan { end: 4 * index + 4 },
        None => CodeRange::NotFound,
    }
}

/// Where a constant-data block begins at the end of `start..end`, or `end` if there is none. Such a block is
/// s_endpgm, a dword count N, N data dwords, then the closing s_endpgm the range ends on. It is recognised only
/// when N matches the distance to the closing s_endpgm and the dword before the count is s_endpgm.
fn data_block_start(b: &[u8], start: usize, end: usize) -> usize {
    let last = end - 4;
    let dwords = (last - start) / 4;
    if dwords == 0 {
        return end;
    }
    for n in 0..=MAX_DATA_DWORDS.min(dwords - 1) {
        let count = last - 4 * (n + 1);
        if count < start + 4 {
            break;
        }
        if u32_at(b, count) as usize == n && b[count - 4..count] == ENDPGM {
            return count;
        }
    }
    end
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p shader-container`
Expected: 17 tests pass.

- [ ] **Step 6: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add crates/shader-container
git commit -m "shader-container: FSSM header, wave size and code range"
```

---

### Task 11: `shader-container` — carving records out of `shaders.rpf`

**Files:**
- Create: `crates/shader-container/src/carve.rs`
- Modify: `crates/shader-container/src/lib.rs`, `crates/shader-container/src/testutil.rs`

**Interfaces:**
- Consumes: `parts`, `shader_model`, `Stage`, `ShaderModel`, `u32_at` (Task 8), `inflate` (Task 9).
- Produces:
  - `pub struct CarvedShader { pub name: String, pub offset: usize, pub kind: u8, pub stage: Option<Stage>, pub shader_model: Option<ShaderModel>, pub dxbc: Vec<u8> }`
  - `pub struct CarveStats { pub carved: usize, pub rejected: usize }`
  - `pub fn carve(archive: &[u8], each: impl FnMut(CarvedShader)) -> CarveStats` — calls `each` once per shader, in archive order; never holds more than one container in memory.
  - `testutil::rpf_entry(name: &str, kind: u8, id: u32, dxbc: &[u8]) -> Vec<u8>`

Record layout (upstream `tools/shader_carve.py`): `<name: 3–200 bytes in 0x21..=0x7E> 00 <u8 kind> <u32 stored> <u32 id> <u32 stored again> <u32 size> <raw deflate, stored bytes>`, inflating to a DXBC container of `size` bytes. A candidate needs `16 < stored < 64 MiB` and `32 < size < 256 MiB`; candidates outside those bounds are skipped without counting. A candidate that fails to inflate to exactly `size` bytes starting with `DXBC`, or whose parts or shader model don't parse, counts as rejected. After a carved shader, scanning resumes after its stored bytes, and names never start before that point.

- [ ] **Step 1: Add the builder**

Append to `src/testutil.rs`:

```rust
use flate2::write::DeflateEncoder;

/// One shader record as stored in shaders.rpf: name, NUL, kind, stored, id, stored, size, raw deflate of `dxbc`.
pub fn rpf_entry(name: &str, kind: u8, id: u32, dxbc: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(dxbc).expect("in-memory write");
    let stored = encoder.finish().expect("in-memory write");
    let mut out = Vec::new();
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.push(kind);
    out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
    out.extend_from_slice(&(dxbc.len() as u32).to_le_bytes());
    out.extend_from_slice(&stored);
    out
}
```

(Merge the `use` lines at the top of `testutil.rs` so `Compression`, `ZlibEncoder`, `DeflateEncoder` and `Write` are imported once.)

- [ ] **Step 2: Write the failing tests**

`src/carve.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{container, fssm, rpf_entry};

    const NOP: u32 = 0xBF80_0000;

    fn collect(archive: &[u8]) -> (Vec<CarvedShader>, CarveStats) {
        let mut found = Vec::new();
        let stats = carve(archive, |shader| found.push(shader));
        (found, stats)
    }

    #[test]
    fn carves_records_between_junk() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let vs = container(1, &[fssm(true, &[NOP])]);
        let mut archive = b"\x01\x02junk\x00".to_vec();
        let first = archive.len();
        archive.extend(rpf_entry("ps_alpha", 3, 7, &ps));
        archive.extend_from_slice(b"\x00\x00more junk\x00");
        archive.extend(rpf_entry("vs_beta", 3, 8, &vs));
        let (found, stats) = collect(&archive);
        assert_eq!(stats, CarveStats { carved: 2, rejected: 0 });
        assert_eq!(found[0].name, "ps_alpha");
        assert_eq!(found[0].offset, first);
        assert_eq!(found[0].kind, 3);
        assert_eq!(found[0].stage, Some(Stage::Pixel));
        assert_eq!(found[0].shader_model, Some(ShaderModel { major: 6, minor: 6 }));
        assert_eq!(found[0].dxbc, ps);
        assert_eq!(found[1].name, "vs_beta");
        assert_eq!(found[1].stage, Some(Stage::Vertex));
        assert_eq!(found[1].dxbc, vs);
    }

    #[test]
    fn corrupt_deflate_is_rejected() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let mut entry = rpf_entry("ps_broken", 3, 1, &ps);
        let data_start = "ps_broken".len() + 1 + 17;
        for byte in &mut entry[data_start..] {
            *byte = 0xFF; // BTYPE 11 is reserved: inflate fails at once
        }
        let mut archive = vec![0u8];
        archive.extend(entry);
        let (found, stats) = collect(&archive);
        assert!(found.is_empty());
        assert_eq!(stats, CarveStats { carved: 0, rejected: 1 });
    }

    #[test]
    fn names_are_at_most_200_bytes() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let long = "a".repeat(250);
        let mut archive = vec![0u8];
        archive.extend(rpf_entry(&long, 3, 1, &ps));
        let (found, _) = collect(&archive);
        assert_eq!(found[0].name.len(), 200);
        assert_eq!(found[0].offset, 1 + 50);
    }

    #[test]
    fn short_names_and_empty_input_find_nothing() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let mut archive = vec![0u8];
        archive.extend(rpf_entry("ab", 3, 1, &ps));
        assert_eq!(collect(&archive).1, CarveStats { carved: 0, rejected: 0 });
        assert_eq!(collect(&[]).1, CarveStats::default());
    }
}
```

Add to `lib.rs`: `mod carve;` and `pub use carve::{CarveStats, CarvedShader, carve};`.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p shader-container`
Expected: FAIL to compile (`carve` not found).

- [ ] **Step 4: Implement** (top of `carve.rs`)

```rust
use crate::dxbc::{ShaderModel, Stage, parts, shader_model, u32_at};
use crate::xdat::inflate;

const MIN_NAME: usize = 3;
const MAX_NAME: usize = 200;
/// kind u8, stored u32, id u32, stored u32, size u32
const HEADER: usize = 17;

/// One shader container found in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarvedShader {
    pub name: String,
    /// Where the record's name starts in the archive.
    pub offset: usize,
    pub kind: u8,
    pub stage: Option<Stage>,
    pub shader_model: Option<ShaderModel>,
    pub dxbc: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CarveStats {
    pub carved: usize,
    /// Candidates whose header looked right but whose data did not inflate to a valid container.
    pub rejected: usize,
}

/// Find every shader record in `archive` (the bytes of shaders.rpf) without reading its encrypted table of
/// contents, and pass each one to `each` in archive order.
pub fn carve(archive: &[u8], mut each: impl FnMut(CarvedShader)) -> CarveStats {
    let mut stats = CarveStats::default();
    let mut floor = 0; // names never start before the end of the last carved record
    let mut pos = 0;
    while let Some(found) = memchr::memchr(0, &archive[pos..]) {
        let nul = pos + found;
        pos = nul + 1;
        let Some(name_start) = name_start(archive, floor, nul) else { continue };
        let Some(header) = archive.get(nul + 1..nul + 1 + HEADER) else { break };
        let kind = header[0];
        let stored = u32_at(header, 1) as usize;
        let size = u32_at(header, 13) as usize;
        let plausible = stored == u32_at(header, 9) as usize
            && (17..64 << 20).contains(&stored)
            && (33..256 << 20).contains(&size);
        if !plausible {
            continue;
        }
        let data_start = nul + 1 + HEADER;
        let data_end = archive.len().min(data_start + stored);
        match decode(&archive[data_start..data_end], size) {
            Some((dxbc, stage, shader_model)) => {
                let name = String::from_utf8_lossy(&archive[name_start..nul]).into_owned();
                each(CarvedShader { name, offset: name_start, kind, stage, shader_model, dxbc });
                stats.carved += 1;
                pos = data_end;
                floor = data_end;
            }
            None => stats.rejected += 1,
        }
    }
    stats
}

/// Start of the name that ends at `nul`: the printable run before it, at most MAX_NAME bytes and never before
/// `floor`; None when shorter than MIN_NAME.
fn name_start(archive: &[u8], floor: usize, nul: usize) -> Option<usize> {
    let lowest = floor.max(nul.saturating_sub(MAX_NAME));
    let mut start = nul;
    while start > lowest && (0x21..=0x7E).contains(&archive[start - 1]) {
        start -= 1;
    }
    (nul - start >= MIN_NAME).then_some(start)
}

/// Inflate a record's raw deflate data to a DXBC container of exactly `size` bytes and read its stage.
fn decode(data: &[u8], size: usize) -> Option<(Vec<u8>, Option<Stage>, Option<ShaderModel>)> {
    let dxbc = inflate(data, size, false, false).ok()?;
    if !dxbc.starts_with(b"DXBC") {
        return None;
    }
    let model = shader_model(&parts(&dxbc).ok()?).ok()?;
    let (stage, shader_model) = match model {
        Some((stage, shader_model)) => (Some(stage), Some(shader_model)),
        None => (None, None),
    };
    Some((dxbc, stage, shader_model))
}
```

Note the bounds: `(17..64 << 20).contains(&stored)` is upstream's `16 < stored < 64 << 20`, and `(33..256 << 20).contains(&size)` is `32 < size < 256 << 20`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p shader-container`
Expected: 21 tests pass.

- [ ] **Step 6: Format, lint, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
git add crates/shader-container
git commit -m "shader-container: carve shader records out of shaders.rpf"
```

---

### Task 12: `leonida shaders scan` and the corpus check

**Files:**
- Create: `crates/leonida/src/cmd_shaders.rs`
- Modify: `crates/leonida/src/cli.rs`, `crates/leonida/src/main.rs`, `crates/leonida/Cargo.toml`
- Test: `crates/leonida/tests/shaders.rs`

**Interfaces:**
- Consumes: `game::resolve_game` (Task 6); `game_package::{Manifest, map_file, package_path}`; `shader_container::{carve, unpack_all, Fssm, WaveSize, CodeRange, CarvedShader, CarveStats}`.
- Produces: CLI `leonida shaders scan [--game <folder>]`. Prints a deterministic report; exit 0 when at least one shader was carved, 1 when none, 2 on errors.

- [ ] **Step 1: Write the failing integration test**

`crates/leonida/Cargo.toml`: add to `[dependencies]` `shader-container.workspace = true`; add to `[dev-dependencies]`:

```toml
shader-container = { workspace = true, features = ["test-support"] }
```

`crates/leonida/tests/shaders.rs`:

```rust
use std::process::Command;

use shader_container::testutil::{container, fssm, rpf_entry};

const NOP: u32 = 0xBF80_0000;

#[test]
fn scan_reports_counts() {
    let dir = tempfile::tempdir().unwrap();
    let mut archive = vec![0u8; 8];
    archive.extend(rpf_entry("ps_one", 3, 1, &container(0, &[fssm(false, &[NOP])])));
    archive.push(0);
    let two_payloads = container(5, &[fssm(true, &[NOP]), fssm(true, &[NOP, NOP])]);
    archive.extend(rpf_entry("cs_two", 3, 2, &two_payloads));
    std::fs::write(dir.path().join("shaders.rpf"), &archive).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_leonida"))
        .args(["shaders", "scan", "--game"])
        .arg(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("shaders.rpf: 2 containers carved, 0 candidates rejected"), "{text}");
    assert!(text.contains("stages: cs 1, ps 1"), "{text}");
    assert!(text.contains("container parts: DXIL 2, PRIV 2"), "{text}");
    assert!(text.contains("FSSM payloads: 3 (1 container has more than one; the last is used below)"), "{text}");
    assert!(text.contains("wave size by stage: cs wave32 1, ps wave64 1"), "{text}");
    assert!(text.contains("wave size: wave32 1, wave64 1"), "{text}");
    assert!(text.contains("code range: header 2"), "{text}");
    assert!(text.contains("problems: none"), "{text}");
}

#[test]
fn missing_archive_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_leonida"))
        .args(["shaders", "scan", "--game"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("shaders.rpf"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p leonida --test shaders`
Expected: FAIL — `shaders` is not a known subcommand.

- [ ] **Step 3: Implement**

`crates/leonida/src/cli.rs` — add to `enum Command`:

```rust
    /// Work with the game's shaders.
    Shaders(ShadersArgs),
```

and append:

```rust
#[derive(Debug, Args)]
pub struct ShadersArgs {
    #[command(subcommand)]
    pub command: ShadersCommand,
}

#[derive(Debug, Subcommand)]
pub enum ShadersCommand {
    /// Read every shader container in shaders.rpf and report what it holds. Writes nothing.
    Scan(ScanArgs),
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// The game folder. Searched for when omitted.
    #[arg(long)]
    pub game: Option<PathBuf>,
}
```

`crates/leonida/src/main.rs` — add `mod cmd_shaders;`, import `ShadersCommand`, and extend the match:

```rust
        Command::Shaders(shaders) => match &shaders.command {
            ShadersCommand::Scan(args) => cmd_shaders::scan(args),
        },
```

(`use cli::{Cli, Command, ShadersCommand};`)

`crates/leonida/src/cmd_shaders.rs`:

```rust
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::process::ExitCode;

use anyhow::{Context, Result};
use game_package::{Manifest, map_file, package_path};
use shader_container::{CarveStats, CarvedShader, CodeRange, Fssm, WaveSize, carve, parts, unpack_all};

use crate::cli::ScanArgs;
use crate::game::resolve_game;

pub fn scan(args: &ScanArgs) -> Result<ExitCode> {
    let game = resolve_game(args.game.as_ref(), &Manifest::builtin())?;
    let path = package_path(&game, "shaders.rpf");
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let archive = map_file(&file).with_context(|| format!("mapping {}", path.display()))?;
    let mut summary = ScanSummary::default();
    let stats = carve(&archive, |shader| summary.add(&shader));
    print!("{}", summary.report(&stats));
    Ok(if stats.carved == 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Counts gathered over every carved container. BTreeMaps keep the printed order stable.
#[derive(Debug, Default)]
struct ScanSummary {
    stages: BTreeMap<String, usize>,
    /// How many containers carry each DXBC part. Reflection parts (RDEF, STAT, PSV0, RDAT, ISG1, OSG1) here would
    /// give constant-buffer layouts for the renderer harness (spec section 13, risk 1).
    parts: BTreeMap<String, usize>,
    payloads: usize,
    multi_payload: usize,
    wave_by_stage: BTreeMap<String, usize>,
    wave: BTreeMap<&'static str, usize>,
    ranges: BTreeMap<&'static str, usize>,
    problems: BTreeMap<String, usize>,
}

impl ScanSummary {
    fn add(&mut self, shader: &CarvedShader) {
        let stage = shader.stage.map_or_else(|| "none".to_string(), |stage| stage.short_name());
        *self.stages.entry(stage.clone()).or_default() += 1;
        if let Ok(container_parts) = parts(&shader.dxbc) {
            for part in container_parts {
                *self.parts.entry(String::from_utf8_lossy(&part.fourcc).into_owned()).or_default() += 1;
            }
        }
        let blobs = match unpack_all(&shader.dxbc) {
            Ok(blobs) => blobs,
            Err(error) => {
                *self.problems.entry(error.to_string()).or_default() += 1;
                return;
            }
        };
        self.payloads += blobs.len();
        if blobs.len() > 1 {
            self.multi_payload += 1;
        }
        let Some(last) = blobs.last() else {
            *self.problems.entry("no XBCH payload".to_string()).or_default() += 1;
            return;
        };
        match Fssm::parse(last) {
            Ok(fssm) => {
                let wave = match fssm.wave_size() {
                    WaveSize::Wave32 => "wave32",
                    WaveSize::Wave64 => "wave64",
                };
                *self.wave.entry(wave).or_default() += 1;
                *self.wave_by_stage.entry(format!("{stage} {wave}")).or_default() += 1;
                let range = match fssm.code_range() {
                    CodeRange::Header { .. } => "header",
                    CodeRange::Scan { .. } => "scan",
                    CodeRange::NotFound => "not found",
                };
                *self.ranges.entry(range).or_default() += 1;
            }
            Err(error) => *self.problems.entry(error.to_string()).or_default() += 1,
        }
    }

    fn report(&self, stats: &CarveStats) -> String {
        let mut out = String::new();
        let w = &mut out;
        writeln!(w, "shaders.rpf: {} containers carved, {} candidates rejected", stats.carved, stats.rejected).ok();
        writeln!(w, "stages: {}", counts(&self.stages)).ok();
        writeln!(w, "container parts: {}", counts(&self.parts)).ok();
        writeln!(
            w,
            "FSSM payloads: {} ({} container{} more than one; the last is used below)",
            self.payloads,
            self.multi_payload,
            if self.multi_payload == 1 { " has" } else { "s have" }
        )
        .ok();
        writeln!(w, "wave size by stage: {}", counts(&self.wave_by_stage)).ok();
        writeln!(w, "wave size: {}", counts(&self.wave)).ok();
        writeln!(w, "code range: {}", counts(&self.ranges)).ok();
        if self.problems.is_empty() {
            writeln!(w, "problems: none").ok();
        } else {
            writeln!(w, "problems:").ok();
            for (problem, count) in &self.problems {
                writeln!(w, "  {count} x {problem}").ok();
            }
        }
        out
    }
}

/// "a 1, b 2" from a map of counts.
fn counts<K: std::fmt::Display>(map: &BTreeMap<K, usize>) -> String {
    let parts: Vec<String> = map.iter().map(|(key, count)| format!("{key} {count}")).collect();
    if parts.is_empty() { "none".to_string() } else { parts.join(", ") }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace --all-features`
Expected: all tests pass (xtask 5, game-package 16, shader-container 21, leonida 6).

- [ ] **Step 5: Corpus check on the real package (manual, results stay local)**

Run: `cargo run --release -p leonida -- shaders scan --game "<your package folder>"`

Expected, from upstream's measurements of the same package:
- `26503 containers carved`
- `wave size: wave32 2662, wave64 23841`
- `195` containers with more than one payload
- stage totals consistent with `ps 12101`, `vs 2236`, `cs 2689`, `ms 9282`, `gs 195` (upstream counted stages per FSSM program; the container stage field may group some differently — record what you see)
- `problems: none`
- Record the `container parts` line: if any reflection part (RDEF, STAT, PSV0, RDAT, ISG1, OSG1) appears, the
  renderer harness can read constant-buffer layouts instead of inferring them (spec section 13, risk 1).

If a number differs, do not adjust the code to match: write down the observed counts and the difference, and raise it before Phase 1 continues. Do not paste shader names or bytes anywhere outside your machine; counts are fine.

- [ ] **Step 6: Format, lint, scan, commit**

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo xtask scan-game-data
git add crates/leonida
git commit -m "leonida shaders scan: carve, unpack and summarise every shader container"
```

---

## After this plan

The remaining milestones, in order, are in `docs/plans/2026-11-26-roadmap.md`. Every dependency is pure Rust (spec section 2.1), so the current `stable-x86_64-pc-windows-gnu` toolchain needs no C/C++ compiler; the toolchain for release builds of the Phase 4 runtime is decided in that milestone's plan.
