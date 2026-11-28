//! The title's description structures, read from its memory (x64 layouts of the PC SDK structures the console
//! shares): resource, view, sampler, pipeline and versioned root signature descriptions.

use std::sync::Arc;

use crate::descriptor::format::{DEFAULT_MAPPING, dxgi};
use crate::descriptor::sampler::{ConsoleSampler, SamplerDesc};
use crate::descriptor::views::{ALL, SrvDesc, SrvKind, TargetDesc, UavDesc, UavKind, ViewResource};
use crate::pipeline::state::{
    BlendState, DepthStencilState, InputElement, RasterizerState, RenderTargetBlend, StencilFace,
};
use crate::pipeline::{Bytecode, ComputePsoDesc, GraphicsPsoDesc, Slot67Parts};
use crate::resource::HeapKind;
use crate::resource::desc::{Dimension, Layout, ResourceDesc};
use crate::root::{
    DescriptorRange, ParameterKind, RangeType, RootParameter, RootSignature, RootVersion,
    StaticSampler, Visibility,
};

/// A pointer into the title's memory with typed reads.
#[derive(Clone, Copy)]
pub struct Mem(pub *const u8);

impl Mem {
    /// # Safety
    /// `at + 4` bytes after the pointer are readable.
    pub unsafe fn u32(self, at: usize) -> u32 {
        // SAFETY: as the caller promises.
        unsafe { self.0.add(at).cast::<u32>().read_unaligned() }
    }

    /// # Safety
    /// `at + 8` bytes after the pointer are readable.
    pub unsafe fn u64(self, at: usize) -> u64 {
        // SAFETY: as the caller promises.
        unsafe { self.0.add(at).cast::<u64>().read_unaligned() }
    }

    /// # Safety
    /// As `u32`.
    pub unsafe fn f32(self, at: usize) -> f32 {
        // SAFETY: as the caller promises.
        f32::from_bits(unsafe { self.u32(at) })
    }

    /// # Safety
    /// `at + 8` bytes after the pointer are readable.
    pub unsafe fn ptr(self, at: usize) -> Mem {
        // SAFETY: as the caller promises.
        Mem(unsafe { self.u64(at) } as usize as *const u8)
    }

    pub fn is_null(self) -> bool {
        self.0.is_null()
    }

    /// # Safety
    /// `at` holds a pointer and a size, and the bytes they name are readable.
    pub unsafe fn bytes(self, at: usize) -> Vec<u8> {
        // SAFETY: as the caller promises.
        unsafe {
            let (p, n) = (self.ptr(at), self.u64(at + 8) as usize);
            if p.is_null() || n == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(p.0, n).to_vec()
            }
        }
    }
}

/// `D3D12_RESOURCE_DESC`, 56 bytes.
///
/// # Safety
/// `p` is 56 readable bytes.
pub unsafe fn read_resource_desc(p: *const u8) -> Option<ResourceDesc> {
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        Some(ResourceDesc {
            dimension: Dimension::from_raw(m.u32(0))?,
            alignment: m.u64(8),
            width: m.u64(16),
            height: m.u32(24),
            depth_or_array_size: u32::from(m.u32(28) as u16),
            mip_levels: u32::from((m.u32(28) >> 16) as u16),
            format: m.u32(32),
            sample_count: m.u32(36).max(1),
            sample_quality: m.u32(40),
            layout: Layout::from_raw(m.u32(44)).unwrap_or(Layout::Unknown),
            flags: m.u32(48),
        })
    }
}

/// The kind of memory a `D3D12_HEAP_PROPERTIES` asks for.
///
/// # Safety
/// `p` is 20 readable bytes or null.
pub unsafe fn read_heap_kind(p: *const u8) -> HeapKind {
    if p.is_null() {
        return HeapKind::Default;
    }
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        match m.u32(0) {
            2 => HeapKind::Upload,
            3 => HeapKind::Readback,
            // Custom: CPU page property 1 is not available, 2 write combine, 3 write back.
            4 if m.u32(4) >= 2 => HeapKind::Upload,
            _ => HeapKind::Default,
        }
    }
}

