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
        eprintln!(
            "{flagged} file(s) look like game data or could not be read. Game data must never be committed."
        );
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
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(out.stdout)
}

fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    File::open(path)?.take(HEAD).read_to_end(&mut head)?;
    Ok(head)
}

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
        assert_eq!(
            classify(b"RIFF\x10\x00\x00\x00WAVEfmt "),
            Some("RIFF WAVE audio")
        );
    }
}
