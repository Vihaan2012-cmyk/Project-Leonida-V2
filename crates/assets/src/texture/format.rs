//! DXGI formats that occur in the texture pools and dictionaries.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatInfo {
    pub dxgi: u32,
    pub name: &'static str,
    /// Bytes per pixel, or per block for block-compressed formats.
    pub element_bytes: u32,
    /// 4 for BCn, 1 otherwise.
    pub block: u32,
    pub srgb: bool,
}

impl FormatInfo {
    pub const fn is_block_compressed(&self) -> bool {
        self.block == 4
    }

    pub const fn log2_element(&self) -> u32 {
        self.element_bytes.trailing_zeros()
    }
}

const fn f(
    dxgi: u32,
    name: &'static str,
    element_bytes: u32,
    block: u32,
    srgb: bool,
) -> FormatInfo {
    FormatInfo {
        dxgi,
        name,
        element_bytes,
        block,
        srgb,
    }
}

static FORMATS: &[FormatInfo] = &[
    f(2, "R32G32B32A32_FLOAT", 16, 1, false),
    f(3, "R32G32B32A32_UINT", 16, 1, false),
    f(4, "R32G32B32A32_SINT", 16, 1, false),
    f(6, "R32G32B32_FLOAT", 12, 1, false),
    f(7, "R32G32B32_UINT", 12, 1, false),
    f(8, "R32G32B32_SINT", 12, 1, false),
    f(10, "R16G16B16A16_FLOAT", 8, 1, false),
    f(11, "R16G16B16A16_UNORM", 8, 1, false),
    f(12, "R16G16B16A16_UINT", 8, 1, false),
    f(13, "R16G16B16A16_SNORM", 8, 1, false),
    f(14, "R16G16B16A16_SINT", 8, 1, false),
    f(16, "R32G32_FLOAT", 8, 1, false),
    f(17, "R32G32_UINT", 8, 1, false),
    f(18, "R32G32_SINT", 8, 1, false),
    f(24, "R10G10B10A2_UNORM", 4, 1, false),
    f(25, "R10G10B10A2_UINT", 4, 1, false),
    f(26, "R11G11B10_FLOAT", 4, 1, false),
    f(28, "R8G8B8A8_UNORM", 4, 1, false),
    f(29, "R8G8B8A8_UNORM_SRGB", 4, 1, true),
    f(30, "R8G8B8A8_UINT", 4, 1, false),
    f(31, "R8G8B8A8_SNORM", 4, 1, false),
    f(32, "R8G8B8A8_SINT", 4, 1, false),
    f(34, "R16G16_FLOAT", 4, 1, false),
    f(35, "R16G16_UNORM", 4, 1, false),
    f(36, "R16G16_UINT", 4, 1, false),
    f(37, "R16G16_SNORM", 4, 1, false),
    f(38, "R16G16_SINT", 4, 1, false),
    f(41, "R32_FLOAT", 4, 1, false),
    f(42, "R32_UINT", 4, 1, false),
    f(43, "R32_SINT", 4, 1, false),
    f(49, "R8G8_UNORM", 2, 1, false),
    f(50, "R8G8_UINT", 2, 1, false),
    f(51, "R8G8_SNORM", 2, 1, false),
    f(52, "R8G8_SINT", 2, 1, false),
    f(54, "R16_FLOAT", 2, 1, false),
    f(56, "R16_UNORM", 2, 1, false),
    f(57, "R16_UINT", 2, 1, false),
    f(58, "R16_SNORM", 2, 1, false),
    f(59, "R16_SINT", 2, 1, false),
    f(61, "R8_UNORM", 1, 1, false),
    f(62, "R8_UINT", 1, 1, false),
    f(63, "R8_SNORM", 1, 1, false),
    f(64, "R8_SINT", 1, 1, false),
    f(65, "A8_UNORM", 1, 1, false),
    f(71, "BC1_UNORM", 8, 4, false),
    f(72, "BC1_UNORM_SRGB", 8, 4, true),
    f(74, "BC2_UNORM", 16, 4, false),
    f(75, "BC2_UNORM_SRGB", 16, 4, true),
    f(77, "BC3_UNORM", 16, 4, false),
    f(78, "BC3_UNORM_SRGB", 16, 4, true),
    f(80, "BC4_UNORM", 8, 4, false),
    f(81, "BC4_SNORM", 8, 4, false),
    f(83, "BC5_UNORM", 16, 4, false),
    f(84, "BC5_SNORM", 16, 4, false),
    f(87, "B8G8R8A8_UNORM", 4, 1, false),
    f(88, "B8G8R8X8_UNORM", 4, 1, false),
    f(91, "B8G8R8A8_UNORM_SRGB", 4, 1, true),
    f(93, "B8G8R8X8_UNORM_SRGB", 4, 1, true),
    f(95, "BC6H_UF16", 16, 4, false),
    f(96, "BC6H_SF16", 16, 4, false),
    f(98, "BC7_UNORM", 16, 4, false),
    f(99, "BC7_UNORM_SRGB", 16, 4, true),
];

pub fn format_info(dxgi: u32) -> Option<FormatInfo> {
    FORMATS.iter().copied().find(|f| f.dxgi == dxgi)
}

/// Bytes of one tightly packed mip of `width` x `height` pixels.
pub fn mip_bytes(info: &FormatInfo, width: u32, height: u32) -> u64 {
    let (w, h) = (
        u64::from(width.div_ceil(info.block).max(1)),
        u64::from(height.div_ceil(info.block).max(1)),
    );
    w * h * u64::from(info.element_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_sizes_are_powers_of_two_except_rgb32() {
        for f in FORMATS {
            if ![6, 7, 8].contains(&f.dxgi) {
                assert!(f.element_bytes.is_power_of_two(), "{}", f.name);
            }
        }
    }

    #[test]
    fn bc_mip_bytes_round_up_to_blocks() {
        let bc1 = format_info(71).unwrap();
        assert_eq!(mip_bytes(&bc1, 512, 256), 128 * 64 * 8);
        assert_eq!(mip_bytes(&bc1, 1, 1), 8);
        assert_eq!(mip_bytes(&bc1, 5, 9), 2 * 3 * 8);
        let rgba = format_info(28).unwrap();
        assert_eq!(mip_bytes(&rgba, 7, 3), 84);
    }

    #[test]
    fn unknown_format_is_none() {
        assert!(format_info(0).is_none());
        assert!(format_info(200).is_none());
    }
}
