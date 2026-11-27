//! `DXGI_FORMAT` to the console's image format code (`FORMAT` of a T#), the channel order that reads the format as
//! red, green, blue, alpha, the Vulkan format and the layout facts a surface needs.

use ash::vk;

use super::gfx10::{
    IDENTITY, SELECT_ONE, SELECT_W, SELECT_X, SELECT_Y, SELECT_Z, SELECT_ZERO, Swizzle,
};

pub mod dxgi {
    pub const UNKNOWN: u32 = 0;
    pub const R32G32B32A32_TYPELESS: u32 = 1;
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R32G32B32A32_UINT: u32 = 3;
    pub const R32G32B32A32_SINT: u32 = 4;
    pub const R32G32B32_TYPELESS: u32 = 5;
    pub const R32G32B32_FLOAT: u32 = 6;
    pub const R32G32B32_UINT: u32 = 7;
    pub const R32G32B32_SINT: u32 = 8;
    pub const R16G16B16A16_TYPELESS: u32 = 9;
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R16G16B16A16_UNORM: u32 = 11;
    pub const R16G16B16A16_UINT: u32 = 12;
    pub const R16G16B16A16_SNORM: u32 = 13;
    pub const R16G16B16A16_SINT: u32 = 14;
    pub const R32G32_TYPELESS: u32 = 15;
    pub const R32G32_FLOAT: u32 = 16;
    pub const R32G32_UINT: u32 = 17;
    pub const R32G32_SINT: u32 = 18;
    pub const R32G8X24_TYPELESS: u32 = 19;
    pub const D32_FLOAT_S8X24_UINT: u32 = 20;
    pub const R32_FLOAT_X8X24_TYPELESS: u32 = 21;
    pub const X32_TYPELESS_G8X24_UINT: u32 = 22;
    pub const R10G10B10A2_TYPELESS: u32 = 23;
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R10G10B10A2_UINT: u32 = 25;
    pub const R11G11B10_FLOAT: u32 = 26;
    pub const R8G8B8A8_TYPELESS: u32 = 27;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const R8G8B8A8_UINT: u32 = 30;
    pub const R8G8B8A8_SNORM: u32 = 31;
    pub const R8G8B8A8_SINT: u32 = 32;
    pub const R16G16_TYPELESS: u32 = 33;
    pub const R16G16_FLOAT: u32 = 34;
    pub const R16G16_UNORM: u32 = 35;
    pub const R16G16_UINT: u32 = 36;
    pub const R16G16_SNORM: u32 = 37;
    pub const R16G16_SINT: u32 = 38;
    pub const R32_TYPELESS: u32 = 39;
    pub const D32_FLOAT: u32 = 40;
    pub const R32_FLOAT: u32 = 41;
    pub const R32_UINT: u32 = 42;
    pub const R32_SINT: u32 = 43;
    pub const R24G8_TYPELESS: u32 = 44;
    pub const D24_UNORM_S8_UINT: u32 = 45;
    pub const R24_UNORM_X8_TYPELESS: u32 = 46;
    pub const X24_TYPELESS_G8_UINT: u32 = 47;
    pub const R8G8_TYPELESS: u32 = 48;
    pub const R8G8_UNORM: u32 = 49;
    pub const R8G8_UINT: u32 = 50;
    pub const R8G8_SNORM: u32 = 51;
    pub const R8G8_SINT: u32 = 52;
    pub const R16_TYPELESS: u32 = 53;
    pub const R16_FLOAT: u32 = 54;
    pub const D16_UNORM: u32 = 55;
    pub const R16_UNORM: u32 = 56;
    pub const R16_UINT: u32 = 57;
    pub const R16_SNORM: u32 = 58;
    pub const R16_SINT: u32 = 59;
    pub const R8_TYPELESS: u32 = 60;
    pub const R8_UNORM: u32 = 61;
    pub const R8_UINT: u32 = 62;
    pub const R8_SNORM: u32 = 63;
    pub const R8_SINT: u32 = 64;
    pub const A8_UNORM: u32 = 65;
    pub const R9G9B9E5_SHAREDEXP: u32 = 67;
    pub const BC1_TYPELESS: u32 = 70;
    pub const BC1_UNORM: u32 = 71;
    pub const BC1_UNORM_SRGB: u32 = 72;
    pub const BC2_TYPELESS: u32 = 73;
    pub const BC2_UNORM: u32 = 74;
    pub const BC2_UNORM_SRGB: u32 = 75;
    pub const BC3_TYPELESS: u32 = 76;
    pub const BC3_UNORM: u32 = 77;
    pub const BC3_UNORM_SRGB: u32 = 78;
    pub const BC4_TYPELESS: u32 = 79;
    pub const BC4_UNORM: u32 = 80;
    pub const BC4_SNORM: u32 = 81;
    pub const BC5_TYPELESS: u32 = 82;
    pub const BC5_UNORM: u32 = 83;
    pub const BC5_SNORM: u32 = 84;
    pub const B5G6R5_UNORM: u32 = 85;
    pub const B5G5R5A1_UNORM: u32 = 86;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const B8G8R8X8_UNORM: u32 = 88;
    pub const B8G8R8A8_TYPELESS: u32 = 90;
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
    pub const B8G8R8X8_TYPELESS: u32 = 92;
    pub const B8G8R8X8_UNORM_SRGB: u32 = 93;
    pub const BC6H_TYPELESS: u32 = 94;
    pub const BC6H_UF16: u32 = 95;
    pub const BC6H_SF16: u32 = 96;
    pub const BC7_TYPELESS: u32 = 97;
    pub const BC7_UNORM: u32 = 98;
    pub const BC7_UNORM_SRGB: u32 = 99;
    pub const B4G4R4A4_UNORM: u32 = 115;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Color,
    Depth,
    DepthStencil,
    /// A shader view of a depth-stencil texture's depth plane.
    DepthPlane,
    /// A shader view of a depth-stencil texture's stencil plane.
    StencilPlane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub dxgi: u32,
    /// `FORMAT` of a T#, 9 bits; below 128 also usable in a V#.
    pub gfx10: u32,
    /// The `DST_SEL` that reads this format's channels as red, green, blue, alpha.
    pub channels: Swizzle,
    pub vulkan: vk::Format,
    /// Texels across and down one block; 1 by 1 unless compressed.
    pub block: [u32; 2],
    /// Bytes a block.
    pub bytes: u32,
    pub kind: Kind,
    pub typeless: bool,
}

