//! Anisotropic filtering and LOD bias applied to the console's sampler words (S#), GFX10 `SQ_IMG_SAMP_WORD0..3`.
//!
//! Fields used (Mesa's gfx10-rsrc.json, as radv's `ac_build_sampler_descriptor` fills them):
//!   WORD0  MAX_ANISO_RATIO [11:9] = log2(anisotropy), at most 4; DEPTH_COMPARE_FUNC [14:12]; FORCE_UNNORMALIZED [15];
//!          ANISO_THRESHOLD [18:16] = ratio >> 1; ANISO_BIAS [26:21] = ratio; FILTER_MODE [30:29]
//!   WORD1  PERF_MIP [27:24] = ratio + 6 (0 without anisotropy)
//!   WORD2  LOD_BIAS [13:0] signed 5.8 fixed; XY_MAG_FILTER [21:20]; XY_MIN_FILTER [23:22]; MIP_FILTER [27:26];
//!          ANISO_OVERRIDE [29]; DERIV_ADJUST_EN [31]
//!   WORD3  bit 12 marks a D3D12 NEVER comparison sampler when DERIV_ADJUST_EN is clear

use crate::graphics::Graphics;

const MAX_ANISO_SHIFT: u32 = 9;
const MAX_ANISO_LOG2: u32 = 4;
const FORCE_UNNORMALIZED: u32 = 1 << 15;
const COMPARE_FUNC_MASK: u32 = 0x7 << 12;
const FILTER_MODE_MASK: u32 = 0x3 << 29;
const ANISO_TUNING_MASK0: u32 = (0x7 << 16) | (0x3F << 21);
const PERF_MIP_MASK: u32 = 0xF << 24;
const LOD_BIAS_MASK: u32 = 0x3FFF;
const FILTER_BILINEAR: u32 = 1;
const FILTER_ANISO_FLAG: u32 = 2;
const MIP_FILTER_POINT: u32 = 1;
const MIP_FILTER_LINEAR: u32 = 2;
const ANISO_OVERRIDE: u32 = 1 << 29;
const DERIV_ADJUST_EN: u32 = 1 << 31;
const COMPARE_NEVER_MARK: u32 = 1 << 12;

fn field(word: u32, shift: u32, width: u32) -> u32 {
    (word >> shift) & ((1 << width) - 1)
}

/// Whether `graphics` may change this sampler at all: a plain blending sampler with a mip filter, reading
/// normalized coordinates and doing no depth comparison. Shadow, min/max, lookup-table and single-level samplers
/// keep the game's own words.
fn adjustable(words: &[u32; 4]) -> bool {
    let deriv_adjust = words[2] & DERIV_ADJUST_EN != 0;
    let compare_never = !deriv_adjust && words[3] & COMPARE_NEVER_MARK != 0;
    let mip_filter = field(words[2], 26, 2);
    words[0] & (FORCE_UNNORMALIZED | COMPARE_FUNC_MASK | FILTER_MODE_MASK) == 0
        && !compare_never
        && (mip_filter == MIP_FILTER_POINT || mip_filter == MIP_FILTER_LINEAR)
}

/// The sampler words with the user's anisotropic filtering and LOD bias applied.
///
/// Anisotropy is raised to `graphics.anisotropic_filtering` on samplers that filter linearly, never lowered. The
/// LOD bias is added to the sampler's own, clamped to the field's range.
pub fn adjust_sampler(words: [u32; 4], graphics: &Graphics) -> [u32; 4] {
    if !adjustable(&words) {
        return words;
    }
    let mut words = words;
    raise_anisotropy(&mut words, graphics.anisotropic_filtering);
    add_lod_bias(&mut words, graphics.effective_lod_bias());
    words
}

