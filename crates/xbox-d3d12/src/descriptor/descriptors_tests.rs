use shader_translate::images::{self, Number};

use super::format::{DEFAULT_MAPPING, dxgi};
use super::gfx10::*;
use super::sampler::SamplerDesc;
use super::views::*;
use super::*;
use crate::memory::testing::TestGpu;
use crate::resource::desc::ResourceDesc;
use crate::resource::image::GpuImage;

fn srv_2d(format: u32) -> SrvDesc {
    SrvDesc {
        format,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::Texture2D {
            most_detailed_mip: 0,
            mip_levels: ALL,
            plane_slice: 0,
            min_lod_clamp: 0.0,
        },
    }
}

fn texture(address: u64, format: u32) -> ViewResource {
    ViewResource {
        address,
        desc: ResourceDesc::texture_2d(format, 64, 64, 0),
    }
}

#[test]
fn a_created_view_is_found_by_the_shaders_lookup() {
    let mut d = Descriptors::new();
    let a = texture(0x100_0000_0000, dxgi::R8G8B8A8_UNORM);
    let b = texture(0x100_0001_0000, dxgi::R32_UINT);
    let (slot_a, reg_a) = d.create_srv(Some(&a), &srv_2d(dxgi::UNKNOWN)).unwrap();
    let (slot_b, reg_b) = d.create_srv(Some(&b), &srv_2d(dxgi::UNKNOWN)).unwrap();
    assert!(reg_a.unwrap().new && reg_b.unwrap().new);
    let tables = d.tables().unwrap().clone();
    assert_eq!(
        images::lookup(&tables.textures, &slot_a),
        images::slot(reg_a.unwrap().index, Number::Float)
    );
    assert_eq!(
        images::lookup(&tables.textures, &slot_b),
        images::slot(reg_b.unwrap().index, Number::Uint)
    );
    assert_eq!(
        d.texture_slot(&slot_a),
        images::lookup(&tables.textures, &slot_a)
    );
}

#[test]
fn two_views_of_the_same_mips_share_a_slot_and_different_mips_do_not() {
    let mut d = Descriptors::new();
    let a = texture(0x100_0000_0000, dxgi::R8G8B8A8_UNORM);
    let first = d
        .create_srv(Some(&a), &srv_2d(dxgi::UNKNOWN))
        .unwrap()
        .1
        .unwrap();
    let again = d
        .create_srv(Some(&a), &srv_2d(dxgi::UNKNOWN))
        .unwrap()
        .1
        .unwrap();
    assert_eq!(first.index, again.index);
    let mut other = srv_2d(dxgi::UNKNOWN);
    other.kind = SrvKind::Texture2D {
        most_detailed_mip: 2,
        mip_levels: 1,
        plane_slice: 0,
        min_lod_clamp: 0.0,
    };
    let mip2 = d.create_srv(Some(&a), &other).unwrap().1.unwrap();
    assert_ne!(mip2.index, first.index);
}

#[test]
fn buffer_views_and_null_views_register_nothing() {
    let mut d = Descriptors::new();
    let buffer = ViewResource {
        address: 0x4_0000,
        desc: ResourceDesc::buffer(4096),
    };
    let raw = SrvDesc {
        format: dxgi::UNKNOWN,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::Buffer {
            first_element: 0,
            num_elements: 16,
            stride: 0,
            raw: true,
        },
    };
    let (slot, registration) = d.create_srv(Some(&buffer), &raw).unwrap();
    assert!(registration.is_none());
    assert!(!is_image(&slot));
    assert_eq!(d.create_srv(None, &raw).unwrap().0, NULL);
    assert_eq!(d.registry.texture_count(), 0);
    assert_eq!(d.create_cbv(0x4_0000, 64)[2], 64);
}

#[test]
fn samplers_are_registered_and_found() {
    let mut d = Descriptors::new();
    let (words, registration) = d.create_sampler(&SamplerDesc {
        filter: 0x15,
        ..SamplerDesc::default()
    });
    assert!(registration.new);
    let tables = d.tables().unwrap().clone();
    assert_eq!(
        images::lookup(&tables.samplers, &images::sampler_key(&words)),
        images::slot(registration.index, Number::Float)
    );
}

