mod common;

use std::f32::consts::TAU;

use common::{gpu, r32, rg32, rgba32};
use upscale::temporal::{TemporalConfig, TemporalUpscaler};
use upscale::{ImageRef, PixelFormat, UpscaleInputs, Upscaler, jitter_offset};

const RENDER: u32 = 32;
const OUTPUT: u32 = 64;
const SCALE: f32 = 2.0;

/// A smooth scene in output-pixel coordinates, shifted right by `shift` pixels.
fn scene(u: f32, v: f32, shift: f32) -> [f32; 4] {
    let u = u - shift;
    [
        0.5 + 0.3 * (TAU * u / 24.0).sin(),
        0.5 + 0.25 * (TAU * v / 20.0 + 1.0).cos(),
        0.45 + 0.2 * (TAU * (u + v) / 30.0).sin(),
        1.0,
    ]
}

/// What a jittered render at 32x32 sees: texel `i` samples the scene at `(i + 0.5 - jitter) * 2`.
fn render(jitter: [f32; 2], shift: f32) -> Vec<u8> {
    let mut texels = Vec::new();
    for y in 0..RENDER {
        for x in 0..RENDER {
            let u = (x as f32 + 0.5 - jitter[0]) * SCALE;
            let v = (y as f32 + 0.5 - jitter[1]) * SCALE;
            texels.push(scene(u, v, shift));
        }
    }
    rgba32(&texels)
}

fn error(image: &upscale::Image, shift: f32) -> f32 {
    let rgba = image.to_rgba_f32();
    let (mut sum, mut count) = (0.0, 0);
    for y in 4..OUTPUT - 4 {
        for x in 4..OUTPUT - 4 {
            let truth = scene(x as f32 + 0.5, y as f32 + 0.5, shift);
            let i = ((y * OUTPUT + x) * 4) as usize;
            for c in 0..3 {
                sum += (rgba[i + c] - truth[c]).abs();
                count += 1;
            }
        }
    }
    sum / count as f32
}

fn frame(
    upscaler: &mut TemporalUpscaler,
    index: u32,
    shift_per_frame: f32,
    motion: Option<[f32; 2]>,
    reset: bool,
) -> upscale::Image {
    let jitter = jitter_offset(index, 32);
    let bytes = render(jitter, shift_per_frame * index as f32);
    let color = ImageRef::new(RENDER, RENDER, PixelFormat::Rgba32Float, &bytes);
    let depth = r32(&vec![0.5; (RENDER * RENDER) as usize]);
    let vectors = rg32(&vec![
        motion.unwrap_or([0.0, 0.0]);
        (RENDER * RENDER) as usize
    ]);
    let mut inputs = UpscaleInputs::new(color, [RENDER, RENDER], [OUTPUT, OUTPUT]);
    inputs.depth = Some(ImageRef::new(RENDER, RENDER, PixelFormat::R32Float, &depth));
    inputs.motion = Some(ImageRef::new(
        RENDER,
        RENDER,
        PixelFormat::Rg32Float,
        &vectors,
    ));
    inputs.output_format = PixelFormat::Rgba32Float;
    inputs.jitter = jitter;
    inputs.sharpness = 0.0;
    inputs.reset = reset;
    upscaler.upscale(&inputs).unwrap()
}

#[test]
fn temporal_accumulation_converges_on_a_static_scene() {
    let Some(compute) = gpu() else { return };
    let mut upscaler = TemporalUpscaler::new(compute, TemporalConfig::default());
    let first = frame(&mut upscaler, 0, 0.0, None, true);
    let first_error = error(&first, 0.0);
    let mut errors = vec![first_error];
    let mut last = first;
    for index in 1..40 {
        last = frame(&mut upscaler, index, 0.0, None, false);
        errors.push(error(&last, 0.0));
    }
    eprintln!("errors: {errors:?}");
    let settled = errors[39];
    assert!(
        settled < first_error * 0.5,
        "{settled} against the first frame's {first_error}"
    );
    assert!(
        (errors[39] - errors[30]).abs() < first_error * 0.1,
        "still changing: {} then {}",
        errors[30],
        errors[39]
    );
    let again = frame(&mut upscaler, 40, 0.0, None, true);
    assert!(
        error(&again, 0.0) > settled,
        "a reset must discard the accumulated history"
    );
    drop(last);
    assert_eq!(gpu::validation_errors(), 0);
}

#[test]
fn reprojection_with_motion_vectors_keeps_a_panning_scene_sharp() {
    let Some(compute) = gpu() else { return };
    let shift = 1.0;
    let run = |motion: [f32; 2]| {
        let mut upscaler = TemporalUpscaler::new(compute.clone(), TemporalConfig::default());
        let mut result = 0.0;
        for index in 0..24 {
            let image = frame(&mut upscaler, index, shift, Some(motion), index == 0);
            result = error(&image, shift * index as f32);
        }
        result
    };
    let with_vectors = run([-shift / SCALE, 0.0]);
    let without = run([0.0, 0.0]);
    eprintln!("with vectors {with_vectors}, without {without}");
    assert!(with_vectors < without * 0.7);
}
