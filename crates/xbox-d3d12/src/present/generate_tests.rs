//! Frame generation on a real Vulkan device, with synthetic images. They skip, with a message, when the machine has no
//! Vulkan device.

use ash::vk;
use upscale::framegen::{FrameInputs, InputResource};
use upscale::image::{f16_to_f32, f32_to_f16};

use super::convert::Rect;
use super::generate::{Capture, FrameGen};
use super::vulkan::{OwnedVulkan, SourceImage, VulkanContext};

pub(super) fn gpu() -> Option<OwnedVulkan> {
    match OwnedVulkan::new(false) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: no Vulkan device ({e})");
            None
        }
    }
}

pub(super) struct Held {
    ctx: VulkanContext,
    image: vk::Image,
    memory: vk::DeviceMemory,
    pub source: SourceImage,
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: the tests wait for the queue before an image goes.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
            self.ctx.device.destroy_image(self.image, None);
            self.ctx.device.free_memory(self.memory, None);
        }
    }
}

fn memory_type(ctx: &VulkanContext, bits: u32, flags: vk::MemoryPropertyFlags) -> u32 {
    // SAFETY: a plain query.
    let props = unsafe {
        ctx.instance
            .get_physical_device_memory_properties(ctx.physical)
    };
    (0..props.memory_type_count)
        .find(|i| {
            bits & (1 << i) != 0
                && props.memory_types[*i as usize]
                    .property_flags
                    .contains(flags)
        })
        .expect("memory type")
}

fn host_buffer(ctx: &VulkanContext, size: u64) -> (vk::Buffer, vk::DeviceMemory) {
    let device = &ctx.device;
    // SAFETY: a short-lived staging buffer the caller destroys.
    unsafe {
        let buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )
            .unwrap();
        let needs = device.get_buffer_memory_requirements(buffer);
        let memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(memory_type(
                        ctx,
                        needs.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
            .unwrap();
        device.bind_buffer_memory(buffer, memory, 0).unwrap();
        (buffer, memory)
    }
}

/// Runs `record` in a one-time command buffer and waits for it.
fn one_shot(ctx: &VulkanContext, record: impl FnOnce(vk::CommandBuffer)) {
    let device = &ctx.device;
    // SAFETY: a private pool and buffer, destroyed before returning.
    unsafe {
        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.family),
                None,
            )
            .unwrap();
        let cmd = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .unwrap()[0];
        device
            .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
            .unwrap();
        record(cmd);
        device.end_command_buffer(cmd).unwrap();
        let cmds = [cmd];
        let queue = ctx.queue.lock().unwrap();
        device
            .queue_submit(
                *queue,
                &[vk::SubmitInfo::default().command_buffers(&cmds)],
                vk::Fence::null(),
            )
            .unwrap();
        device.queue_wait_idle(*queue).unwrap();
        drop(queue);
        device.destroy_command_pool(pool, None);
    }
}

fn barrier(
    ctx: &VulkanContext,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
) {
    let b = vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .level_count(1)
                .layer_count(1),
        );
    // SAFETY: recording into an open command buffer.
    unsafe {
        ctx.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[b],
        );
    }
}

/// An image of `format` holding `data` (tightly packed), left in `layout`.
pub(super) fn upload(
    ctx: &VulkanContext,
    size: (u32, u32),
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
    data: &[u8],
    layout: vk::ImageLayout,
) -> Held {
    let device = &ctx.device;
    // SAFETY: a short-lived upload; the staging objects are destroyed before returning.
    unsafe {
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D {
                        width: size.0,
                        height: size.1,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::TRANSFER_DST
                            | vk::ImageUsageFlags::TRANSFER_SRC
                            | vk::ImageUsageFlags::SAMPLED,
                    ),
                None,
            )
            .unwrap();
        let needs = device.get_image_memory_requirements(image);
        let memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(memory_type(
                        ctx,
                        needs.memory_type_bits,
                        vk::MemoryPropertyFlags::empty(),
                    )),
                None,
            )
            .unwrap();
        device.bind_image_memory(image, memory, 0).unwrap();
        let (staging, smemory) = host_buffer(ctx, data.len() as u64);
        let ptr = device
            .map_memory(smemory, 0, data.len() as u64, vk::MemoryMapFlags::empty())
            .unwrap();
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.cast::<u8>(), data.len());
        device.unmap_memory(smemory);
        one_shot(ctx, |cmd| {
            barrier(
                ctx,
                cmd,
                image,
                aspect,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            device.cmd_copy_buffer_to_image(
                cmd,
                staging,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(aspect)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: size.0,
                        height: size.1,
                        depth: 1,
                    })],
            );
            barrier(
                ctx,
                cmd,
                image,
                aspect,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                layout,
            );
        });
        device.destroy_buffer(staging, None);
        device.free_memory(smemory, None);
        Held {
            ctx: ctx.clone(),
            image,
            memory,
            source: SourceImage {
                image,
                format,
                extent: size,
                layout,
                usage: vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED,
                samples: vk::SampleCountFlags::TYPE_1,
                waits: Vec::new(),
            },
        }
    }
}

