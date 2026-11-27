use std::sync::Arc;

use super::rawdesc::*;
use crate::descriptor::format::dxgi;
use crate::descriptor::views::{SrvKind, UavKind, ViewResource};
use crate::resource::HeapKind;
use crate::resource::desc::{Dimension, Layout, ResourceDesc};
use crate::root::{ParameterKind, RangeType, RootSignature, RootVersion, Visibility};

struct Block(Vec<u8>);

impl Block {
    fn new(len: usize) -> Block {
        Block(vec![0; len])
    }

    fn u32(&mut self, at: usize, v: u32) -> &mut Block {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
        self
    }

    fn u64(&mut self, at: usize, v: u64) -> &mut Block {
        self.0[at..at + 8].copy_from_slice(&v.to_le_bytes());
        self
    }

    fn f32(&mut self, at: usize, v: f32) -> &mut Block {
        self.u32(at, v.to_bits())
    }

    fn p(&self) -> *const u8 {
        self.0.as_ptr()
    }
}

fn resource(desc: ResourceDesc) -> ViewResource {
    ViewResource {
        address: 0x1_0000,
        desc,
    }
}

#[test]
fn a_resource_description_is_read_with_its_packed_depth_and_mips() {
    let mut b = Block::new(56);
    b.u32(0, 3)
        .u64(8, 0x10000)
        .u64(16, 1920)
        .u32(24, 1080)
        .u32(28, 6 | 11 << 16)
        .u32(32, dxgi::R8G8B8A8_UNORM)
        .u32(36, 1)
        .u32(44, 0x101)
        .u32(48, 0x0300_0001);
    // SAFETY: a 56-byte block.
    let d = unsafe { read_resource_desc(b.p()) }.unwrap();
    assert_eq!(d.dimension, Dimension::Texture2D);
    assert_eq!((d.width, d.height), (1920, 1080));
    assert_eq!((d.depth_or_array_size, d.mip_levels), (6, 11));
    assert_eq!(d.layout, Layout::ConsoleTiled);
    assert_eq!(d.flags, 0x0300_0001);
    b.u32(0, 9);
    // SAFETY: as above.
    assert!(unsafe { read_resource_desc(b.p()) }.is_none());
}

#[test]
fn heap_properties_choose_the_memory_kind() {
    let mut b = Block::new(20);
    // SAFETY: a 20-byte block, then null.
    unsafe {
        assert_eq!(read_heap_kind(std::ptr::null()), HeapKind::Default);
        assert_eq!(read_heap_kind(b.u32(0, 2).p()), HeapKind::Upload);
        assert_eq!(read_heap_kind(b.u32(0, 3).p()), HeapKind::Readback);
        assert_eq!(read_heap_kind(b.u32(0, 4).u32(4, 3).p()), HeapKind::Upload);
        assert_eq!(read_heap_kind(b.u32(0, 4).u32(4, 1).p()), HeapKind::Default);
    }
}

#[test]
fn a_shader_resource_view_is_read_by_dimension_and_a_null_one_is_the_default() {
    let tex = resource(ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 64, 64, 0));
    let mut b = Block::new(40);
    b.u32(0, dxgi::R8G8B8A8_UNORM_SRGB)
        .u32(4, 4)
        .u32(8, 0x1688)
        .u32(16, 1)
        .u32(20, 3)
        .u32(24, 0)
        .f32(28, 2.0);
    // SAFETY: a 40-byte block.
    let d = unsafe { read_srv(b.p(), &tex) }.unwrap();
    assert_eq!(d.format, dxgi::R8G8B8A8_UNORM_SRGB);
    assert_eq!(d.mapping, [0, 1, 2, 3]);
    assert_eq!(
        d.kind,
        SrvKind::Texture2D {
            most_detailed_mip: 1,
            mip_levels: 3,
            plane_slice: 0,
            min_lod_clamp: 2.0
        }
    );
    b.u32(4, 10).u32(24, 12).u32(28, 2);
    // SAFETY: as above.
    let cube = unsafe { read_srv(b.p(), &tex) }.unwrap();
    assert!(matches!(
        cube.kind,
        SrvKind::TextureCubeArray {
            first_cube: 2,
            num_cubes: 2,
            ..
        }
    ));
    b.u32(4, 11);
    // SAFETY: as above.
    assert!(unsafe { read_srv(b.p(), &tex) }.is_none());
    // SAFETY: a null description.
    let default = unsafe { read_srv(std::ptr::null(), &tex) }.unwrap();
    assert!(matches!(default.kind, SrvKind::Texture2D { .. }));
}