const R: Swizzle = [SELECT_X, SELECT_ZERO, SELECT_ZERO, SELECT_ONE];
const RG: Swizzle = [SELECT_X, SELECT_Y, SELECT_ZERO, SELECT_ONE];
const RGB: Swizzle = [SELECT_X, SELECT_Y, SELECT_Z, SELECT_ONE];
const BGR: Swizzle = [SELECT_Z, SELECT_Y, SELECT_X, SELECT_ONE];
const BGRA: Swizzle = [SELECT_Z, SELECT_Y, SELECT_X, SELECT_W];
const ALPHA: Swizzle = [SELECT_ZERO, SELECT_ZERO, SELECT_ZERO, SELECT_X];

const fn color(dxgi: u32, gfx10: u32, channels: Swizzle, vulkan: vk::Format, bytes: u32) -> Format {
    Format {
        dxgi,
        gfx10,
        channels,
        vulkan,
        block: [1, 1],
        bytes,
        kind: Kind::Color,
        typeless: false,
    }
}

const fn typeless(dxgi: u32, like: Format) -> Format {
    Format {
        dxgi,
        typeless: true,
        ..like
    }
}

const fn block(dxgi: u32, gfx10: u32, channels: Swizzle, vulkan: vk::Format, bytes: u32) -> Format {
    Format {
        block: [4, 4],
        ..color(dxgi, gfx10, channels, vulkan, bytes)
    }
}

const fn special(dxgi: u32, gfx10: u32, vulkan: vk::Format, bytes: u32, kind: Kind) -> Format {
    Format {
        kind,
        ..color(dxgi, gfx10, R, vulkan, bytes)
    }
}

use vk::Format as V;

