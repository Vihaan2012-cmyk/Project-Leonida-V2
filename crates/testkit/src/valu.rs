use rdna::{Arg, Inst, Operand};

use crate::Result;
use crate::state::{EXEC, VCC, Wave, arg, bad};

type I1 = fn(u32) -> u32;
type I2 = fn(u32, u32) -> u32;
type I3 = fn(u32, u32, u32) -> u32;
type F1 = fn(f32) -> f32;
type F2 = fn(f32, f32) -> f32;
type F3 = fn(f32, f32, f32) -> f32;

enum Kind {
    I1(I1),
    I2(I2),
    I3(I3),
    F1(F1),
    F2(F2),
    F3(F3),
    FloatToInt(fn(f32) -> u32),
    IntToFloat(fn(u32) -> f32),
}

fn bfe(value: u32, offset: u32, width: u32) -> u32 {
    let width = width & 31;
    if width == 0 {
        0
    } else {
        (value >> (offset & 31)) & (u32::MAX >> (32 - width))
    }
}

fn med3<T: PartialOrd + Copy>(a: T, b: T, c: T) -> T {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    if c < lo {
        lo
    } else if c > hi {
        hi
    } else {
        c
    }
}

fn kind(name: &str) -> Option<Kind> {
    use Kind::*;
    Some(match name {
        "v_mov_b32" => I1(|a| a),
        "v_not_b32" => I1(|a| !a),
        "v_bfrev_b32" => I1(u32::reverse_bits),
        "v_ffbl_b32" => I1(|a| if a == 0 { u32::MAX } else { a.trailing_zeros() }),
        "v_ffbh_u32" => I1(|a| if a == 0 { u32::MAX } else { a.leading_zeros() }),
        "v_cvt_f32_ubyte0" => IntToFloat(|a| (a & 0xFF) as f32),
        "v_cvt_f32_ubyte1" => IntToFloat(|a| ((a >> 8) & 0xFF) as f32),
        "v_cvt_f32_ubyte2" => IntToFloat(|a| ((a >> 16) & 0xFF) as f32),
        "v_cvt_f32_ubyte3" => IntToFloat(|a| (a >> 24) as f32),
        "v_cvt_f32_u32" => IntToFloat(|a| a as f32),
        "v_cvt_f32_i32" => IntToFloat(|a| a as i32 as f32),
        "v_cvt_u32_f32" => FloatToInt(|a| a as u32),
        "v_cvt_i32_f32" => FloatToInt(|a| a as i32 as u32),
        "v_add_nc_u32" => I2(u32::wrapping_add),
        "v_sub_nc_u32" => I2(u32::wrapping_sub),
        "v_subrev_nc_u32" => I2(|a, b| b.wrapping_sub(a)),
        "v_mul_lo_u32" => I2(u32::wrapping_mul),
        "v_mul_hi_u32" => I2(|a, b| ((u64::from(a) * u64::from(b)) >> 32) as u32),
        "v_mul_u32_u24" => I2(|a, b| (a & 0xFF_FFFF).wrapping_mul(b & 0xFF_FFFF)),
        "v_mul_i32_i24" => {
            I2(|a, b| (((a << 8) as i32 >> 8).wrapping_mul((b << 8) as i32 >> 8)) as u32)
        }
        "v_and_b32" => I2(|a, b| a & b),
        "v_or_b32" => I2(|a, b| a | b),
        "v_xor_b32" => I2(|a, b| a ^ b),
        "v_xnor_b32" => I2(|a, b| !(a ^ b)),
        "v_lshlrev_b32" => I2(|a, b| b << (a & 31)),
        "v_lshrrev_b32" => I2(|a, b| b >> (a & 31)),
        "v_ashrrev_i32" => I2(|a, b| ((b as i32) >> (a & 31)) as u32),
        "v_lshl_b32" => I2(|a, b| a << (b & 31)),
        "v_lshr_b32" => I2(|a, b| a >> (b & 31)),
        "v_min_u32" => I2(u32::min),
        "v_max_u32" => I2(u32::max),
        "v_min_i32" => I2(|a, b| (a as i32).min(b as i32) as u32),
        "v_max_i32" => I2(|a, b| (a as i32).max(b as i32) as u32),
        "v_bfm_b32" => I2(|a, b| ((1u64 << (a & 31)) as u32).wrapping_sub(1) << (b & 31)),
        "v_bcnt_u32_b32" => I2(|a, b| a.count_ones().wrapping_add(b)),
        "v_add3_u32" => I3(|a, b, c| a.wrapping_add(b).wrapping_add(c)),
        "v_lshl_add_u32" => I3(|a, b, c| (a << (b & 31)).wrapping_add(c)),
        "v_add_lshl_u32" => I3(|a, b, c| a.wrapping_add(b) << (c & 31)),
        "v_lshl_or_b32" => I3(|a, b, c| (a << (b & 31)) | c),
        "v_and_or_b32" => I3(|a, b, c| (a & b) | c),
        "v_or3_b32" => I3(|a, b, c| a | b | c),
        "v_xor3_b32" => I3(|a, b, c| a ^ b ^ c),
        "v_xad_u32" => I3(|a, b, c| (a ^ b).wrapping_add(c)),
        "v_mad_u32_u24" => {
            I3(|a, b, c| (a & 0xFF_FFFF).wrapping_mul(b & 0xFF_FFFF).wrapping_add(c))
        }
        "v_bfe_u32" => I3(bfe),
        "v_perm_b32" => I3(perm),
        "v_bfe_i32" => I3(|a, b, c| {
            let width = c & 31;
            if width == 0 {
                0
            } else {
                (((a >> (b & 31)) << (32 - width)) as i32 >> (32 - width)) as u32
            }
        }),
        "v_bfi_b32" => I3(|a, b, c| (a & b) | (!a & c)),
        "v_alignbit_b32" => {
            I3(|a, b, c| (((u64::from(a) << 32) | u64::from(b)) >> (c & 31)) as u32)
        }
        "v_alignbyte_b32" => {
            I3(|a, b, c| (((u64::from(a) << 32) | u64::from(b)) >> (8 * (c & 3))) as u32)
        }
        "v_med3_u32" => I3(med3),
        "v_med3_i32" => I3(|a, b, c| med3(a as i32, b as i32, c as i32) as u32),
        "v_min3_u32" => I3(|a, b, c| a.min(b).min(c)),
        "v_max3_u32" => I3(|a, b, c| a.max(b).max(c)),
        "v_min3_i32" => I3(|a, b, c| (a as i32).min(b as i32).min(c as i32) as u32),
        "v_max3_i32" => I3(|a, b, c| (a as i32).max(b as i32).max(c as i32) as u32),
        "v_add_f32" => F2(|a, b| a + b),
        "v_sub_f32" => F2(|a, b| a - b),
        "v_subrev_f32" => F2(|a, b| b - a),
        "v_mul_f32" => F2(|a, b| a * b),
        "v_mul_legacy_f32" => F2(|a, b| if a == 0.0 || b == 0.0 { 0.0 } else { a * b }),
        "v_min_f32" => F2(fmin),
        "v_max_f32" => F2(fmax),
        "v_med3_f32" => F3(|a, b, c| {
            if a.is_nan() || b.is_nan() || c.is_nan() {
                fmin(fmin(a, b), c)
            } else {
                fmax(fmin(a, b), fmin(fmax(a, b), c))
            }
        }),
        "v_min3_f32" => F3(|a, b, c| fmin(fmin(a, b), c)),
        "v_max3_f32" => F3(|a, b, c| fmax(fmax(a, b), c)),
        "v_rcp_f32" | "v_rcp_iflag_f32" => F1(|a| 1.0 / a),
        "v_rsq_f32" => F1(|a| 1.0 / a.sqrt()),
        "v_sqrt_f32" => F1(f32::sqrt),
        "v_exp_f32" => F1(f32::exp2),
        "v_log_f32" => F1(f32::log2),
        "v_floor_f32" => F1(f32::floor),
        "v_ceil_f32" => F1(f32::ceil),
        "v_trunc_f32" => F1(f32::trunc),
        "v_rndne_f32" => F1(f32::round_ties_even),
        "v_fract_f32" => F1(|a| a - a.floor()),
        "v_sin_f32" => F1(|a| (a * std::f32::consts::TAU).sin()),
        "v_cos_f32" => F1(|a| (a * std::f32::consts::TAU).cos()),
        _ => return None,
    })
}

