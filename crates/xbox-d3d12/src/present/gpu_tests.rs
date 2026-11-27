//! Presentation on a real Vulkan device, drawing into an offscreen image that the tests read back. They skip, with a
//! message, when the machine has no Vulkan device.

use std::sync::Arc;

use ash::vk;

use super::convert::{self, COLOR_SPACE_HDR10, Params, Space};
use super::vulkan::{OffscreenTarget, OwnedVulkan, SourceImage, VulkanContext};
use super::*;

fn gpu() -> Option<OwnedVulkan> {
    match OwnedVulkan::new(false) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: no Vulkan device ({e})");
            None
        }
    }
}

struct TestImage {
    ctx: VulkanContext,
    image: vk::Image,
    memory: vk::DeviceMemory,
    source: SourceImage,
}

impl Drop for TestImage {
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

/// An image of `format` holding `data` (tightly packed), left in `layout`.
fn make_image(
    ctx: &VulkanContext,
    size: (u32, u32),
    format: vk::Format,
    data: &[u8],
    layout: vk::ImageLayout,
) -> TestImage {
    let device = &ctx.device;
    // SAFETY: a short-lived upload; everything made here is destroyed before returning, except the image the result owns.
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
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(data.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC),
                None,
            )
            .unwrap();
        let sneeds = device.get_buffer_memory_requirements(staging);
        let smemory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(sneeds.size)
                    .memory_type_index(memory_type(
                        ctx,
                        sneeds.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
            .unwrap();
        device.bind_buffer_memory(staging, smemory, 0).unwrap();
        let ptr = device
            .map_memory(smemory, 0, data.len() as u64, vk::MemoryMapFlags::empty())
            .unwrap();
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.cast::<u8>(), data.len());
        device.unmap_memory(smemory);
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
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to = |old, new, from: vk::AccessFlags, into: vk::AccessFlags| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(from)
                .dst_access_mask(into)
                .old_layout(old)
                .new_layout(new)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range)
        };
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to(
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            )],
        );
        device.cmd_copy_buffer_to_image(
            cmd,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: size.0,
                    height: size.1,
                    depth: 1,
                })],
        );
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to(
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                layout,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::MEMORY_READ,
            )],
        );
        device.end_command_buffer(cmd).unwrap();
        let cmds = [cmd];
        {
            let queue = ctx.queue.lock().unwrap();
            device
                .queue_submit(
                    *queue,
                    &[vk::SubmitInfo::default().command_buffers(&cmds)],
                    vk::Fence::null(),
                )
                .unwrap();
            device.queue_wait_idle(*queue).unwrap();
        }
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        device.free_memory(smemory, None);
        TestImage {
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

fn solid(size: (u32, u32), texel: [u8; 4]) -> Vec<u8> {
    (0..size.0 * size.1).flat_map(|_| texel).collect()
}

fn presentation(
    ctx: &VulkanContext,
    size: (u32, u32),
    format: vk::Format,
    hdr10: bool,
    config: PresentConfig,
) -> Presentation {
    let target = OffscreenTarget::new(ctx, size, format, hdr10).unwrap();
    let backend = VulkanBackend::offscreen(ctx, target).unwrap();
    Presentation::new(Box::new(backend), Arc::new(ConsolePolicy), config)
}

fn read(p: &Presentation) -> Vec<u8> {
    p.with_backend::<VulkanBackend<OffscreenTarget>, _>(|b| b.target.read_pixels().unwrap())
        .unwrap()
}

fn plane(image: &TestImage, space: i32) -> Plane {
    Plane {
        token: 0,
        resource_count: 1,
        source: Some(image.source.clone()),
        src_rect: None,
        placement: None,
        color_space: space,
        scale_filter: 0,
    }
}

const RGBA: vk::Format = vk::Format::R8G8B8A8_UNORM;

fn px(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

#[test]
fn a_blank_present_is_black() {
    let Some(g) = gpu() else { return };
    let p = presentation(&g.ctx, (32, 18), RGBA, false, PresentConfig::default());
    let red = make_image(
        &g.ctx,
        (32, 18),
        RGBA,
        &solid((32, 18), [255, 0, 0, 255]),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    assert_eq!(p.present_planes(&[plane(&red, 0)], 0.0), S_OK);
    assert_eq!(px(&read(&p), 32, 5, 5), [255, 0, 0, 255]);
    assert_eq!(p.present_planes(&[], 0.0), S_OK);
    assert!(
        read(&p)
            .chunks(4)
            .all(|t| t[0] == 0 && t[1] == 0 && t[2] == 0)
    );
}

#[test]
fn a_same_size_frame_is_copied_unchanged() {
    let Some(g) = gpu() else { return };
    let mut data = Vec::new();
    for y in 0..18u32 {
        for x in 0..32u32 {
            data.extend_from_slice(&[(x * 8) as u8, (y * 14) as u8, (x ^ y) as u8, 255]);
        }
    }
    let img = make_image(
        &g.ctx,
        (32, 18),
        RGBA,
        &data,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    let p = presentation(&g.ctx, (32, 18), RGBA, false, PresentConfig::default());
    assert_eq!(p.present_planes(&[plane(&img, 0)], 0.0), S_OK);
    assert_eq!(read(&p), data);
}

#[test]
fn the_frame_is_fitted_with_black_bars_and_follows_a_resize() {
    let Some(g) = gpu() else { return };
    let img = make_image(
        &g.ctx,
        (64, 36),
        RGBA,
        &solid((64, 36), [10, 200, 30, 255]),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    );
    let p = presentation(&g.ctx, (64, 64), RGBA, false, PresentConfig::default());
    assert_eq!(p.present_planes(&[plane(&img, 0)], 0.0), S_OK);
    let data = read(&p);
    assert_eq!(px(&data, 64, 32, 5), [0, 0, 0, 0]);
    assert_eq!(px(&data, 64, 32, 32), [10, 200, 30, 255]);
    assert_eq!(px(&data, 64, 0, 14), [10, 200, 30, 255]);
    assert_eq!(px(&data, 64, 63, 49), [10, 200, 30, 255]);
    assert_eq!(px(&data, 64, 32, 50), [0, 0, 0, 0]);

    p.with_backend::<VulkanBackend<OffscreenTarget>, _>(|b| b.target.resize((128, 36)).unwrap());
    assert_eq!(p.present_planes(&[plane(&img, 0)], 0.0), S_OK);
    let data = read(&p);
    assert_eq!(data.len(), 128 * 36 * 4);
    assert_eq!(px(&data, 128, 10, 18), [0, 0, 0, 0]);
    assert_eq!(px(&data, 128, 64, 18), [10, 200, 30, 255]);
}

#[test]
fn stretch_fills_the_window() {
    let Some(g) = gpu() else { return };
    let img = make_image(
        &g.ctx,
        (64, 36),
        RGBA,
        &solid((64, 36), [1, 2, 3, 255]),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    let config = PresentConfig {
        scale: ScaleMode::Stretch,
        ..PresentConfig::default()
    };
    let p = presentation(&g.ctx, (64, 64), RGBA, false, config);
    assert_eq!(p.present_planes(&[plane(&img, 0)], 0.0), S_OK);
    let data = read(&p);
    assert_eq!(px(&data, 64, 0, 0), [1, 2, 3, 255]);
    assert_eq!(px(&data, 64, 63, 63), [1, 2, 3, 255]);
}

#[test]
fn a_source_rectangle_crops_and_bgra_is_swapped_to_the_output_order() {
    let Some(g) = gpu() else { return };
    let mut data = Vec::new();
    for _y in 0..8 {
        for x in 0..16 {
            let blue_green_red_alpha = if x < 8 {
                [0, 0, 255, 255]
            } else {
                [0, 255, 0, 255]
            };
            data.extend_from_slice(&blue_green_red_alpha);
        }
    }
    let img = make_image(
        &g.ctx,
        (16, 8),
        vk::Format::B8G8R8A8_UNORM,
        &data,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    let config = PresentConfig {
        point_filter: true,
        scale: ScaleMode::Stretch,
        ..PresentConfig::default()
    };
    let p = presentation(&g.ctx, (16, 8), RGBA, false, config);
    let mut left = plane(&img, 0);
    left.src_rect = Some(convert::Rect {
        left: 0,
        top: 0,
        right: 8,
        bottom: 8,
    });
    assert_eq!(p.present_planes(&[left], 0.0), S_OK);
    let out = read(&p);
    assert!(
        out.chunks(4).all(|t| t == [255, 0, 0, 255]),
        "{:?}",
        &out[..8]
    );
    let mut right = plane(&img, 0);
    right.src_rect = Some(convert::Rect {
        left: 8,
        top: 0,
        right: 16,
        bottom: 8,
    });
    assert_eq!(p.present_planes(&[right], 0.0), S_OK);
    assert!(read(&p).chunks(4).all(|t| t == [0, 255, 0, 255]));
}

#[test]
fn placement_puts_the_frame_in_part_of_the_output() {
    let Some(g) = gpu() else { return };
    let img = make_image(
        &g.ctx,
        (32, 18),
        RGBA,
        &solid((32, 18), [200, 100, 50, 255]),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    let p = presentation(&g.ctx, (64, 36), RGBA, false, PresentConfig::default());
    let mut half = plane(&img, 0);
    half.placement = Some([0.0, 0.0, 0.5, 1.0]);
    assert_eq!(p.present_planes(&[half], 0.0), S_OK);
    let data = read(&p);
    assert_eq!(px(&data, 64, 10, 10), [200, 100, 50, 255]);
    assert_eq!(px(&data, 64, 50, 10), [0, 0, 0, 0]);
}

fn pack_2101010(rgb: [u16; 3]) -> [u8; 4] {
    let word =
        u32::from(rgb[0]) | (u32::from(rgb[1]) << 10) | (u32::from(rgb[2]) << 20) | (3 << 30);
    word.to_le_bytes()
}

fn unpack_2101010(texel: [u8; 4]) -> [u16; 3] {
    let w = u32::from_le_bytes(texel);
    [
        (w & 0x3ff) as u16,
        ((w >> 10) & 0x3ff) as u16,
        ((w >> 20) & 0x3ff) as u16,
    ]
}

#[test]
fn an_hdr10_frame_on_an_sdr_output_goes_through_the_compose_pass() {
    let Some(g) = gpu() else { return };
    let params = Params::default();
    let cases = [
        [pq_code(203.0); 3],
        [pq_code(20.0), pq_code(100.0), pq_code(500.0)],
        [pq_code(1000.0); 3],
    ];
    for codes in cases {
        let img = make_image(
            &g.ctx,
            (16, 16),
            vk::Format::A2B10G10R10_UNORM_PACK32,
            &(0..256)
                .flat_map(|_| pack_2101010(codes))
                .collect::<Vec<u8>>(),
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        let p = presentation(&g.ctx, (16, 16), RGBA, false, PresentConfig::default());
        assert_eq!(
            p.present_planes(&[plane(&img, COLOR_SPACE_HDR10)], 0.0),
            S_OK
        );
        let got = px(&read(&p), 16, 8, 8);
        let input = codes.map(|c| f32::from(c) / 1023.0);
        let want = convert::convert(input, Space::Hdr10, Space::Sdr, &params);
        for i in 0..3 {
            let expected = (want[i] * 255.0).round() as i32;
            assert!(
                (i32::from(got[i]) - expected).abs() <= 3,
                "{codes:?} channel {i}: {got:?} vs {want:?}"
            );
        }
        assert_eq!(got[3], 255);
    }
}

fn pq_code(nits: f32) -> u16 {
    (convert::pq_encode(nits) * 1023.0).round() as u16
}

#[test]
fn an_sdr_frame_on_an_hdr10_output_is_encoded_as_pq() {
    let Some(g) = gpu() else { return };
    let params = Params::default();
    let img = make_image(
        &g.ctx,
        (16, 16),
        RGBA,
        &solid((16, 16), [200, 100, 50, 255]),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    );
    let p = presentation(
        &g.ctx,
        (16, 16),
        vk::Format::A2B10G10R10_UNORM_PACK32,
        true,
        PresentConfig::default(),
    );
    assert_eq!(p.present_planes(&[plane(&img, 0)], 0.0), S_OK);
    let got = unpack_2101010(px(&read(&p), 16, 8, 8));
    let want = convert::convert(
        [200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0],
        Space::Sdr,
        Space::Hdr10,
        &params,
    );
    for i in 0..3 {
        let expected = (want[i] * 1023.0).round() as i32;
        assert!(
            (i32::from(got[i]) - expected).abs() <= 3,
            "channel {i}: {got:?} vs {want:?}"
        );
    }
}

#[test]
fn hdr10_to_hdr10_keeps_the_codes() {
    let Some(g) = gpu() else { return };
    let codes = [pq_code(50.0), pq_code(400.0), pq_code(900.0)];
    let img = make_image(
        &g.ctx,
        (16, 16),
        vk::Format::A2B10G10R10_UNORM_PACK32,
        &(0..256)
            .flat_map(|_| pack_2101010(codes))
            .collect::<Vec<u8>>(),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    let p = presentation(
        &g.ctx,
        (16, 16),
        vk::Format::A2B10G10R10_UNORM_PACK32,
        true,
        PresentConfig::default(),
    );
    assert_eq!(
        p.present_planes(&[plane(&img, COLOR_SPACE_HDR10)], 0.0),
        S_OK
    );
    let got = unpack_2101010(px(&read(&p), 16, 8, 8));
    for i in 0..3 {
        assert_eq!(got[i], codes[i], "channel {i}");
    }
}

#[test]
fn frames_that_cannot_be_blitted_are_refused_without_drawing() {
    let Some(g) = gpu() else { return };
    let mut img = make_image(
        &g.ctx,
        (16, 16),
        RGBA,
        &solid((16, 16), [9, 9, 9, 255]),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    let p = presentation(&g.ctx, (16, 16), RGBA, false, PresentConfig::default());
    img.source.samples = vk::SampleCountFlags::TYPE_4;
    assert_eq!(
        p.present_planes(&[plane(&img, 0)], 0.0),
        DXGI_ERROR_UNSUPPORTED
    );
    img.source.samples = vk::SampleCountFlags::TYPE_1;
    img.source.usage = vk::ImageUsageFlags::STORAGE;
    assert_eq!(
        p.present_planes(&[plane(&img, 0)], 0.0),
        DXGI_ERROR_UNSUPPORTED
    );
    img.source.usage = vk::ImageUsageFlags::TRANSFER_SRC;
    img.source.format = vk::Format::D32_SFLOAT;
    assert_eq!(
        p.present_planes(&[plane(&img, 0)], 0.0),
        DXGI_ERROR_UNSUPPORTED
    );
    img.source.format = RGBA;
    img.source.layout = vk::ImageLayout::UNDEFINED;
    assert_eq!(
        p.present_planes(&[plane(&img, 0)], 0.0),
        DXGI_ERROR_UNSUPPORTED
    );
}

#[test]
fn frames_flow_through_events_tokens_and_presents_in_real_time() {
    let Some(g) = gpu() else { return };
    let img = make_image(
        &g.ctx,
        (16, 16),
        RGBA,
        &solid((16, 16), [30, 60, 90, 255]),
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    let p = presentation(&g.ctx, (16, 16), RGBA, false, PresentConfig::default());
    assert_eq!(p.set_frame_interval_x(0, 10_000, 1, 0), S_OK);
    assert_eq!(p.schedule_frame_event_x(FRAME_EVENT_ORIGIN, 0, 0), S_OK);
    let start = std::time::Instant::now();
    let mut last = 0;
    for _ in 0..8 {
        let w = p.wait_frame_event_x(FRAME_EVENT_ORIGIN, 1000, 0);
        assert_eq!(w.hresult, S_OK);
        assert!(w.token > last);
        last = w.token;
        let mut frame = plane(&img, 0);
        frame.token = w.token;
        assert_eq!(p.present_planes(&[frame], 0.0), S_OK);
    }
    assert!(start.elapsed() >= std::time::Duration::from_millis(60));
    assert_eq!(px(&read(&p), 16, 3, 3), [30, 60, 90, 255]);
    let (hr, stats) =
        p.get_frame_statistics_x(last - 1, StatKind::Present.bit() | StatKind::Event.bit(), 4);
    assert_eq!(hr, S_OK);
    assert_eq!(stats.len(), 2);
}