fn raise_anisotropy(words: &mut [u32; 4], level: u32) {
    let wanted = level.max(1).ilog2().min(MAX_ANISO_LOG2);
    let current = field(words[0], MAX_ANISO_SHIFT, 3).min(MAX_ANISO_LOG2);
    let mag = field(words[2], 20, 2);
    let min = field(words[2], 22, 2);
    let filters_linearly = mag & FILTER_BILINEAR != 0 || min & FILTER_BILINEAR != 0;
    if wanted <= current || !filters_linearly {
        return;
    }
    words[0] = words[0] & !((0x7 << MAX_ANISO_SHIFT) | ANISO_TUNING_MASK0)
        | (wanted << MAX_ANISO_SHIFT)
        | ((wanted >> 1) << 16)
        | (wanted << 21);
    words[1] = words[1] & !PERF_MIP_MASK | ((wanted + 6) << 24);
    words[2] = words[2] & !((0xF) << 20)
        | ((mag | FILTER_ANISO_FLAG) << 20)
        | ((min | FILTER_ANISO_FLAG) << 22)
        | ANISO_OVERRIDE;
}

fn add_lod_bias(words: &mut [u32; 4], bias: f32) {
    let delta = (bias * 256.0).round();
    if !delta.is_finite() || delta == 0.0 {
        return;
    }
    let current = ((words[2] & LOD_BIAS_MASK) ^ 0x2000) as i32 - 0x2000;
    let raised = (current as f32 + delta).clamp(-8192.0, 8191.0) as i32;
    words[2] = words[2] & !LOD_BIAS_MASK | (raised as u32 & LOD_BIAS_MASK);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRILINEAR: [u32; 4] = [0, 0xF00 << 12, (1 << 20) | (1 << 22) | (2 << 26), 0];

    /// Native resolution with no upscaler, so only the field under test moves the bias.
    fn native() -> Graphics {
        Graphics {
            upscaler: crate::Upscaler::Off,
            resolution_scale: 1.0,
            ..Graphics::default()
        }
    }

    fn plain() -> Graphics {
        Graphics {
            anisotropic_filtering: 1,
            ..native()
        }
    }

    fn aniso(level: u32) -> Graphics {
        Graphics {
            anisotropic_filtering: level,
            ..native()
        }
    }

    fn bias(value: f32) -> Graphics {
        Graphics {
            texture_lod_bias: value,
            ..plain()
        }
    }

    #[test]
    fn sixteen_times_matches_the_words_radv_writes() {
        let radv = [
            (4 << 9) | (2 << 16) | (4 << 21),
            (0xF00 << 12) | (10 << 24),
            (3 << 20) | (3 << 22) | (2 << 26) | (1 << 29),
            0,
        ];
        assert_eq!(adjust_sampler(TRILINEAR, &aniso(16)), radv);
    }

    #[test]
    fn each_level_writes_its_ratio() {
        for (level, log2) in [(2, 1), (4, 2), (8, 3), (16, 4)] {
            let out = adjust_sampler(TRILINEAR, &aniso(level));
            assert_eq!(field(out[0], 9, 3), log2, "level {level}");
            assert_eq!(field(out[0], 16, 3), log2 >> 1);
            assert_eq!(field(out[0], 21, 6), log2);
            assert_eq!(field(out[1], 24, 4), log2 + 6);
            assert_eq!(field(out[2], 20, 2), 3);
            assert_eq!(field(out[2], 22, 2), 3);
        }
    }

    #[test]
    fn anisotropy_is_never_lowered_and_one_changes_nothing() {
        let game = adjust_sampler(TRILINEAR, &aniso(8));
        assert_eq!(adjust_sampler(game, &aniso(4)), game);
        assert_eq!(adjust_sampler(game, &aniso(8)), game);
        assert_eq!(adjust_sampler(TRILINEAR, &aniso(1)), TRILINEAR);
        assert_ne!(adjust_sampler(game, &aniso(16)), game);
    }

    #[test]
    fn point_filtered_samplers_stay_unfiltered() {
        let point = [0, 0xF00 << 12, MIP_FILTER_POINT << 26, 0];
        assert_eq!(adjust_sampler(point, &aniso(16)), point);
    }

    #[test]
    fn a_linear_sampler_with_a_point_filter_on_one_side_gets_the_matching_aniso_filters() {
        let mixed = [0, 0, (1 << 20) | (2 << 26), 0];
        let out = adjust_sampler(mixed, &aniso(4));
        assert_eq!(field(out[2], 20, 2), 3);
        assert_eq!(field(out[2], 22, 2), 2);
    }

    #[test]
    fn lod_bias_is_added_in_signed_fixed_point() {
        assert_eq!(
            adjust_sampler(TRILINEAR, &bias(1.0))[2],
            TRILINEAR[2] + 0x100
        );
        let minus = adjust_sampler(TRILINEAR, &bias(-1.5));
        assert_eq!(minus[2] & 0x3FFF, 0x3E80);
        assert_eq!(minus[2] & !0x3FFF, TRILINEAR[2]);
        let from_negative = [0, 0, (2 << 26) | 0x3F00, 0];
        assert_eq!(adjust_sampler(from_negative, &bias(1.0))[2] & 0x3FFF, 0);
        assert_eq!(
            adjust_sampler(from_negative, &bias(0.5))[2] & 0x3FFF,
            0x3F80
        );
        assert_eq!(adjust_sampler(TRILINEAR, &bias(0.0)), TRILINEAR);
    }

    #[test]
    fn lod_bias_stops_at_the_field_limits() {
        let near_top = [0, 0, (2 << 26) | 0x1F00, 0];
        assert_eq!(adjust_sampler(near_top, &bias(4.0))[2] & 0x3FFF, 0x1FFF);
        let near_bottom = [0, 0, (2 << 26) | 0x2100, 0];
        assert_eq!(adjust_sampler(near_bottom, &bias(-4.0))[2] & 0x3FFF, 0x2000);
    }

    #[test]
    fn upscaling_sharpens_by_the_resolution_scale() {
        let graphics = Graphics {
            upscaler: crate::Upscaler::Temporal,
            resolution_scale: 0.5,
            ..plain()
        };
        assert_eq!(adjust_sampler(TRILINEAR, &graphics)[2] & 0x3FFF, 0x3F00);
    }

    #[test]
    fn bias_and_anisotropy_apply_together_without_touching_other_fields() {
        let console = [
            6 | (1 << 3) | (0x2 << 27),
            (0x80) | (0xA00 << 12),
            (1 << 20) | (1 << 22) | (2 << 26) | 0x40,
            (2 << 30) | 0xABC,
        ];
        let graphics = Graphics {
            texture_lod_bias: 0.25,
            ..aniso(16)
        };
        let out = adjust_sampler(console, &graphics);
        assert_eq!(out[0] & 0x7 | out[0] & (0x7 << 3), 6 | (1 << 3));
        assert_eq!(out[0] >> 27 & 0x3, 0x2);
        assert_eq!(field(out[0], 9, 3), 4);
        assert_eq!(out[1] & 0xFFF, 0x80);
        assert_eq!(field(out[1], 12, 12), 0xA00);
        assert_eq!(out[2] & 0x3FFF, 0x40 + 0x40);
        assert_eq!(out[3], console[3]);
    }

    #[test]
    fn shadow_unnormalized_min_max_and_single_level_samplers_are_left_alone() {
        let graphics = Graphics {
            texture_lod_bias: 1.0,
            ..aniso(16)
        };
        let shadow = [0x31B6, 0, (1 << 20) | (1 << 22) | (2 << 26), 2 << 30];
        let never = [0, 0, (1 << 20) | (1 << 22) | (2 << 26), 1 << 12];
        let unnormalized = [1 << 15, 0, (1 << 20) | (1 << 22) | (2 << 26), 0];
        let maximum = [2 << 29, 0, (1 << 20) | (1 << 22) | (2 << 26), 0];
        let single_level = [0, 0, (1 << 20) | (1 << 22), 0];
        for words in [shadow, never, unnormalized, maximum, single_level] {
            assert_eq!(adjust_sampler(words, &graphics), words);
        }
    }

    #[test]
    fn word3_bit_12_is_not_a_comparison_mark_with_deriv_adjust() {
        let words = [0, 0, (1 << 31) | (1 << 20) | (1 << 22) | (2 << 26), 1 << 12];
        let out = adjust_sampler(words, &aniso(16));
        assert_eq!(field(out[0], 9, 3), 4);
        assert_eq!(out[3], words[3]);
    }

    #[test]
    fn adjusting_twice_changes_nothing_more_for_anisotropy() {
        let once = adjust_sampler(TRILINEAR, &aniso(16));
        assert_eq!(adjust_sampler(once, &aniso(16)), once);
    }
}
