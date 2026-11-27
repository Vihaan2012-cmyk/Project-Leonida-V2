use ash::vk;
use shader_translate::images::Number;

struct Info {
    vk: vk::Format,
    block_bytes: u32,
    block: [u32; 2],
    number: Number,
}

macro_rules! formats {
    ($($name:ident => $vk:ident, $bytes:expr, $block:expr, $number:ident;)*) => {
        /// Every format the translator's image arrays and the console's texture and target formats need.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Format { $($name,)* }

        impl Format {
            pub const ALL: &'static [Format] = &[$(Format::$name,)*];

            fn info(self) -> Info {
                match self {
                    $(Format::$name => Info {
                        vk: vk::Format::$vk,
                        block_bytes: $bytes,
                        block: $block,
                        number: Number::$number,
                    },)*
                }
            }
        }
    };
}

formats! {
    R8Unorm => R8_UNORM, 1, [1, 1], Float;
    R8Snorm => R8_SNORM, 1, [1, 1], Float;
    R8Uint => R8_UINT, 1, [1, 1], Uint;
    R8Sint => R8_SINT, 1, [1, 1], Sint;
    Rg8Unorm => R8G8_UNORM, 2, [1, 1], Float;
    Rg8Snorm => R8G8_SNORM, 2, [1, 1], Float;
    Rg8Uint => R8G8_UINT, 2, [1, 1], Uint;
    Rg8Sint => R8G8_SINT, 2, [1, 1], Sint;
    Rgba8Unorm => R8G8B8A8_UNORM, 4, [1, 1], Float;
    Rgba8Srgb => R8G8B8A8_SRGB, 4, [1, 1], Float;
    Rgba8Snorm => R8G8B8A8_SNORM, 4, [1, 1], Float;
    Rgba8Uint => R8G8B8A8_UINT, 4, [1, 1], Uint;
    Rgba8Sint => R8G8B8A8_SINT, 4, [1, 1], Sint;
    Bgra8Unorm => B8G8R8A8_UNORM, 4, [1, 1], Float;
    Bgra8Srgb => B8G8R8A8_SRGB, 4, [1, 1], Float;
    R16Unorm => R16_UNORM, 2, [1, 1], Float;
    R16Snorm => R16_SNORM, 2, [1, 1], Float;
    R16Uint => R16_UINT, 2, [1, 1], Uint;
    R16Sint => R16_SINT, 2, [1, 1], Sint;
    R16Float => R16_SFLOAT, 2, [1, 1], Float;
    Rg16Unorm => R16G16_UNORM, 4, [1, 1], Float;
    Rg16Snorm => R16G16_SNORM, 4, [1, 1], Float;
    Rg16Uint => R16G16_UINT, 4, [1, 1], Uint;
    Rg16Sint => R16G16_SINT, 4, [1, 1], Sint;
    Rg16Float => R16G16_SFLOAT, 4, [1, 1], Float;
    Rgba16Unorm => R16G16B16A16_UNORM, 8, [1, 1], Float;
    Rgba16Snorm => R16G16B16A16_SNORM, 8, [1, 1], Float;
    Rgba16Uint => R16G16B16A16_UINT, 8, [1, 1], Uint;
    Rgba16Sint => R16G16B16A16_SINT, 8, [1, 1], Sint;
    Rgba16Float => R16G16B16A16_SFLOAT, 8, [1, 1], Float;
    R32Uint => R32_UINT, 4, [1, 1], Uint;
    R32Sint => R32_SINT, 4, [1, 1], Sint;
    R32Float => R32_SFLOAT, 4, [1, 1], Float;
    Rg32Uint => R32G32_UINT, 8, [1, 1], Uint;
    Rg32Sint => R32G32_SINT, 8, [1, 1], Sint;
    Rg32Float => R32G32_SFLOAT, 8, [1, 1], Float;
    Rgba32Uint => R32G32B32A32_UINT, 16, [1, 1], Uint;
    Rgba32Sint => R32G32B32A32_SINT, 16, [1, 1], Sint;
    Rgba32Float => R32G32B32A32_SFLOAT, 16, [1, 1], Float;
    R64Uint => R64_UINT, 8, [1, 1], Uint;
    Rgb10A2Unorm => A2B10G10R10_UNORM_PACK32, 4, [1, 1], Float;
    Rgb10A2Uint => A2B10G10R10_UINT_PACK32, 4, [1, 1], Uint;
    Rg11B10Float => B10G11R11_UFLOAT_PACK32, 4, [1, 1], Float;
    Bgr10A2Unorm => A2R10G10B10_UNORM_PACK32, 4, [1, 1], Float;
    Rgb9E5Float => E5B9G9R9_UFLOAT_PACK32, 4, [1, 1], Float;
    B5G6R5Unorm => R5G6B5_UNORM_PACK16, 2, [1, 1], Float;
    D16Unorm => D16_UNORM, 2, [1, 1], Float;
    D32Float => D32_SFLOAT, 4, [1, 1], Float;
    D24UnormS8Uint => D24_UNORM_S8_UINT, 4, [1, 1], Float;
    D32FloatS8Uint => D32_SFLOAT_S8_UINT, 8, [1, 1], Float;
    Bc1Unorm => BC1_RGBA_UNORM_BLOCK, 8, [4, 4], Float;
    Bc1Srgb => BC1_RGBA_SRGB_BLOCK, 8, [4, 4], Float;
    Bc2Unorm => BC2_UNORM_BLOCK, 16, [4, 4], Float;
    Bc2Srgb => BC2_SRGB_BLOCK, 16, [4, 4], Float;
    Bc3Unorm => BC3_UNORM_BLOCK, 16, [4, 4], Float;
    Bc3Srgb => BC3_SRGB_BLOCK, 16, [4, 4], Float;
    Bc4Unorm => BC4_UNORM_BLOCK, 8, [4, 4], Float;
    Bc4Snorm => BC4_SNORM_BLOCK, 8, [4, 4], Float;
    Bc5Unorm => BC5_UNORM_BLOCK, 16, [4, 4], Float;
    Bc5Snorm => BC5_SNORM_BLOCK, 16, [4, 4], Float;
    Bc6hUfloat => BC6H_UFLOAT_BLOCK, 16, [4, 4], Float;
    Bc6hSfloat => BC6H_SFLOAT_BLOCK, 16, [4, 4], Float;
    Bc7Unorm => BC7_UNORM_BLOCK, 16, [4, 4], Float;
    Bc7Srgb => BC7_SRGB_BLOCK, 16, [4, 4], Float;
}

