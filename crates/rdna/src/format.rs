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
        0x1F => sign | 0x7F80_0000 | (mantissa << 13) | if quiet { 0x40_0000 } else { 0 },
        0 => sign | (mantissa as f32 * f32::from_bits(0x3380_0000)).to_bits(),
        _ => sign | ((exponent + 112) << 23) | (mantissa << 13),
    }
}

pub fn f32_to_f16(single: u32) -> u32 {
    let sign = (single >> 16) & 0x8000;
    let exponent = ((single >> 23) & 0xFF) as i32;
    let mantissa = single & 0x7F_FFFF;
    if exponent == 0xFF {
        return sign
            | 0x7C00
            | if mantissa != 0 {
                0x200 | (mantissa >> 13)
            } else {
                0
            };
    }
    let biased = exponent - 112;
    if biased >= 31 {
        return sign | 0x7C00;
    }
    let round = |value: u32, shift: u32| {
        let half = 1 << (shift - 1);
        let rest = value & ((1 << shift) - 1);
        let quotient = value >> shift;
        if rest > half || (rest == half && quotient & 1 != 0) {
            quotient + 1
        } else {
            quotient
        }
    };
    if biased <= 0 {
        if biased < -10 {
            return sign;
        }
        return sign | round(mantissa | 0x80_0000, (14 - biased) as u32);
    }
    sign | round(((biased as u32) << 23) | mantissa, 13)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(code: u32, bytes: &[u8]) -> [u32; 4] {
        buffer_format(code).unwrap().decode(bytes)
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