pub fn fmin(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

pub fn fmax(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() || a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

pub fn nan_rule(result: f32, inputs: &[f32]) -> f32 {
    if !result.is_nan() {
        return result;
    }
    inputs
        .iter()
        .find(|x| x.is_nan())
        .map_or(f32::from_bits(0x7FC0_0000), |x| {
            f32::from_bits(x.to_bits() | 0x40_0000)
        })
}

fn output_modifiers(wave: &Wave, inst: &Inst, value: f32) -> f32 {
    let mut value = wave.flush(value);
    for modifier in &inst.mods {
        value = wave.flush(match modifier.as_str() {
            "mul:2" => nan_rule(value * 2.0, &[value]),
            "mul:4" => nan_rule(value * 4.0, &[value]),
            "div:2" => nan_rule(value * 0.5, &[value]),
            _ => value,
        });
    }
    if inst.mods.iter().any(|m| m == "clamp") {
        value = clamp_unit(value);
    }
    value
}

pub fn execute(name: &str, inst: &Inst, wave: &mut Wave) -> Result<bool> {
    if !name.starts_with("v_") {
        return Ok(false);
    }
    if let Some(op) = name.strip_suffix("_sdwa") {
        return sdwa(op, inst, wave);
    }
    if name.starts_with("v_cmp_") || name.starts_with("v_cmpx_") {
        compare(name, inst, wave)?;
        return Ok(true);
    }
    match name {
        "v_nop" => return Ok(true),
        "v_cndmask_b32" => {
            let condition = if inst.args.len() > 3 {
                wave.mask_operand(arg(inst, 3)?, inst)?
            } else {
                wave.mask(VCC)
            };
            for lane in wave.active_lanes() {
                let pick = if condition >> lane & 1 != 0 { 2 } else { 1 };
                let value = wave.read_lane(arg(inst, pick)?, lane, inst)?;
                wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
            }
            return Ok(true);
        }
        "v_readfirstlane_b32" => {
            let lane = wave.active_lanes().first().copied().unwrap_or(0);
            let value = wave.read_lane(arg(inst, 1)?, lane, inst)?;
            wave.write_scalar(arg(inst, 0)?, value, inst)?;
            return Ok(true);
        }
        "v_readlane_b32" => {
            let lane = wave.read_scalar(arg(inst, 2)?, inst)? as usize % wave.lanes();
            let value = wave.read_lane(arg(inst, 1)?, lane, inst)?;
            wave.write_scalar(arg(inst, 0)?, value, inst)?;
            return Ok(true);
        }
        "v_writelane_b32" => {
            let lane = wave.read_scalar(arg(inst, 2)?, inst)? as usize % wave.lanes();
            let value = wave.read_scalar(arg(inst, 1)?, inst)?;
            wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
            return Ok(true);
        }
        "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
            let high = name.contains("_hi_");
            for lane in wave.active_lanes() {
                let mask = wave.read_lane(arg(inst, 1)?, lane, inst)?;
                let before = if high {
                    if lane > 32 {
                        mask & ((1u64 << (lane - 32)) - 1) as u32
                    } else {
                        0
                    }
                } else {
                    mask & ((1u64 << lane.min(32)) - 1) as u32
                };
                let value =
                    before
                        .count_ones()
                        .wrapping_add(wave.read_lane(arg(inst, 2)?, lane, inst)?);
                wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
            }
            return Ok(true);
        }
        "v_add_co_u32" | "v_sub_co_u32" | "v_subrev_co_u32" | "v_add_co_ci_u32"
        | "v_sub_co_ci_u32" | "v_subrev_co_ci_u32" => {
            carry(name, inst, wave)?;
            return Ok(true);
        }
        "v_fma_f32" | "v_mad_f32" | "v_fmaak_f32" | "v_madak_f32" | "v_fmamk_f32"
        | "v_madmk_f32" | "v_mad_legacy_f32" => {
            for lane in wave.active_lanes() {
                let a = wave.read_lane_f32(arg(inst, 1)?, lane, inst)?;
                let b = wave.read_lane_f32(arg(inst, 2)?, lane, inst)?;
                let c = wave.read_lane_f32(arg(inst, 3)?, lane, inst)?;
                let value = if name.starts_with("v_fma") {
                    nan_rule(a.mul_add(b, c), &[a, b, c])
                } else {
                    let product = if name == "v_mad_legacy_f32" && (a == 0.0 || b == 0.0) {
                        0.0
                    } else {
                        a * b
                    };
                    let product = wave.flush(nan_rule(product, &[a, b]));
                    nan_rule(product + c, &[product, c])
                };
                let value = output_modifiers(wave, inst, value);
                wave.write_lane(arg(inst, 0)?, lane, value.to_bits(), inst)?;
            }
            return Ok(true);
        }
        "v_cvt_f32_f16" | "v_cvt_f16_f32" | "v_pack_b32_f16" | "v_fma_mix_f32"
        | "v_fma_mixlo_f16" | "v_fma_mixhi_f16" => {
            half_op(name, inst, wave)?;
            return Ok(true);
        }
        "v_cvt_pk_u8_f32" => {
            for lane in wave.active_lanes() {
                let x = wave.read_lane_f32(arg(inst, 1)?, lane, inst)?;
                let byte = (x.round_ties_even() as u32).min(255);
                let shift = 8 * (wave.read_lane(arg(inst, 2)?, lane, inst)? & 3);
                let old = wave.read_lane(arg(inst, 3)?, lane, inst)?;
                let value = (old & !(0xFF << shift)) | (byte << shift);
                wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
            }
            return Ok(true);
        }
        "v_lshlrev_b64" => {
            let Operand::Vgpr { reg, .. } = arg(inst, 0)?.operand else {
                return Err(bad(inst));
            };
            for lane in wave.active_lanes() {
                let shift = wave.read_lane(arg(inst, 1)?, lane, inst)? & 63;
                let value = match arg(inst, 2)?.operand {
                    Operand::Vgpr { reg: source, .. } => {
                        let source = usize::from(source);
                        u64::from(wave.vgpr[source][lane])
                            | (u64::from(wave.vgpr[source + 1][lane]) << 32)
                    }
                    _ => wave.read_scalar64(arg(inst, 2)?, inst)?,
                } << shift;
                wave.vgpr[usize::from(reg)][lane] = value as u32;
                wave.vgpr[usize::from(reg) + 1][lane] = (value >> 32) as u32;
            }
            return Ok(true);
        }
        "v_fmac_f32" | "v_mac_f32" => {
            for lane in wave.active_lanes() {
                let a = wave.read_lane_f32(arg(inst, 1)?, lane, inst)?;
                let b = wave.read_lane_f32(arg(inst, 2)?, lane, inst)?;
                let c = wave.read_lane_f32(arg(inst, 0)?, lane, inst)?;
                let value = if name == "v_fmac_f32" {
                    nan_rule(a.mul_add(b, c), &[a, b, c])
                } else {
                    let product = wave.flush(nan_rule(a * b, &[a, b]));
                    nan_rule(product + c, &[product, c])
                };
                let value = output_modifiers(wave, inst, value);
                wave.write_lane(arg(inst, 0)?, lane, value.to_bits(), inst)?;
            }
            return Ok(true);
        }
        _ => {}
    }
    let Some(kind) = kind(name) else {
        return Ok(false);
    };
    for lane in wave.active_lanes() {
        let int = |index: usize| wave.read_lane(arg(inst, index)?, lane, inst);
        let float = |index: usize| wave.read_lane_f32(arg(inst, index)?, lane, inst);
        let value = match &kind {
            Kind::I1(op) => op(int(1)?),
            Kind::I2(op) => op(int(1)?, int(2)?),
            Kind::I3(op) => op(int(1)?, int(2)?, int(3)?),
            Kind::F1(op) => {
                let x = float(1)?;
                output_modifiers(wave, inst, nan_rule(op(x), &[x])).to_bits()
            }
            Kind::F2(op) => {
                let (x, y) = (float(1)?, float(2)?);
                let order = if name == "v_subrev_f32" {
                    [y, x]
                } else {
                    [x, y]
                };
                output_modifiers(wave, inst, nan_rule(op(x, y), &order)).to_bits()
            }
            Kind::F3(op) => {
                let (x, y, z) = (float(1)?, float(2)?, float(3)?);
                output_modifiers(wave, inst, nan_rule(op(x, y, z), &[x, y, z])).to_bits()
            }
            Kind::FloatToInt(op) => op(float(1)?),
            Kind::IntToFloat(op) => output_modifiers(wave, inst, op(int(1)?)).to_bits(),
        };
        wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
    }
    Ok(true)
}

fn carry(name: &str, inst: &Inst, wave: &mut Wave) -> Result<()> {
    let carry_in = if name.contains("_ci_") {
        let last = arg(inst, inst.args.len() - 1)?;
        wave.mask_operand(last, inst)?
    } else {
        0
    };
    let mut carry_out = 0u64;
    for lane in wave.active_lanes() {
        let a = u64::from(wave.read_lane(arg(inst, 2)?, lane, inst)?);
        let b = u64::from(wave.read_lane(arg(inst, 3)?, lane, inst)?);
        let c = carry_in >> lane & 1;
        let (value, out) = match name {
            "v_add_co_u32" | "v_add_co_ci_u32" => {
                let sum = a + b + c;
                (sum as u32, sum >> 32 != 0)
            }
            "v_sub_co_u32" | "v_sub_co_ci_u32" => {
                (a.wrapping_sub(b).wrapping_sub(c) as u32, b + c > a)
            }
            _ => (b.wrapping_sub(a).wrapping_sub(c) as u32, a + c > b),
        };
        wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
        carry_out |= u64::from(out) << lane;
    }
    let target = crate::state::sgpr_slot(arg(inst, 1)?).ok_or_else(|| bad(inst))?;
    wave.set_mask(target, carry_out);
    Ok(())
}

fn compare(name: &str, inst: &Inst, wave: &mut Wave) -> Result<()> {
    let exec_form = name.starts_with("v_cmpx_");
    let body = name
        .trim_start_matches("v_cmpx_")
        .trim_start_matches("v_cmp_");
    let (op, ty) = body.rsplit_once('_').ok_or_else(|| bad(inst))?;
    let first_source = usize::from(!exec_form);
    let mut result = 0u64;
    for lane in wave.active_lanes() {
        let a = arg(inst, first_source)?;
        let b = arg(inst, first_source + 1)?;
        let outcome = match ty {
            "f32" if op == "class" => {
                let mut bits = wave.read_lane(a, lane, inst)?;
                if a.abs {
                    bits &= 0x7FFF_FFFF;
                }
                if a.neg {
                    bits ^= 0x8000_0000;
                }
                let mask = wave.read_lane(b, lane, inst)?;
                (mask >> float_class(bits)) & 1 != 0
            }
            "f32" => {
                use std::cmp::Ordering::{Equal, Greater, Less};
                let x = wave.read_lane_f32(a, lane, inst)?;
                let y = wave.read_lane_f32(b, lane, inst)?;
                let ord = x.partial_cmp(&y);
                let (lt, eq, gt) = (ord == Some(Less), ord == Some(Equal), ord == Some(Greater));
                match op {
                    "f" => false,
                    "lt" => lt,
                    "eq" => eq,
                    "le" => lt || eq,
                    "gt" => gt,
                    "lg" => lt || gt,
                    "ge" => gt || eq,
                    "o" => ord.is_some(),
                    "u" => ord.is_none(),
                    "nge" => !(gt || eq),
                    "nlg" => !(lt || gt),
                    "ngt" => !gt,
                    "nle" => !(lt || eq),
                    "neq" => !eq,
                    "nlt" => !lt,
                    "t" | "tru" => true,
                    _ => return Err(bad(inst)),
                }
            }
            "u32" | "i32" => {
                let (x, y) = (
                    wave.read_lane(a, lane, inst)?,
                    wave.read_lane(b, lane, inst)?,
                );
                let signed = ty == "i32";
                let ordering = if signed {
                    (x as i32).cmp(&(y as i32))
                } else {
                    x.cmp(&y)
                };
                match op {
                    "f" => false,
                    "lt" => ordering.is_lt(),
                    "eq" => ordering.is_eq(),
                    "le" => ordering.is_le(),
                    "gt" => ordering.is_gt(),
                    "ne" | "lg" => ordering.is_ne(),
                    "ge" => ordering.is_ge(),
                    "t" => true,
                    _ => return Err(bad(inst)),
                }
            }
            _ => return Err(bad(inst)),
        };
        result |= u64::from(outcome) << lane;
    }
    if exec_form {
        wave.set_mask(EXEC, result);
    } else {
        let target = crate::state::sgpr_slot(arg(inst, 0)?).ok_or_else(|| bad(inst))?;
        wave.set_mask(target, result);
    }
    Ok(())
}

fn clamp_unit(value: f32) -> f32 {
    if value > 0.0 { value.min(1.0) } else { 0.0 }
}

fn float_class(bits: u32) -> u32 {
    let negative = bits >> 31 != 0;
    let exponent = (bits >> 23) & 0xFF;
    let mantissa = bits & 0x7F_FFFF;
    match (exponent, mantissa) {
        (0xFF, 0) => {
            if negative {
                2
            } else {
                9
            }
        }
        (0xFF, m) => u32::from(m & 0x40_0000 != 0),
        (0, 0) => {
            if negative {
                5
            } else {
                6
            }
        }
        (0, _) => {
            if negative {
                4
            } else {
                7
            }
        }
        _ => {
            if negative {
                3
            } else {
                8
            }
        }
    }
}

fn perm(a: u32, b: u32, selectors: u32) -> u32 {
    let data = (u64::from(a) << 32) | u64::from(b);
    let mut result = 0;
    for i in 0..4 {
        let selector = (selectors >> (8 * i)) & 0xFF;
        let byte = match selector {
            13.. => 0xFF,
            12 => 0,
            8..=11 => {
                let bit = 16 * (selector - 8) + 15;
                if data >> bit & 1 != 0 { 0xFF } else { 0 }
            }
            _ => (data >> (8 * selector)) as u32 & 0xFF,
        };
        result |= byte << (8 * i);
    }
    result
}

const SCRATCH: usize = 252;

fn sdwa_field(text: &str) -> Option<(u32, u32)> {
    match text {
        "BYTE_0" => Some((0, 8)),
        "BYTE_1" => Some((8, 8)),
        "BYTE_2" => Some((16, 8)),
        "BYTE_3" => Some((24, 8)),
        "WORD_0" => Some((0, 16)),
        "WORD_1" => Some((16, 16)),
        _ => None,
    }
}

fn sdwa_mod<'a>(inst: &'a Inst, key: &str) -> Option<&'a str> {
    inst.mods
        .iter()
        .find_map(|m| m.strip_prefix(key)?.strip_prefix(':'))
}

