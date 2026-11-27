use super::format::{DEFAULT_MAPPING, dxgi};
use super::gfx10::*;
use super::views::*;
use crate::resource::desc::{Dimension, ResourceDesc};

fn texture(format: u32, w: u32, h: u32, mips: u32) -> ViewResource {
    ViewResource {
        address: 0x100_0000_0000,
        desc: ResourceDesc::texture_2d(format, w, h, mips),
    }
}

fn buffer(size: u64) -> ViewResource {
    ViewResource {
        address: 0x4_0000,
        desc: ResourceDesc::buffer(size),
    }
}

fn srv_2d(format: u32, first: u32, count: u32) -> SrvDesc {
    SrvDesc {
        format,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::Texture2D {
            most_detailed_mip: first,
            mip_levels: count,
            plane_slice: 0,
            min_lod_clamp: 0.0,
        },
    }
}

fn v_words(slot: Slot) -> BufferWords {
    slot[..4].try_into().unwrap()
}

#[test]
fn a_plain_2d_view_is_the_documented_rgba8_descriptor() {
    let res = texture(dxgi::R8G8B8A8_UNORM, 1920, 1080, 11);
    let slot = srv(&res, &srv_2d(dxgi::R8G8B8A8_UNORM, 0, ALL)).unwrap();
    let image = decode_image(&slot).unwrap();
    assert_eq!((slot[1] >> 20) & 0x1FF, 56);
    assert_eq!(slot[3] >> 28, 9);
    assert_eq!(slot[3] & 0xFFF, 0xFAC);
    assert_eq!((image.width, image.height), (1920, 1080));
    assert_eq!(
        (image.first_mip, image.mip_count, image.resource_mips),
        (0, 11, 11)
    );
    assert_eq!(image.address, res.address);
}

#[test]
fn the_view_format_decides_the_code_and_the_component_mapping_the_swizzle() {
    let res = texture(dxgi::B8G8R8A8_TYPELESS, 64, 64, 1);
    let bgra = decode_image(&srv(&res, &srv_2d(dxgi::B8G8R8A8_UNORM, 0, 1)).unwrap()).unwrap();
    assert_eq!(bgra.format, 56);
    assert_eq!(bgra.swizzle, [SELECT_Z, SELECT_Y, SELECT_X, SELECT_W]);
    let srgb = decode_image(&srv(&res, &srv_2d(dxgi::B8G8R8A8_UNORM_SRGB, 0, 1)).unwrap()).unwrap();
    assert_eq!(srgb.format, 130);
    let r8 = texture(dxgi::R8_UNORM, 8, 8, 1);
    let mut desc = srv_2d(dxgi::UNKNOWN, 0, 1);
    desc.mapping = [0, 0, 0, 5];
    let image = decode_image(&srv(&r8, &desc).unwrap()).unwrap();
    assert_eq!(image.format, 1);
    assert_eq!(image.swizzle, [SELECT_X, SELECT_X, SELECT_X, SELECT_ONE]);
}

#[test]
fn mip_and_slice_ranges_are_checked_and_expanded() {
    let mut array = texture(dxgi::R8G8B8A8_UNORM, 256, 256, 0);
    array.desc.depth_or_array_size = 6;
    let view = SrvDesc {
        format: dxgi::UNKNOWN,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::Texture2DArray {
            most_detailed_mip: 2,
            mip_levels: ALL,
            first_slice: 1,
            array_size: 4,
            plane_slice: 0,
            min_lod_clamp: 1.5,
        },
    };
    let image = decode_image(&srv(&array, &view).unwrap()).unwrap();
    assert_eq!(image.ty, ImageType::D2Array);
    assert_eq!((image.first_mip, image.mip_count), (2, 7));
    assert_eq!((image.first_layer, image.layer_count), (1, 4));
    assert_eq!(image.min_lod, 1.5);
    let mut bad = view;
    bad.kind = SrvKind::Texture2DArray {
        most_detailed_mip: 0,
        mip_levels: 1,
        first_slice: 4,
        array_size: 4,
        plane_slice: 0,
        min_lod_clamp: 0.0,
    };
    assert_eq!(
        srv(&array, &bad),
        Err(ViewError::Slices {
            first: 4,
            count: 4,
            have: 6
        })
    );
    assert!(matches!(
        srv(&array, &srv_2d(dxgi::UNKNOWN, 9, 1)),
        Err(ViewError::Mips { .. })
    ));
}