/// The 4-bit-per-channel component mapping as the four selections.
fn mapping(raw: u32) -> [u32; 4] {
    // `D3D12_ENCODE_SHADER_4_COMPONENT_MAPPING` puts a 3-bit selection per channel, with bit 12 always set.
    if raw == 0 {
        return DEFAULT_MAPPING;
    }
    [0, 1, 2, 3].map(|i| (raw >> (3 * i)) & 7)
}

/// `D3D12_SHADER_RESOURCE_VIEW_DESC`; `None` when the view is of a kind this layer has no descriptor for.
/// A null description is the default view of the resource.
///
/// # Safety
/// `p` is null or a readable description.
pub unsafe fn read_srv(p: *const u8, resource: &ViewResource) -> Option<SrvDesc> {
    if p.is_null() {
        return Some(default_srv(resource));
    }
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        let kind = match m.u32(4) {
            1 => SrvKind::Buffer {
                first_element: m.u64(16),
                num_elements: m.u32(24),
                stride: m.u32(28),
                raw: m.u32(32) & 1 != 0,
            },
            2 => SrvKind::Texture1D {
                most_detailed_mip: m.u32(16),
                mip_levels: m.u32(20),
                min_lod_clamp: m.f32(24),
            },
            3 => SrvKind::Texture1DArray {
                most_detailed_mip: m.u32(16),
                mip_levels: m.u32(20),
                first_slice: m.u32(24),
                array_size: m.u32(28),
                min_lod_clamp: m.f32(32),
            },
            4 => SrvKind::Texture2D {
                most_detailed_mip: m.u32(16),
                mip_levels: m.u32(20),
                plane_slice: m.u32(24),
                min_lod_clamp: m.f32(28),
            },
            5 => SrvKind::Texture2DArray {
                most_detailed_mip: m.u32(16),
                mip_levels: m.u32(20),
                first_slice: m.u32(24),
                array_size: m.u32(28),
                plane_slice: m.u32(32),
                min_lod_clamp: m.f32(36),
            },
            6 => SrvKind::Texture2DMs,
            7 => SrvKind::Texture2DMsArray {
                first_slice: m.u32(16),
                array_size: m.u32(20),
            },
            8 => SrvKind::Texture3D {
                most_detailed_mip: m.u32(16),
                mip_levels: m.u32(20),
                min_lod_clamp: m.f32(24),
            },
            9 => SrvKind::TextureCube {
                most_detailed_mip: m.u32(16),
                mip_levels: m.u32(20),
                min_lod_clamp: m.f32(24),
            },
            10 => SrvKind::TextureCubeArray {
                most_detailed_mip: m.u32(16),
                mip_levels: m.u32(20),
                first_cube: m.u32(24) / 6,
                num_cubes: m.u32(28),
                min_lod_clamp: m.f32(32),
            },
            _ => return None,
        };
        Some(SrvDesc {
            format: m.u32(0),
            mapping: mapping(m.u32(8)),
            kind,
        })
    }
}

/// What a null SRV description means: the whole resource in its own format.
pub fn default_srv(resource: &ViewResource) -> SrvDesc {
    let d = &resource.desc;
    let multisampled = d.sample_count > 1;
    let array = d.array_size() > 1;
    let kind = match (d.dimension, array, multisampled) {
        (Dimension::Buffer, ..) => SrvKind::Buffer {
            first_element: 0,
            num_elements: (d.width / 4) as u32,
            stride: 0,
            raw: true,
        },
        (Dimension::Texture1D, false, _) => SrvKind::Texture1D {
            most_detailed_mip: 0,
            mip_levels: ALL,
            min_lod_clamp: 0.0,
        },
        (Dimension::Texture1D, true, _) => SrvKind::Texture1DArray {
            most_detailed_mip: 0,
            mip_levels: ALL,
            first_slice: 0,
            array_size: ALL,
            min_lod_clamp: 0.0,
        },
        (Dimension::Texture2D, false, false) => SrvKind::Texture2D {
            most_detailed_mip: 0,
            mip_levels: ALL,
            plane_slice: 0,
            min_lod_clamp: 0.0,
        },
        (Dimension::Texture2D, true, false) => SrvKind::Texture2DArray {
            most_detailed_mip: 0,
            mip_levels: ALL,
            first_slice: 0,
            array_size: ALL,
            plane_slice: 0,
            min_lod_clamp: 0.0,
        },
        (Dimension::Texture2D, false, true) => SrvKind::Texture2DMs,
        (Dimension::Texture2D, true, true) => SrvKind::Texture2DMsArray {
            first_slice: 0,
            array_size: ALL,
        },
        (Dimension::Texture3D, ..) => SrvKind::Texture3D {
            most_detailed_mip: 0,
            mip_levels: ALL,
            min_lod_clamp: 0.0,
        },
    };
    SrvDesc {
        format: dxgi::UNKNOWN,
        mapping: DEFAULT_MAPPING,
        kind,
    }
}

