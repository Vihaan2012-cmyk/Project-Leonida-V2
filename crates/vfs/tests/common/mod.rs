#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

pub fn put(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

pub fn manifest(title: &str, kind: &str, deps: &[&str], game: Option<&str>) -> String {
    let deps: Vec<String> = deps.iter().map(|d| format!("\"{d}\"")).collect();
    let game = game.map_or(String::new(), |g| format!(",\"game_version\":\"{g}\""));
    format!(
        "{{\"title\":\"{title}\",\"author\":\"test\",\"version\":\"1.0\",\"type\":\"{kind}\",\"dependencies\":[{}]{game}}}",
        deps.join(",")
    )
}

/// A package folder `<mods>/<id>` with the manifest and the given files.
pub fn package(
    mods: &Path,
    id: &str,
    kind: &str,
    deps: &[&str],
    files: &[(&str, &str)],
) -> PathBuf {
    let root = mods.join(id);
    put(
        &root,
        "manifest.json",
        manifest(id, kind, deps, Some("1.0.2.1")).as_bytes(),
    );
    for (path, text) in files {
        put(&root, path, text.as_bytes());
    }
    root
}