/// The four half-float channels of every texel of a held frame.
pub(super) fn read_held(ctx: &VulkanContext, image: &SourceImage) -> Vec<f32> {
    let (w, h) = image.extent;
    let bytes = u64::from(w) * u64::from(h) * 8;
    let (buffer, memory) = host_buffer(ctx, bytes);
    let device = &ctx.device;
    one_shot(ctx, |cmd| {
        barrier(
            ctx,
            cmd,
            image.image,
            vk::ImageAspectFlags::COLOR,
            image.layout,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        // SAFETY: recording into an open command buffer.
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                image.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: w,
                        height: h,
                        depth: 1,
                    })],
            );
        }
        barrier(
            ctx,
            cmd,
            image.image,
            vk::ImageAspectFlags::COLOR,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            image.layout,
        );
    });
    // SAFETY: the copy finished (the queue was waited on); the memory is host coherent.
    unsafe {
        let ptr = device
            .map_memory(memory, 0, bytes, vk::MemoryMapFlags::empty())
            .unwrap();
        let raw = std::slice::from_raw_parts(ptr.cast::<u8>(), bytes as usize);
        let out = raw
            .chunks_exact(2)
            .map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]])))
            .collect();
        device.unmap_memory(memory);
        device.destroy_buffer(buffer, None);
        device.free_memory(memory, None);
        out
    }
}

pub(super) const N: u32 = 64;
pub(super) const SIZE: u32 = 16;
pub(super) const STEP: i32 = 8;

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

pub(super) fn in_square(x: u32, y: u32, left: i32) -> bool {
    (x as i32) >= left && (x as i32) < left + SIZE as i32 && (24..24 + SIZE).contains(&y)
}

/// A red square over a static noise background; the square's colour is what a missing frame would show.
pub(super) fn colour(x: u32, y: u32, left: i32) -> [f32; 3] {
    if in_square(x, y, left) {
        [1.0, 0.2, 0.2]
    } else {
        noise(x as i32, y as i32)
    }
}

/// Synthetic colour, depth and motion for a square at `left`, each as the bytes of the format it is made in.
pub(super) struct SceneBytes {
    pub color: Vec<u8>,
    pub depth: Vec<u8>,
    pub motion: Vec<u8>,
}

pub(super) fn scene(left: i32, srgb: bool) -> SceneBytes {
    let to_srgb = |l: f32| {
        if l <= 0.0031308 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        }
    };
    let mut color = Vec::new();
    let mut depth = Vec::new();
    let mut motion = Vec::new();
    for y in 0..N {
        for x in 0..N {
            let c = colour(x, y, left);
            if srgb {
                color.extend(c.iter().map(|v| (to_srgb(*v) * 255.0).round() as u8));
                color.push(255);
            } else {
                for v in c {
                    color.extend(f32_to_f16(v).to_le_bytes());
                }
                color.extend(f32_to_f16(1.0).to_le_bytes());
            }
            let square = in_square(x, y, left);
            depth.extend(if square { 0.8f32 } else { 0.2 }.to_le_bytes());
            let mv = if square { -(STEP as f32) } else { 0.0 };
            motion.extend(mv.to_le_bytes());
            motion.extend(0.0f32.to_le_bytes());
        }
    }
    SceneBytes {
        color,
        depth,
        motion,
    }
}

