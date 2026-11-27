mod common;

use common::{package, put};
use vfs::{
    BuildError, ContentFilter, LayerKind, LooseSource, MemorySource, ModStatus, PackageError, Vfs,
};

const GAME: &str = "1.0.2.1";

fn status<'a>(report: &'a vfs::LoadReport, id: &str) -> &'a ModStatus {
    &report.mods.find(id).unwrap().status
}

fn error<'a>(report: &'a vfs::LoadReport, id: &str) -> &'a PackageError {
    match status(report, id) {
        ModStatus::Error(e) => e,
        other => panic!("{id}: {other:?}"),
    }
}

fn active(report: &vfs::LoadReport) -> Vec<&str> {
    report.mods.active.iter().map(|e| e.id.as_str()).collect()
}

#[test]
fn layers_stack_base_then_fixes_then_mods_and_paths_ignore_case() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    put(&game, "Systems/Vehicles/handling.xml", b"base");
    put(&game, "data/only_base.txt", b"b");
    let fixes = package(
        dir.path(),
        "leonida-fixes",
        "patch",
        &[],
        &[("Systems/Vehicles/handling.xml", "fix")],
    );
    let mods = dir.path().join("Mods");
    package(
        &mods,
        "tune",
        "systems",
        &[],
        &[("systems/vehicles/HANDLING.XML", "tune")],
    );
    package(
        &mods,
        "other",
        "assets",
        &[],
        &[("Characters/Lucia/Lucia.json", "{}")],
    );

    let (vfs, report) = Vfs::builder(GAME)
        .unwrap()
        .base(LooseSource::new(&game))
        .fixes(&fixes)
        .mods_dir(&mods)
        .build()
        .unwrap();

    let layers = vfs.layers();
    assert_eq!(layers.len(), 4);
    assert_eq!(layers[0].kind, LayerKind::Base);
    assert_eq!(layers[1].kind, LayerKind::Fixes);
    assert_eq!(
        active(&report),
        ["other", "tune"],
        "assets sort below systems"
    );

    assert_eq!(
        vfs.read("SYSTEMS\\vehicles\\Handling.XML").unwrap(),
        b"tune"
    );
    assert_eq!(
        vfs.winner_of("systems/vehicles/handling.xml")
            .unwrap()
            .as_deref(),
        Some("tune")
    );
    assert_eq!(vfs.read("Data/ONLY_BASE.txt").unwrap(), b"b");
    assert_eq!(vfs.read("characters/lucia/lucia.JSON").unwrap(), b"{}");
    assert!(
        vfs.stat("manifest.json").unwrap().is_none(),
        "manifests are not content"
    );
    assert!(vfs.is_dir("CHARACTERS/lucia").unwrap());
    assert!(!vfs.is_dir("characters/lucia/lucia.json").unwrap());

    let systems: Vec<_> = vfs
        .list("systems")
        .unwrap()
        .into_iter()
        .map(|e| (e.name.to_lowercase(), e.is_dir))
        .collect();
    assert_eq!(systems, [("vehicles".to_string(), true)]);
    let mut root: Vec<_> = vfs
        .list("")
        .unwrap()
        .into_iter()
        .map(|e| e.name.to_lowercase())
        .collect();
    root.sort();
    assert_eq!(root, ["characters", "data", "systems"]);
}

#[test]
fn the_winner_can_be_read_without_a_real_file() {
    let (vfs, _) = Vfs::builder(GAME)
        .unwrap()
        .base(MemorySource::new().with("Archive/Thing.bin", b"packed".to_vec()))
        .build()
        .unwrap();
    let info = vfs.stat("archive/thing.bin").unwrap().unwrap();
    assert_eq!(info.size, 6);
    assert!(vfs.host_path(&info).is_none());
    assert_eq!(vfs.read_source(&info).unwrap(), b"packed");
    let mut head = [0u8; 3];
    std::io::Read::read_exact(&mut vfs.open(&info).unwrap(), &mut head).unwrap();
    assert_eq!(&head, b"pac");
}

