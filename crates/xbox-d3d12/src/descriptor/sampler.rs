//! `D3D12_SAMPLER_DESC` to the sampler descriptor (S#).

use super::gfx10::{AddressMode, Border, Compare, Filter, MipFilter, Reduction, Sampler};

/// `D3D12_FILTER` is bit fields: the mip filter in bit 0, magnification in bit 2, minimisation in bit 4, anisotropy
/// in bit 6 and the reduction (0 weighted average, 1 comparison, 2 minimum, 3 maximum) in bits 7 and 8.
const MIP_LINEAR: u32 = 1;
const MAG_LINEAR: u32 = 1 << 2;
const MIN_LINEAR: u32 = 1 << 4;
const ANISOTROPIC: u32 = 1 << 6;

pub const FLAG_UINT_BORDER_COLOR: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerDesc {
    pub filter: u32,
    /// `D3D12_TEXTURE_ADDRESS_MODE`: wrap 1, mirror 2, clamp 3, border 4, mirror once 5.
    pub address_u: u32,
    pub address_v: u32,
    pub address_w: u32,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    /// `D3D12_COMPARISON_FUNC`: never 1 up to always 8.
    pub comparison_func: u32,
    pub border_color: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
    pub flags: u32,
}

impl Default for SamplerDesc {
    fn default() -> SamplerDesc {
        SamplerDesc {
            filter: 0,
            address_u: 1,
            address_v: 1,
            address_w: 1,
            mip_lod_bias: 0.0,
            max_anisotropy: 1,
            comparison_func: 1,
            border_color: [0.0; 4],
            min_lod: 0.0,
            max_lod: f32::MAX,
            flags: 0,
        }
    }
}

fn address(mode: u32) -> AddressMode {
    match mode {
        2 => AddressMode::Mirror,
        3 => AddressMode::ClampLastTexel,
        4 => AddressMode::ClampBorder,
        5 => AddressMode::MirrorOnceLastTexel,
        _ => AddressMode::Wrap,
    }
}

/// Border colours kept for samplers whose colour is none of the three the descriptor can name; the descriptor
/// carries the index.
#[derive(Debug, Default)]
pub struct BorderTable(Vec<[f32; 4]>);

impl BorderTable {
    pub fn get(&self, index: u32) -> Option<[f32; 4]> {
        self.0.get(index as usize).copied()
    }

    fn index_of(&mut self, color: [f32; 4]) -> u32 {
        let bits = |c: [f32; 4]| c.map(f32::to_bits);
        match self.0.iter().position(|known| bits(*known) == bits(color)) {
            Some(index) => index as u32,
            None => {
                self.0.push(color);
                (self.0.len() - 1) as u32
            }
        }
    }
}

pub fn sampler(desc: &SamplerDesc, borders: &mut BorderTable) -> Sampler {
    let reduction = (desc.filter >> 7) & 3;
    let aniso = desc.filter & ANISOTROPIC != 0;
    let linear = |bit: u32| desc.filter & bit != 0 || aniso;
    let pick = |bit: u32| match (linear(bit), aniso) {
        (true, true) => Filter::AnisoBilinear,
        (true, false) => Filter::Bilinear,
        (false, _) => Filter::Point,
    };
    let comparison = reduction == 1;
    let function = desc.comparison_func.clamp(1, 8) - 1;
    let mut s = Sampler {
        address_u: address(desc.address_u),
        address_v: address(desc.address_v),
        address_w: address(desc.address_w),
        mag: pick(MAG_LINEAR),
        min: pick(MIN_LINEAR),
        mip: if linear(MIP_LINEAR) {
            MipFilter::Linear
        } else {
            MipFilter::Point
        },
        max_anisotropy: if aniso {
            desc.max_anisotropy.clamp(1, 16).next_power_of_two().min(16)
        } else {
            1
        },
        min_lod: desc.min_lod,
        max_lod: desc.max_lod,
        lod_bias: desc.mip_lod_bias,
        reduction: match reduction {
            2 => Reduction::Minimum,
            3 => Reduction::Maximum,
            _ => Reduction::Blend,
        },
        integer_border: desc.flags & FLAG_UINT_BORDER_COLOR != 0,
        ..Sampler::default()
    };
    if comparison {
        s.compare = [
            Compare::Never,
            Compare::Less,
            Compare::Equal,
            Compare::LessEqual,
            Compare::Greater,
            Compare::NotEqual,
            Compare::GreaterEqual,
            Compare::Always,
        ][function as usize];
        s.compare_never = function == 0;
    }
    let uses_border = [desc.address_u, desc.address_v, desc.address_w].contains(&4);
    if uses_border {
        match desc.border_color {
            [0.0, 0.0, 0.0, 0.0] => s.border = Border::TransparentBlack,
            [0.0, 0.0, 0.0, 1.0] => s.border = Border::OpaqueBlack,
            [1.0, 1.0, 1.0, 1.0] => s.border = Border::OpaqueWhite,
            custom => {
                s.border = Border::Table;
                s.border_index = borders.index_of(custom) & 0xFFF;
            }
        }
    }
    s
}
