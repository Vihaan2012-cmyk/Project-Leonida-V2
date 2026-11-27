mod common;

use common::{gpu, r32, rg32, rgba32};
use upscale::framegen::InterpolatingFrameGenerator;
use upscale::{FrameGenInputs, FrameGenerator, ImageRef, PixelFormat};

const N: u32 = 64;
const SIZE: u32 = 16;
const STEP: i32 = 8;

fn in_square(x: u32, y: u32, left: i32) -> bool {
    (x as i32) >= left && (x as i32) < left + SIZE as i32 && (24..24 + SIZE).contains(&y)
}

fn colour(x: u32, y: u32, left: i32) -> [f32; 4] {
    if in_square(x, y, left) {
        [1.0, 0.2, 0.2, 1.0]
    } else {
        [
            0.2 + 0.5 * x as f32 / N as f32,
            0.3 + 0.4 * y as f32 / N as f32,
            0.4,
            1.0,
        ]
    }
}

struct Frame {
    color: Vec<u8>,
    depth: Vec<u8>,
    motion: Vec<u8>,
}

fn frame(left: i32) -> Frame {
    let mut texels = Vec::new();
    let mut depth = Vec::new();
    let mut motion = Vec::new();
    for y in 0..N {
        for x in 0..N {
            texels.push(colour(x, y, left));
            let square = in_square(x, y, left);
            depth.push(if square { 0.8 } else { 0.2 });
            motion.push(if square {
                [-(STEP as f32), 0.0]
            } else {
                [0.0, 0.0]
            });
        }
    }
    Frame {
        color: rgba32(&texels),
        depth: r32(&depth),
        motion: rg32(&motion),
    }
}

fn inputs(frame: &Frame) -> FrameGenInputs<'_> {
    let mut inputs = FrameGenInputs::new(
        ImageRef::new(N, N, PixelFormat::Rgba32Float, &frame.color),
        ImageRef::new(N, N, PixelFormat::R32Float, &frame.depth),
        ImageRef::new(N, N, PixelFormat::Rg32Float, &frame.motion),
        [N, N],
    );
    inputs.output_format = PixelFormat::Rgba32Float;
    inputs
}

#[test]
fn a_moving_square_is_interpolated_halfway() {
    let Some(compute) = gpu() else { return };
    let mut generator = InterpolatingFrameGenerator::new(compute);
    let prev = frame(16);
    let cur = frame(16 + STEP);
    assert!(generator.generate(&inputs(&prev)).unwrap().is_none());
    let image = generator.generate(&inputs(&cur)).unwrap().unwrap();
    let rgba = image.to_rgba_f32();
    let (mut sum, mut wrong) = (0.0, 0);
    for y in 0..N {
        for x in 0..N {
            let truth = colour(x, y, 16 + STEP / 2);
            let i = ((y * N + x) * 4) as usize;
            let diff = (0..3).map(|c| (rgba[i + c] - truth[c]).abs()).sum::<f32>() / 3.0;
            sum += diff;
            if diff > 0.05 {
                wrong += 1;
            }
        }
    }
    let mean = sum / (N * N) as f32;
    eprintln!("mean error {mean}, {wrong} texels off");
    assert!(mean < 0.005, "mean error {mean}");
    assert!(wrong < 24, "{wrong} texels off");
    let centre = ((32 * N + 28) * 4) as usize;
    assert!(rgba[centre] > 0.95 && rgba[centre + 1] < 0.25);
    let uncovered = ((32 * N + 18) * 4) as usize;
    assert!(
        rgba[uncovered + 1] > 0.3 && rgba[uncovered] < 0.6,
        "the region the square left must show the background"
    );
    assert_eq!(gpu::validation_errors(), 0);
}

#[test]
fn a_reset_drops_the_previous_frame_and_bad_phases_are_refused() {
    let Some(compute) = gpu() else { return };
    let mut generator = InterpolatingFrameGenerator::new(compute);
    let a = frame(16);
    let b = frame(24);
    generator.generate(&inputs(&a)).unwrap();
    let mut reset = inputs(&b);
    reset.reset = true;
    assert!(generator.generate(&reset).unwrap().is_none());
    let mut bad = inputs(&a);
    bad.phase = 1.0;
    assert!(generator.generate(&bad).is_err());
    assert!(generator.generate(&inputs(&a)).unwrap().is_some());
}

fn noise(x: i32, y: i32) -> [f32; 3] {
    let h = |a: i32, b: i32, salt: u32| {
        let mut v = (a as u32)
            .wrapping_mul(0x9E37_79B1)
            .wrapping_add((b as u32).wrapping_mul(0x85EB_CA77))
            .wrapping_add(salt.wrapping_mul(0xC2B2_AE3D));
        v ^= v >> 15;
        v = v.wrapping_mul(0x2C1B_3C6D);
        v ^= v >> 12;
        (v & 0xFFFF) as f32 / 65535.0
    };
    [
        0.1 + 0.8 * h(x, y, 1),
        0.1 + 0.8 * h(x, y, 2),
        0.1 + 0.8 * h(x, y, 3),
    ]
}

struct Scene {
    shift: i32,
    scroll: f32,
    square: Option<i32>,
    hud: bool,
}

const HUD: (u32, u32, u32, u32) = (8, 8, 28, 20);

fn in_hud(x: u32, y: u32) -> bool {
    (HUD.0..HUD.2).contains(&x) && (HUD.1..HUD.3).contains(&y)
}