const RGBA32_FLOAT: Format = color(
    dxgi::R32G32B32A32_FLOAT,
    77,
    IDENTITY,
    V::R32G32B32A32_SFLOAT,
    16,
);
const RGB32_FLOAT: Format = color(dxgi::R32G32B32_FLOAT, 74, RGB, V::R32G32B32_SFLOAT, 12);
const RGBA16_UNORM: Format = color(
    dxgi::R16G16B16A16_UNORM,
    65,
    IDENTITY,
    V::R16G16B16A16_UNORM,
    8,
);
const RG32_FLOAT: Format = color(dxgi::R32G32_FLOAT, 64, RG, V::R32G32_SFLOAT, 8);
const RGB10A2_UNORM: Format = color(
    dxgi::R10G10B10A2_UNORM,
    50,
    IDENTITY,
    V::A2B10G10R10_UNORM_PACK32,
    4,
);
const RGBA8_UNORM: Format = color(dxgi::R8G8B8A8_UNORM, 56, IDENTITY, V::R8G8B8A8_UNORM, 4);
const RG16_FLOAT: Format = color(dxgi::R16G16_FLOAT, 29, RG, V::R16G16_SFLOAT, 4);
const R32_FLOAT: Format = color(dxgi::R32_FLOAT, 22, R, V::R32_SFLOAT, 4);
const RG8_UNORM: Format = color(dxgi::R8G8_UNORM, 14, RG, V::R8G8_UNORM, 2);
const R16_FLOAT: Format = color(dxgi::R16_FLOAT, 13, R, V::R16_SFLOAT, 2);
const R8_UNORM: Format = color(dxgi::R8_UNORM, 1, R, V::R8_UNORM, 1);
const BGRA8_UNORM: Format = color(dxgi::B8G8R8A8_UNORM, 56, BGRA, V::B8G8R8A8_UNORM, 4);
const BGRX8_UNORM: Format = color(dxgi::B8G8R8X8_UNORM, 56, BGR, V::B8G8R8A8_UNORM, 4);
const BC1_UNORM: Format = block(dxgi::BC1_UNORM, 169, IDENTITY, V::BC1_RGBA_UNORM_BLOCK, 8);
const BC2_UNORM: Format = block(dxgi::BC2_UNORM, 171, IDENTITY, V::BC2_UNORM_BLOCK, 16);
const BC3_UNORM: Format = block(dxgi::BC3_UNORM, 173, IDENTITY, V::BC3_UNORM_BLOCK, 16);
const BC4_UNORM: Format = block(dxgi::BC4_UNORM, 175, R, V::BC4_UNORM_BLOCK, 8);
const BC5_UNORM: Format = block(dxgi::BC5_UNORM, 177, RG, V::BC5_UNORM_BLOCK, 16);
const BC6H_UF16: Format = block(dxgi::BC6H_UF16, 179, RGB, V::BC6H_UFLOAT_BLOCK, 16);
const BC7_UNORM: Format = block(dxgi::BC7_UNORM, 181, IDENTITY, V::BC7_UNORM_BLOCK, 16);