#[test]
fn cube_views_cover_whole_cubes_of_six_layers() {
    let mut array = texture(dxgi::BC7_UNORM, 128, 128, 8);
    array.desc.depth_or_array_size = 12;
    let cube = SrvDesc {
        format: dxgi::UNKNOWN,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::TextureCubeArray {
            most_detailed_mip: 0,
            mip_levels: ALL,
            first_cube: 1,
            num_cubes: 1,
            min_lod_clamp: 0.0,
        },
    };
    let image = decode_image(&srv(&array, &cube).unwrap()).unwrap();
    assert_eq!(image.ty, ImageType::Cube);
    assert_eq!((image.first_layer, image.layer_count), (6, 6));
    assert_eq!(image.format, 181);
}

#[test]
fn volumes_and_multisampled_views() {
    let mut volume = texture(dxgi::R8G8B8A8_UNORM, 32, 32, 1);
    volume.desc.dimension = Dimension::Texture3D;
    volume.desc.depth_or_array_size = 16;
    let view = SrvDesc {
        format: dxgi::UNKNOWN,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::Texture3D {
            most_detailed_mip: 0,
            mip_levels: 1,
            min_lod_clamp: 0.0,
        },
    };
    let image = decode_image(&srv(&volume, &view).unwrap()).unwrap();
    assert_eq!((image.ty, image.depth), (ImageType::D3, 16));
    let mut ms = texture(dxgi::R8G8B8A8_UNORM, 64, 64, 1);
    ms.desc.sample_count = 4;
    let view = SrvDesc {
        kind: SrvKind::Texture2DMs,
        ..view
    };
    let image = decode_image(&srv(&ms, &view).unwrap()).unwrap();
    assert_eq!((image.ty, image.samples), (ImageType::D2Msaa, 4));
}

#[test]
fn depth_planes_use_the_depth_and_stencil_codes() {
    let res = texture(dxgi::R24G8_TYPELESS, 64, 64, 1);
    let depth =
        decode_image(&srv(&res, &srv_2d(dxgi::R24_UNORM_X8_TYPELESS, 0, 1)).unwrap()).unwrap();
    assert_eq!(depth.format, 141);
    let stencil =
        decode_image(&srv(&res, &srv_2d(dxgi::X24_TYPELESS_G8_UINT, 0, 1)).unwrap()).unwrap();
    assert_eq!(stencil.format, 5);
}

#[test]
fn views_of_the_wrong_kind_of_resource_are_refused() {
    let tex = texture(dxgi::R8G8B8A8_UNORM, 8, 8, 1);
    let raw = SrvDesc {
        format: dxgi::UNKNOWN,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::Buffer {
            first_element: 0,
            num_elements: 4,
            stride: 0,
            raw: true,
        },
    };
    assert!(matches!(srv(&tex, &raw), Err(ViewError::Dimension { .. })));
    assert!(matches!(
        srv(&buffer(256), &srv_2d(dxgi::UNKNOWN, 0, 1)),
        Err(ViewError::Dimension { .. })
    ));
}

#[test]
fn buffer_srvs_are_raw_structured_or_typed() {
    let res = buffer(65536);
    let make = |format, stride, raw| SrvDesc {
        format,
        mapping: DEFAULT_MAPPING,
        kind: SrvKind::Buffer {
            first_element: 10,
            num_elements: 100,
            stride,
            raw,
        },
    };
    let raw = decode_buffer(&v_words(
        srv(&res, &make(dxgi::R32_TYPELESS, 0, true)).unwrap(),
    ))
    .unwrap();
    assert_eq!(
        (raw.address, raw.stride, raw.num_records, raw.out_of_bounds),
        (0x4_0000 + 40, 0, 400, OutOfBounds::Raw)
    );
    let structured = decode_buffer(&v_words(
        srv(&res, &make(dxgi::UNKNOWN, 48, false)).unwrap(),
    ))
    .unwrap();
    assert_eq!(
        (
            structured.address,
            structured.stride,
            structured.num_records,
            structured.out_of_bounds
        ),
        (0x4_0000 + 480, 48, 100, OutOfBounds::StructuredWithOffset)
    );
    let typed = decode_buffer(&v_words(
        srv(&res, &make(dxgi::R16G16_FLOAT, 0, false)).unwrap(),
    ))
    .unwrap();
    assert_eq!(
        (
            typed.address,
            typed.stride,
            typed.format,
            typed.out_of_bounds
        ),
        (0x4_0000 + 40, 4, 29, OutOfBounds::Structured)
    );
    assert_eq!(typed.swizzle, [SELECT_X, SELECT_Y, SELECT_ZERO, SELECT_ONE]);
    assert_eq!(
        srv(&res, &make(dxgi::BC1_UNORM, 0, false)),
        Err(ViewError::BufferFormat(dxgi::BC1_UNORM))
    );
    assert_eq!(
        srv(&res, &make(dxgi::R8G8B8A8_UNORM_SRGB, 0, false)),
        Err(ViewError::BufferFormat(dxgi::R8G8B8A8_UNORM_SRGB))
    );
}

