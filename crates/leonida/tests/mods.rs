use std::fs;
use std::path::Path;
use std::process::Command;

fn leonida(root: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_leonida"))
        .arg("mods")
        .args(args)
        .arg("--root")
        .arg(root)
        .arg("--game-version")
        .arg("1.0.2.1")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn package(root: &Path, id: &str, kind: &str, deps: &str, files: &[(&str, &str)]) {
    let dir = root.join("Mods").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{"title":"{id} title","author":"me","version":"1.2","type":"{kind}","dependencies":[{deps}],"game_version":"1.0.2.1"}}"#
        ),
    )
    .unwrap();
    for (path, text) in files {
        let file = dir.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
}

#[test]
fn lists_packages_in_load_order_with_problems() {
    let dir = tempfile::tempdir().unwrap();
    package(dir.path(), "shiny", "assets", "", &[("a.txt", "1")]);
    package(dir.path(), "tuning", "systems", "", &[("a.txt", "2")]);
    package(dir.path(), "lonely", "assets", "\"missing-base\"", &[]);
    let (ok, text) = leonida(dir.path(), &["list"]);
    assert!(ok);
    let shiny = text.find("shiny title 1.2").unwrap();
    let tuning = text.find("tuning title 1.2").unwrap();
    assert!(shiny < tuning, "assets load below systems:\n{text}");
    assert!(text.contains("[assets]") && text.contains("by me"));
    assert!(
        text.contains("ERR  lonely  needs `missing-base`, which is not installed"),
        "{text}"
    );
}

#[test]
fn conflicts_name_the_winner_and_honour_mods_toml() {
    let dir = tempfile::tempdir().unwrap();
    package(
        dir.path(),
        "one",
        "assets",
        "",
        &[("Shared/x.dds", "1"), ("own.txt", "1")],
    );
    package(dir.path(), "two", "assets", "", &[("shared/X.dds", "2")]);
    let (ok, text) = leonida(dir.path(), &["conflicts"]);
    assert!(ok);
    assert!(text.contains("two (wins) over one"), "{text}");
    assert!(text.contains("1 contested file(s)"));

    fs::create_dir_all(dir.path().join("Config")).unwrap();
    fs::write(
        dir.path().join("Config/mods.toml"),
        "order = [\"two\", \"one\"]\n",
    )
    .unwrap();
    let (_, text) = leonida(dir.path(), &["conflicts"]);
    assert!(text.contains("one (wins) over two"), "{text}");
}

#[test]
fn conflicts_with_the_game_folder_include_base_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    fs::create_dir_all(game.join("data")).unwrap();
    fs::write(game.join("data/config.xml"), "base").unwrap();
    package(
        dir.path(),
        "tweak",
        "systems",
        "",
        &[("Data/Config.xml", "mod")],
    );
    let (_, without) = leonida(dir.path(), &["conflicts"]);
    assert!(without.contains("0 contested"), "{without}");
    let game = game.display().to_string();
    let (_, with) = leonida(dir.path(), &["conflicts", "--game", &game]);
    assert!(with.contains("tweak (wins) over base game"), "{with}");
}

#[test]
fn an_empty_root_lists_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, text) = leonida(dir.path(), &["list"]);
    assert!(ok);
    assert!(text.contains("no packages in"), "{text}");
}