fn sdwa(name: &str, inst: &Inst, wave: &mut Wave) -> Result<bool> {
    let compare = name.starts_with("v_cmp");
    let sources: Vec<usize> = if name.starts_with("v_cmpx") {
        vec![0, 1]
    } else {
        (1..inst.args.len()).collect()
    };
    let saved: Vec<[u32; 64]> = (0..4).map(|i| wave.vgpr[SCRATCH + i]).collect();
    let mut modified = inst.clone();
    for (n, index) in sources.iter().enumerate() {
        let source = arg(inst, *index)?;
        let select = sdwa_mod(inst, &format!("src{n}_sel")).unwrap_or("DWORD");
        let mut values = [0u32; 64];
        for (lane, slot) in values.iter_mut().enumerate().take(wave.lanes()) {
            let raw = wave.read_lane(source, lane, inst)?;
            *slot = match sdwa_field(select) {
                None => raw,
                Some((shift, width)) if source.sext => {
                    (((raw >> shift) << (32 - width)) as i32 >> (32 - width)) as u32
                }
                Some((shift, width)) => (raw >> shift) & (u32::MAX >> (32 - width)),
            };
        }
        wave.vgpr[SCRATCH + n] = values;
        modified.args[*index] = Arg {
            operand: Operand::Vgpr {
                reg: (SCRATCH + n) as u8,
                count: 1,
            },
            neg: source.neg,
            abs: source.abs,
            sext: false,
        };
    }
    let target = if compare {
        None
    } else {
        let Operand::Vgpr { reg, .. } = arg(inst, 0)?.operand else {
            return Err(bad(inst));
        };
        modified.args[0] = Arg::new(Operand::Vgpr {
            reg: (SCRATCH + 3) as u8,
            count: 1,
        });
        wave.vgpr[SCRATCH + 3] = [0; 64];
        Some(usize::from(reg))
    };
    let done = execute(name, &modified, wave);
    if let (Ok(true), Some(target)) = (&done, target) {
        let select = sdwa_mod(inst, "dst_sel").unwrap_or("DWORD");
        let unused = sdwa_mod(inst, "dst_unused").unwrap_or("UNUSED_PRESERVE");
        for lane in wave.active_lanes() {
            let value = wave.vgpr[SCRATCH + 3][lane];
            let old = wave.vgpr[target][lane];
            wave.vgpr[target][lane] = match sdwa_field(select) {
                None => value,
                Some((shift, width)) => {
                    let mask = u32::MAX >> (32 - width);
                    match unused {
                        "UNUSED_SEXT" => {
                            ((((value << (32 - width)) as i32) >> (32 - width)) as u32) << shift
                        }
                        "UNUSED_PRESERVE" => (old & !(mask << shift)) | ((value & mask) << shift),
                        _ => (value & mask) << shift,
                    }
                }
            };
        }
    }
    for (i, lanes) in saved.into_iter().enumerate() {
        wave.vgpr[SCRATCH + i] = lanes;
    }
    done
}