pub(super) fn frame_inputs(reset: bool) -> FrameInputs {
    let r = InputResource {
        handle: 1,
        width: N,
        height: N,
        format: 0,
    };
    FrameInputs {
        depth: r,
        motion: r,
        render_size: [N, N],
        output_size: [N, N],
        motion_vector_scale: [1.0, 1.0],
        depth_reversed: true,
        camera_near: 0.1,
        camera_far: f32::INFINITY,
        reset,
        ..FrameInputs::default()
    }
}

pub(super) struct Uploaded {
    pub color: Held,
    pub depth: Held,
    pub motion: Held,
}

pub(super) fn upload_scene(
    ctx: &VulkanContext,
    left: i32,
    srgb: bool,
    depth_format: vk::Format,
) -> Uploaded {
    let s = scene(left, srgb);
    let read_only = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
    let color_format = if srgb {
        vk::Format::R8G8B8A8_SRGB
    } else {
        vk::Format::R16G16B16A16_SFLOAT
    };
    let depth = if depth_format == vk::Format::R32_SFLOAT {
        upload(
            ctx,
            (N, N),
            depth_format,
            vk::ImageAspectFlags::COLOR,
            &s.depth,
            read_only,
        )
    } else {
        upload(
            ctx,
            (N, N),
            depth_format,
            vk::ImageAspectFlags::DEPTH,
            &s.depth,
            vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        )
    };
    Uploaded {
        color: upload(
            ctx,
            (N, N),
            color_format,
            vk::ImageAspectFlags::COLOR,
            &s.color,
            read_only,
        ),
        depth,
        motion: upload(
            ctx,
            (N, N),
            vk::Format::R32G32_SFLOAT,
            vk::ImageAspectFlags::COLOR,
            &s.motion,
            read_only,
        ),
    }
}

fn capture<'a>(u: &'a Uploaded, inputs: &'a FrameInputs, generate: bool) -> Capture<'a> {
    Capture {
        color: &u.color.source,
        src_rect: None,
        inputs,
        depth: &u.depth.source,
        motion: &u.motion.source,
        generate,
        phase: 0.5,
        hud_threshold: 0.06,
    }
}

fn mean_error(rgba: &[f32], left: i32) -> (f32, u32) {
    let (mut sum, mut wrong) = (0.0, 0);
    for y in 0..N {
        for x in 0..N {
            let truth = colour(x, y, left);
            let i = ((y * N + x) * 4) as usize;
            let d = (0..3).map(|c| (rgba[i + c] - truth[c]).abs()).sum::<f32>() / 3.0;
            sum += d;
            wrong += u32::from(d > 0.05);
        }
    }
    (sum / (N * N) as f32, wrong)
}

fn midpoint_of(depth_format: vk::Format, srgb: bool) {
    let Some(vk) = gpu() else { return };
    let ctx = &vk.ctx;
    let mut generator = FrameGen::new(ctx).unwrap();
    let prev = upload_scene(ctx, 16, srgb, depth_format);
    let cur = upload_scene(ctx, 16 + STEP, srgb, depth_format);
    let inputs = frame_inputs(false);
    let first = generator.capture(&capture(&prev, &inputs, true)).unwrap();
    assert!(first.generated.is_none(), "no frame before the first");
    let second = generator.capture(&capture(&cur, &inputs, true)).unwrap();
    let generated = second.generated.expect("a generated frame");
    assert_eq!(second.srgb_values, srgb);
    let rgba = read_held(ctx, &generated);
    let (mean, wrong) = mean_error(&rgba, 16 + STEP / 2);
    eprintln!("{depth_format:?} srgb {srgb}: mean error {mean}, {wrong} texels off");
    assert!(mean < 0.01, "mean error {mean}");
    assert!(wrong < 40, "{wrong} texels off");
    let real = read_held(ctx, &second.real);
    let (mean, wrong) = mean_error(&real, 16 + STEP);
    assert!(
        mean < 0.004 && wrong == 0,
        "the held real frame is the title's frame: {mean}, {wrong}"
    );
}

