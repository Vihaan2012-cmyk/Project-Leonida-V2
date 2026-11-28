//! DLSS frame generation on a real GPU: the feature is created and generates a frame between two synthetic frames. Needs
//! a GeForce RTX 40 series GPU or newer and NVIDIA's `nvngx_dlssg.dll`: set `LEONIDA_DLSSG_DLL` to it and run with
//! `--ignored`.
#![allow(unsafe_code)]

use std::sync::Arc;

use ash::vk;
use upscale::dlss::dlssg::{self, CreateParams, EvalOptions, Resources, Session, support};
use upscale::dlss::ngx::Api;
use upscale::dlss::vk::{GpuImage, VkContext, transition};
use upscale::dlss::{DlssOptions, ffi};
use upscale::{Image, PixelFormat};

const SIZE: [u32; 2] = [640, 360];
/// How far the pattern moves between the two real frames, in pixels.
const SHIFT: f32 = 48.0;
const BORDER: usize = 40;

fn library_dir() -> Option<std::path::PathBuf> {
    let located = support::locate(None)?;
    eprintln!(
        "nvngx_dlssg.dll: {} ({})",
        located.path.display(),
        dlssg::support::describe(located.source)
    );
    located.path.parent().map(std::path::Path::to_path_buf)
}

struct Rig {
    ctx: VkContext,
    api: Arc<Api>,
    options: DlssOptions,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
}

impl Rig {
    fn new() -> Rig {
        let api = Arc::new(Api::open(None).expect("the NGX runtime of the NVIDIA driver"));
        let options = DlssOptions {
            log_level: ffi::LOGGING_OFF,
            ..DlssOptions::default()
        };
        let ctx =
            dlssg::open_device(&api, &options).expect("an NVIDIA device with NGX's extensions");
        eprintln!("GPU: {}", ctx.device_name);
        // SAFETY: plain creation calls on the live device; destroyed in Drop.
        let (pool, cmd, fence) = unsafe {
            let pool = ctx
                .device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(ctx.queue_family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .unwrap();
            let cmd = ctx
                .device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            let fence = ctx
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            (pool, cmd, fence)
        };
        Rig {
            ctx,
            api,
            options,
            pool,
            cmd,
            fence,
        }
    }

    /// Records `body` into the command buffer, submits it and waits.
    fn run<T>(&self, body: impl FnOnce(&ash::Device, vk::CommandBuffer) -> T) -> T {
        let device = &self.ctx.device;
        // SAFETY: the command buffer, fence and queue belong to this device and are used from this thread only.
        unsafe {
            device
                .reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())
                .unwrap();
            device
                .begin_command_buffer(
                    self.cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .unwrap();
            let value = body(device, self.cmd);
            device.end_command_buffer(self.cmd).unwrap();
            let cmds = [self.cmd];
            device.reset_fences(&[self.fence]).unwrap();
            device
                .queue_submit(
                    self.ctx.queue,
                    &[vk::SubmitInfo::default().command_buffers(&cmds)],
                    self.fence,
                )
                .unwrap();
            device
                .wait_for_fences(&[self.fence], true, u64::MAX)
                .unwrap();
            value
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: the queue is drained before the objects are destroyed.
        unsafe {
            self.ctx.device.queue_wait_idle(self.ctx.queue).ok();
            self.ctx.device.destroy_fence(self.fence, None);
            self.ctx.device.destroy_command_pool(self.pool, None);
        }
    }
}

fn session(rig: &Rig, dir: Option<&std::path::Path>) -> Session {
    Session::new(
        rig.api.clone(),
        &dlssg::vk_init(&rig.ctx),
        &rig.options,
        dir,
    )
    .expect("NGX starts frame generation")
}

fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    }
}

fn input_usage() -> vk::ImageUsageFlags {
    vk::ImageUsageFlags::SAMPLED
        | vk::ImageUsageFlags::TRANSFER_DST
        | vk::ImageUsageFlags::TRANSFER_SRC
        | vk::ImageUsageFlags::STORAGE
}

