//! Checks against the real executable, read as plain bytes (never loaded or run). Skipped without
//! `LEONIDA_GAME_DIR`.

use enhancements::framerate::{self, STEP_FLOOR};
use enhancements::patch::Finder;
use pe_image::Image;

fn executable() -> Option<Vec<u8>> {
    let dir = std::env::var_os("LEONIDA_GAME_DIR")?;
    let largest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "exe"))
        .max_by_key(|e| e.metadata().map(|m| m.len()).unwrap_or(0))?;
    std::fs::read(largest.path()).ok()
}

#[test]
fn the_step_floor_site_is_where_the_notes_say() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let function = image.function_containing(0x0222_64d0).unwrap();
    let sites = finder.find_in(function, &framerate::step_floor_pattern());
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].rva(), 0x0222_6854);

    let patch = framerate::floor_patch(&finder, &sites[0]).unwrap();
    assert_eq!(patch.rva, 0x0222_6854 + 4);
    let target = 0x0222_6854i64
        + 8
        + i64::from(i32::from_le_bytes(
            patch.replacement[..4].try_into().unwrap(),
        ));
    assert_eq!(target, 0x0684_7660);
    let old =
        0x0222_6854i64 + 8 + i64::from(i32::from_le_bytes(patch.original[..4].try_into().unwrap()));
    assert_eq!(old, 0x0684_9844);
}

/// Decodes every function: slow in a debug build, so run with `--release -- --ignored`.
#[test]
#[ignore]
fn the_step_floor_pattern_is_unique_in_the_whole_image() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let patches = STEP_FLOOR.patches(&finder).unwrap();
    assert_eq!(patches.len(), 1);
}
