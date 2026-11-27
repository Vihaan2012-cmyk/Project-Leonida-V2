//! The render state of a graphics pipeline, with D3D12's numeric enum values, and how it maps to Vulkan.
//!
//! The structs hold the raw D3D12 values (`D3D12_BLEND`, `D3D12_COMPARISON_FUNC`, ...) so the code that reads a
//! `D3D12_GRAPHICS_PIPELINE_STATE_DESC` can fill them without a table of its own.

use ash::vk;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderTargetBlend {
    pub blend_enable: bool,
    pub logic_enable: bool,
    pub src: u32,
    pub dst: u32,
    pub op: u32,
    pub src_alpha: u32,
    pub dst_alpha: u32,
    pub op_alpha: u32,
    pub logic_op: u32,
    pub write_mask: u8,
}

impl Default for RenderTargetBlend {
    fn default() -> Self {
        RenderTargetBlend {
            blend_enable: false,
            logic_enable: false,
            src: 2,
            dst: 1,
            op: 1,
            src_alpha: 2,
            dst_alpha: 1,
            op_alpha: 1,
            logic_op: 4,
            write_mask: 0xF,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlendState {
    pub alpha_to_coverage: bool,
    pub independent: bool,
    pub targets: [RenderTargetBlend; 8],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterizerState {
    /// 2 wireframe, 3 solid.
    pub fill: u32,
    /// 1 none, 2 front, 3 back.
    pub cull: u32,
    pub front_ccw: bool,
    pub depth_bias: i32,
    pub depth_bias_clamp: f32,
    pub slope_scaled_bias: f32,
    pub depth_clip: bool,
    pub multisample: bool,
    pub forced_sample_count: u32,
    pub conservative: bool,
}

impl Default for RasterizerState {
    fn default() -> Self {
        RasterizerState {
            fill: 3,
            cull: 3,
            front_ccw: false,
            depth_bias: 0,
            depth_bias_clamp: 0.0,
            slope_scaled_bias: 0.0,
            depth_clip: true,
            multisample: false,
            forced_sample_count: 0,
            conservative: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StencilFace {
    pub fail: u32,
    pub depth_fail: u32,
    pub pass: u32,
    pub func: u32,
}

impl Default for StencilFace {
    fn default() -> Self {
        StencilFace {
            fail: 1,
            depth_fail: 1,
            pass: 1,
            func: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DepthStencilState {
    pub depth_enable: bool,
    pub depth_write: bool,
    /// 1 never ... 8 always.
    pub depth_func: u32,
    pub stencil_enable: bool,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front: StencilFace,
    pub back: StencilFace,
}

impl Default for DepthStencilState {
    fn default() -> Self {
        DepthStencilState {
            depth_enable: true,
            depth_write: true,
            depth_func: 2,
            stencil_enable: false,
            stencil_read_mask: 0xFF,
            stencil_write_mask: 0xFF,
            front: StencilFace::default(),
            back: StencilFace::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputElement {
    pub semantic: String,
    pub semantic_index: u32,
    /// A `DXGI_FORMAT` value.
    pub format: u32,
    pub slot: u32,
    /// `0xFFFF_FFFF` appends after the previous element of the slot.
    pub offset: u32,
    /// 0 per vertex, 1 per instance.
    pub class: u32,
    pub step_rate: u32,
}

/// 0 undefined, 1 point, 2 line, 3 triangle, 4 patch.
pub type TopologyType = u32;

pub fn vk_blend_factor(value: u32) -> vk::BlendFactor {
    match value {
        1 => vk::BlendFactor::ZERO,
        3 => vk::BlendFactor::SRC_COLOR,
        4 => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
        5 => vk::BlendFactor::SRC_ALPHA,
        6 => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        7 => vk::BlendFactor::DST_ALPHA,
        8 => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
        9 => vk::BlendFactor::DST_COLOR,
        10 => vk::BlendFactor::ONE_MINUS_DST_COLOR,
        11 => vk::BlendFactor::SRC_ALPHA_SATURATE,
        14 => vk::BlendFactor::CONSTANT_COLOR,
        15 => vk::BlendFactor::ONE_MINUS_CONSTANT_COLOR,
        16 => vk::BlendFactor::SRC1_COLOR,
        17 => vk::BlendFactor::ONE_MINUS_SRC1_COLOR,
        18 => vk::BlendFactor::SRC1_ALPHA,
        19 => vk::BlendFactor::ONE_MINUS_SRC1_ALPHA,
        20 => vk::BlendFactor::CONSTANT_ALPHA,
        21 => vk::BlendFactor::ONE_MINUS_CONSTANT_ALPHA,
        _ => vk::BlendFactor::ONE,
    }
}

pub fn vk_blend_op(value: u32) -> vk::BlendOp {
    match value {
        2 => vk::BlendOp::SUBTRACT,
        3 => vk::BlendOp::REVERSE_SUBTRACT,
        4 => vk::BlendOp::MIN,
        5 => vk::BlendOp::MAX,
        _ => vk::BlendOp::ADD,
    }
}

pub fn vk_logic_op(value: u32) -> vk::LogicOp {
    match value {
        0 => vk::LogicOp::CLEAR,
        1 => vk::LogicOp::SET,
        2 => vk::LogicOp::COPY,
        3 => vk::LogicOp::COPY_INVERTED,
        5 => vk::LogicOp::INVERT,
        6 => vk::LogicOp::AND,
        7 => vk::LogicOp::NAND,
        8 => vk::LogicOp::OR,
        9 => vk::LogicOp::NOR,
        10 => vk::LogicOp::XOR,
        11 => vk::LogicOp::EQUIVALENT,
        12 => vk::LogicOp::AND_REVERSE,
        13 => vk::LogicOp::AND_INVERTED,
        14 => vk::LogicOp::OR_REVERSE,
        15 => vk::LogicOp::OR_INVERTED,
        _ => vk::LogicOp::NO_OP,
    }
}

pub fn vk_compare(value: u32) -> vk::CompareOp {
    match value {
        1 => vk::CompareOp::NEVER,
        2 => vk::CompareOp::LESS,
        3 => vk::CompareOp::EQUAL,
        4 => vk::CompareOp::LESS_OR_EQUAL,
        5 => vk::CompareOp::GREATER,
        6 => vk::CompareOp::NOT_EQUAL,
        7 => vk::CompareOp::GREATER_OR_EQUAL,
        _ => vk::CompareOp::ALWAYS,
    }
}

pub fn vk_stencil_op(value: u32) -> vk::StencilOp {
    match value {
        2 => vk::StencilOp::ZERO,
        3 => vk::StencilOp::REPLACE,
        4 => vk::StencilOp::INCREMENT_AND_CLAMP,
        5 => vk::StencilOp::DECREMENT_AND_CLAMP,
        6 => vk::StencilOp::INVERT,
        7 => vk::StencilOp::INCREMENT_AND_WRAP,
        8 => vk::StencilOp::DECREMENT_AND_WRAP,
        _ => vk::StencilOp::KEEP,
    }
}

/// A D3D primitive topology (`D3D_PRIMITIVE_TOPOLOGY`) as Vulkan's.
pub fn vk_topology(value: u32) -> Option<vk::PrimitiveTopology> {
    Some(match value {
        1 => vk::PrimitiveTopology::POINT_LIST,
        2 => vk::PrimitiveTopology::LINE_LIST,
        3 => vk::PrimitiveTopology::LINE_STRIP,
        4 => vk::PrimitiveTopology::TRIANGLE_LIST,
        5 => vk::PrimitiveTopology::TRIANGLE_STRIP,
        10 => vk::PrimitiveTopology::LINE_LIST_WITH_ADJACENCY,
        11 => vk::PrimitiveTopology::LINE_STRIP_WITH_ADJACENCY,
        12 => vk::PrimitiveTopology::TRIANGLE_LIST_WITH_ADJACENCY,
        13 => vk::PrimitiveTopology::TRIANGLE_STRIP_WITH_ADJACENCY,
        33..=64 => vk::PrimitiveTopology::PATCH_LIST,
        _ => return None,
    })
}

/// The topology a pipeline is created with for a `D3D12_PRIMITIVE_TOPOLOGY_TYPE`; draws set the exact one.
pub fn default_topology(kind: TopologyType, geometry: bool) -> vk::PrimitiveTopology {
    match (kind, geometry) {
        (1, _) => vk::PrimitiveTopology::POINT_LIST,
        (2, _) => vk::PrimitiveTopology::LINE_LIST,
        _ => vk::PrimitiveTopology::TRIANGLE_LIST,
    }
}

pub fn is_strip(topology: vk::PrimitiveTopology) -> bool {
    matches!(
        topology,
        vk::PrimitiveTopology::LINE_STRIP
            | vk::PrimitiveTopology::TRIANGLE_STRIP
            | vk::PrimitiveTopology::LINE_STRIP_WITH_ADJACENCY
            | vk::PrimitiveTopology::TRIANGLE_STRIP_WITH_ADJACENCY
    )
}

/// The Vulkan format of a `DXGI_FORMAT`, for the formats render targets, depth buffers and vertex attributes use.
/// Typeless formats take their unsigned-normalized or float reading.
pub fn dxgi_to_vk(format: u32) -> Option<vk::Format> {
    use vk::Format as F;
    Some(match format {
        1..=2 => F::R32G32B32A32_SFLOAT,
        3 => F::R32G32B32A32_UINT,
        4 => F::R32G32B32A32_SINT,
        5..=6 => F::R32G32B32_SFLOAT,
        7 => F::R32G32B32_UINT,
        8 => F::R32G32B32_SINT,
        9 | 11 => F::R16G16B16A16_UNORM,
        10 => F::R16G16B16A16_SFLOAT,
        12 => F::R16G16B16A16_UINT,
        13 => F::R16G16B16A16_SNORM,
        14 => F::R16G16B16A16_SINT,
        15..=16 => F::R32G32_SFLOAT,
        17 => F::R32G32_UINT,
        18 => F::R32G32_SINT,
        19 | 20 => F::D32_SFLOAT_S8_UINT,
        21 => F::D32_SFLOAT_S8_UINT,
        23..=24 => F::A2B10G10R10_UNORM_PACK32,
        25 => F::A2B10G10R10_UINT_PACK32,
        26 => F::B10G11R11_UFLOAT_PACK32,
        27 | 28 => F::R8G8B8A8_UNORM,
        29 => F::R8G8B8A8_SRGB,
        30 => F::R8G8B8A8_UINT,
        31 => F::R8G8B8A8_SNORM,
        32 => F::R8G8B8A8_SINT,
        33 | 35 => F::R16G16_UNORM,
        34 => F::R16G16_SFLOAT,
        36 => F::R16G16_UINT,
        37 => F::R16G16_SNORM,
        38 => F::R16G16_SINT,
        39 | 41 => F::R32_SFLOAT,
        40 => F::D32_SFLOAT,
        42 => F::R32_UINT,
        43 => F::R32_SINT,
        44..=47 => F::D24_UNORM_S8_UINT,
        48 | 49 => F::R8G8_UNORM,
        50 => F::R8G8_UINT,
        51 => F::R8G8_SNORM,
        52 => F::R8G8_SINT,
        53 | 56 => F::R16_UNORM,
        54 => F::R16_SFLOAT,
        55 => F::D16_UNORM,
        57 => F::R16_UINT,
        58 => F::R16_SNORM,
        59 => F::R16_SINT,
        60 | 61 => F::R8_UNORM,
        62 => F::R8_UINT,
        63 => F::R8_SNORM,
        64 => F::R8_SINT,
        65 => F::A8_UNORM_KHR,
        67 => F::E5B9G9R9_UFLOAT_PACK32,
        87..=88 | 90 => F::B8G8R8A8_UNORM,
        91 => F::B8G8R8A8_SRGB,
        _ => return None,
    })
}

/// Bytes in one texel, or in one block of a block-compressed format.
pub fn format_bytes(format: vk::Format) -> u32 {
    use vk::Format as F;
    match format {
        F::R8_UNORM | F::R8_SNORM | F::R8_UINT | F::R8_SINT | F::A8_UNORM_KHR => 1,
        F::R8G8_UNORM | F::R8G8_SNORM | F::R8G8_UINT | F::R8G8_SINT => 2,
        F::R16_UNORM | F::R16_SNORM | F::R16_UINT | F::R16_SINT | F::R16_SFLOAT => 2,
        F::D16_UNORM => 2,
        F::R16G16_UNORM | F::R16G16_SNORM | F::R16G16_UINT | F::R16G16_SINT | F::R16G16_SFLOAT => 4,
        F::R32_UINT | F::R32_SINT | F::R32_SFLOAT | F::D32_SFLOAT | F::D24_UNORM_S8_UINT => 4,
        F::R8G8B8A8_UNORM
        | F::R8G8B8A8_SRGB
        | F::R8G8B8A8_SNORM
        | F::R8G8B8A8_UINT
        | F::R8G8B8A8_SINT
        | F::B8G8R8A8_UNORM
        | F::B8G8R8A8_SRGB
        | F::A2B10G10R10_UNORM_PACK32
        | F::A2B10G10R10_UINT_PACK32
        | F::B10G11R11_UFLOAT_PACK32
        | F::E5B9G9R9_UFLOAT_PACK32 => 4,
        F::R16G16B16A16_UNORM
        | F::R16G16B16A16_SNORM
        | F::R16G16B16A16_UINT
        | F::R16G16B16A16_SINT
        | F::R16G16B16A16_SFLOAT
        | F::R32G32_UINT
        | F::R32G32_SINT
        | F::R32G32_SFLOAT
        | F::D32_SFLOAT_S8_UINT => 8,
        F::R32G32B32_UINT | F::R32G32B32_SINT | F::R32G32B32_SFLOAT => 12,
        F::R32G32B32A32_UINT | F::R32G32B32A32_SINT | F::R32G32B32A32_SFLOAT => 16,
        _ => 4,
    }
}

pub fn is_depth_format(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::D16_UNORM
            | vk::Format::D32_SFLOAT
            | vk::Format::D24_UNORM_S8_UINT
            | vk::Format::D32_SFLOAT_S8_UINT
    )
}

pub fn has_stencil(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::D24_UNORM_S8_UINT | vk::Format::D32_SFLOAT_S8_UINT
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_formats_map() {
        assert_eq!(dxgi_to_vk(28), Some(vk::Format::R8G8B8A8_UNORM));
        assert_eq!(dxgi_to_vk(2), Some(vk::Format::R32G32B32A32_SFLOAT));
        assert_eq!(dxgi_to_vk(40), Some(vk::Format::D32_SFLOAT));
        assert_eq!(dxgi_to_vk(0), None);
        assert!(is_depth_format(vk::Format::D32_SFLOAT));
        assert_eq!(format_bytes(vk::Format::R32G32B32_SFLOAT), 12);
        assert_eq!(format_bytes(vk::Format::R16G16B16A16_SFLOAT), 8);
        assert!(has_stencil(vk::Format::D24_UNORM_S8_UINT));
    }

    #[test]
    fn default_blend_is_a_pass_through() {
        let state = BlendState::default();
        assert!(!state.targets[0].blend_enable);
        assert_eq!(vk_blend_factor(state.targets[0].src), vk::BlendFactor::ONE);
        assert_eq!(vk_blend_factor(state.targets[0].dst), vk::BlendFactor::ZERO);
        assert_eq!(vk_blend_op(1), vk::BlendOp::ADD);
    }

    #[test]
    fn topologies() {
        assert_eq!(vk_topology(4), Some(vk::PrimitiveTopology::TRIANGLE_LIST));
        assert_eq!(vk_topology(35), Some(vk::PrimitiveTopology::PATCH_LIST));
        assert_eq!(vk_topology(0), None);
        assert!(is_strip(vk::PrimitiveTopology::TRIANGLE_STRIP));
        assert!(!is_strip(vk::PrimitiveTopology::TRIANGLE_LIST));
    }
}
