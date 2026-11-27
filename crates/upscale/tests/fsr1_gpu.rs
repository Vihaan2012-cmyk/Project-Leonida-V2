use std::sync::Arc;

use upscale::fsr1::Fsr1Upscaler;
use upscale::{ColorSpace, ImageRef, PixelFormat, SharedCompute, UpscaleInputs, Upscaler};

fn gpu() -> Option<SharedCompute> {
    match gpu::Gpu::new() {
        Ok(gpu) => Some(Arc::new(gpu)),
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

fn test_image(w: u32, h: u32) -> Vec<u8> {
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let edge = if (x + y / 2) % 7 < 3 { 0.9f32 } else { 0.1 };
            let texel = [
                edge,
                y as f32 / h as f32 * 2.0,
                ((x * 5 + y * 3) % 11) as f32 * 0.09,
                1.0,
            ];
            for v in texel {
                data.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    data
}

fn run(upscaler: &mut Fsr1Upscaler, bytes: &[u8], space: ColorSpace, sharpness: f32) -> Vec<f32> {
    let color = ImageRef::new(16, 12, PixelFormat::Rgba32Float, bytes);
    let mut inputs = UpscaleInputs::new(color, [16, 12], [32, 24]);
    inputs.output_format = PixelFormat::Rgba32Float;
    inputs.color_space = space;
    inputs.sharpness = sharpness;
    upscaler.upscale(&inputs).unwrap().to_rgba_f32()
}

#[test]
fn fsr1_on_the_gpu_matches_the_cpu_reference() {
    let Some(compute) = gpu() else { return };
    let bytes = test_image(16, 12);
    let mut on_gpu = Fsr1Upscaler::new(compute);
    let mut on_cpu = Fsr1Upscaler::cpu();
    for (space, sharpness) in [
        (ColorSpace::Perceptual, 0.0),
        (ColorSpace::Perceptual, 0.8),
        (ColorSpace::Linear, 0.5),
        (ColorSpace::Linear, 0.0),
    ] {
        let gpu_out = run(&mut on_gpu, &bytes, space, sharpness);
        let cpu_out = run(&mut on_cpu, &bytes, space, sharpness);
        assert_eq!(gpu_out.len(), 32 * 24 * 4);
        let worst = gpu_out
            .iter()
            .zip(&cpu_out)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 2e-3,
            "{space:?} sharpness {sharpness}: worst {worst}"
        );
        let spread = cpu_out.iter().cloned().fold(0.0f32, f32::max)
            - cpu_out.iter().cloned().fold(1.0f32, f32::min);
        assert!(spread > 0.5, "the test image should have contrast");
    }
    assert_eq!(gpu::validation_errors(), 0);
}