fn scene_colour(scene: &Scene, x: u32, y: u32) -> [f32; 4] {
    if scene.hud && in_hud(x, y) {
        return [0.05, 0.95, 0.05, 1.0];
    }
    if let Some(left) = scene.square {
        if in_square(x, y, left) {
            return [1.0, 0.2, 0.2, 1.0];
        }
    }
    let n = noise(x as i32 - scene.shift, y as i32);
    [n[0], n[1], n[2], 1.0]
}

fn scene_frame(scene: &Scene) -> Frame {
    let mut texels = Vec::new();
    let mut depth = Vec::new();
    let mut motion = Vec::new();
    for y in 0..N {
        for x in 0..N {
            texels.push(scene_colour(scene, x, y));
            let square = scene.square.is_some_and(|left| in_square(x, y, left));
            depth.push(if square { 0.8 } else { 0.2 });
            motion.push(if square {
                [-(STEP as f32), 0.0]
            } else {
                [-scene.scroll, 0.0]
            });
        }
    }
    Frame {
        color: rgba32(&texels),
        depth: r32(&depth),
        motion: rg32(&motion),
    }
}

fn diff(rgba: &[f32], x: u32, y: u32, truth: [f32; 4]) -> f32 {
    let i = ((y * N + x) * 4) as usize;
    (0..3)
        .map(|c| (rgba[i + c] - truth[c]).abs())
        .fold(0.0, f32::max)
}

fn generated(a: &Scene, b: &Scene, hud_threshold: f32) -> Vec<f32> {
    let compute = gpu().unwrap();
    let mut generator = InterpolatingFrameGenerator::new(compute);
    let (fa, fb) = (scene_frame(a), scene_frame(b));
    assert!(generator.generate(&inputs(&fa)).unwrap().is_none());
    let mut second = inputs(&fb);
    second.hud_threshold = hud_threshold;
    generator.generate(&second).unwrap().unwrap().to_rgba_f32()
}

#[test]
fn a_disocclusion_is_filled_from_the_frame_that_shows_it() {
    if gpu().is_none() {
        return;
    }
    let prev = Scene {
        shift: 0,
        scroll: 0.0,
        square: Some(16),
        hud: false,
    };
    let cur = Scene {
        shift: 0,
        scroll: 0.0,
        square: Some(16 + STEP),
        hud: false,
    };
    let mid = Scene {
        shift: 0,
        scroll: 0.0,
        square: Some(16 + STEP / 2),
        hud: false,
    };
    let rgba = generated(&prev, &cur, 0.06);
    let mut worst_trailing: f32 = 0.0;
    let mut worst_leading: f32 = 0.0;
    for y in 24..24 + SIZE {
        for x in 16..20 {
            worst_trailing = worst_trailing.max(diff(&rgba, x, y, scene_colour(&mid, x, y)));
        }
        for x in 36..40 {
            worst_leading = worst_leading.max(diff(&rgba, x, y, scene_colour(&mid, x, y)));
        }
    }
    eprintln!("trailing band {worst_trailing}, leading band {worst_leading}");
    assert!(
        worst_trailing < 0.02,
        "the uncovered band must come from the newest frame: {worst_trailing}"
    );
    assert!(
        worst_leading < 0.02,
        "the band the square has not reached yet must come from the previous frame: {worst_leading}"
    );
}

#[test]
fn a_static_hud_over_a_scrolling_scene_stays_sharp() {
    if gpu().is_none() {
        return;
    }
    let scroll = 6;
    let prev = Scene {
        shift: 0,
        scroll: scroll as f32,
        square: None,
        hud: true,
    };
    let cur = Scene {
        shift: scroll,
        scroll: scroll as f32,
        square: None,
        hud: true,
    };
    let mid = Scene {
        shift: scroll / 2,
        scroll: scroll as f32,
        square: None,
        hud: true,
    };
    let count_bad = |rgba: &[f32]| {
        let mut hud_bad = 0;
        let mut scene_bad = 0;
        for y in 0..N {
            for x in 0..N {
                let bad = diff(rgba, x, y, scene_colour(&mid, x, y)) > 0.05;
                if in_hud(x, y) {
                    hud_bad += bad as u32;
                } else {
                    scene_bad += bad as u32;
                }
            }
        }
        (hud_bad, scene_bad)
    };
    let with_mask = count_bad(&generated(&prev, &cur, 0.06));
    let without_mask = count_bad(&generated(&prev, &cur, 0.0));
    eprintln!("wrong texels (hud, scene): with the mask {with_mask:?}, without {without_mask:?}");
    assert_eq!(with_mask.0, 0, "no HUD texel may be warped");
    assert!(
        with_mask.1 <= without_mask.1,
        "the mask must not damage the scene"
    );
    assert!(
        without_mask.0 > 10,
        "the scene alone would smear the HUD edges"
    );
}

#[test]
fn nothing_is_generated_between_a_cut_and_the_frame_before_it() {
    let Some(compute) = gpu() else { return };
    let mut generator = InterpolatingFrameGenerator::new(compute);
    let before = frame(4);
    let cut = frame(24);
    let after = frame(24 + STEP);
    generator.generate(&inputs(&before)).unwrap();
    let mut reset = inputs(&cut);
    reset.reset = true;
    assert!(generator.generate(&reset).unwrap().is_none());
    let image = generator.generate(&inputs(&after)).unwrap().unwrap();
    let rgba = image.to_rgba_f32();
    for y in 24..24 + SIZE {
        for x in 0..N {
            let truth = colour(x, y, 24 + STEP / 2);
            let d = diff(&rgba, x, y, truth);
            assert!(
                d < 0.1
                    || (x as i32 - 24 - STEP / 2).abs() < 2
                    || (x as i32 - 24 - STEP / 2 - SIZE as i32).abs() < 2,
                "texel {x},{y} is {d} off: the frame before the cut leaked in"
            );
        }
    }
}
