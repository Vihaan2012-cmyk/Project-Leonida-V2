//! Fingerprints the sources that decide what a translated shader looks like, so stored shaders are retranslated
//! when the translator changes.

use std::fs;
use std::path::{Path, PathBuf};

const WATCHED: [&str; 5] = [
    "../shader-translate/src",
    "../shader-ir/src",
    "../rdna/src",
    "../shader-container/src",
    "src/translate.rs",
];

fn files(path: &Path, into: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            files(&entry.path(), into);
        }
    } else {
        into.push(path.to_path_buf());
    }
}

fn main() {
    let mut list = Vec::new();
    for path in WATCHED {
        println!("cargo:rerun-if-changed={path}");
        files(Path::new(path), &mut list);
    }
    list.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    for path in &list {
        path.to_string_lossy()
            .bytes()
            .map(|b| if b == b'\\' { b'/' } else { b })
            .for_each(&mut feed);
        // Line endings differ between checkouts and must not change the fingerprint.
        fs::read(path)
            .unwrap_or_default()
            .into_iter()
            .filter(|b| *b != b'\r')
            .for_each(&mut feed);
    }
    println!("cargo:rustc-env=LEONIDA_TRANSLATOR_HASH={hash:016x}");
}
