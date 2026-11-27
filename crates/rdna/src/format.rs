#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Number {
    Unorm,
    Snorm,
    Uscaled,
    Sscaled,
    Uint,
    Sint,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferFormat {
    pub widths: [u8; 4],
    pub count: usize,
    pub number: Number,
}

const LAYOUTS: [(u32, &[u8], &[Number]); 14] = {
    use Number::*;
    const SIX: &[Number] = &[Unorm, Snorm, Uscaled, Sscaled, Uint, Sint];
    const SEVEN: &[Number] = &[Unorm, Snorm, Uscaled, Sscaled, Uint, Sint, Float];
    const WORD: &[Number] = &[Uint, Sint, Float];
    [
        (1, &[8], SIX),
        (7, &[16], SEVEN),
        (14, &[8, 8], SIX),
        (20, &[32], WORD),
        (23, &[16, 16], SEVEN),
        (30, &[11, 11, 10], SEVEN),
        (37, &[10, 11, 11], SEVEN),
        (44, &[2, 10, 10, 10], SIX),
        (50, &[10, 10, 10, 2], SIX),
        (56, &[8, 8, 8, 8], SIX),
        (62, &[32, 32], WORD),
        (65, &[16, 16, 16, 16], SEVEN),
        (72, &[32, 32, 32], WORD),
        (75, &[32, 32, 32, 32], WORD),
    ]
};

pub fn buffer_format(code: u32) -> Option<BufferFormat> {
    LAYOUTS.iter().find_map(|(first, widths, numbers)| {
        let number = *numbers.get(code.checked_sub(*first)? as usize)?;
        let mut array = [0; 4];
        array[..widths.len()].copy_from_slice(widths);
        Some(BufferFormat {
            widths: array,
            count: widths.len(),
            number,
        })
    })
}

impl BufferFormat {
    pub fn bytes(&self) -> usize {
        self.widths.iter().map(|w| usize::from(*w)).sum::<usize>() / 8
    }

    pub fn offset(&self, channel: usize) -> u32 {
        self.widths[..channel].iter().map(|w| u32::from(*w)).sum()
    }

    pub fn integer(&self) -> bool {
        matches!(self.number, Number::Uint | Number::Sint)
    }

    pub fn convert(&self, channel: usize, field: u32) -> u32 {
        let width = u32::from(self.widths[channel]);
        let signed = if width == 32 {
            field as i32
        } else {
            ((field << (32 - width)) as i32) >> (32 - width)
        };
        match self.number {
            Number::Uint => field,
            Number::Sint => signed as u32,
            Number::Uscaled => (field as f32).to_bits(),
            Number::Sscaled => (signed as f32).to_bits(),
            Number::Unorm => {
                let max = (1u64 << width) - 1;
                ((f64::from(field) / max as f64) as f32).to_bits()
            }
            Number::Snorm => {
                let max = (1u64 << (width - 1)) - 1;
                ((f64::from(signed) / max as f64).max(-1.0) as f32).to_bits()
            }
            Number::Float if width == 32 => field,
            Number::Float if width == 16 => f16_to_f32(field, false),
            Number::Float => f16_to_f32(field << (15 - width), false),
        }
    }

    pub fn decode(&self, element: &[u8]) -> [u32; 4] {
        let mut bits = 0u128;
        for (i, byte) in element.iter().enumerate().take(16) {
            bits |= u128::from(*byte) << (8 * i);
        }
        let mut channels = [0; 4];
        for (channel, slot) in channels.iter_mut().enumerate().take(self.count) {
            let width = u32::from(self.widths[channel]);
            let field = (bits >> self.offset(channel)) as u32 & (u32::MAX >> (32 - width));
            *slot = self.convert(channel, field);
        }
        channels
    }
}

pub fn code(name: &str) -> Option<u32> {
    let inner = name.strip_prefix("format:")?;
    if let Some(list) = inner.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let wanted = list.replace("BUF_FMT_", "GFX10_FORMAT_");
        return crate::formats::FORMATS
            .iter()
            .find(|(_, n)| *n == wanted)
            .map(|(c, _)| u32::from(*c));
    }
    inner.parse().ok()
}

pub fn name(code: u32) -> Option<String> {
    crate::formats::FORMATS
        .iter()
        .find(|(c, _)| u32::from(*c) == code)
        .map(|(_, n)| n.replace("GFX10_FORMAT_", "BUF_FMT_"))
}