impl Format {
    pub fn vk(self) -> vk::Format {
        self.info().vk
    }

    pub fn from_vk(format: vk::Format) -> Option<Format> {
        Format::ALL.iter().copied().find(|f| f.vk() == format)
    }

    /// Bytes in one texel, or in one 4x4 block for the compressed formats.
    pub fn block_bytes(self) -> u32 {
        self.info().block_bytes
    }

    /// Texels per block in each direction: 1x1, or 4x4 for BCn.
    pub fn block_extent(self) -> [u32; 2] {
        self.info().block
    }

    pub fn is_compressed(self) -> bool {
        self.info().block[0] > 1
    }

    pub fn is_depth(self) -> bool {
        matches!(
            self,
            Format::D16Unorm | Format::D32Float | Format::D24UnormS8Uint | Format::D32FloatS8Uint
        )
    }

    pub fn has_stencil(self) -> bool {
        matches!(self, Format::D24UnormS8Uint | Format::D32FloatS8Uint)
    }

    /// The image array a texture of this format belongs in: the number type of the T# FORMAT field.
    pub fn number(self) -> Number {
        self.info().number
    }

    /// Every aspect of the format.
    pub fn aspect(self) -> vk::ImageAspectFlags {
        match (self.is_depth(), self.has_stencil()) {
            (false, _) => vk::ImageAspectFlags::COLOR,
            (true, false) => vk::ImageAspectFlags::DEPTH,
            (true, true) => vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
        }
    }

    /// The aspect a shader samples: depth for depth formats.
    pub fn sampled_aspect(self) -> vk::ImageAspectFlags {
        if self.is_depth() {
            vk::ImageAspectFlags::DEPTH
        } else {
            vk::ImageAspectFlags::COLOR
        }
    }

    /// Bytes of one mip level of a tightly packed image.
    pub fn level_bytes(self, extent: [u32; 3], mip: u32) -> u64 {
        let [w, h, d] = level_extent(extent, mip);
        let [bw, bh] = self.block_extent();
        u64::from(w.div_ceil(bw))
            * u64::from(h.div_ceil(bh))
            * u64::from(d)
            * u64::from(self.block_bytes())
    }

    /// Bytes per row of blocks in one mip level of a tightly packed image.
    pub fn row_bytes(self, width: u32) -> u64 {
        u64::from(width.div_ceil(self.block_extent()[0])) * u64::from(self.block_bytes())
    }
}

pub fn level_extent(extent: [u32; 3], mip: u32) -> [u32; 3] {
    extent.map(|e| (e >> mip).max(1))
}

pub fn mip_count(extent: [u32; 3]) -> u32 {
    32 - extent.into_iter().max().unwrap_or(1).max(1).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_round_trips_and_has_a_size() {
        for &format in Format::ALL {
            assert_eq!(Format::from_vk(format.vk()), Some(format));
            assert!(format.block_bytes() > 0);
        }
    }

    #[test]
    fn block_math() {
        assert_eq!(Format::Bc1Unorm.level_bytes([16, 16, 1], 0), 4 * 4 * 8);
        assert_eq!(Format::Bc7Unorm.level_bytes([5, 5, 1], 0), 2 * 2 * 16);
        assert_eq!(Format::Rgba8Unorm.level_bytes([64, 32, 1], 1), 32 * 16 * 4);
        assert_eq!(Format::Rgba8Unorm.level_bytes([4, 4, 4], 5), 4);
        assert_eq!(mip_count([256, 64, 1]), 9);
        assert_eq!(mip_count([1, 1, 1]), 1);
    }

    #[test]
    fn number_types() {
        assert_eq!(Format::Rgba8Srgb.number(), Number::Float);
        assert_eq!(Format::R32Uint.number(), Number::Uint);
        assert_eq!(Format::Rg16Sint.number(), Number::Sint);
        assert_eq!(Format::D32Float.number(), Number::Float);
    }
}
