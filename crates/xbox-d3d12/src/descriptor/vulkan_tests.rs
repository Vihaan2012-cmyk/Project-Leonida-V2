use ash::vk;

use super::format::{dxgi, format};
use super::gfx10::*;
use super::vulkan::*;

fn fmt(dxgi: u32) -> &'static super::format::Format {
    format(dxgi).unwrap()
}

#[test]
fn a_view_in_the_resources_own_format_keeps_it() {
    let image = Image {
        format: 56,
        mip_count: 4,
        resource_mips: 4,
        ..Image::default()
    };
    let info = view_info(&image, fmt(dxgi::R8G8B8A8_UNORM)).unwrap();
    assert_eq!(info.format, vk::Format::R8G8B8A8_UNORM);
    assert_eq!(info.view_type, vk::ImageViewType::TYPE_2D);
    assert_eq!((info.base_mip, info.mips), (0, 4));
    assert_eq!(
        info.components,
        [
            vk::ComponentSwizzle::R,
            vk::ComponentSwizzle::G,
            vk::ComponentSwizzle::B,
            vk::ComponentSwizzle::A
        ]
    );
}

#[test]
fn an_srgb_view_of_a_typeless_image_takes_the_srgb_format() {
    let image = Image {
        format: 130,
        ..Image::default()
    };
    let info = view_info(&image, fmt(dxgi::R8G8B8A8_TYPELESS)).unwrap();
    assert_eq!(info.format, vk::Format::R8G8B8A8_SRGB);
    let bgra = view_info(&image, fmt(dxgi::B8G8R8A8_TYPELESS)).unwrap();
    assert_eq!(bgra.format, vk::Format::B8G8R8A8_SRGB);
}

#[test]
fn the_channel_order_of_the_format_turns_dst_sel_into_a_component_mapping() {
    let bgra = fmt(dxgi::B8G8R8A8_UNORM);
    let read_as_rgba = components([SELECT_Z, SELECT_Y, SELECT_X, SELECT_W], bgra);
    use vk::ComponentSwizzle as C;
    assert_eq!(read_as_rgba, [C::R, C::G, C::B, C::A]);
    let r8 = fmt(dxgi::R8_UNORM);
    assert_eq!(
        components([SELECT_X, SELECT_ZERO, SELECT_ZERO, SELECT_ONE], r8),
        [C::R, C::ZERO, C::ZERO, C::ONE]
    );
    assert_eq!(
        components([SELECT_Y, SELECT_X, SELECT_Z, SELECT_W], r8),
        [C::ZERO, C::R, C::ZERO, C::ZERO]
    );
}

#[test]
fn cubes_and_volumes_pick_their_view_types() {
    let cube = Image {
        ty: ImageType::Cube,
        format: 181,
        layer_count: 12,
        ..Image::default()
    };
    let info = view_info(&cube, fmt(dxgi::BC7_UNORM)).unwrap();
    assert_eq!(
        (info.view_type, info.layers),
        (vk::ImageViewType::TYPE_2D_ARRAY, 12)
    );
    let volume = Image {
        ty: ImageType::D3,
        format: 56,
        depth: 16,
        ..Image::default()
    };
    let info = view_info(&volume, fmt(dxgi::R8G8B8A8_UNORM)).unwrap();
    assert_eq!(
        (info.view_type, info.layers),
        (vk::ImageViewType::TYPE_3D, 1)
    );
}

#[test]
fn depth_images_are_viewed_as_themselves_and_unknown_codes_are_refused() {
    let depth = Image {
        format: 22,
        ..Image::default()
    };
    let info = view_info(&depth, fmt(dxgi::D32_FLOAT)).unwrap();
    assert_eq!(info.format, vk::Format::D32_SFLOAT);
    assert!(
        view_info(
            &Image {
                format: 300,
                ..Image::default()
            },
            fmt(dxgi::R8G8B8A8_UNORM)
        )
        .is_none()
    );
}

#[test]
fn a_shadow_sampler_compares_and_clamps_to_a_border() {
    let s = Sampler {
        address_u: AddressMode::ClampBorder,
        mag: Filter::Bilinear,
        min: Filter::Bilinear,
        compare: Compare::LessEqual,
        border: Border::OpaqueWhite,
        max_lod: 4.0,
        mip: MipFilter::Linear,
        ..Sampler::default()
    };
    let p = sampler_params(&s, 16.0);
    assert_eq!(p.compare, Some(vk::CompareOp::LESS_OR_EQUAL));
    assert_eq!(p.address[0], vk::SamplerAddressMode::CLAMP_TO_BORDER);
    assert_eq!(p.border, vk::BorderColor::FLOAT_OPAQUE_WHITE);
    assert_eq!(
        (p.mag, p.mipmap),
        (vk::Filter::LINEAR, vk::SamplerMipmapMode::LINEAR)
    );
    assert_eq!(p.max_lod, 4.0);
}

#[test]
fn no_mip_filter_holds_the_sampler_to_the_base_level() {
    let p = sampler_params(
        &Sampler {
            max_lod: 9.0,
            ..Sampler::default()
        },
        16.0,
    );
    assert_eq!((p.min_lod, p.max_lod), (0.0, 0.25));
    assert_eq!(p.compare, None);
    assert_eq!(p.anisotropy, None);
    let never = sampler_params(
        &Sampler {
            compare_never: true,
            ..Sampler::default()
        },
        16.0,
    );
    assert_eq!(never.compare, Some(vk::CompareOp::NEVER));
}

#[test]
fn unnormalised_samplers_get_what_vulkan_requires() {
    let s = Sampler {
        unnormalized: true,
        address_u: AddressMode::Wrap,
        mag: Filter::Bilinear,
        min: Filter::Point,
        mip: MipFilter::Linear,
        max_lod: 6.0,
        max_anisotropy: 8,
        compare: Compare::Less,
        ..Sampler::default()
    };
    let p = sampler_params(&s, 16.0);
    assert_eq!(p.address[0], vk::SamplerAddressMode::CLAMP_TO_EDGE);
    assert_eq!(p.min, p.mag);
    assert_eq!((p.min_lod, p.max_lod), (0.0, 0.0));
    assert_eq!((p.anisotropy, p.compare), (None, None));
    assert!(p.create_info().unnormalized_coordinates == vk::TRUE);
}

#[test]
fn reductions_integer_borders_and_table_borders() {
    let p = sampler_params(
        &Sampler {
            reduction: Reduction::Maximum,
            border: Border::OpaqueBlack,
            integer_border: true,
            ..Sampler::default()
        },
        16.0,
    );
    assert_eq!(p.reduction, vk::SamplerReductionMode::MAX);
    assert_eq!(p.border, vk::BorderColor::INT_OPAQUE_BLACK);
    let table = sampler_params(
        &Sampler {
            border: Border::Table,
            border_index: 7,
            lod_bias: 100.0,
            ..Sampler::default()
        },
        4.0,
    );
    assert_eq!(table.custom_border, Some(7));
    assert_eq!(table.lod_bias, 4.0);
}