pub fn instruction_format(mods: &[String]) -> Option<u32> {
    match mods.iter().find(|m| m.starts_with("format:")) {
        Some(text) => code(text),
        None => Some(1),
    }
}

pub fn select(
    format: u32,
    selector: u32,
    in_range: bool,
    element: impl FnOnce(&BufferFormat) -> [u32; 4],
) -> u32 {
    match selector {
        0 => 0,
        1 => one(format),
        4..=7 if in_range => match buffer_format(format) {
            Some(layout) => element(&layout)[(selector as usize - 4) % layout.count],
            None => 0,
        },
        _ => 0,
    }
}

/// The number type of any GFX10 format code, image-only formats included, read from its name.
pub fn number(code: u32) -> Option<Number> {
    let (_, name) = crate::formats::FORMATS
        .iter()
        .find(|(c, _)| u32::from(*c) == code)?;
    Some(match name.trim().rsplit('_').next()? {
        "UNORM" | "SRGB" => Number::Unorm,
        "SNORM" => Number::Snorm,
        "USCALED" => Number::Uscaled,
        "SSCALED" => Number::Sscaled,
        "UINT" => Number::Uint,
        "SINT" => Number::Sint,
        "FLOAT" | "UFLOAT" | "SFLOAT" => Number::Float,
        _ => return None,
    })
}

pub fn one(code: u32) -> u32 {
    match buffer_format(code) {
        Some(format) if !format.integer() => 0x3F80_0000,
        _ => 1,
    }
}

pub fn f16_to_f32(half: u32, quiet: bool) -> u32 {
    let sign = (half >> 15 & 1) << 31;
    let exponent = (half >> 10) & 0x1F;
    let mantissa = half & 0x3FF;
    match exponent {
        0x1F if mantissa == 0 => sign | 0x7F80_0000,
        0x1F if quiet => sign | 0x7FC0_0000,
        0x1F => sign | 0x7F80_0000 | (mantissa << 13),
        0 => sign | (mantissa as f32 * f32::from_bits(0x3380_0000)).to_bits(),
        _ => sign | ((exponent + 112) << 23) | (mantissa << 13),
    }
}

/// The four rounding modes of the MODE register's FP_ROUND fields, in field order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Round {
    #[default]
    Even,
    Up,
    Down,
    Zero,
}

impl Round {
    pub fn from_field(field: u32) -> Round {
        [Round::Even, Round::Up, Round::Down, Round::Zero][(field & 3) as usize]
    }

    /// The half-precision magnitude a value beyond the largest finite one becomes.
    pub fn overflow(self, negative: bool) -> u32 {
        match (self, negative) {
            (Round::Even, _) | (Round::Up, false) | (Round::Down, true) => 0x7C00,
            _ => 0x7BFF,
        }
    }

    /// Whether a magnitude with a nonzero remainder moves away from zero.
    pub fn away(self, negative: bool, rest: u64, half: u64, odd: bool) -> bool {
        match self {
            Round::Even => rest > half || (rest == half && odd),
            Round::Up => rest != 0 && !negative,
            Round::Down => rest != 0 && negative,
            Round::Zero => false,
        }
    }
}

pub fn f32_to_f16(single: u32) -> u32 {
    f32_to_f16_round(single, Round::Even)
}

pub fn f32_to_f16_round(single: u32, mode: Round) -> u32 {
    let negative = single >> 31 != 0;
    let sign = (single >> 16) & 0x8000;
    let exponent = ((single >> 23) & 0xFF) as i32;
    let mantissa = single & 0x7F_FFFF;
    if exponent == 0xFF {
        return sign | if mantissa != 0 { 0x7E00 } else { 0x7C00 };
    }
    let biased = exponent - 112;
    let largest = mode.overflow(negative);
    if biased >= 31 {
        return sign | largest;
    }
    let round = |value: u32, shift: u32| {
        let half = 1u64 << (shift - 1);
        let rest = u64::from(value) & ((1u64 << shift) - 1);
        let quotient = value >> shift;
        quotient + u32::from(mode.away(negative, rest, half, quotient & 1 != 0))
    };
    if biased <= 0 {
        if biased < -10 {
            let nonzero = exponent != 0 || mantissa != 0;
            return sign | u32::from(mode.away(negative, u64::from(nonzero), 1, false));
        }
        return sign | round(mantissa | 0x80_0000, (14 - biased) as u32);
    }
    sign | round(((biased as u32) << 23) | mantissa, 13).min(largest)
}

