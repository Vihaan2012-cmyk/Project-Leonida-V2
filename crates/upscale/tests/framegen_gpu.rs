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
