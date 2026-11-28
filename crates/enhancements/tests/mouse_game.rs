//! The mouse look patch against the real executable, read as plain bytes (never loaded or run). Skipped without
//! `LEONIDA_GAME_DIR`.

use enhancements::camera_mouse::{self, GAME, HOOK_LEN, MouseLookSettings};
use enhancements::frame_timers;
use enhancements::patch::Finder;
use iced_x86::{Decoder, DecoderOptions, Mnemonic};
use pe_image::Image;

const BASE: u64 = 0x1_4000_0000;

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
fn the_patch_finds_its_site_in_the_look_controller_update() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let built = camera_mouse::patches(&finder, &GAME, &MouseLookSettings::default()).unwrap();
    let [cave, cell, hook] = built.patches.as_slice() else {
        panic!("three patches");
    };
    assert_eq!(hook.rva, 0x0420_a1e7);
    assert_eq!(hook.original.len(), HOOK_LEN);
    assert_eq!(hook.replacement[0], 0xE9);
    assert_eq!(built.cave, cave.rva);
    assert_eq!(built.cell, cell.rva);
    assert!(
        cave.original.iter().all(|b| *b == 0),
        "the cave is zeroed slack"
    );
    assert!(cell.original.iter().all(|b| *b == 0));
}

#[test]
fn the_routine_decodes_and_returns_to_the_instruction_after_the_hook() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let built = camera_mouse::patches(&finder, &GAME, &MouseLookSettings::default()).unwrap();
    let cave = &built.patches[0];
    let mut decoder = Decoder::with_ip(
        64,
        &cave.replacement,
        BASE + u64::from(cave.rva),
        DecoderOptions::NONE,
    );
    let mut last = None;
    while decoder.can_decode() {
        let i = decoder.decode();
        assert!(!i.is_invalid());
        last = Some(i);
    }
    let last = last.unwrap();
    assert_eq!(last.mnemonic(), Mnemonic::Jmp);
    assert_eq!(last.near_branch_target() - BASE, 0x0420_a20f);
}

#[test]
fn the_patch_and_the_frame_timer_patches_do_not_overlap() {
    let Some(bytes) = executable() else {
        return;
    };
    let image = Image::parse(&bytes).unwrap();
    let finder = Finder::new(&image);
    let mine = camera_mouse::patches(&finder, &GAME, &MouseLookSettings::default()).unwrap();
    let theirs = frame_timers::find_frame_timers(&finder).unwrap();
    for a in &mine.patches {
        for b in &theirs {
            let (a0, a1) = (a.rva, a.rva + a.replacement.len() as u32);
            let (b0, b1) = (b.rva, b.rva + b.replacement.len() as u32);
            assert!(a1 <= b0 || b1 <= a0, "{} overlaps {}", a.name, b.name);
        }
    }
}