/// `D3D12_UNORDERED_ACCESS_VIEW_DESC`; a null description is the default view.
///
/// # Safety
/// `p` is null or a readable description.
pub unsafe fn read_uav(p: *const u8, resource: &ViewResource) -> Option<UavDesc> {
    let d = &resource.desc;
    if p.is_null() {
        let kind = match (d.dimension, d.array_size() > 1) {
            (Dimension::Buffer, _) => UavKind::Buffer {
                first_element: 0,
                num_elements: (d.width / 4) as u32,
                stride: 0,
                counter_offset: 0,
                raw: true,
            },
            (Dimension::Texture1D, false) => UavKind::Texture1D { mip_slice: 0 },
            (Dimension::Texture1D, true) => UavKind::Texture1DArray {
                mip_slice: 0,
                first_slice: 0,
                array_size: ALL,
            },
            (Dimension::Texture2D, false) => UavKind::Texture2D {
                mip_slice: 0,
                plane_slice: 0,
            },
            (Dimension::Texture2D, true) => UavKind::Texture2DArray {
                mip_slice: 0,
                first_slice: 0,
                array_size: ALL,
                plane_slice: 0,
            },
            (Dimension::Texture3D, _) => UavKind::Texture3D { mip_slice: 0 },
        };
        return Some(UavDesc {
            format: dxgi::UNKNOWN,
            kind,
        });
    }
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        let kind = match m.u32(4) {
            1 => UavKind::Buffer {
                first_element: m.u64(8),
                num_elements: m.u32(16),
                stride: m.u32(20),
                counter_offset: m.u64(24),
                raw: m.u32(32) & 1 != 0,
            },
            2 => UavKind::Texture1D {
                mip_slice: m.u32(8),
            },
            3 => UavKind::Texture1DArray {
                mip_slice: m.u32(8),
                first_slice: m.u32(12),
                array_size: m.u32(16),
            },
            4 => UavKind::Texture2D {
                mip_slice: m.u32(8),
                plane_slice: m.u32(12),
            },
            5 => UavKind::Texture2DArray {
                mip_slice: m.u32(8),
                first_slice: m.u32(12),
                array_size: m.u32(16),
                plane_slice: m.u32(20),
            },
            8 => UavKind::Texture3D {
                mip_slice: m.u32(8),
            },
            _ => return None,
        };
        Some(UavDesc {
            format: m.u32(0),
            kind,
        })
    }
}

fn default_target(resource: &ViewResource) -> TargetDesc {
    let d = &resource.desc;
    TargetDesc {
        format: dxgi::UNKNOWN,
        mip_slice: 0,
        first_slice: 0,
        slice_count: if d.array_size() > 1 { ALL } else { 1 },
        multisampled: d.sample_count > 1,
        array: d.array_size() > 1,
    }
}

/// `D3D12_RENDER_TARGET_VIEW_DESC`; a null description is the whole first mip.
///
/// # Safety
/// `p` is null or a readable description.
pub unsafe fn read_rtv(p: *const u8, resource: &ViewResource) -> Option<TargetDesc> {
    if p.is_null() {
        return Some(default_target(resource));
    }
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        let format = m.u32(0);
        let one = |mip_slice| TargetDesc {
            format,
            mip_slice,
            first_slice: 0,
            slice_count: 1,
            multisampled: false,
            array: false,
        };
        Some(match m.u32(4) {
            2 | 4 | 8 => one(m.u32(8)),
            3 => TargetDesc {
                first_slice: m.u32(12),
                slice_count: m.u32(16),
                array: true,
                ..one(m.u32(8))
            },
            5 => TargetDesc {
                first_slice: m.u32(12),
                slice_count: m.u32(16),
                array: true,
                ..one(m.u32(8))
            },
            6 => TargetDesc {
                multisampled: true,
                ..one(0)
            },
            7 => TargetDesc {
                first_slice: m.u32(8),
                slice_count: m.u32(12),
                multisampled: true,
                array: true,
                ..one(0)
            },
            _ => return None,
        })
    }
}

