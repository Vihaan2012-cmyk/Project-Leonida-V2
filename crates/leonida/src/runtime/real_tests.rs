//! The whole patch selection against the game's executable read as plain bytes (never loaded or run). Skipped without
//! `LEONIDA_GAME_DIR`.

use std::path::Path;

use enhancements::camera_mouse;
use enhancements::patch::{CodeMemory, PatchError};
use pe_image::Image;

use super::patches::select_all;
use super::plan::{RuntimePlan, Settings};
use crate::cli::RunArgs;

const BASE: usize = 0x1_4000_0000;

/// The image as the loader would map it, for reading only.
struct Mapped<'a>(&'a Image<'a>);

impl CodeMemory for Mapped<'_> {
    fn read(&self, rva: u32, len: usize) -> Result<Vec<u8>, PatchError> {
        self.0
            .read(rva, len)
            .map(<[u8]>::to_vec)
            .ok_or(PatchError::Outside { rva })
    }

    fn write(&mut self, rva: u32, _: &[u8]) -> Result<(), PatchError> {
        Err(PatchError::Outside { rva })
    }
}

fn plan(settings: &Settings) -> RuntimePlan {
    let args = RunArgs {
        game: None,
        file: None,
        dry_run: true,
        start_the_game: false,
        all: false,
        trace: false,
        output: None,
        config: r"E:\Leo\Config".into(),
        frame_limit: None,
    };
    RuntimePlan::from(&args, settings, Path::new(r"E:\Game")).unwrap()
}

fn settings(mouse_camera: bool) -> Settings {
    let mut settings = Settings {
        graphics: config::Graphics {
            frame_rate_mode: config::FrameRateMode::Uncapped,
            aspect_mode: config::AspectMode::Native,
            fov_scale: 1.2,
            ..config::Graphics::default()
        },
        ..Settings::default()
    };
    settings.input.mouse_camera = mouse_camera;
    settings
}

#[test]
fn frame_timers_mouse_and_widescreen_build_together_without_overlap() {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        return;
    };
    let path = crate::cmd_exe::executable(Path::new(&dir)).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let image = Image::parse(&bytes).unwrap();
    let memory = Mapped(&image);
    let mut slot = |_: f32| Ok(BASE + 0x1000);

    let with_mouse = plan(&settings(true));
    assert!(with_mouse.custom_limit && with_mouse.graphics.fix_frame_timers);
    let all = select_all(
        &with_mouse,
        &bytes,
        &memory,
        BASE,
        &mut slot,
        &camera_mouse::GAME,
    )
    .unwrap();
    assert!(all.notes.is_empty(), "{:?}", all.notes);
    assert!(all.mouse_cell.is_some());
    assert!(
        all.base > 10,
        "{} frame rate and widescreen patches",
        all.base
    );
    assert_eq!(all.patches.len(), all.base + 3);
    for (i, a) in all.patches.iter().enumerate() {
        for b in &all.patches[i + 1..] {
            assert!(
                super::mouse::overlap(std::slice::from_ref(a), std::slice::from_ref(b)).is_none(),
                "{} overlaps {}",
                a.name,
                b.name
            );
        }
    }

    let without = select_all(
        &plan(&settings(false)),
        &bytes,
        &memory,
        BASE,
        &mut slot,
        &camera_mouse::GAME,
    )
    .unwrap();
    assert_eq!(without.mouse_cell, None);
    assert_eq!(without.patches.len(), all.base);
    assert!(
        without
            .patches
            .iter()
            .zip(&all.patches)
            .all(|(a, b)| a.name == b.name && a.rva == b.rva && a.replacement == b.replacement)
    );
}

#[test]
fn the_dry_run_line_says_the_patch_will_be_applied() {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        return;
    };
    let bytes = std::fs::read(crate::cmd_exe::executable(Path::new(&dir)).unwrap()).unwrap();
    let line = super::describe_mouse_patch(&plan(&settings(true)), &bytes);
    assert!(
        line.starts_with("mouse camera patch: will be applied (3 patches"),
        "{line}"
    );
    let off = super::describe_mouse_patch(&plan(&settings(false)), &bytes);
    assert!(off.contains("off"), "{off}");
}