/// Fills `image` from `bytes` and leaves it readable by shaders.
fn upload(rig: &Rig, image: &GpuImage, bytes: &[u8]) {
    let mut staging = rig
        .ctx
        .create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC)
        .unwrap();
    staging.bytes()[..bytes.len()].copy_from_slice(bytes);
    rig.run(|device, cmd| {
        transition(
            device,
            cmd,
            image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let region = vk::BufferImageCopy::default()
            .image_subresource(color_layers())
            .image_extent(vk::Extent3D {
                width: image.size[0],
                height: image.size[1],
                depth: 1,
            });
        // SAFETY: recording; the buffer and image belong to this device.
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                staging.buffer,
                image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }
        transition(
            device,
            cmd,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
    });
    rig.ctx.destroy_buffer(staging);
}

/// The generated image back as floats (four per texel).
fn read_back(rig: &Rig, image: &GpuImage) -> Vec<f32> {
    let bytes = image.size[0] as u64 * image.size[1] as u64 * 8;
    let mut buffer = rig
        .ctx
        .create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST)
        .unwrap();
    rig.run(|device, cmd| {
        transition(
            device,
            cmd,
            image,
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        let region = vk::BufferImageCopy::default()
            .image_subresource(color_layers())
            .image_extent(vk::Extent3D {
                width: image.size[0],
                height: image.size[1],
                depth: 1,
            });
        // SAFETY: recording; the buffer and image belong to this device.
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                image.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer.buffer,
                &[region],
            );
        }
    });
    let data = buffer.bytes()[..bytes as usize].to_vec();
    rig.ctx.destroy_buffer(buffer);
    upscale::ImageRef::new(SIZE[0], SIZE[1], PixelFormat::Rgba16Float, &data)
        .rgba_f32(SIZE[0], SIZE[1], "generated frame")
        .unwrap()
}

/// The colour of the moving pattern at a texel after it has moved `shift` pixels to the right.
fn pattern(x: usize, y: usize, shift: f32) -> [f32; 3] {
    let u = x as f32 - shift;
    let tau = std::f32::consts::TAU;
    [
        0.5 + 0.4 * (tau * u / 160.0).sin(),
        0.5 + 0.4 * (tau * y as f32 / 120.0 + tau * u / 320.0).cos(),
        0.2 + 0.6 * (u / SIZE[0] as f32).clamp(0.0, 1.0),
    ]
}

fn frame(shift: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity((SIZE[0] * SIZE[1] * 4) as usize);
    for y in 0..SIZE[1] as usize {
        for x in 0..SIZE[0] as usize {
            let [r, g, b] = pattern(x, y, shift);
            out.extend([r, g, b, 1.0]);
        }
    }
    out
}

fn encode(format: PixelFormat, rgba: &[f32]) -> Vec<u8> {
    Image::from_rgba_f32(SIZE[0], SIZE[1], format, rgba)
        .unwrap()
        .data
}

/// Mean absolute difference over the interior of two frames of four floats per texel.
fn distance(a: &[f32], b: &[f32]) -> f64 {
    let (w, h) = (SIZE[0] as usize, SIZE[1] as usize);
    let (mut sum, mut count) = (0.0f64, 0u32);
    for y in BORDER..h - BORDER {
        for x in BORDER..w - BORDER {
            for c in 0..3 {
                let at = (y * w + x) * 4 + c;
                sum += f64::from((a[at] - b[at]).abs());
                count += 1;
            }
        }
    }
    sum / f64::from(count)
}

#[test]
#[ignore = "needs an RTX 40 series GPU and nvngx_dlssg.dll (set LEONIDA_DLSSG_DLL)"]
fn ngx_names_the_extensions_and_reports_frame_generation_available() {
    let rig = Rig::new();
    let wanted = dlssg::required_extensions(
        &rig.api,
        &rig.options,
        Some((rig.ctx.instance.handle(), rig.ctx.physical)),
    )
    .unwrap();
    eprintln!("instance extensions: {:?}", wanted.instance);
    eprintln!("device extensions: {:?}", wanted.device);
    let session = session(&rig, library_dir().as_deref());
    eprintln!("{:?}", session.capabilities());
    assert!(session.capabilities().available);
    assert!(session.capabilities().max_multiplier() >= 2);
}