/// `D3D12_DEPTH_STENCIL_VIEW_DESC`; a null description is the whole first mip.
///
/// # Safety
/// `p` is null or a readable description.
pub unsafe fn read_dsv(p: *const u8, resource: &ViewResource) -> Option<TargetDesc> {
    if p.is_null() {
        return Some(default_target(resource));
    }
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        let format = m.u32(0);
        let one = |mip_slice| TargetDesc {
            format,
            mip_slice,
            first_slice: 0,
            slice_count: 1,
            multisampled: false,
            array: false,
        };
        Some(match m.u32(4) {
            1 | 3 => one(m.u32(12)),
            2 | 4 => TargetDesc {
                first_slice: m.u32(16),
                slice_count: m.u32(20),
                array: true,
                ..one(m.u32(12))
            },
            5 => TargetDesc {
                multisampled: true,
                ..one(0)
            },
            6 => TargetDesc {
                first_slice: m.u32(12),
                slice_count: m.u32(16),
                multisampled: true,
                array: true,
                ..one(0)
            },
            _ => return None,
        })
    }
}

/// `D3D12_SAMPLER_DESC`, 52 bytes.
///
/// # Safety
/// `p` is 52 readable bytes.
pub unsafe fn read_sampler(p: *const u8) -> SamplerDesc {
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        SamplerDesc {
            filter: m.u32(0),
            address_u: m.u32(4),
            address_v: m.u32(8),
            address_w: m.u32(12),
            mip_lod_bias: m.f32(16),
            max_anisotropy: m.u32(20),
            comparison_func: m.u32(24),
            border_color: [m.f32(28), m.f32(32), m.f32(36), m.f32(40)],
            min_lod: m.f32(44),
            max_lod: m.f32(48),
            flags: 0,
        }
    }
}

/// The console sampler description the title hands to `CreateSamplerX`, 0x54 bytes: magnification and
/// minimisation filter codes at 4 and 8, the mip filter at 0x10 (the one at 0xC is the Z filter), the anisotropy
/// at 0x14, the three clamp codes at 0x18, 0x1C and 0x20, the LOD range at 0x24 and 0x28, the bias at 0x30, the
/// compare field at 0x34 and the border colour at 0x44.
///
/// # Safety
/// `p` is 0x54 readable bytes.
pub unsafe fn read_console_sampler(p: *const u8) -> ConsoleSampler {
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        ConsoleSampler {
            mag: m.u32(4),
            min: m.u32(8),
            mip: m.u32(0x10),
            aniso: m.u32(0x14),
            clamp: [m.u32(0x18), m.u32(0x1C), m.u32(0x20)],
            min_lod: m.f32(0x24),
            max_lod: m.f32(0x28),
            lod_bias: m.f32(0x30),
            compare: m.u32(0x34),
            border_color: [m.f32(0x44), m.f32(0x48), m.f32(0x4C), m.f32(0x50)],
        }
    }
}

fn stencil_face(m: Mem, at: usize) -> StencilFace {
    // SAFETY: the caller of `read_graphics_pso` promises the description is readable.
    unsafe {
        StencilFace {
            fail: m.u32(at),
            depth_fail: m.u32(at + 4),
            pass: m.u32(at + 8),
            func: m.u32(at + 12),
        }
    }
}