pub static FORMATS: &[Format] = &[
    typeless(dxgi::R32G32B32A32_TYPELESS, RGBA32_FLOAT),
    RGBA32_FLOAT,
    color(
        dxgi::R32G32B32A32_UINT,
        75,
        IDENTITY,
        V::R32G32B32A32_UINT,
        16,
    ),
    color(
        dxgi::R32G32B32A32_SINT,
        76,
        IDENTITY,
        V::R32G32B32A32_SINT,
        16,
    ),
    typeless(dxgi::R32G32B32_TYPELESS, RGB32_FLOAT),
    RGB32_FLOAT,
    color(dxgi::R32G32B32_UINT, 72, RGB, V::R32G32B32_UINT, 12),
    color(dxgi::R32G32B32_SINT, 73, RGB, V::R32G32B32_SINT, 12),
    typeless(dxgi::R16G16B16A16_TYPELESS, RGBA16_UNORM),
    color(
        dxgi::R16G16B16A16_FLOAT,
        71,
        IDENTITY,
        V::R16G16B16A16_SFLOAT,
        8,
    ),
    RGBA16_UNORM,
    color(
        dxgi::R16G16B16A16_UINT,
        69,
        IDENTITY,
        V::R16G16B16A16_UINT,
        8,
    ),
    color(
        dxgi::R16G16B16A16_SNORM,
        66,
        IDENTITY,
        V::R16G16B16A16_SNORM,
        8,
    ),
    color(
        dxgi::R16G16B16A16_SINT,
        70,
        IDENTITY,
        V::R16G16B16A16_SINT,
        8,
    ),
    typeless(dxgi::R32G32_TYPELESS, RG32_FLOAT),
    RG32_FLOAT,
    color(dxgi::R32G32_UINT, 62, RG, V::R32G32_UINT, 8),
    color(dxgi::R32G32_SINT, 63, RG, V::R32G32_SINT, 8),
    Format {
        typeless: true,
        ..special(
            dxgi::R32G8X24_TYPELESS,
            146,
            V::D32_SFLOAT_S8_UINT,
            8,
            Kind::DepthStencil,
        )
    },
    special(
        dxgi::D32_FLOAT_S8X24_UINT,
        146,
        V::D32_SFLOAT_S8_UINT,
        8,
        Kind::DepthStencil,
    ),
    special(
        dxgi::R32_FLOAT_X8X24_TYPELESS,
        146,
        V::D32_SFLOAT_S8_UINT,
        8,
        Kind::DepthPlane,
    ),
    special(
        dxgi::X32_TYPELESS_G8X24_UINT,
        5,
        V::D32_SFLOAT_S8_UINT,
        8,
        Kind::StencilPlane,
    ),
    typeless(dxgi::R10G10B10A2_TYPELESS, RGB10A2_UNORM),
    RGB10A2_UNORM,
    color(
        dxgi::R10G10B10A2_UINT,
        54,
        IDENTITY,
        V::A2B10G10R10_UINT_PACK32,
        4,
    ),
    color(
        dxgi::R11G11B10_FLOAT,
        36,
        RGB,
        V::B10G11R11_UFLOAT_PACK32,
        4,
    ),
    typeless(dxgi::R8G8B8A8_TYPELESS, RGBA8_UNORM),
    RGBA8_UNORM,
    color(
        dxgi::R8G8B8A8_UNORM_SRGB,
        130,
        IDENTITY,
        V::R8G8B8A8_SRGB,
        4,
    ),
    color(dxgi::R8G8B8A8_UINT, 60, IDENTITY, V::R8G8B8A8_UINT, 4),
    color(dxgi::R8G8B8A8_SNORM, 57, IDENTITY, V::R8G8B8A8_SNORM, 4),
    color(dxgi::R8G8B8A8_SINT, 61, IDENTITY, V::R8G8B8A8_SINT, 4),
    typeless(dxgi::R16G16_TYPELESS, RG16_FLOAT),
    RG16_FLOAT,
    color(dxgi::R16G16_UNORM, 23, RG, V::R16G16_UNORM, 4),
    color(dxgi::R16G16_UINT, 27, RG, V::R16G16_UINT, 4),
    color(dxgi::R16G16_SNORM, 24, RG, V::R16G16_SNORM, 4),
    color(dxgi::R16G16_SINT, 28, RG, V::R16G16_SINT, 4),
    typeless(dxgi::R32_TYPELESS, R32_FLOAT),
    special(dxgi::D32_FLOAT, 22, V::D32_SFLOAT, 4, Kind::Depth),
    R32_FLOAT,
    color(dxgi::R32_UINT, 20, R, V::R32_UINT, 4),
    color(dxgi::R32_SINT, 21, R, V::R32_SINT, 4),
    Format {
        typeless: true,
        ..special(
            dxgi::R24G8_TYPELESS,
            141,
            V::D24_UNORM_S8_UINT,
            4,
            Kind::DepthStencil,
        )
    },
    special(
        dxgi::D24_UNORM_S8_UINT,
        141,
        V::D24_UNORM_S8_UINT,
        4,
        Kind::DepthStencil,
    ),
    special(
        dxgi::R24_UNORM_X8_TYPELESS,
        141,
        V::D24_UNORM_S8_UINT,
        4,
        Kind::DepthPlane,
    ),
    special(
        dxgi::X24_TYPELESS_G8_UINT,
        5,
        V::D24_UNORM_S8_UINT,
        4,
        Kind::StencilPlane,
    ),
    typeless(dxgi::R8G8_TYPELESS, RG8_UNORM),
    RG8_UNORM,
    color(dxgi::R8G8_UINT, 18, RG, V::R8G8_UINT, 2),
    color(dxgi::R8G8_SNORM, 15, RG, V::R8G8_SNORM, 2),
    color(dxgi::R8G8_SINT, 19, RG, V::R8G8_SINT, 2),
    typeless(dxgi::R16_TYPELESS, R16_FLOAT),
    R16_FLOAT,
    special(dxgi::D16_UNORM, 7, V::D16_UNORM, 2, Kind::Depth),
    color(dxgi::R16_UNORM, 7, R, V::R16_UNORM, 2),
    color(dxgi::R16_UINT, 11, R, V::R16_UINT, 2),
    color(dxgi::R16_SNORM, 8, R, V::R16_SNORM, 2),
    color(dxgi::R16_SINT, 12, R, V::R16_SINT, 2),
    typeless(dxgi::R8_TYPELESS, R8_UNORM),
    R8_UNORM,
    color(dxgi::R8_UINT, 5, R, V::R8_UINT, 1),
    color(dxgi::R8_SNORM, 2, R, V::R8_SNORM, 1),
    color(dxgi::R8_SINT, 6, R, V::R8_SINT, 1),
    color(dxgi::A8_UNORM, 1, ALPHA, V::R8_UNORM, 1),
    color(
        dxgi::R9G9B9E5_SHAREDEXP,
        132,
        RGB,
        V::E5B9G9R9_UFLOAT_PACK32,
        4,
    ),
    typeless(dxgi::BC1_TYPELESS, BC1_UNORM),
    BC1_UNORM,
    block(
        dxgi::BC1_UNORM_SRGB,
        170,
        IDENTITY,
        V::BC1_RGBA_SRGB_BLOCK,
        8,
    ),
    typeless(dxgi::BC2_TYPELESS, BC2_UNORM),
    BC2_UNORM,
    block(dxgi::BC2_UNORM_SRGB, 172, IDENTITY, V::BC2_SRGB_BLOCK, 16),
    typeless(dxgi::BC3_TYPELESS, BC3_UNORM),
    BC3_UNORM,
    block(dxgi::BC3_UNORM_SRGB, 174, IDENTITY, V::BC3_SRGB_BLOCK, 16),
    typeless(dxgi::BC4_TYPELESS, BC4_UNORM),
    BC4_UNORM,
    block(dxgi::BC4_SNORM, 176, R, V::BC4_SNORM_BLOCK, 8),
    typeless(dxgi::BC5_TYPELESS, BC5_UNORM),
    BC5_UNORM,
    block(dxgi::BC5_SNORM, 178, RG, V::BC5_SNORM_BLOCK, 16),
    color(dxgi::B5G6R5_UNORM, 133, BGR, V::R5G6B5_UNORM_PACK16, 2),
    color(dxgi::B5G5R5A1_UNORM, 134, BGRA, V::A1R5G5B5_UNORM_PACK16, 2),
    BGRA8_UNORM,
    BGRX8_UNORM,
    typeless(dxgi::B8G8R8A8_TYPELESS, BGRA8_UNORM),
    color(dxgi::B8G8R8A8_UNORM_SRGB, 130, BGRA, V::B8G8R8A8_SRGB, 4),
    typeless(dxgi::B8G8R8X8_TYPELESS, BGRX8_UNORM),
    color(dxgi::B8G8R8X8_UNORM_SRGB, 130, BGR, V::B8G8R8A8_SRGB, 4),
    typeless(dxgi::BC6H_TYPELESS, BC6H_UF16),
    BC6H_UF16,
    block(dxgi::BC6H_SF16, 180, RGB, V::BC6H_SFLOAT_BLOCK, 16),
    typeless(dxgi::BC7_TYPELESS, BC7_UNORM),
    BC7_UNORM,
    block(dxgi::BC7_UNORM_SRGB, 182, IDENTITY, V::BC7_SRGB_BLOCK, 16),
    color(dxgi::B4G4R4A4_UNORM, 136, BGRA, V::A4R4G4B4_UNORM_PACK16, 2),
];