#[test]
fn the_midpoint_frame_is_built_from_a_depth_format_image() {
    midpoint_of(vk::Format::D32_SFLOAT, false);
}

#[test]
fn the_midpoint_frame_is_built_from_a_colour_format_depth() {
    midpoint_of(vk::Format::R32_SFLOAT, false);
}

#[test]
fn an_srgb_back_buffer_is_held_linear_and_flagged() {
    midpoint_of(vk::Format::R32_SFLOAT, true);
}

#[test]
fn nothing_is_generated_when_not_asked_and_the_chain_restarts_after_forgetting() {
    let Some(vk) = gpu() else { return };
    let ctx = &vk.ctx;
    let mut generator = FrameGen::new(ctx).unwrap();
    let a = upload_scene(ctx, 16, false, vk::Format::R32_SFLOAT);
    let b = upload_scene(ctx, 24, false, vk::Format::R32_SFLOAT);
    let inputs = frame_inputs(false);
    generator.capture(&capture(&a, &inputs, true)).unwrap();
    let held = generator.capture(&capture(&b, &inputs, false)).unwrap();
    assert!(held.generated.is_none(), "generation was not asked for");
    generator.forget();
    let after = generator.capture(&capture(&a, &inputs, true)).unwrap();
    assert!(
        after.generated.is_none(),
        "nothing before a forgotten frame"
    );
    let again = generator.capture(&capture(&b, &inputs, true)).unwrap();
    assert!(again.generated.is_some());
}

#[test]
fn a_cropped_source_is_the_frame() {
    let Some(vk) = gpu() else { return };
    let ctx = &vk.ctx;
    let mut generator = FrameGen::new(ctx).unwrap();
    let a = upload_scene(ctx, 16, false, vk::Format::R32_SFLOAT);
    let inputs = frame_inputs(false);
    let mut request = capture(&a, &inputs, false);
    request.src_rect = Some(Rect {
        left: 8,
        top: 8,
        right: 40,
        bottom: 48,
    });
    let cropped = FrameInputs {
        render_size: [32, 40],
        ..inputs
    };
    request.inputs = &cropped;
    let held = generator.capture(&request).unwrap();
    assert_eq!(held.real.extent, (32, 40));
    let rgba = read_held(ctx, &held.real);
    let truth = colour(8 + 5, 8 + 7, 16);
    let i = ((7 * 32 + 5) * 4) as usize;
    assert!((rgba[i] - truth[0]).abs() < 0.002 && (rgba[i + 2] - truth[2]).abs() < 0.002);
}

#[test]
fn unusable_images_are_refused_with_a_reason() {
    let Some(vk) = gpu() else { return };
    let ctx = &vk.ctx;
    let mut generator = FrameGen::new(ctx).unwrap();
    let mut a = upload_scene(ctx, 16, false, vk::Format::R32_SFLOAT);
    let inputs = frame_inputs(false);
    a.depth.source.usage = vk::ImageUsageFlags::TRANSFER_SRC;
    let error = generator
        .capture(&capture(&a, &inputs, true))
        .err()
        .expect("refused");
    assert!(error.to_string().contains("depth image cannot be sampled"));
    a.depth.source.usage = vk::ImageUsageFlags::SAMPLED;
    let small = FrameInputs {
        render_size: [N * 2, N * 2],
        ..inputs
    };
    let error = generator
        .capture(&capture(&a, &small, true))
        .err()
        .expect("refused");
    assert!(error.to_string().contains("smaller than the render size"));
}

mod through_presentation {
    use std::any::Any;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::pacing::Clock;
    use crate::present::vulkan::OffscreenTarget;
    use crate::present::{
        ConsolePolicy, Frame, FrameGeneration, Plane, PresentBackend, PresentConfig, PresentError,
        Presentation, ResolvedInputs, S_OK, ScaleMode, Shown, VulkanBackend,
    };

    #[derive(Clone, Default)]
    struct FakeClock(Arc<Mutex<Duration>>);

    impl FakeClock {
        fn set(&self, to: Duration) {
            let mut now = self.0.lock().unwrap();
            *now = (*now).max(to);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            *self.0.lock().unwrap()
        }

        fn sleep_until(&self, deadline: Duration) {
            self.set(deadline);
        }
    }

