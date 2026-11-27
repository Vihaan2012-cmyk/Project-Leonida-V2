#![allow(unsafe_code)]

use render::{
    Buffer, BufferDesc, Device, DeviceOptions, Format, Image, ImageDesc, MemoryLocation, vk,
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