/// `D3D12_GRAPHICS_PIPELINE_STATE_DESC` (656 bytes) with the root signature already resolved.
///
/// # Safety
/// `p` is a readable description whose shader bytecode and input layout pointers are readable.
pub unsafe fn read_graphics_pso(p: *const u8, root: Arc<RootSignature>) -> GraphicsPsoDesc {
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        let mut desc = GraphicsPsoDesc::new(root);
        desc.vs = Bytecode(m.bytes(8));
        desc.ps = Bytecode(m.bytes(24));
        desc.gs = Bytecode(m.bytes(72));
        let mut blend = BlendState {
            alpha_to_coverage: m.u32(120) != 0,
            independent: m.u32(124) != 0,
            ..BlendState::default()
        };
        for (i, target) in blend.targets.iter_mut().enumerate() {
            let at = 128 + 40 * i;
            *target = RenderTargetBlend {
                blend_enable: m.u32(at) != 0,
                logic_enable: m.u32(at + 4) != 0,
                src: m.u32(at + 8),
                dst: m.u32(at + 12),
                op: m.u32(at + 16),
                src_alpha: m.u32(at + 20),
                dst_alpha: m.u32(at + 24),
                op_alpha: m.u32(at + 28),
                logic_op: m.u32(at + 32),
                write_mask: m.u32(at + 36) as u8,
            };
        }
        desc.blend = blend;
        desc.sample_mask = m.u32(448);
        desc.raster = RasterizerState {
            fill: m.u32(452),
            cull: m.u32(456),
            front_ccw: m.u32(460) != 0,
            depth_bias: m.u32(464) as i32,
            depth_bias_clamp: m.f32(468),
            slope_scaled_bias: m.f32(472),
            depth_clip: m.u32(476) != 0,
            multisample: m.u32(480) != 0,
            forced_sample_count: m.u32(488),
            conservative: m.u32(492) != 0,
        };
        let stencil = m.u32(496 + 16);
        desc.depth_stencil = DepthStencilState {
            depth_enable: m.u32(496) != 0,
            depth_write: m.u32(500) != 0,
            depth_func: m.u32(504),
            stencil_enable: m.u32(508) != 0,
            stencil_read_mask: stencil as u8,
            stencil_write_mask: (stencil >> 8) as u8,
            front: stencil_face(m, 516),
            back: stencil_face(m, 532),
        };
        let (elements, count) = (m.ptr(552), m.u32(560) as usize);
        if !elements.is_null() {
            for i in 0..count {
                let e = Mem(elements.0.add(32 * i));
                let name = e.ptr(0);
                let mut text = Vec::new();
                let mut k = 0;
                while !name.is_null() && *name.0.add(k) != 0 && k < 64 {
                    text.push(*name.0.add(k));
                    k += 1;
                }
                desc.input_layout.push(InputElement {
                    semantic: String::from_utf8_lossy(&text).into_owned(),
                    semantic_index: e.u32(8),
                    format: e.u32(12),
                    slot: e.u32(16),
                    offset: e.u32(20),
                    class: e.u32(24),
                    step_rate: e.u32(28),
                });
            }
        }
        desc.strip_cut = m.u32(568);
        desc.topology = m.u32(572);
        let targets = (m.u32(576) as usize).min(8);
        desc.render_targets = (0..targets).map(|i| m.u32(580 + 4 * i)).collect();
        desc.depth_format = m.u32(612);
        desc.sample_count = m.u32(616).max(1);
        desc
    }
}

/// The root signature of a PSO description: its first word.
///
/// # Safety
/// `p` is a readable description.
pub unsafe fn pso_root(p: *const u8) -> *mut std::ffi::c_void {
    // SAFETY: as the caller promises.
    unsafe { Mem(p).u64(0) as usize as *mut std::ffi::c_void }
}

/// `D3D12_COMPUTE_PIPELINE_STATE_DESC`: root signature, then the shader bytecode.
///
/// # Safety
/// `p` is a readable description whose bytecode is readable.
pub unsafe fn read_compute_pso(p: *const u8, root: Option<Arc<RootSignature>>) -> ComputePsoDesc {
    // SAFETY: as the caller promises.
    ComputePsoDesc {
        root,
        cs: Bytecode(unsafe { Mem(p).bytes(8) }),
    }
}