#[test]
fn an_unordered_access_view_of_a_buffer_carries_its_counter_and_raw_flag() {
    let buf = resource(ResourceDesc::buffer(4096));
    let mut b = Block::new(40);
    b.u32(0, dxgi::UNKNOWN)
        .u32(4, 1)
        .u64(8, 4)
        .u32(16, 100)
        .u32(20, 16)
        .u64(24, 0x40)
        .u32(32, 1);
    // SAFETY: a 40-byte block.
    let d = unsafe { read_uav(b.p(), &buf) }.unwrap();
    assert_eq!(
        d.kind,
        UavKind::Buffer {
            first_element: 4,
            num_elements: 100,
            stride: 16,
            counter_offset: 0x40,
            raw: true
        }
    );
}

#[test]
fn target_views_read_their_slices_and_samples() {
    let tex = resource(ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 64, 64, 0));
    let mut b = Block::new(32);
    b.u32(0, dxgi::R8G8B8A8_UNORM)
        .u32(4, 5)
        .u32(8, 2)
        .u32(12, 1)
        .u32(16, 3);
    // SAFETY: a 32-byte block.
    let rtv = unsafe { read_rtv(b.p(), &tex) }.unwrap();
    assert!(rtv.array && !rtv.multisampled);
    assert_eq!((rtv.mip_slice, rtv.first_slice, rtv.slice_count), (2, 1, 3));
    let mut d = Block::new(32);
    d.u32(0, dxgi::D32_FLOAT)
        .u32(4, 4)
        .u32(12, 1)
        .u32(16, 2)
        .u32(20, 4);
    // SAFETY: as above.
    let dsv = unsafe { read_dsv(d.p(), &tex) }.unwrap();
    assert_eq!((dsv.mip_slice, dsv.first_slice, dsv.slice_count), (1, 2, 4));
    // SAFETY: null descriptions.
    let default = unsafe { read_rtv(std::ptr::null(), &tex) }.unwrap();
    assert_eq!((default.array, default.slice_count), (false, 1));
}

#[test]
fn a_graphics_pipeline_description_is_read_field_by_field() {
    let vs = [1u8, 2, 3];
    let name = b"POSITION\0";
    let mut layout = Block::new(32);
    layout
        .u64(0, name.as_ptr() as u64)
        .u32(8, 1)
        .u32(12, dxgi::R32G32B32_FLOAT)
        .u32(16, 2)
        .u32(20, 0xFFFF_FFFF)
        .u32(24, 1)
        .u32(28, 1);
    let mut b = Block::new(656);
    b.u64(8, vs.as_ptr() as u64)
        .u64(16, 3)
        .u32(120, 1)
        .u32(128, 1)
        .u32(136, 5)
        .u32(164, 0xF)
        .u32(448, 0xFFFF_FFFF)
        .u32(452, 3)
        .u32(456, 2)
        .u32(460, 1)
        .u32(464, (-5i32) as u32)
        .f32(468, 1.5)
        .u32(476, 1)
        .u32(496, 1)
        .u32(500, 1)
        .u32(504, 4)
        .u32(508, 1)
        .u32(512, 0xAB | 0xCD << 8)
        .u32(516, 2)
        .u32(540, 7)
        .u64(552, layout.p() as u64)
        .u32(560, 1)
        .u32(568, 2)
        .u32(572, 3)
        .u32(576, 2)
        .u32(580, dxgi::R8G8B8A8_UNORM)
        .u32(584, dxgi::R16G16B16A16_FLOAT)
        .u32(612, dxgi::D32_FLOAT)
        .u32(616, 4);
    let root = Arc::new(RootSignature {
        version: RootVersion::V1_1,
        parameters: Vec::new(),
        static_samplers: Vec::new(),
        flags: 0,
    });
    // SAFETY: a 656-byte description whose pointers name the live arrays above.
    let d = unsafe { read_graphics_pso(b.p(), root) };
    assert_eq!(d.vs.0, vs);
    assert!(d.ps.is_empty() && d.gs.is_empty());
    assert!(d.blend.alpha_to_coverage && !d.blend.independent);
    assert!(d.blend.targets[0].blend_enable);
    assert_eq!(d.blend.targets[0].src, 5);
    assert_eq!(d.blend.targets[0].write_mask, 0xF);
    assert_eq!(d.sample_mask, 0xFFFF_FFFF);
    assert_eq!((d.raster.fill, d.raster.cull), (3, 2));
    assert!(d.raster.front_ccw && d.raster.depth_clip);
    assert_eq!((d.raster.depth_bias, d.raster.depth_bias_clamp), (-5, 1.5));
    assert_eq!(d.depth_stencil.depth_func, 4);
    assert_eq!(d.depth_stencil.stencil_read_mask, 0xAB);
    assert_eq!(d.depth_stencil.stencil_write_mask, 0xCD);
    assert_eq!(d.depth_stencil.front.fail, 2);
    assert_eq!(d.depth_stencil.back.pass, 7);
    assert_eq!(d.input_layout.len(), 1);
    assert_eq!(d.input_layout[0].semantic, "POSITION");
    assert_eq!(d.input_layout[0].slot, 2);
    assert_eq!(d.input_layout[0].class, 1);
    assert_eq!((d.strip_cut, d.topology), (2, 3));
    assert_eq!(
        d.render_targets,
        vec![dxgi::R8G8B8A8_UNORM, dxgi::R16G16B16A16_FLOAT]
    );
    assert_eq!((d.depth_format, d.sample_count), (dxgi::D32_FLOAT, 4));
}