pub fn format(dxgi: u32) -> Option<&'static Format> {
    FORMATS.iter().find(|f| f.dxgi == dxgi)
}

impl Format {
    pub fn compressed(&self) -> bool {
        self.block != [1, 1]
    }

    pub fn has_depth(&self) -> bool {
        matches!(
            self.kind,
            Kind::Depth | Kind::DepthStencil | Kind::DepthPlane
        )
    }

    /// Blocks across and down a surface of `width` by `height` texels.
    pub fn blocks(&self, width: u32, height: u32) -> (u32, u32) {
        (
            width.div_ceil(self.block[0]),
            height.div_ceil(self.block[1]),
        )
    }

    /// The `DST_SEL` for a view with D3D12's component mapping (`D3D12_SHADER_COMPONENT_MAPPING`: 0 to 3 pick the
    /// view format's red, green, blue or alpha, 4 forces zero, 5 forces one).
    pub fn view_swizzle(&self, mapping: [u32; 4]) -> Swizzle {
        mapping.map(|m| match m {
            0..=3 => self.channels[m as usize],
            5 => SELECT_ONE,
            _ => SELECT_ZERO,
        })
    }
}

/// The default `D3D12_SHADER_COMPONENT_MAPPING`: red, green, blue, alpha.
pub const DEFAULT_MAPPING: [u32; 4] = [0, 1, 2, 3];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dxgi_value_appears_once() {
        for (i, f) in FORMATS.iter().enumerate() {
            assert!(
                FORMATS[i + 1..].iter().all(|g| g.dxgi != f.dxgi),
                "{} twice",
                f.dxgi
            );
            assert!(f.gfx10 < 512 && f.bytes > 0);
        }
    }

    #[test]
    fn well_known_formats_have_their_console_codes() {
        let code = |d| format(d).unwrap().gfx10;
        assert_eq!(code(dxgi::R8G8B8A8_UNORM), 56);
        assert_eq!(code(dxgi::R8G8B8A8_UNORM_SRGB), 130);
        assert_eq!(code(dxgi::B8G8R8A8_UNORM), 56);
        assert_eq!(code(dxgi::R32_UINT), 20);
        assert_eq!(code(dxgi::R32_SINT), 21);
        assert_eq!(code(dxgi::R32_FLOAT), 22);
        assert_eq!(code(dxgi::BC7_UNORM), 181);
        assert_eq!(code(dxgi::R24_UNORM_X8_TYPELESS), 141);
        assert_eq!(code(dxgi::X24_TYPELESS_G8_UINT), 5);
        assert_eq!(code(dxgi::R9G9B9E5_SHAREDEXP), 132);
    }

    #[test]
    fn bgra_reads_the_same_code_through_a_swapped_swizzle() {
        let bgra = format(dxgi::B8G8R8A8_UNORM).unwrap();
        assert_eq!(bgra.channels, [SELECT_Z, SELECT_Y, SELECT_X, SELECT_W]);
        let view = bgra.view_swizzle([2, 1, 0, 3]);
        assert_eq!(view, IDENTITY);
    }

    #[test]
    fn component_mapping_forces_constants() {
        let r8 = format(dxgi::R8_UNORM).unwrap();
        assert_eq!(
            r8.view_swizzle([0, 0, 0, 5]),
            [SELECT_X, SELECT_X, SELECT_X, SELECT_ONE]
        );
        assert_eq!(
            r8.view_swizzle([4, 4, 4, 0]),
            [SELECT_ZERO, SELECT_ZERO, SELECT_ZERO, SELECT_X]
        );
        let a8 = format(dxgi::A8_UNORM).unwrap();
        assert_eq!(a8.view_swizzle(DEFAULT_MAPPING)[3], SELECT_X);
    }

    #[test]
    fn blocks_round_up() {
        let bc1 = format(dxgi::BC1_UNORM).unwrap();
        assert_eq!(bc1.blocks(6, 5), (2, 2));
        assert_eq!(bc1.bytes, 8);
        assert!(bc1.compressed());
        assert_eq!(format(dxgi::R8G8B8A8_UNORM).unwrap().blocks(6, 5), (6, 5));
    }

    #[test]
    fn typeless_siblings_share_layout_with_their_typed_formats() {
        let t = format(dxgi::R8G8B8A8_TYPELESS).unwrap();
        assert!(t.typeless);
        assert_eq!(t.vulkan, vk::Format::R8G8B8A8_UNORM);
        assert_eq!(t.bytes, 4);
        assert!(format(dxgi::D32_FLOAT).unwrap().has_depth());
        assert!(!format(dxgi::R32_FLOAT).unwrap().has_depth());
    }
}
