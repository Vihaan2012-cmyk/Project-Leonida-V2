use super::desc::{ALLOW_RENDER_TARGET, ResourceDesc};
use super::image::*;
use crate::descriptor::format::dxgi;
use crate::memory::testing::TestGpu;

#[test]
fn texels_survive_an_upload_and_a_read_back_on_every_gpu() {
    let gpus = TestGpu::open_all();
    if gpus.is_empty() {
        eprintln!("skipped: no GPU");
    }
    for gpu in &gpus {
        let h = &gpu.handles;
        let desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 37, 21, 3);
        let image = GpuImage::create(h, &desc).unwrap();
        assert_eq!((image.mips, image.layers), (3, 1));
        for mip in 0..3u32 {
            let (w, hgt) = ((37 >> mip).max(1), (21 >> mip).max(1));
            let data: Vec<u8> = (0..w * hgt * 4).map(|i| (i * 11 + mip * 5) as u8).collect();
            image.upload(h, mip, 0, &data).unwrap();
            assert_eq!(
                image.download(h, mip, 0, data.len()).unwrap(),
                data,
                "{}",
                gpu.name()
            );
        }
        assert!(matches!(
            image.upload(h, 3, 0, &[0; 4]),
            Err(ImageError::Subresource { mip: 3, layer: 0 })
        ));
        image.destroy(h);
    }
}

#[test]
fn a_compressed_array_texture_keeps_each_slice() {
    for gpu in &TestGpu::open_all() {
        let h = &gpu.handles;
        let mut desc = ResourceDesc::texture_2d(dxgi::BC1_UNORM, 16, 16, 1);
        desc.depth_or_array_size = 2;
        let image = GpuImage::create(h, &desc).unwrap();
        let slice = |s: u8| -> Vec<u8> { (0..4 * 4 * 8).map(|i| (i as u8) ^ s).collect() };
        image.upload(h, 0, 0, &slice(1)).unwrap();
        image.upload(h, 0, 1, &slice(2)).unwrap();
        assert_eq!(image.download(h, 0, 0, 128).unwrap(), slice(1));
        assert_eq!(image.download(h, 0, 1, 128).unwrap(), slice(2));
        image.destroy(h);
    }
}

#[test]
fn render_targets_and_depth_buffers_are_created() {
    for gpu in &TestGpu::open_all() {
        let h = &gpu.handles;
        let mut target = ResourceDesc::texture_2d(dxgi::R16G16B16A16_FLOAT, 64, 64, 1);
        target.flags = ALLOW_RENDER_TARGET;
        let a = GpuImage::create(h, &target).unwrap();
        let depth = ResourceDesc::texture_2d(dxgi::D32_FLOAT, 64, 64, 1);
        let b = GpuImage::create(h, &depth).unwrap();
        assert!(b.aspect.contains(ash::vk::ImageAspectFlags::DEPTH));
        let ds = ResourceDesc::texture_2d(dxgi::D24_UNORM_S8_UINT, 64, 64, 1);
        let c = GpuImage::create(h, &ds).unwrap();
        assert!(c.aspect.contains(ash::vk::ImageAspectFlags::STENCIL));
        a.destroy(h);
        b.destroy(h);
        c.destroy(h);
    }
}