#[test]
fn typed_uavs_follow_the_formats_own_channel_order() {
    let res = buffer(4096);
    let view = |format| UavDesc {
        format,
        kind: UavKind::Buffer {
            first_element: 0,
            num_elements: 64,
            stride: 0,
            counter_offset: 0,
            raw: false,
        },
    };
    let r8 = decode_buffer(&v_words(uav(&res, &view(dxgi::R8_UNORM), None).unwrap())).unwrap();
    assert_eq!(r8.swizzle, [SELECT_X, SELECT_ZERO, SELECT_ZERO, SELECT_ONE]);
    let bgra = decode_buffer(&v_words(
        uav(&res, &view(dxgi::B8G8R8A8_UNORM), None).unwrap(),
    ))
    .unwrap();
    assert_eq!(bgra.swizzle, [SELECT_Z, SELECT_Y, SELECT_X, SELECT_W]);
    assert_eq!((bgra.format, bgra.stride, bgra.num_records), (56, 4, 64));
}

#[test]
fn a_uav_counter_is_a_raw_four_byte_buffer_in_the_slots_second_half() {
    let res = buffer(4096);
    let counter = ViewResource {
        address: 0x9_0000,
        desc: ResourceDesc::buffer(4096),
    };
    let desc = UavDesc {
        format: dxgi::UNKNOWN,
        kind: UavKind::Buffer {
            first_element: 0,
            num_elements: 16,
            stride: 16,
            counter_offset: 4096,
            raw: false,
        },
    };
    let slot = uav(&res, &desc, Some(&counter)).unwrap();
    let tail = decode_buffer(&slot[4..].try_into().unwrap()).unwrap();
    assert_eq!(
        (
            tail.address,
            tail.stride,
            tail.num_records,
            tail.out_of_bounds
        ),
        (0x9_1000, 0, 4, OutOfBounds::Raw)
    );
    assert_eq!(decode_buffer(&v_words(slot)).unwrap().stride, 16);
}

#[test]
fn texture_uavs_name_one_mip() {
    let res = texture(dxgi::R32_UINT, 64, 64, 0);
    let view = UavDesc {
        format: dxgi::UNKNOWN,
        kind: UavKind::Texture2D {
            mip_slice: 3,
            plane_slice: 0,
        },
    };
    let image = decode_image(&uav(&res, &view, None).unwrap()).unwrap();
    assert_eq!((image.first_mip, image.mip_count, image.format), (3, 1, 20));
}

#[test]
fn constant_buffers_are_raw_descriptors_of_their_size() {
    let slot = cbv(0x12_3456_7800, 256);
    let b = decode_buffer(&v_words(slot)).unwrap();
    assert_eq!(
        (b.address, b.stride, b.num_records, b.out_of_bounds),
        (0x12_3456_7800, 0, 256, OutOfBounds::Raw)
    );
    assert_eq!(slot[4..], [0; 4]);
}

#[test]
fn render_target_and_depth_views_are_the_descriptor_of_their_image() {
    let mut array = texture(dxgi::R16G16B16A16_FLOAT, 64, 64, 4);
    array.desc.depth_or_array_size = 4;
    let rt = TargetDesc {
        format: dxgi::R16G16B16A16_FLOAT,
        mip_slice: 1,
        first_slice: 2,
        slice_count: 2,
        multisampled: false,
        array: true,
    };
    let image = decode_image(&rtv(&array, &rt).unwrap()).unwrap();
    assert_eq!(
        (
            image.ty,
            image.first_mip,
            image.mip_count,
            image.first_layer,
            image.layer_count
        ),
        (ImageType::D2Array, 1, 1, 2, 2)
    );
    let depth = texture(dxgi::D32_FLOAT, 64, 64, 1);
    let ds = TargetDesc {
        format: dxgi::D32_FLOAT,
        mip_slice: 0,
        first_slice: 0,
        slice_count: 1,
        multisampled: false,
        array: false,
    };
    assert!(dsv(&depth, &ds).is_ok());
    assert_eq!(
        dsv(
            &texture(dxgi::R8G8B8A8_UNORM, 8, 8, 1),
            &TargetDesc {
                format: dxgi::R8G8B8A8_UNORM,
                ..ds
            }
        ),
        Err(ViewError::Format(dxgi::R8G8B8A8_UNORM))
    );
    assert!(matches!(
        rtv(&buffer(256), &rt),
        Err(ViewError::Dimension { .. })
    ));
}

#[test]
fn a_null_view_is_all_zero_and_not_an_image() {
    assert_eq!(NULL, [0; 8]);
    assert!(!is_image(&NULL));
}
