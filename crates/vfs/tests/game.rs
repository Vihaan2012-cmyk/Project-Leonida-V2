mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{package, put};
use vfs::{
    Area, Assumption, ContentFilter, EntryKind, GameView, LooseSource, MemorySource, ReadFrom,
    ResolveError, Vfs, WriteError, WriteMode, WritePolicy,
};

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    game: PathBuf,
    view: GameView,
}

fn world_with(filter: Option<ContentFilter>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let game = root.join("Game");
    put(&game, "data/cdimages/main.dat", b"base-main");
    put(&game, "data/Config.xml", b"base-config");
    put(&game, "common.rpf", b"archive");
    let mods = root.join("Mods");
    package(
        &mods,
        "tweak",
        "systems",
        &[],
        &[("Data/config.xml", "modded-config")],
    );
    let mut builder = Vfs::builder("1.0.2.1")
        .unwrap()
        .base(LooseSource::new(&game))
        .mods_dir(&mods)
        .extra_layer(
            "archive",
            MemorySource::new().with("common/data/levels/heightmap.dat", b"hm".to_vec()),
        );
    if let Some(filter) = filter {
        builder = builder.filter(filter);
    }
    let (vfs, _) = builder.build().unwrap();
    let view = GameView::new(Arc::new(vfs), &game, WritePolicy::new(root.join("Output")));
    World {
        _dir: dir,
        root,
        game,
        view,
    }
}

fn world() -> World {
    world_with(None)
}

fn read(world: &World, path: &str) -> Vec<u8> {
    let resolved = world.view.resolve(path).unwrap();
    world.view.read(&resolved).unwrap()
}