/// The console's slot 67 description: the root signature, then pointer and size pairs for FSPX kinds 0, 1, 6, 7.
///
/// # Safety
/// `p` is a readable description whose four pairs name readable bytes.
pub unsafe fn read_slot67(p: *const u8) -> Slot67Parts {
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        Slot67Parts {
            dispatch_state: m.bytes(8),
            kind1: m.bytes(24),
            program: m.bytes(40),
            records: m.bytes(56),
        }
    }
}

/// `D3D12_VERSIONED_ROOT_SIGNATURE_DESC`.
///
/// # Safety
/// `p` is a readable description whose parameter, range and static sampler arrays are readable.
pub unsafe fn read_versioned_root_signature(p: *const u8) -> Option<RootSignature> {
    // SAFETY: as the caller promises.
    unsafe {
        let m = Mem(p);
        let version = match m.u32(0) {
            1 => RootVersion::V1_0,
            2 => RootVersion::V1_1,
            3 => RootVersion::V1_2,
            _ => return None,
        };
        let with_flags = version != RootVersion::V1_0;
        let body = Mem(p.add(8));
        let (count, params) = (body.u32(0) as usize, body.ptr(8));
        let (samplers, sampler_at) = (body.u32(16) as usize, body.ptr(24));
        let flags = body.u32(32);
        let mut parameters = Vec::new();
        for i in 0..count {
            let q = Mem(params.0.add(32 * i));
            let visibility = match q.u32(24) {
                0 => Visibility::All,
                1 => Visibility::Vertex,
                2 => Visibility::Hull,
                3 => Visibility::Domain,
                4 => Visibility::Geometry,
                5 => Visibility::Pixel,
                6 => Visibility::Amplification,
                7 => Visibility::Mesh,
                _ => return None,
            };
            let view = |q: Mem| (q.u32(8), q.u32(12), if with_flags { q.u32(16) } else { 0 });
            let kind = match q.u32(0) {
                0 => {
                    let (n, at) = (q.u32(8) as usize, q.ptr(16));
                    let stride = if with_flags { 24 } else { 20 };
                    let mut ranges = Vec::new();
                    for k in 0..n {
                        let r = Mem(at.0.add(stride * k));
                        ranges.push(DescriptorRange {
                            kind: match r.u32(0) {
                                0 => RangeType::Srv,
                                1 => RangeType::Uav,
                                2 => RangeType::Cbv,
                                _ => RangeType::Sampler,
                            },
                            count: r.u32(4),
                            base_register: r.u32(8),
                            space: r.u32(12),
                            flags: if with_flags { r.u32(16) } else { 0 },
                            offset: r.u32(if with_flags { 20 } else { 16 }),
                        });
                    }
                    ParameterKind::Table(ranges)
                }
                1 => ParameterKind::Constants {
                    register: q.u32(8),
                    space: q.u32(12),
                    values: q.u32(16),
                },
                2 => {
                    let (register, space, flags) = view(q);
                    ParameterKind::Cbv {
                        register,
                        space,
                        flags,
                    }
                }
                3 => {
                    let (register, space, flags) = view(q);
                    ParameterKind::Srv {
                        register,
                        space,
                        flags,
                    }
                }
                4 => {
                    let (register, space, flags) = view(q);
                    ParameterKind::Uav {
                        register,
                        space,
                        flags,
                    }
                }
                _ => return None,
            };
            parameters.push(RootParameter { kind, visibility });
        }
        let stride = if version == RootVersion::V1_2 { 56 } else { 52 };
        let mut static_samplers = Vec::new();
        for i in 0..samplers {
            let s = Mem(sampler_at.0.add(stride * i));
            static_samplers.push(StaticSampler {
                filter: s.u32(0),
                address_u: s.u32(4),
                address_v: s.u32(8),
                address_w: s.u32(12),
                mip_lod_bias: s.f32(16),
                max_anisotropy: s.u32(20),
                comparison: s.u32(24),
                border_color: s.u32(28),
                min_lod: s.f32(32),
                max_lod: s.f32(36),
                register: s.u32(40),
                space: s.u32(44),
                visibility: s.u32(48),
                flags: if stride == 56 { s.u32(52) } else { 0 },
            });
        }
        Some(RootSignature {
            version,
            parameters,
            static_samplers,
            flags,
        })
    }
}
