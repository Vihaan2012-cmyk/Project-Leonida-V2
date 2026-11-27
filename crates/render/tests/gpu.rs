#![allow(unsafe_code)]

use render::shaders::SCALE_ADD;
use render::{
    BindlessSet, Buffer, BufferDesc, CommandStream, ComputePipeline, DescriptorSet, Device,
    DeviceOptions, Format, Image, ImageDesc, MemoryLocation, PipelineLayout, SetLayout, StagingRing,
    vk,
};

fn device() -> Option<Device> {
    let options = DeviceOptions {
        validation: true,
        ..DeviceOptions::default()
    };
    match Device::new(&options) {
        Ok(device) => Some(device),
        Err(e) => {
            eprintln!("skipped, no usable GPU: {e}");
            None
        }
    }
}

fn bytes(count: usize, seed: u8) -> Vec<u8> {
    (0..count)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn no_validation_errors() {
    assert_eq!(
        render::validation_errors(),
        0,
        "the validation layer reported errors"
    );
}

fn readback_buffer(device: &Device, size: u64) -> Buffer {
    Buffer::new(
        device,
        &BufferDesc {
            name: "readback",
            size,
            usage: vk::BufferUsageFlags::empty(),
            location: MemoryLocation::Readback,
        },
    )
    .unwrap()
}

#[test]
fn device_reports_what_it_picked() {
    let Some(device) = device() else { return };
    assert!(!device.info().name.is_empty());
    assert!(device.queue_count() >= 1);
    assert!(device.features().max_push_constants >= 128);
    no_validation_errors();
}

#[test]
fn buffer_upload_and_readback() {
    let Some(device) = device() else { return };
    let data = bytes(10_000, 3);
    let gpu_buffer =
        Buffer::with_data(&device, "data", vk::BufferUsageFlags::STORAGE_BUFFER, &data).unwrap();
    assert_ne!(gpu_buffer.device_address(), 0);
    let readback = readback_buffer(&device, data.len() as u64);
    device
        .queue(0)
        .immediate(|cb| {
            cb.copy_buffer(
                &gpu_buffer,
                &readback,
                &[vk::BufferCopy::default().size(data.len() as u64)],
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(readback.read_vec(0, data.len()).unwrap(), data);
    assert!(readback.read_vec(1, data.len()).is_err());
    assert!(gpu_buffer.write(0, &[1]).is_err());
    no_validation_errors();
}

#[test]
fn image_round_trips_every_mip_and_layer() {
    let Some(device) = device() else { return };
    let desc = ImageDesc::new_2d(
        "rgba8",
        Format::Rgba8Unorm,
        13,
        7,
        vk::ImageUsageFlags::SAMPLED,
    )
    .mips(4)
    .layers(2);
    let image = Image::new(&device, &desc).unwrap();
    let (regions, total) = image.packed_layout();
    assert_eq!(regions.len(), 8);
    let data = bytes(total as usize, 9);
    let queue = device.queue(0);
    image
        .upload_packed(&queue, &data, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .unwrap();
    let back = image
        .read_back(&queue, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .unwrap();
    for region in &regions {
        let mip = region.image_subresource.mip_level;
        let start = region.buffer_offset as usize;
        let len = Format::Rgba8Unorm.level_bytes([13, 7, 1], mip) as usize;
        assert_eq!(
            back[start..start + len],
            data[start..start + len],
            "mip {mip}"
        );
    }
    no_validation_errors();
}

#[test]
fn block_compressed_image_round_trips() {
    let Some(device) = device() else { return };
    if !device.features().bc_textures {
        return;
    }
    let desc = ImageDesc::new_2d("bc1", Format::Bc1Unorm, 10, 6, vk::ImageUsageFlags::SAMPLED);
    let image = Image::new(&device, &desc).unwrap();
    let data = bytes(Format::Bc1Unorm.level_bytes([10, 6, 1], 0) as usize, 1);
    let queue = device.queue(0);
    image
        .upload_packed(&queue, &data, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .unwrap();
    assert_eq!(
        image
            .read_back(&queue, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .unwrap(),
        data
    );
    no_validation_errors();
}

#[test]
fn volume_and_depth_images_are_created() {
    let Some(device) = device() else { return };
    let volume = Image::new(
        &device,
        &ImageDesc::new_3d(
            "volume",
            Format::Rgba16Float,
            [8, 8, 4],
            vk::ImageUsageFlags::SAMPLED,
        ),
    )
    .unwrap();
    assert_eq!(
        volume.default_view().unwrap().view_type(),
        vk::ImageViewType::TYPE_3D
    );
    let depth = Image::new(
        &device,
        &ImageDesc::new_2d(
            "depth",
            Format::D32Float,
            64,
            64,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        ),
    )
    .unwrap();
    assert_eq!(
        depth.default_view().unwrap().view_type(),
        vk::ImageViewType::TYPE_2D
    );
    no_validation_errors();
}

fn run_scale_add(device: &Device, bindless: Option<&BindlessSet>) {
    let input: Vec<u32> = (0..1000).collect();
    let input_bytes: Vec<u8> = input.iter().flat_map(|v| v.to_le_bytes()).collect();
    let src = Buffer::with_data(
        device,
        "in",
        vk::BufferUsageFlags::STORAGE_BUFFER,
        &input_bytes,
    )
    .unwrap();
    let dst = Buffer::with_data(
        device,
        "out",
        vk::BufferUsageFlags::STORAGE_BUFFER,
        &vec![0; input_bytes.len()],
    )
    .unwrap();
    let readback = readback_buffer(device, input_bytes.len() as u64);
    let set0 = SetLayout::storage_buffers(device, 2).unwrap();
    let layout = match bindless {
        Some(set) => set.pipeline_layout(Some(&set0), 8).unwrap(),
        None => PipelineLayout::new(device, std::slice::from_ref(&set0), 8).unwrap(),
    };
    let pipeline = ComputePipeline::new(device, &layout, SCALE_ADD, None).unwrap();
    let set = DescriptorSet::new(&set0).unwrap();
    set.write_storage_buffer(0, &src);
    set.write_storage_buffer(1, &dst);
    let queue = device.queue(0);
    let mut stream = CommandStream::new(&queue).unwrap();
    let mut cb = stream.begin().unwrap();
    cb.bind_compute_pipeline(&pipeline);
    cb.bind_descriptor_set(vk::PipelineBindPoint::COMPUTE, &layout, 0, &set);
    if let Some(bindless) = bindless {
        bindless.bind(&mut cb, vk::PipelineBindPoint::COMPUTE, &layout);
    }
    let push: Vec<u8> = [7u32, 5].iter().flat_map(|v| v.to_le_bytes()).collect();
    cb.push_constants(&layout, 0, &push);
    cb.dispatch(1000u32.div_ceil(64), 1, 1);
    cb.memory_barrier(
        vk::PipelineStageFlags2::COMPUTE_SHADER,
        vk::AccessFlags2::SHADER_WRITE,
        vk::PipelineStageFlags2::TRANSFER,
        vk::AccessFlags2::TRANSFER_READ,
    );
    cb.copy_buffer(
        &dst,
        &readback,
        &[vk::BufferCopy::default().size(input_bytes.len() as u64)],
    );
    stream.submit(cb, &[], &[]).unwrap().wait().unwrap();
    let out = readback.read_vec(0, input_bytes.len()).unwrap();
    for (i, chunk) in out.chunks(4).enumerate() {
        let value = u32::from_le_bytes(chunk.try_into().unwrap());
        assert_eq!(value, i as u32 * 7 + 5, "element {i}");
    }
}

#[test]
fn compute_dispatch_runs_a_wgsl_shader() {
    let Some(device) = device() else { return };
    run_scale_add(&device, None);
    no_validation_errors();
}

#[test]
fn staging_ring_wraps_and_waits_for_the_gpu() {
    let Some(device) = device() else { return };
    let queue = device.queue(0);
    let mut ring = StagingRing::new(&queue, 1024, MemoryLocation::Upload).unwrap();
    let target = Buffer::new(
        &device,
        &BufferDesc {
            name: "target",
            size: 1 << 14,
            usage: vk::BufferUsageFlags::empty(),
            location: MemoryLocation::GpuOnly,
        },
    )
    .unwrap();
    let mut stream = CommandStream::new(&queue).unwrap();
    let mut last = None;
    for round in 0u8..20 {
        let data = bytes(300, round);
        let slice = ring.upload(&data, 16).unwrap();
        assert_eq!(slice.offset % 16, 0);
        assert!(slice.offset + slice.size <= ring.capacity());
        let mut cb = stream.begin().unwrap();
        cb.copy_buffer(
            ring.buffer(),
            &target,
            &[vk::BufferCopy::default()
                .src_offset(slice.offset)
                .dst_offset(u64::from(round) * 512)
                .size(slice.size)],
        );
        let submission = stream.submit(cb, &[], &[]).unwrap();
        ring.mark_submitted(&submission);
        last = Some(submission);
    }
    last.unwrap().wait().unwrap();
    let mut readback = StagingRing::new(&queue, 1 << 14, MemoryLocation::Readback).unwrap();
    let slice = readback.alloc(1 << 14, 4).unwrap();
    queue
        .immediate(|cb| {
            cb.copy_buffer(
                &target,
                readback.buffer(),
                &[vk::BufferCopy::default()
                    .dst_offset(slice.offset)
                    .size(slice.size)],
            );
            Ok(())
        })
        .unwrap();
    let all = readback.read(slice).unwrap();
    for round in 0u8..20 {
        let start = usize::from(round) * 512;
        assert_eq!(
            all[start..start + 300],
            bytes(300, round)[..],
            "round {round}"
        );
    }
    assert!(ring.alloc(4096, 1).is_err());
    no_validation_errors();
}

#[test]
fn timeline_orders_work_across_queues() {
    let Some(device) = device() else { return };
    if device.queue_count() < 2 {
        return;
    }
    let (first, second) = (device.queue(0), device.queue(1));
    let target = readback_buffer(&device, 256);
    let mut a = CommandStream::new(&first).unwrap();
    let mut cb = a.begin().unwrap();
    cb.fill_buffer(&target, 0, 256, 0x1111_1111);
    let one = a.submit(cb, &[], &[]).unwrap();
    let mut b = CommandStream::new(&second).unwrap();
    let mut cb = b.begin().unwrap();
    cb.fill_buffer(&target, 0, 128, 0x2222_2222);
    let two = b
        .submit(
            cb,
            &[one.as_wait(vk::PipelineStageFlags2::ALL_COMMANDS)],
            &[],
        )
        .unwrap();
    two.wait().unwrap();
    assert!(one.is_done().unwrap());
    let out = target.read_vec(0, 256).unwrap();
    assert!(out[..128].iter().all(|&b| b == 0x22));
    assert!(out[128..].iter().all(|&b| b == 0x11));
    no_validation_errors();
}

#[test]
fn host_memory_can_be_aliased_by_a_gpu_buffer() {
    let Some(device) = device() else { return };
    if !device.features().host_import {
        eprintln!("skipped: no VK_EXT_external_memory_host");
        return;
    }
    let align = device.features().host_import_alignment as usize;
    let size = align * 2;
    let layout = std::alloc::Layout::from_size_align(size, align).unwrap();
    // SAFETY: the layout is non-zero; the memory is freed below after the buffer and all GPU work are gone.
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
    assert!(!ptr.is_null());
    {
        // SAFETY: `ptr` is valid for `size` bytes, aligned as the import demands, and outlives `aliased`.
        let aliased = unsafe {
            Buffer::from_host_memory(
                &device,
                ptr,
                size as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
            )
        }
        .unwrap();
        let source = Buffer::with_data(
            &device,
            "src",
            vk::BufferUsageFlags::empty(),
            &bytes(256, 5),
        )
        .unwrap();
        device
            .queue(0)
            .immediate(|cb| {
                cb.copy_buffer(
                    &source,
                    &aliased,
                    &[vk::BufferCopy::default().dst_offset(64).size(256)],
                );
                Ok(())
            })
            .unwrap();
        // SAFETY: the GPU finished (`immediate` waits), so reading the host range is race-free.
        let host = unsafe { std::slice::from_raw_parts(ptr, size) };
        assert_eq!(host[64..320], bytes(256, 5)[..]);
        assert!(host[..64].iter().all(|&b| b == 0));
        // SAFETY: the call validates its arguments before touching the memory, and `ptr + 1` is inside it.
        let refused = unsafe {
            Buffer::from_host_memory(
                &device,
                ptr.add(1),
                size as u64 - 1,
                vk::BufferUsageFlags::empty(),
            )
        };
        assert!(refused.is_err());
    }
    // SAFETY: allocated above with the same layout; the buffer is dropped.
    unsafe { std::alloc::dealloc(ptr, layout) };
    no_validation_errors();
}