#[test]
fn default_order_is_type_then_name_and_mods_toml_overrides_it() {
    let dir = tempfile::tempdir().unwrap();
    let mods = dir.path().join("Mods");
    for (id, kind) in [
        ("zeta", "assets"),
        ("alpha", "patch"),
        ("mid", "systems"),
        ("beta", "assets"),
        ("shade", "shaders"),
        ("mix", "mixed"),
    ] {
        package(&mods, id, kind, &[], &[("shared.txt", id)]);
    }
    let build = |toml: Option<&str>| {
        let mut builder = Vfs::builder(GAME).unwrap().mods_dir(&mods);
        if let Some(text) = toml {
            builder = builder.mods_settings(vfs::ModsConfig::from_toml(text).unwrap());
        }
        builder.build().unwrap()
    };
    let (vfs, report) = build(None);
    assert_eq!(
        active(&report),
        ["beta", "zeta", "shade", "mid", "mix", "alpha"]
    );
    assert_eq!(
        vfs.winner_of("shared.txt").unwrap().as_deref(),
        Some("alpha")
    );

    let (vfs, report) = build(Some(
        "order = [\"ZETA\", \"beta\", \"ghost\"]\ndisabled = [\"mix\"]",
    ));
    assert_eq!(active(&report), ["shade", "mid", "alpha", "zeta", "beta"]);
    assert_eq!(
        vfs.winner_of("shared.txt").unwrap().as_deref(),
        Some("beta")
    );
    assert_eq!(status(&report, "mix"), &ModStatus::Disabled);
    assert!(report.mods.notes.iter().any(|n| n.contains("ghost")));
}

#[test]
fn mods_toml_file_is_read_and_rejects_typos() {
    let dir = tempfile::tempdir().unwrap();
    let mods = dir.path().join("Mods");
    package(&mods, "a", "assets", &[], &[("x.txt", "a")]);
    package(&mods, "b", "assets", &[], &[("x.txt", "b")]);
    let config = put(dir.path(), "Config/mods.toml", b"order = [\"b\", \"a\"]\n");
    let (vfs, _) = Vfs::builder(GAME)
        .unwrap()
        .mods_dir(&mods)
        .mods_config(&config)
        .build()
        .unwrap();
    assert_eq!(vfs.winner_of("x.txt").unwrap().as_deref(), Some("a"));

    let missing = dir.path().join("Config/none.toml");
    assert!(
        Vfs::builder(GAME)
            .unwrap()
            .mods_dir(&mods)
            .mods_config(&missing)
            .build()
            .is_ok()
    );
    let bad = put(dir.path(), "Config/bad.toml", b"oder = [\"b\"]\n");
    let result = Vfs::builder(GAME)
        .unwrap()
        .mods_dir(&mods)
        .mods_config(bad)
        .build();
    assert!(matches!(result, Err(BuildError::Config(_))));
}

#[test]
fn startup_checks_say_what_is_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let mods = dir.path().join("Mods");
    package(&mods, "core", "systems", &[], &[("core.txt", "1")]);
    package(
        &mods,
        "good",
        "assets",
        &["core >= 1.0"],
        &[("good.txt", "1")],
    );
    package(&mods, "needs-missing", "assets", &["nothing-here"], &[]);
    package(&mods, "needs-new", "assets", &["core >= 2.5"], &[]);
    package(&mods, "chain", "assets", &["needs-missing"], &[]);
    package(&mods, "cyc-a", "assets", &["cyc-b"], &[]);
    package(&mods, "cyc-b", "assets", &["cyc-a"], &[]);
    package(&mods, "off", "assets", &[], &[]);
    package(&mods, "needs-off", "assets", &["off"], &[]);
    put(
        &mods,
        "old/manifest.json",
        common::manifest("old", "assets", &[], Some("0.9")).as_bytes(),
    );
    put(&mods, "nomanifest/readme.txt", b"hi");
    put(&mods, "broken/manifest.json", b"{ nope");
    let config = vfs::ModsConfig::from_toml("disabled = [\"off\"]").unwrap();
    let (vfs, report) = Vfs::builder(GAME)
        .unwrap()
        .mods_dir(&mods)
        .mods_settings(config)
        .build()
        .unwrap();

    assert_eq!(active(&report), ["core", "good"]);
    assert!(vfs.exists("good.txt").unwrap());
    assert_eq!(
        error(&report, "needs-missing").to_string(),
        "needs `nothing-here`, which is not installed"
    );
    assert_eq!(
        error(&report, "needs-new").to_string(),
        "needs `core` version 2.5 or newer, but 1.0 is installed"
    );
    assert!(
        matches!(error(&report, "chain"), PackageError::BrokenDependency { dependency } if dependency == "needs-missing")
    );
    assert!(
        matches!(error(&report, "cyc-a"), PackageError::Cycle(text) if text.contains("cyc-a -> cyc-b -> cyc-a"))
    );
    assert!(matches!(error(&report, "cyc-b"), PackageError::Cycle(_)));
    assert!(matches!(
        error(&report, "needs-off"),
        PackageError::DisabledDependency { .. }
    ));
    assert_eq!(
        error(&report, "old").to_string(),
        "made for game version 0.9, but the game is 1.0.2.1"
    );
    assert_eq!(error(&report, "nomanifest"), &PackageError::ManifestMissing);
    assert!(matches!(
        error(&report, "broken"),
        PackageError::ManifestInvalid(_)
    ));
    assert_eq!(report.problems().len(), 9);

    let strict = Vfs::builder(GAME)
        .unwrap()
        .mods_dir(&mods)
        .strict(true)
        .build();
    assert!(matches!(strict, Err(BuildError::Package { .. })));
}