fn inside(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

#[test]
fn every_spelling_of_the_game_folder_reads_through_the_tree() {
    let w = world();
    for path in [
        "GAME:\\data\\config.xml",
        "game:/DATA/Config.XML",
        "Game:data/config.xml",
        "G:\\data\\config.xml",
        "data\\config.xml",
        "\\data\\config.xml",
        "./data/../data/config.xml",
        &format!("{}\\Data\\Config.xml", w.game.display()),
        &format!("\\\\?\\{}\\data\\config.xml", w.game.display()),
    ] {
        assert_eq!(read(&w, path), b"modded-config", "{path}");
    }
    let r = w.view.resolve("GAME:\\data\\cdimages\\main.dat").unwrap();
    assert_eq!(r.area, Area::Game);
    assert_eq!(r.kind, EntryKind::File);
    assert!(matches!(&r.read_from, Some(ReadFrom::File(p)) if inside(p, &w.game)));
    assert!(!r.in_overlay);
    assert_eq!(r.assumed, None, "GAME: is documented, not assumed");
    assert_eq!(
        w.view.resolve("G:\\x").unwrap().assumed,
        Some(Assumption::GameDrive)
    );
    assert_eq!(
        w.view.resolve("x").unwrap().assumed,
        Some(Assumption::RelativeToGameFolder)
    );

    assert_eq!(w.view.resolve("GAME:\\data").unwrap().kind, EntryKind::Dir);
    assert_eq!(w.view.resolve("GAME:\\").unwrap().kind, EntryKind::Dir);
    assert_eq!(
        w.view.resolve("GAME:\\nope.bin").unwrap().kind,
        EntryKind::Missing
    );
}

#[test]
fn engine_device_names_map_into_the_tree_and_archive_members_read_through_it() {
    let w = world();
    for path in [
        "common:/data/levels/heightmap.dat",
        "COMMON:\\data\\levels\\heightmap.dat",
        "$(common)/data/levels/heightmap.dat",
    ] {
        let r = w.view.resolve(path).unwrap();
        assert_eq!(r.assumed, Some(Assumption::DeviceName));
        assert!(matches!(r.read_from, Some(ReadFrom::Tree(_))), "{path}");
        assert_eq!(w.view.read(&r).unwrap(), b"hm");
    }
    assert!(matches!(
        w.view.resolve("$(target)/x"),
        Err(ResolveError::UnmeasuredToken(_))
    ));
    assert!(matches!(
        w.view.resolve("xyz:/x"),
        Err(ResolveError::UnknownDevice(_))
    ));
}

#[test]
fn refused_and_escaping_paths_fail_with_a_reason() {
    let w = world();
    for path in [
        "tcpip:\\server\\file",
        "\\\\host\\share\\x",
        "https://example.invalid/x",
    ] {
        assert!(
            matches!(w.view.resolve(path), Err(ResolveError::Refused { .. })),
            "{path}"
        );
    }
    for path in ["GAME:\\..\\outside", "..\\..\\x", "G:/a/../../b"] {
        assert!(
            matches!(w.view.resolve(path), Err(ResolveError::OutsideRoot(_))),
            "{path}"
        );
    }
    assert!(matches!(
        w.view.resolve("GAME:\\data\\file:stream"),
        Err(ResolveError::Path(_))
    ));
}

#[test]
fn writes_land_in_output_and_later_reads_see_them_first() {
    let w = world();
    let output = w.root.join("Output");
    let r = w.view.resolve("GAME:\\data\\settings.cfg").unwrap();
    assert_eq!(r.kind, EntryKind::Missing);
    assert!(inside(&r.write_to, &output.join("runtime").join("overlay")));
    let target = w.view.prepare_write(&r, WriteMode::Truncate).unwrap();
    assert_eq!(target, r.write_to);
    fs::write(&target, b"mine").unwrap();
    assert_eq!(read(&w, "game:/DATA/SETTINGS.cfg"), b"mine");
    assert!(
        w.view
            .resolve("game:/data/settings.cfg")
            .unwrap()
            .in_overlay
    );
    assert!(
        !w.game.join("data/settings.cfg").exists(),
        "the package is never written"
    );

    let existing = w.view.resolve("GAME:\\data\\cdimages\\main.dat").unwrap();
    let target = w
        .view
        .prepare_write(&existing, WriteMode::KeepContents)
        .unwrap();
    assert!(inside(&target, &output));
    assert_eq!(fs::read(&target).unwrap(), b"base-main", "copied up first");
    fs::write(&target, b"changed").unwrap();
    assert_eq!(read(&w, "G:\\data\\cdimages\\main.dat"), b"changed");
    assert_eq!(
        fs::read(w.game.join("data/cdimages/main.dat")).unwrap(),
        b"base-main"
    );

    let truncated = w.view.resolve("GAME:\\common.rpf").unwrap();
    let target = w
        .view
        .prepare_write(&truncated, WriteMode::Truncate)
        .unwrap();
    assert!(!target.exists());
}

#[test]
fn archive_members_are_copied_up_through_the_tree() {
    let w = world();
    let r = w.view.resolve("common:/data/levels/heightmap.dat").unwrap();
    let target = w.view.prepare_write(&r, WriteMode::KeepContents).unwrap();
    assert_eq!(fs::read(target).unwrap(), b"hm");
}

#[test]
fn a_write_needs_its_folder_to_exist_in_the_games_view() {
    let w = world();
    let r = w.view.resolve("GAME:\\nofolder\\a.bin").unwrap();
    assert!(matches!(
        w.view.prepare_write(&r, WriteMode::Truncate),
        Err(WriteError::ParentMissing(_))
    ));
    let dir = w.view.resolve("GAME:\\nofolder").unwrap();
    w.view.create_dir(&dir).unwrap();
    assert!(matches!(
        w.view.create_dir(&dir),
        Err(WriteError::AlreadyExists(_))
    ));
    assert_eq!(
        w.view.resolve("GAME:\\NOFOLDER").unwrap().kind,
        EntryKind::Dir
    );
    assert!(w.view.prepare_write(&r, WriteMode::Truncate).is_ok());
    let package_dir = w.view.resolve("GAME:\\data").unwrap();
    assert!(matches!(
        w.view.create_dir(&package_dir),
        Err(WriteError::AlreadyExists(_))
    ));
    assert!(matches!(
        w.view.prepare_write(&package_dir, WriteMode::Truncate),
        Err(WriteError::IsDirectory(_))
    ));
}

#[test]
fn deleting_a_package_file_hides_it_without_touching_the_package() {
    let w = world();
    let r = w.view.resolve("GAME:\\common.rpf").unwrap();
    w.view.delete_file(&r).unwrap();
    assert_eq!(
        w.view.resolve("GAME:\\COMMON.RPF").unwrap().kind,
        EntryKind::Missing
    );
    assert!(w.game.join("common.rpf").exists());
    let names: Vec<_> = {
        let root = w.view.resolve("GAME:\\").unwrap();
        w.view
            .list(&root)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect()
    };
    assert!(!names.contains(&"common.rpf".to_string()));
    assert!(matches!(
        w.view.delete_file(&r),
        Err(WriteError::NotFound(_))
    ));

    let again = w.view.resolve("GAME:\\common.rpf").unwrap();
    let target = w.view.prepare_write(&again, WriteMode::Truncate).unwrap();
    fs::write(target, b"new").unwrap();
    assert_eq!(read(&w, "GAME:\\common.rpf"), b"new");

    let mine = w.view.resolve("GAME:\\data\\temp.bin").unwrap();
    fs::write(
        w.view.prepare_write(&mine, WriteMode::Truncate).unwrap(),
        b"t",
    )
    .unwrap();
    w.view
        .delete_file(&w.view.resolve("GAME:\\data\\temp.bin").unwrap())
        .unwrap();
    assert_eq!(
        w.view.resolve("GAME:\\data\\temp.bin").unwrap().kind,
        EntryKind::Missing
    );
}

#[test]
fn listings_merge_the_tree_and_the_games_own_files() {
    let w = world();
    let r = w.view.resolve("GAME:\\data\\extra.bin").unwrap();
    fs::write(
        w.view.prepare_write(&r, WriteMode::Truncate).unwrap(),
        b"12345",
    )
    .unwrap();
    let r = w.view.resolve("GAME:\\data\\config.xml").unwrap();
    fs::write(
        w.view.prepare_write(&r, WriteMode::Truncate).unwrap(),
        b"xy",
    )
    .unwrap();
    w.view
        .create_dir(&w.view.resolve("GAME:\\data\\newdir").unwrap())
        .unwrap();

    let dir = w.view.resolve("game:/DATA").unwrap();
    let entries: Vec<_> = w
        .view
        .list(&dir)
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.is_dir, e.size))
        .collect();
    assert_eq!(
        entries,
        [
            ("cdimages", true, 0),
            ("config.xml", false, 2),
            ("extra.bin", false, 5),
            ("newdir", true, 0),
        ]
        .map(|(n, d, s)| (n.to_string(), d, s))
    );
}

