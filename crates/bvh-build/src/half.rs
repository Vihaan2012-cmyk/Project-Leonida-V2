/// A half-precision value rounded toward +infinity (`up`) or -infinity from `value`, as the builder's 16-bit
/// boxes need: a bound past the half range becomes an infinity on its outward side and the largest finite half
/// on its inward side, and a magnitude below the smallest normal half becomes zero or the smallest normal half,
/// whichever lies outward, never a denormal.
pub fn round(value: f32, up: bool) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let magnitude = bits & 0x7FFF_FFFF;
    let away = up != (sign != 0);
    if magnitude > 0x7F80_0000 {
        return 0x7E00;
    }
    if magnitude == 0x7F80_0000 {
        return sign | 0x7C00;
    }
    if magnitude >= 0x4780_0000 {
        return sign | if away { 0x7C00 } else { 0x7BFF };
    }
    if magnitude < 0x3880_0000 {
        return if magnitude != 0 && away {
            sign | 0x0400
        } else {
            0
        };
    }
    let mut half = (magnitude >> 13) - (112 << 10);
    if away && magnitude & 0x1FFF != 0 {
        half += 1;
    }
    sign | half as u16
}

/// The exact value of a half.
pub fn to_f32(half: u16) -> f32 {
    let sign = u32::from(half & 0x8000) << 16;
    let exponent = u32::from((half >> 10) & 31);
    let mantissa = u32::from(half & 0x3FF);
    match exponent {
        0 => {
            let magnitude = mantissa as f32 * (1.0 / 16_777_216.0);
            f32::from_bits(sign | magnitude.to_bits())
        }
        31 => f32::from_bits(sign | 0x7F80_0000 | (mantissa << 13)),
        _ => f32::from_bits(sign | ((exponent + 112) << 23) | (mantissa << 13)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_outward() {
        assert_eq!(round(1.0, true), 0x3C00);
        assert_eq!(round(1.0, false), 0x3C00);
        let above = f32::from_bits(1.0f32.to_bits() + 1);
        assert_eq!(round(above, true), 0x3C01);
        assert_eq!(round(above, false), 0x3C00);
        assert_eq!(round(-above, false), 0xBC01);
        assert_eq!(round(-above, true), 0xBC00);
    }

    #[test]
    fn range_limits() {
        assert_eq!(round(1.0e6, true), 0x7C00);
        assert_eq!(round(1.0e6, false), 0x7BFF);
        assert_eq!(round(-1.0e6, false), 0xFC00);
        assert_eq!(round(-1.0e6, true), 0xFBFF);
        assert_eq!(round(f32::INFINITY, false), 0x7C00);
        assert_eq!(round(f32::NEG_INFINITY, true), 0xFC00);
        assert_eq!(round(65504.0, true), 0x7BFF);
        assert_eq!(round(65510.0, true), 0x7C00);
    }

    #[test]
    fn no_denormals() {
        let tiny = 1.0e-6;
        assert_eq!(round(tiny, true), 0x0400);
        assert_eq!(round(tiny, false), 0);
        assert_eq!(round(-tiny, false), 0x8400);
        assert_eq!(round(-tiny, true), 0);
        assert_eq!(round(0.0, true), 0);
    }

    #[test]
    fn rounding_brackets_the_value() {
        let mut state = 12345u32;
        for _ in 0..20000 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let value = f32::from_bits(state);
            if !value.is_finite() || value.abs() > 70000.0 {
                continue;
            }
            let (low, high) = (to_f32(round(value, false)), to_f32(round(value, true)));
            assert!(low <= value && value <= high, "{value}: {low} {high}");
        }
    }
}
