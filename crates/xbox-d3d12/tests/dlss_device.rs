//! DLSS on the device the layer assembles: the extensions NGX names must be enabled on it. Needs an NVIDIA GPU and
//! `nvngx_dlss.dll` (`LEONIDA_DLSS_DLL`), so it only runs when asked for.
#![allow(unsafe_code)]

use upscale::dlss::vk::VkHandles;
use upscale::dlss::{DlssConfig, DlssOptions, DlssUpscaler};
use upscale::{Image, ImageRef, PixelFormat, UpscaleInputs, Upscaler};
use xbox_d3d12::assembly::gpu::{Gpu, GpuConfig};

#[test]
#[ignore = "needs an NVIDIA GPU and nvngx_dlss.dll (set LEONIDA_DLSS_DLL)"]
fn dlss_runs_on_the_assembled_device() {
    let gpu = Gpu::create(&GpuConfig {
        gpu: Some("nvidia".into()),
        ..GpuConfig::default()
    })
    .expect("an NVIDIA device");
    eprintln!("device: {}", gpu.name);
    assert!(
        gpu.optional.dlss,
        "the assembled device lacks the extensions NGX names"
    );
    let queue = *gpu.vulkan.queue.lock().unwrap();
    let handles = VkHandles {
        instance: gpu.vulkan.instance.handle(),
        physical: gpu.vulkan.physical,
        device: gpu.vulkan.device.handle(),
        queue,
        queue_family: gpu.vulkan.family,
    };
    let config = DlssConfig::default();
    // SAFETY: the handles belong to `gpu`, which outlives the upscaler, and nothing else uses the queue.
    let mut dlss = unsafe { DlssUpscaler::with_device(&config, DlssOptions::default(), handles) }
        .expect("DLSS starts on the assembled device");
    let (render, output) = ([640u32, 360], [1280u32, 720]);
    let texels = (render[0] * render[1]) as usize;
    let color = Image::from_rgba_f32(
        render[0],
        render[1],
        PixelFormat::Rgba16Float,
        &[0.4, 0.5, 0.6, 1.0].repeat(texels),
    )
    .unwrap();
    let depth: Vec<u8> = std::iter::repeat_n(0.5f32.to_le_bytes(), texels)
        .flatten()
        .collect();
    let motion = vec![0u8; texels * 8];
    let mut result = None;
    for frame in 0..4 {
        let mut inputs = UpscaleInputs::new(color.as_ref(), render, output);
        inputs.depth = Some(ImageRef::new(
            render[0],
            render[1],
            PixelFormat::R32Float,
            &depth,
        ));
        inputs.motion = Some(ImageRef::new(
            render[0],
            render[1],
            PixelFormat::Rg32Float,
            &motion,
        ));
        inputs.reset = frame == 0;
        result = Some(dlss.upscale(&inputs).expect("DLSS evaluates"));
    }
    let result = result.unwrap();
    assert_eq!([result.width, result.height], output);
    let rgba = result
        .as_ref()
        .rgba_f32(output[0], output[1], "result")
        .unwrap();
    assert!(rgba.iter().all(|v| v.is_finite()));
    let mean = rgba.chunks_exact(4).map(|p| p[1]).sum::<f32>() / (output[0] * output[1]) as f32;
    assert!(
        (mean - 0.5).abs() < 0.05,
        "a flat 0.5 channel came back as {mean}"
    );
}