#[test]
fn a_package_loads_above_what_it_depends_on_even_against_the_order() {
    let dir = tempfile::tempdir().unwrap();
    let mods = dir.path().join("Mods");
    package(&mods, "lib", "assets", &[], &[("x.txt", "lib")]);
    package(&mods, "app", "assets", &["lib"], &[("x.txt", "app")]);
    let config = vfs::ModsConfig::from_toml("order = [\"app\", \"lib\"]").unwrap();
    let (vfs, report) = Vfs::builder(GAME)
        .unwrap()
        .mods_dir(&mods)
        .mods_settings(config)
        .build()
        .unwrap();
    assert_eq!(active(&report), ["lib", "app"]);
    assert_eq!(vfs.read("x.txt").unwrap(), b"app");
    assert!(
        report
            .mods
            .notes
            .iter()
            .any(|n| n.contains("`lib` loads before `app`"))
    );
}

#[test]
fn dependencies_may_be_the_fixes_package() {
    let dir = tempfile::tempdir().unwrap();
    let fixes = package(dir.path(), "leonida-fixes", "patch", &[], &[("f.txt", "f")]);
    let mods = dir.path().join("Mods");
    package(
        &mods,
        "uses-fixes",
        "assets",
        &["Leonida-Fixes >= 1.0"],
        &[],
    );
    let (_, report) = Vfs::builder(GAME)
        .unwrap()
        .fixes(&fixes)
        .mods_dir(&mods)
        .build()
        .unwrap();
    assert_eq!(active(&report), ["uses-fixes"]);
    let (_, report) = Vfs::builder(GAME).unwrap().mods_dir(&mods).build().unwrap();
    assert!(matches!(
        error(&report, "uses-fixes"),
        PackageError::MissingDependency { .. }
    ));
}

#[test]
fn a_fixes_package_for_another_game_version_stops_the_build() {
    let dir = tempfile::tempdir().unwrap();
    let fixes = dir.path().join("fixes");
    put(
        &fixes,
        "manifest.json",
        common::manifest("f", "patch", &[], Some("2.0")).as_bytes(),
    );
    let result = Vfs::builder(GAME).unwrap().fixes(&fixes).build();
    assert!(matches!(result, Err(BuildError::Fixes { .. })));
}

#[test]
fn conflicts_list_every_contested_path_and_its_winner() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    put(&game, "Systems/a.xml", b"base");
    put(&game, "Systems/b.xml", b"base");
    let mods = dir.path().join("Mods");
    package(
        &mods,
        "one",
        "systems",
        &[],
        &[("systems/a.xml", "1"), ("only-one.txt", "1")],
    );
    package(
        &mods,
        "two",
        "patch",
        &[],
        &[("Systems/A.xml", "2"), ("Systems/b.xml", "2")],
    );
    let (vfs, _) = Vfs::builder(GAME)
        .unwrap()
        .base(LooseSource::new(&game))
        .mods_dir(&mods)
        .build()
        .unwrap();

    let between = vfs.conflicts(false);
    assert_eq!(between.len(), 1);
    assert_eq!(between[0].path.to_lowercase(), "systems/a.xml");
    assert_eq!(between[0].layers, ["one", "two"]);
    assert_eq!(between[0].winner(), "two");

    let all = vfs.conflicts(true);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].layers, ["base game", "one", "two"]);
    assert_eq!(all[1].layers, ["base game", "two"]);
}

#[test]
fn paths_the_offline_policy_names_are_refused() {
    let (vfs, _) = Vfs::builder(GAME).unwrap().build().unwrap();
    for bad in [
        "tcpip:host/x",
        "\\\\server\\share\\f",
        "http://example.invalid/a",
        "../up",
    ] {
        assert!(vfs.stat(bad).is_err(), "{bad}");
        assert!(vfs.read(bad).is_err(), "{bad}");
    }
}