    type Log = Arc<Mutex<Vec<(Duration, Option<u32>)>>>;

    /// An offscreen backend that notes, after every present, the clock and where the red square is.
    struct Watch {
        inner: VulkanBackend<OffscreenTarget>,
        clock: FakeClock,
        log: Log,
    }

    impl PresentBackend for Watch {
        fn supports_hdr10(&self) -> bool {
            false
        }

        fn present(&mut self, frame: Frame) -> Result<Shown, PresentError> {
            let shown = self.inner.present(frame)?;
            let pixels = self.inner.target.read_pixels()?;
            let row = 32 * N as usize;
            let left = (0..N as usize).find(|x| {
                let i = (row + x) * 8;
                f16_to_f32(u16::from_le_bytes([pixels[i], pixels[i + 1]])) > 0.97
            });
            self.log
                .lock()
                .unwrap()
                .push((self.clock.now(), left.map(|x| x as u32)));
            Ok(shown)
        }

        fn as_any(&mut self) -> &mut dyn Any {
            self
        }

        fn capture(
            &mut self,
            request: &Capture<'_>,
        ) -> Result<super::super::generate::Captured, PresentError> {
            self.inner.capture(request)
        }

        fn forget_generated(&mut self) {
            self.inner.forget_generated();
        }
    }

    fn plane(image: &Held) -> Plane {
        Plane {
            token: 0,
            resource_count: 1,
            source: Some(image.source.clone()),
            src_rect: None,
            placement: None,
            color_space: 0,
            scale_filter: 0,
        }
    }

    #[test]
    fn real_and_generated_frames_reach_the_target_in_order_and_never_across_a_cut_or_a_menu() {
        let Some(vk) = gpu() else { return };
        let ctx = &vk.ctx;
        let clock = FakeClock::default();
        let log: Log = Arc::default();
        let target =
            OffscreenTarget::new(ctx, (N, N), vk::Format::R16G16B16A16_SFLOAT, false).unwrap();
        let watch = Watch {
            inner: VulkanBackend::offscreen(ctx, target).unwrap(),
            clock: clock.clone(),
            log: log.clone(),
        };
        let config = PresentConfig {
            scale: ScaleMode::Stretch,
            frame_generation: FrameGeneration::on(),
            ..PresentConfig::default()
        };
        let mut presentation = Presentation::new(Box::new(watch), Arc::new(ConsolePolicy), config);
        presentation.set_pacing_clock(Arc::new(clock.clone()));
        let interval = Duration::from_micros(16_667);

        // (square position, camera cut, inputs given)
        let frames = [
            (16, false, true),
            (24, false, true),
            (32, false, true),
            (40, true, true),
            (48, false, false),
        ];
        for (k, (left, cut, described)) in frames.into_iter().enumerate() {
            presentation.flush_generated();
            clock.set(interval * k as u32);
            let scene = upload_scene(ctx, left, false, vk::Format::R32_SFLOAT);
            let resolved = described.then(|| ResolvedInputs {
                inputs: frame_inputs(cut),
                depth: scene.depth.source.clone(),
                motion: scene.motion.source.clone(),
            });
            let code =
                presentation.present_planes_with(&[plane(&scene.color)], 0.0, resolved.as_ref());
            assert_eq!(code, S_OK);
            presentation.flush_generated();
        }
        let shown: Vec<Option<u32>> = log.lock().unwrap().iter().map(|e| e.1).collect();
        let near = |a: Option<u32>, b: u32| a.is_some_and(|a| a.abs_diff(b) <= 1);
        let expect = [16, 20, 24, 28, 32, 40, 48];
        assert_eq!(shown.len(), expect.len(), "{shown:?}");
        for (got, want) in shown.iter().zip(expect) {
            assert!(near(*got, want), "shown {shown:?}, wanted {expect:?}");
        }
        let times: Vec<Duration> = log.lock().unwrap().iter().map(|e| e.0).collect();
        let gen_gap = times[2] - times[1];
        let real_gap = times[3] - times[2];
        assert!(
            gen_gap.abs_diff(real_gap) < Duration::from_micros(2),
            "{times:?}"
        );
    }
}