/// Rounds a double to half precision, for results that are exact before their one rounding.
pub fn f64_to_f16_round(double: u64, mode: Round) -> u32 {
    let negative = double >> 63 != 0;
    let sign = ((double >> 48) & 0x8000) as u32;
    let exponent = ((double >> 52) & 0x7FF) as i32;
    let mantissa = double & 0xF_FFFF_FFFF_FFFF;
    if exponent == 0x7FF {
        return sign | if mantissa != 0 { 0x7E00 } else { 0x7C00 };
    }
    let largest = mode.overflow(negative);
    let biased = exponent - 1008;
    if biased >= 31 {
        return sign | largest;
    }
    let round = |value: u64, shift: u32| {
        let half = 1u64 << (shift - 1);
        let rest = value & ((1u64 << shift) - 1);
        let quotient = (value >> shift) as u32;
        quotient + u32::from(mode.away(negative, rest, half, quotient & 1 != 0))
    };
    if biased <= 0 {
        if biased < -10 {
            let nonzero = exponent != 0 || mantissa != 0;
            return sign | u32::from(mode.away(negative, u64::from(nonzero), 1, false));
        }
        return sign | round(mantissa | (1 << 52), (43 - biased) as u32);
    }
    sign | round(((biased as u64) << 52) | mantissa, 42).min(largest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(code: u32, bytes: &[u8]) -> [u32; 4] {
        buffer_format(code).unwrap().decode(bytes)
    }

    #[test]
    fn halves_round_in_every_mode() {
        let above_one = 1.0f32.to_bits() + 1;
        let below_minus_one = (-1.0f32).to_bits() + 1;
        for (mode, up, down) in [
            (Round::Even, 0x3C00, 0xBC00),
            (Round::Up, 0x3C01, 0xBC00),
            (Round::Down, 0x3C00, 0xBC01),
            (Round::Zero, 0x3C00, 0xBC00),
        ] {
            assert_eq!(f32_to_f16_round(above_one, mode), up, "{mode:?}");
            assert_eq!(f32_to_f16_round(below_minus_one, mode), down, "{mode:?}");
            let double_above = 1.0f64.to_bits() + 1;
            let double_below = (-1.0f64).to_bits() + 1;
            assert_eq!(f64_to_f16_round(double_above, mode), up, "{mode:?}");
            assert_eq!(f64_to_f16_round(double_below, mode), down, "{mode:?}");
        }
        let big = 70000.0f32.to_bits();
        assert_eq!(f32_to_f16_round(big, Round::Even), 0x7C00);
        assert_eq!(f32_to_f16_round(big, Round::Zero), 0x7BFF);
        assert_eq!(f32_to_f16_round(big | 0x8000_0000, Round::Up), 0xFBFF);
        assert_eq!(f32_to_f16_round(65519.0f32.to_bits(), Round::Even), 0x7BFF);
        assert_eq!(f32_to_f16_round(65520.0f32.to_bits(), Round::Even), 0x7C00);
        assert_eq!(f32_to_f16_round(65535.0f32.to_bits(), Round::Zero), 0x7BFF);
        let tiny = 1.0e-10f32.to_bits();
        assert_eq!(f32_to_f16_round(tiny, Round::Even), 0);
        assert_eq!(f32_to_f16_round(tiny, Round::Up), 1);
        assert_eq!(f32_to_f16_round(tiny | 0x8000_0000, Round::Down), 0x8001);
        assert_eq!(f32_to_f16_round(0x8000_0000, Round::Down), 0x8000);
        assert_eq!(f64_to_f16_round(1.0e-10f64.to_bits(), Round::Up), 1);
        assert_eq!(f32_to_f16_round(f32::NAN.to_bits(), Round::Zero), 0x7E00);
        assert_eq!(
            f32_to_f16_round(f32::INFINITY.to_bits(), Round::Zero),
            0x7C00
        );
        for half in 0..0x7C00u32 {
            let single = f16_to_f32(half, true);
            for mode in [Round::Even, Round::Up, Round::Down, Round::Zero] {
                assert_eq!(f32_to_f16_round(single, mode), half);
                let double = f64::from(f32::from_bits(single)).to_bits();
                assert_eq!(f64_to_f16_round(double, mode), half);
            }
        }
    }

    #[test]
    fn number_types_come_from_format_names() {
        assert_eq!(number(1), Some(Number::Unorm));
        assert_eq!(number(20), Some(Number::Uint));
        assert_eq!(number(21), Some(Number::Sint));
        assert_eq!(number(22), Some(Number::Float));
        assert_eq!(number(173), Some(Number::Unorm));
        assert_eq!(number(179), Some(Number::Float));
        assert_eq!(number(283), Some(Number::Unorm));
        assert_eq!(number(0), None);
        assert_eq!(number(511), None);
    }

    #[test]
    fn unorm_is_the_correctly_rounded_quotient() {
        assert_eq!(load(56, &[3, 3, 3, 3]), [0x3C40_C0C1; 4]);
        assert_eq!(load(1, &[0xFF])[0], 0x3F80_0000);
        assert_eq!(load(1, &[0xEF])[0], ((239.0f64 / 255.0) as f32).to_bits());
    }

    #[test]
    fn signed_and_scaled_formats() {
        assert_eq!(load(6, &[0x80])[0], 0xFFFF_FF80);
        assert_eq!(load(12, &[0x00, 0x80])[0], 0xFFFF_8000);
        assert_eq!(load(3, &[0xFF])[0], 0x437F_0000);
        assert_eq!(load(4, &[0x80])[0], 0xC300_0000);
        assert_eq!(load(10, &[0x00, 0x80])[0], 0xC700_0000);
        let word: u32 = (1 << 30) | (0x201 << 20) | (0x1FF << 10) | 0x200;
        assert_eq!(
            load(55, &word.to_le_bytes()),
            [0xFFFF_FE00, 511, 0xFFFF_FE01, 1]
        );
        let snorm = load(51, &word.to_le_bytes());
        assert_eq!(&snorm[..3], &[0xBF80_0000, 0x3F80_0000, 0xBF80_0000]);
    }

    #[test]
    fn small_floats_keep_nan_payloads() {
        assert_eq!(load(36, &(15u32 << 6).to_le_bytes())[0], 0x3F80_0000);
        assert_eq!(load(43, &(15u32 << 5).to_le_bytes())[0], 0x3F80_0000);
        assert_eq!(load(36, &0x7C0u32.to_le_bytes())[0], 0x7F80_0000);
        assert_eq!(load(36, &0x7C1u32.to_le_bytes())[0], 0x7F82_0000);
        assert_eq!(load(36, &1u32.to_le_bytes())[0], 0x3580_0000);
        assert_eq!(load(43, &(1u32 << 22).to_le_bytes())[2], 0x3600_0000);
        assert_eq!(
            load(36, &((15u32 << 5) << 22).to_le_bytes())[2],
            0x3F80_0000
        );
    }

    #[test]
    fn half_conversions_round_to_nearest_even() {
        for half in 0..=0xFFFFu32 {
            let single = f16_to_f32(half, true);
            let back = f32_to_f16(single);
            let nan = half & 0x7C00 == 0x7C00 && half & 0x3FF != 0;
            if nan {
                assert_eq!(back & 0x7E00, 0x7E00, "{half:#x}");
            } else {
                assert_eq!(back, half, "{half:#x}");
            }
        }
        assert_eq!(f32_to_f16(0x3F80_0000), 0x3C00);
        assert_eq!(f32_to_f16(0x3F80_1000), 0x3C00);
        assert_eq!(f32_to_f16(0x3F80_3000), 0x3C02);
        assert_eq!(f32_to_f16(0x4780_0000), 0x7C00);
        assert_eq!(f32_to_f16(0x3380_0000), 0x0001);
        assert_eq!(f32_to_f16(0x3300_0000), 0x0000);
        assert_eq!(f32_to_f16(0x3300_0001), 0x0001);
    }

    #[test]
    fn missing_formats_and_one() {
        assert!(buffer_format(0).is_none());
        assert!(buffer_format(78).is_none());
        assert_eq!(one(22), 0x3F80_0000);
        assert_eq!(one(20), 1);
        assert_eq!(one(0), 1);
    }
}