#[test]
fn render_target_views_are_registered_like_images() {
    let mut d = Descriptors::new();
    let target = texture(0x100_0002_0000, dxgi::R16G16B16A16_FLOAT);
    let desc = TargetDesc {
        format: dxgi::R16G16B16A16_FLOAT,
        mip_slice: 0,
        first_slice: 0,
        slice_count: 1,
        multisampled: false,
        array: false,
    };
    let (slot, registration) = d.create_rtv(&target, &desc).unwrap();
    assert!(registration.is_some());
    assert_eq!(decode_image(&slot).unwrap().address, 0x100_0002_0000);
}

#[test]
fn descriptors_survive_being_written_to_and_copied_through_heap_memory() {
    let mut d = Descriptors::new();
    let (slot, registration) = d
        .create_srv(
            Some(&texture(0x100_0000_0000, dxgi::BC7_UNORM)),
            &srv_2d(dxgi::UNKNOWN),
        )
        .unwrap();
    let mut heap = vec![0u32; 24];
    let base = heap.as_mut_ptr() as u64;
    // SAFETY: the vector holds three 32-byte slots.
    unsafe {
        write_slot(base + SLOT_BYTES, &slot);
        let copy = read_slot(base + SLOT_BYTES);
        write_slot(base + 2 * SLOT_BYTES, &copy);
    }
    let found = d.scan_textures(&heap, 8);
    assert_eq!(found.len(), 2);
    assert!(
        found
            .iter()
            .all(|(_, r)| r.index == registration.unwrap().index && !r.new)
    );
    let (words, _) = d.create_sampler(&SamplerDesc::default());
    let mut samplers = vec![0xFFFF_FFFFu32; 16];
    // SAFETY: the vector holds two 32-byte slots.
    unsafe { write_sampler_slot(samplers.as_mut_ptr() as u64, &words) };
    assert_eq!(&samplers[..4], &words);
    assert_eq!(&samplers[4..8], &[0; 4]);
}

#[test]
fn a_registered_view_makes_a_real_vulkan_view_of_a_real_image() {
    for gpu in &TestGpu::open_all() {
        let h = &gpu.handles;
        let desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_TYPELESS, 64, 64, 0);
        let image = GpuImage::create(h, &desc).unwrap();
        let mut d = Descriptors::new();
        let resource = ViewResource {
            address: 0x100_0000_0000,
            desc,
        };
        let mut view = srv_2d(dxgi::R8G8B8A8_UNORM_SRGB);
        view.kind = SrvKind::Texture2D {
            most_detailed_mip: 1,
            mip_levels: 3,
            plane_slice: 0,
            min_lod_clamp: 0.0,
        };
        let (_, registration) = d.create_srv(Some(&resource), &view).unwrap();
        let entry = d.registry.texture(registration.unwrap().index).unwrap();
        let info = vulkan::view_info(&entry.image, desc.format_info().unwrap()).unwrap();
        assert_eq!(info.format, ash::vk::Format::R8G8B8A8_SRGB);
        let create = ash::vk::ImageViewCreateInfo::default()
            .image(image.image)
            .view_type(info.view_type)
            .format(info.format)
            .components(ash::vk::ComponentMapping {
                r: info.components[0],
                g: info.components[1],
                b: info.components[2],
                a: info.components[3],
            })
            .subresource_range(
                ash::vk::ImageSubresourceRange::default()
                    .aspect_mask(ash::vk::ImageAspectFlags::COLOR)
                    .base_mip_level(info.base_mip)
                    .level_count(info.mips)
                    .base_array_layer(info.base_layer)
                    .layer_count(info.layers),
            );
        // SAFETY: the view is made and destroyed here on the image just created.
        unsafe {
            let made = h.device.create_image_view(&create, None).unwrap();
            h.device.destroy_image_view(made, None);
        }
        let (words, registration) = d.create_sampler(&SamplerDesc {
            filter: 0x15,
            address_u: 3,
            address_v: 3,
            address_w: 3,
            ..SamplerDesc::default()
        });
        let params = vulkan::sampler_params(
            &d.registry.sampler(registration.index).unwrap().sampler,
            16.0,
        );
        assert_eq!(decode_sampler(&words).mag, Filter::Bilinear);
        // SAFETY: made and destroyed here.
        unsafe {
            let sampler = h
                .device
                .create_sampler(&params.create_info(), None)
                .unwrap();
            h.device.destroy_sampler(sampler, None);
        }
        image.destroy(h);
    }
}