#[test]
fn removing_folders_follows_the_games_view() {
    let w = world();
    let data = w.view.resolve("GAME:\\data").unwrap();
    assert!(matches!(
        w.view.remove_dir(&data),
        Err(WriteError::NotEmpty(_))
    ));
    let made = w.view.resolve("GAME:\\scratch").unwrap();
    w.view.create_dir(&made).unwrap();
    let made = w.view.resolve("GAME:\\scratch").unwrap();
    w.view.remove_dir(&made).unwrap();
    assert_eq!(
        w.view.resolve("GAME:\\scratch").unwrap().kind,
        EntryKind::Missing
    );
}

#[test]
fn temp_persistent_output_and_host_areas_never_leave_output() {
    let w = world();
    let output = w.root.join("Output");
    let t = w.view.resolve("T:\\cache\\a.tmp").unwrap();
    assert_eq!(t.area, Area::Temp);
    assert_eq!(t.assumed, Some(Assumption::TempDrive));
    let target = w.view.prepare_write(&t, WriteMode::Truncate).unwrap();
    assert!(inside(&target, &output.join("runtime").join("temp")));
    fs::write(&target, b"temp").unwrap();
    assert_eq!(read(&w, "t:/CACHE/a.tmp"), b"temp");
    assert!(
        w.view.resolve("GAME:\\cache\\a.tmp").unwrap().kind == EntryKind::Missing,
        "GAME: does not see T:"
    );

    let p = w
        .view
        .resolve(&format!("{}\\x.bin", w.view.persistent_dir().display()))
        .unwrap();
    assert_eq!(p.area, Area::Persistent);
    assert_eq!(p.assumed, Some(Assumption::HostPathInsideKnownFolder));

    let saved = output
        .join("saves")
        .join("local-user")
        .join("slot")
        .join("blob");
    let s = w.view.resolve(&saved.display().to_string()).unwrap();
    assert_eq!(s.area, Area::Output);
    assert_eq!(s.write_to, saved);

    let other = w.root.parent().unwrap();
    let outside = format!("{}\\elsewhere\\f.txt", other.display());
    let h = w.view.resolve(&outside).unwrap();
    assert_eq!(h.area, Area::Host);
    assert_eq!(h.assumed, Some(Assumption::HostRedirect));
    assert!(inside(&h.write_to, &output.join("runtime").join("host")));
    let target = w.view.prepare_write(&h, WriteMode::Truncate).unwrap();
    fs::write(&target, b"redirected").unwrap();
    assert_eq!(read(&w, &outside), b"redirected");
}

#[test]
fn host_reads_pass_through_when_nothing_was_redirected() {
    let w = world();
    let loose = put(
        w.root.parent().unwrap_or(&w.root),
        "leonida-vfs-probe/real.txt",
        b"real",
    );
    let r = w.view.resolve(&loose.display().to_string());
    let _ = fs::remove_dir_all(loose.parent().unwrap());
    let r = r.unwrap();
    assert_eq!(r.area, Area::Host);
    assert_eq!(r.kind, EntryKind::File);
    assert!(matches!(r.read_from, Some(ReadFrom::File(_))));
}

#[test]
fn the_filter_applies_to_what_the_game_opens() {
    let filter = ContentFilter::from_toml(
        r#"
        [[rule]]
        id = "hide-main"
        name = "main.dat"
        action = "hide"

        [[rule]]
        id = "swap-cfg"
        path = "data/config.xml"
        action = "replace"
        with = "common.rpf"
        "#,
    )
    .unwrap();
    let w = world_with(Some(filter));
    assert_eq!(
        w.view
            .resolve("GAME:\\data\\cdimages\\main.dat")
            .unwrap()
            .kind,
        EntryKind::Missing
    );
    assert_eq!(
        w.view.resolve("GAME:\\data\\cdimages").unwrap().kind,
        EntryKind::Missing
    );
    assert_eq!(read(&w, "GAME:\\data\\config.xml"), b"archive");
}

#[test]
fn save_blob_paths_are_encoded_and_inside_output() {
    let w = world();
    let path = w.view.policy().save_blob("Slot 1", "profile/../x").unwrap();
    assert!(inside(&path, &w.root.join("Output").join("saves")));
    assert!(path.ends_with("profile%2F..%2Fx"));
}