fn selects(inst: &Inst, key: &str) -> [bool; 4] {
    let mut bits = [false; 4];
    if let Some(list) = inst
        .mods
        .iter()
        .find_map(|m| m.strip_prefix(key)?.strip_prefix(":["))
        .and_then(|rest| rest.strip_suffix(']'))
    {
        for (slot, value) in bits.iter_mut().zip(list.split(',')) {
            *slot = value.trim() == "1";
        }
    }
    bits
}

fn output_modified(inst: &Inst) -> bool {
    inst.mods
        .iter()
        .any(|m| m == "clamp" || m.starts_with("mul:") || m.starts_with("div:"))
}

fn half_of(
    wave: &Wave,
    source: &Arg,
    lane: usize,
    high: bool,
    modifiers: bool,
    inst: &Inst,
) -> Result<u32> {
    let mut half = match source.operand {
        Operand::Float(_) => rdna::format::f32_to_f16(source.operand.float_bits().unwrap_or(0)),
        _ => {
            let raw = wave.read_lane(source, lane, inst)?;
            (if high { raw >> 16 } else { raw }) & 0xFFFF
        }
    };
    if modifiers && source.abs {
        half &= 0x7FFF;
    }
    if modifiers && source.neg {
        half ^= 0x8000;
    }
    Ok(half)
}