#[test]
fn a_versioned_root_signature_in_memory_serializes_to_what_parses_back() {
    let mut range = Block::new(24);
    range
        .u32(0, 0)
        .u32(4, 4)
        .u32(8, 2)
        .u32(12, 1)
        .u32(16, 8)
        .u32(20, 0xFFFF_FFFF);
    let mut params = Block::new(64);
    params
        .u32(0, 0)
        .u32(8, 1)
        .u64(16, range.p() as u64)
        .u32(24, 5)
        .u32(32, 1)
        .u32(40, 3)
        .u32(44, 1)
        .u32(48, 4)
        .u32(56, 0);
    let mut sampler = Block::new(52);
    sampler
        .u32(0, 0x15)
        .u32(4, 3)
        .u32(8, 3)
        .u32(12, 3)
        .u32(20, 8)
        .u32(24, 4)
        .u32(40, 2)
        .f32(32, 0.0)
        .f32(36, 16.0)
        .u32(48, 5);
    let mut desc = Block::new(48);
    desc.u32(0, 2)
        .u32(8, 2)
        .u64(16, params.p() as u64)
        .u32(24, 1)
        .u64(32, sampler.p() as u64)
        .u32(40, 0x1000);
    // SAFETY: a description whose arrays are the live blocks above.
    let parsed = unsafe { read_versioned_root_signature(desc.p()) }.unwrap();
    assert_eq!(parsed.version, RootVersion::V1_1);
    assert_eq!(parsed.parameters.len(), 2);
    assert_eq!(parsed.parameters[0].visibility, Visibility::Pixel);
    let ParameterKind::Table(ranges) = &parsed.parameters[0].kind else {
        panic!();
    };
    assert_eq!(ranges[0].kind, RangeType::Srv);
    assert_eq!((ranges[0].count, ranges[0].flags), (4, 8));
    assert!(matches!(
        parsed.parameters[1].kind,
        ParameterKind::Constants {
            register: 3,
            space: 1,
            values: 4
        }
    ));
    assert_eq!(parsed.static_samplers[0].visibility, 5);
    assert_eq!(parsed.flags, 0x1000);
    let blob = parsed.serialize();
    assert_eq!(RootSignature::parse(&blob).unwrap(), parsed);
}

#[test]
fn a_sampler_description_is_read_from_its_52_bytes() {
    let mut b = Block::new(52);
    b.u32(0, 0x55)
        .u32(4, 2)
        .f32(16, -0.5)
        .u32(20, 16)
        .u32(24, 8)
        .f32(28, 1.0)
        .f32(40, 0.25)
        .f32(44, 1.0)
        .f32(48, 9.0);
    // SAFETY: a 52-byte block.
    let s = unsafe { read_sampler(b.p()) };
    assert_eq!((s.filter, s.address_u, s.max_anisotropy), (0x55, 2, 16));
    assert_eq!((s.mip_lod_bias, s.comparison_func), (-0.5, 8));
    assert_eq!(s.border_color, [1.0, 0.0, 0.0, 0.25]);
    assert_eq!((s.min_lod, s.max_lod), (1.0, 9.0));
}

#[test]
fn the_console_compute_description_is_four_pairs_after_the_root_signature() {
    let (a, b, c, d) = ([1u8; 4], [2u8; 5], [3u8; 6], [4u8; 7]);
    let mut m = Block::new(72);
    m.u64(8, a.as_ptr() as u64)
        .u64(16, 4)
        .u64(24, b.as_ptr() as u64)
        .u64(32, 5)
        .u64(40, c.as_ptr() as u64)
        .u64(48, 6)
        .u64(56, d.as_ptr() as u64)
        .u64(64, 7);
    // SAFETY: a description whose pairs name the live arrays above.
    let parts = unsafe { read_slot67(m.p()) };
    assert_eq!(parts.dispatch_state, a);
    assert_eq!(parts.kind1, b);
    assert_eq!(parts.program, c);
    assert_eq!(parts.records, d);
}