#[test]
fn files_that_cannot_be_named_are_skipped_with_a_note() {
    let (vfs, report) = Vfs::builder(GAME)
        .unwrap()
        .base(
            MemorySource::new()
                .with("ok/a.txt", b"a".to_vec())
                .with("bad/con.txt", b"x".to_vec()),
        )
        .build()
        .unwrap();
    assert!(vfs.exists("ok/a.txt").unwrap());
    assert_eq!(report.notes.len(), 1);
    assert!(report.notes[0].contains("con.txt"));
}

#[cfg(unix)]
#[test]
fn links_in_a_package_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let secret = put(dir.path(), "secret/key.txt", b"s");
    let mods = dir.path().join("Mods");
    let root = package(&mods, "linky", "assets", &[], &[]);
    std::os::unix::fs::symlink(secret.parent().unwrap(), root.join("out")).unwrap();
    let (vfs, _) = Vfs::builder(GAME).unwrap().mods_dir(&mods).build().unwrap();
    assert!(!vfs.exists("out/key.txt").unwrap());
}

#[test]
fn filter_hides_replaces_and_matches_content_through_every_read() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let mods = dir.path().join("Mods");
    package(
        &mods,
        "skins",
        "assets",
        &[],
        &[
            ("Characters/Lucia/skin_a.dds", "revealing"),
            ("Characters/Lucia/skin_b.dds", "revealing"),
            ("Characters/Lucia/face.dds", "face"),
            ("Characters/Mandy/hidden_dir/only.dds", "hide me"),
            ("Filters/clothed.dds", "clothed"),
        ],
    );
    let digest: [u8; 32] = Sha256::digest(b"hide me").into();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let filter = ContentFilter::from_toml(&format!(
        r#"
        [[rule]]
        id = "swap-a"
        path = "characters/**/skin_a.dds"
        action = "replace"
        with = "Filters/clothed.dds"

        [[rule]]
        id = "hide-b"
        name = "skin_b"
        action = "hide"

        [[rule]]
        id = "by-content"
        sha256 = "{hex}"
        action = "hide"

        [[rule]]
        id = "broken-swap"
        name = "face"
        action = "replace"
        with = "Filters/missing.dds"
        "#
    ))
    .unwrap();
    let (vfs, _) = Vfs::builder(GAME)
        .unwrap()
        .mods_dir(&mods)
        .filter(filter)
        .build()
        .unwrap();

    assert_eq!(vfs.read("Characters/Lucia/skin_a.dds").unwrap(), b"clothed");
    let info = vfs.stat("Characters/Lucia/skin_a.dds").unwrap().unwrap();
    assert!(info.replaced);
    assert_eq!(info.size, 7);
    assert!(vfs.stat("Characters/Lucia/SKIN_B.dds").unwrap().is_none());
    assert!(vfs.read("Characters/Lucia/skin_b.dds").is_err());
    assert!(
        vfs.stat("Characters/Mandy/hidden_dir/only.dds")
            .unwrap()
            .is_none()
    );
    assert!(
        vfs.stat("Characters/Lucia/face.dds").unwrap().is_none(),
        "a missing replacement fails closed"
    );

    let lucia: Vec<_> = vfs
        .list("Characters/Lucia")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(lucia, ["skin_a.dds"]);
    let characters: Vec<_> = vfs
        .list("Characters")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(
        characters,
        ["Lucia"],
        "a folder with nothing visible disappears"
    );
    assert!(!vfs.is_dir("Characters/Mandy").unwrap());
    let all: Vec<_> = vfs.files().into_iter().map(|f| f.path).collect();
    assert!(!all.iter().any(|p| p.contains("skin_b")));
}

#[test]
fn a_package_cannot_bring_back_what_the_filter_hides() {
    let dir = tempfile::tempdir().unwrap();
    let mods = dir.path().join("Mods");
    package(&mods, "low", "assets", &[], &[("a/x.dds", "1")]);
    package(&mods, "high", "patch", &[], &[("a/x.dds", "2")]);
    let filter =
        ContentFilter::from_toml("[[rule]]\nid=\"h\"\npath=\"a/x.dds\"\naction=\"hide\"\n")
            .unwrap();
    let (vfs, _) = Vfs::builder(GAME)
        .unwrap()
        .mods_dir(&mods)
        .filter(filter)
        .build()
        .unwrap();
    assert!(!vfs.exists("a/x.dds").unwrap());
}

#[test]
fn the_settings_files_load_order_is_read_as_the_order() {
    let config = vfs::ModsConfig::from_toml("load_order = [\"first\", \"second\"]").unwrap();
    assert_eq!(config.order, ["first", "second"]);
}