#[test]
#[ignore = "needs an RTX 40 series GPU and nvngx_dlssg.dll (set LEONIDA_DLSSG_DLL)"]
fn a_generated_frame_between_two_moving_gradients_lies_between_them() {
    let rig = Rig::new();
    let mut session = session(&rig, library_dir().as_deref());
    let format = vk::Format::R16G16B16A16_SFLOAT;
    let color: Vec<GpuImage> = (0..2)
        .map(|_| rig.ctx.create_image(format, SIZE, input_usage()).unwrap())
        .collect();
    let depth: Vec<GpuImage> = (0..2)
        .map(|_| {
            rig.ctx
                .create_image(vk::Format::R32_SFLOAT, SIZE, input_usage())
                .unwrap()
        })
        .collect();
    let motion: Vec<GpuImage> = (0..2)
        .map(|_| {
            rig.ctx
                .create_image(vk::Format::R32G32_SFLOAT, SIZE, input_usage())
                .unwrap()
        })
        .collect();
    let output = rig
        .ctx
        .create_image(
            format,
            SIZE,
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .unwrap();

    let create = CreateParams::new(SIZE, format, SIZE);
    let created = rig.run(|_, cmd| {
        session
            .prepare(cmd, &create)
            .expect("the feature is created")
    });
    assert!(created);

    let texels = (SIZE[0] * SIZE[1]) as usize;
    let depth_bytes: Vec<u8> = (0..texels)
        .flat_map(|i| (0.3 + 0.4 * (i % SIZE[0] as usize) as f32 / SIZE[0] as f32).to_le_bytes())
        .collect();
    // Frame B shows the pattern `SHIFT` pixels further right than frame A, so a pixel of B was `SHIFT` pixels left of
    // here in A: the motion vector points from the current frame back to the previous one.
    let still: Vec<u8> = vec![0u8; texels * 8];
    let moving: Vec<u8> = (0..texels)
        .flat_map(|_| {
            let mut texel = [0u8; 8];
            texel[..4].copy_from_slice(&(-SHIFT).to_le_bytes());
            texel
        })
        .collect();
    let a = frame(0.0);
    let b = frame(SHIFT);
    for (index, (rgba, motion_bytes)) in [(&a, &still), (&b, &moving)].into_iter().enumerate() {
        upload(&rig, &color[index], &encode(PixelFormat::Rgba16Float, rgba));
        upload(&rig, &depth[index], &depth_bytes);
        upload(&rig, &motion[index], motion_bytes);
    }

    let mut options = dlssg::params::options_from_projection(SIZE, 1.0, 0.1, 1000.0, false);
    options.mvec_scale = [1.0, 1.0];
    for (index, reset) in [(0usize, true), (1usize, false)] {
        let (c, d, m, o) = (
            color[index].resource(false),
            depth[index].resource(false),
            motion[index].resource(false),
            output.resource(true),
        );
        let resources = Resources {
            backbuffer: Some(&c),
            depth: Some(&d),
            motion_vectors: Some(&m),
            output_interpolated: Some(&o),
            ..Resources::default()
        };
        let options = EvalOptions {
            reset,
            ..options.clone()
        };
        rig.run(|device, cmd| {
            transition(
                device,
                cmd,
                &output,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
            );
            session
                .evaluate(cmd, &resources, &options)
                .expect("the frame is generated");
        });
    }
    let generated = read_back(&rig, &output);

    assert!(
        generated.iter().all(|v| v.is_finite()),
        "NaN or infinity in the generated frame"
    );
    let middle = frame(SHIFT / 2.0);
    let blend: Vec<f32> = a.iter().zip(&b).map(|(x, y)| (x + y) * 0.5).collect();
    let to_a = distance(&generated, &a);
    let to_b = distance(&generated, &b);
    let to_middle = distance(&generated, &middle);
    let blend_to_middle = distance(&blend, &middle);
    let mean: f64 = generated
        .chunks_exact(4)
        .map(|p| f64::from(p[0] + p[1] + p[2]) / 3.0)
        .sum::<f64>()
        / texels as f64;
    eprintln!(
        "mean {mean:.3}; distance to the midpoint {to_middle:.4}, to frame A {to_a:.4}, to frame B {to_b:.4}; \
         a plain blend of A and B is {blend_to_middle:.4} from the midpoint; A and B are {:.4} apart",
        distance(&a, &b)
    );
    assert!(mean > 0.1, "the generated frame is nearly black: {mean}");
    assert!(
        to_middle < to_a && to_middle < to_b,
        "not between the frames"
    );
    assert!(
        to_middle < blend_to_middle,
        "no better than a plain blend of the two frames"
    );
    drop(session);
    for image in color.into_iter().chain(depth).chain(motion) {
        rig.ctx.destroy_image(image);
    }
    rig.ctx.destroy_image(output);
}