fn half_op(name: &str, inst: &Inst, wave: &mut Wave) -> Result<()> {
    let Operand::Vgpr { reg, .. } = arg(inst, 0)?.operand else {
        return Err(bad(inst));
    };
    let target = usize::from(reg);
    if matches!(name, "v_cvt_f16_f32" | "v_pack_b32_f16") && output_modified(inst) {
        return Err(crate::ExecError::Unsupported {
            pc: inst.pc,
            text: inst.text(),
        });
    }
    for lane in wave.active_lanes() {
        let old = wave.vgpr[target][lane];
        let value = match name {
            "v_cvt_f32_f16" => {
                let half = half_of(wave, arg(inst, 1)?, lane, false, true, inst)?;
                let single = rdna::format::f16_to_f32(half, true);
                if output_modified(inst) {
                    output_modifiers(wave, inst, f32::from_bits(single)).to_bits()
                } else {
                    single
                }
            }
            "v_cvt_f16_f32" => {
                let source = arg(inst, 1)?;
                let mut bits = wave.read_lane(source, lane, inst)?;
                if source.abs {
                    bits &= 0x7FFF_FFFF;
                }
                if source.neg {
                    bits ^= 0x8000_0000;
                }
                (old & 0xFFFF_0000) | rdna::format::f32_to_f16(bits)
            }
            "v_pack_b32_f16" => {
                let select = selects(inst, "op_sel");
                let low = half_of(wave, arg(inst, 1)?, lane, select[0], true, inst)?;
                let high = half_of(wave, arg(inst, 2)?, lane, select[1], true, inst)?;
                low | (high << 16)
            }
            _ => {
                let halves = selects(inst, "op_sel_hi");
                let select = selects(inst, "op_sel");
                let mut values = [0.0f32; 3];
                for (i, slot) in values.iter_mut().enumerate() {
                    let source = arg(inst, i + 1)?;
                    *slot = if halves[i] {
                        let half = half_of(wave, source, lane, select[i], false, inst)?;
                        let mut value = f32::from_bits(rdna::format::f16_to_f32(half, true));
                        if source.abs {
                            value = value.abs();
                        }
                        if source.neg {
                            value = -value;
                        }
                        wave.flush(value)
                    } else {
                        wave.read_lane_f32(source, lane, inst)?
                    };
                }
                let [a, b, c] = values;
                let result = output_modifiers(wave, inst, nan_rule(a.mul_add(b, c), &[a, b, c]));
                match name {
                    "v_fma_mix_f32" => result.to_bits(),
                    "v_fma_mixlo_f16" => {
                        (old & 0xFFFF_0000) | rdna::format::f32_to_f16(result.to_bits())
                    }
                    _ => (old & 0xFFFF) | (rdna::format::f32_to_f16(result.to_bits()) << 16),
                }
            }
        };
        wave.vgpr[target][lane] = value;
    }
    Ok(())
}
