use rdna::{Inst, Operand};

use crate::state::{EXEC, VCC, Wave, arg, bad};
use crate::{Flow, Result};

pub fn execute(name: &str, inst: &Inst, wave: &mut Wave) -> Result<Option<Flow>> {
    if !name.starts_with("s_") {
        return Ok(None);
    }
    if let Some(flow) = program_flow(name, inst, wave)? {
        return Ok(Some(flow));
    }
    if let Some(op) = name
        .strip_prefix("s_cmp_")
        .or_else(|| name.strip_prefix("s_cmpk_"))
    {
        let a = wave.read_scalar(arg(inst, 0)?, inst)?;
        let b = wave.read_scalar(arg(inst, 1)?, inst)?;
        wave.scc = compare(op, a, b).ok_or_else(|| bad(inst))?;
        return Ok(Some(Flow::Next));
    }
    if let Some(op) = name
        .strip_suffix("_saveexec_b32")
        .or_else(|| name.strip_suffix("_saveexec_b64"))
    {
        let source = if name.ends_with("_b64") {
            wave.read_scalar64(arg(inst, 1)?, inst)?
        } else {
            u64::from(wave.read_scalar(arg(inst, 1)?, inst)?)
        };
        let old = wave.exec();
        let new = match op {
            "s_and" => source & old,
            "s_or" => source | old,
            "s_xor" => source ^ old,
            "s_andn2" => source & !old,
            "s_orn2" => source | !old,
            _ => return Err(bad(inst)),
        } & wave.full_mask();
        if name.ends_with("_b64") {
            wave.write_scalar64(arg(inst, 0)?, old, inst)?;
        } else {
            wave.write_scalar(arg(inst, 0)?, old as u32, inst)?;
        }
        wave.set_mask(EXEC, new);
        wave.scc = new != 0;
        return Ok(Some(Flow::Next));
    }
    if name.ends_with("_b64") {
        return wide(name, inst, wave).map(|done| done.then_some(Flow::Next));
    }
    narrow(name, inst, wave).map(|done| done.then_some(Flow::Next))
}

fn compare(op: &str, a: u32, b: u32) -> Option<bool> {
    let (ia, ib) = (a as i32, b as i32);
    Some(match op {
        "eq_u32" | "eq_i32" => a == b,
        "lg_u32" | "lg_i32" => a != b,
        "gt_u32" => a > b,
        "ge_u32" => a >= b,
        "lt_u32" => a < b,
        "le_u32" => a <= b,
        "gt_i32" => ia > ib,
        "ge_i32" => ia >= ib,
        "lt_i32" => ia < ib,
        "le_i32" => ia <= ib,
        _ => return None,
    })
}

fn program_flow(name: &str, inst: &Inst, wave: &mut Wave) -> Result<Option<Flow>> {
    let offset = || match inst.args.first().map(|a| &a.operand) {
        Some(Operand::Int(offset)) => Ok(*offset),
        _ => Err(bad(inst)),
    };
    let taken = match name {
        "s_endpgm" | "s_endpgm_saved" | "s_endpgm_ordered_ps_done" => return Ok(Some(Flow::End)),
        "s_nop" | "s_waitcnt" | "s_waitcnt_vscnt" | "s_barrier" | "s_clause"
        | "s_inst_prefetch" | "s_code_end" => {
            return Ok(Some(Flow::Next));
        }
        "s_branch" => true,
        "s_cbranch_scc0" => !wave.scc,
        "s_cbranch_scc1" => wave.scc,
        "s_cbranch_vccz" => wave.mask(VCC) == 0,
        "s_cbranch_vccnz" => wave.mask(VCC) != 0,
        "s_cbranch_execz" => wave.exec() == 0,
        "s_cbranch_execnz" => wave.exec() != 0,
        _ => return Ok(None),
    };
    Ok(Some(if taken {
        Flow::Jump(offset()?)
    } else {
        Flow::Next
    }))
}

fn narrow(name: &str, inst: &Inst, wave: &mut Wave) -> Result<bool> {
    let src = |index: usize| -> Result<u32> { wave.read_scalar(arg(inst, index)?, inst) };
    let (value, scc): (u32, Option<bool>) = match name {
        "s_mov_b32" | "s_movk_i32" => (src(1)?, None),
        "s_cmov_b32" | "s_cmovk_i32" => {
            if !wave.scc {
                return Ok(true);
            }
            (src(1)?, None)
        }
        "s_not_b32" => {
            let v = !src(1)?;
            (v, Some(v != 0))
        }
        "s_brev_b32" => (src(1)?.reverse_bits(), None),
        "s_bcnt1_i32_b32" => {
            let v = src(1)?.count_ones();
            (v, Some(v != 0))
        }
        "s_ff1_i32_b32" => {
            let v = src(1)?;
            (if v == 0 { u32::MAX } else { v.trailing_zeros() }, None)
        }
        "s_sext_i32_i8" => (src(1)? as u8 as i8 as i32 as u32, None),
        "s_sext_i32_i16" => (src(1)? as u16 as i16 as i32 as u32, None),
        "s_addk_i32" => {
            let (a, b) = (
                wave.read_scalar(arg(inst, 0)?, inst)? as i32,
                src(1)? as i32,
            );
            let (v, overflow) = a.overflowing_add(b);
            (v as u32, Some(overflow))
        }
        "s_mulk_i32" => (
            (wave.read_scalar(arg(inst, 0)?, inst)? as i32).wrapping_mul(src(1)? as i32) as u32,
            None,
        ),
        _ => {
            let Some(result) = binary(name, src(1)?, src(2).unwrap_or(0), wave.scc) else {
                return Ok(false);
            };
            result
        }
    };
    wave.write_scalar(arg(inst, 0)?, value, inst)?;
    if let Some(scc) = scc {
        wave.scc = scc;
    }
    Ok(true)
}

fn binary(name: &str, a: u32, b: u32, scc_in: bool) -> Option<(u32, Option<bool>)> {
    let nonzero = |v: u32| (v, Some(v != 0));
    Some(match name {
        "s_add_u32" => {
            let (v, carry) = a.overflowing_add(b);
            (v, Some(carry))
        }
        "s_addc_u32" => {
            let wide = u64::from(a) + u64::from(b) + u64::from(scc_in);
            (wide as u32, Some(wide >> 32 != 0))
        }
        "s_sub_u32" => (a.wrapping_sub(b), Some(b > a)),
        "s_subb_u32" => {
            let wide = i64::from(a) - i64::from(b) - i64::from(scc_in);
            (wide as u32, Some(wide < 0))
        }
        "s_add_i32" => {
            let (v, overflow) = (a as i32).overflowing_add(b as i32);
            (v as u32, Some(overflow))
        }
        "s_sub_i32" => {
            let (v, overflow) = (a as i32).overflowing_sub(b as i32);
            (v as u32, Some(overflow))
        }
        "s_mul_i32" => ((a as i32).wrapping_mul(b as i32) as u32, None),
        "s_and_b32" => nonzero(a & b),
        "s_or_b32" => nonzero(a | b),
        "s_xor_b32" => nonzero(a ^ b),
        "s_andn2_b32" => nonzero(a & !b),
        "s_orn2_b32" => nonzero(a | !b),
        "s_nand_b32" => nonzero(!(a & b)),
        "s_nor_b32" => nonzero(!(a | b)),
        "s_xnor_b32" => nonzero(!(a ^ b)),
        "s_lshl_b32" => nonzero(a << (b & 31)),
        "s_lshr_b32" => nonzero(a >> (b & 31)),
        "s_ashr_i32" => nonzero(((a as i32) >> (b & 31)) as u32),
        "s_min_u32" => (a.min(b), Some(a < b)),
        "s_max_u32" => (a.max(b), Some(a > b)),
        "s_min_i32" => (
            (a as i32).min(b as i32) as u32,
            Some((a as i32) < (b as i32)),
        ),
        "s_max_i32" => (
            (a as i32).max(b as i32) as u32,
            Some((a as i32) > (b as i32)),
        ),
        "s_cselect_b32" => (if scc_in { a } else { b }, None),
        "s_bfe_u32" => {
            let offset = b & 31;
            let width = (b >> 16) & 0x7F;
            let v = if width == 0 {
                0
            } else {
                (a >> offset) & (u32::MAX >> (32 - width.min(32)))
            };
            nonzero(v)
        }
        "s_lshl1_add_u32" | "s_lshl2_add_u32" | "s_lshl3_add_u32" | "s_lshl4_add_u32" => {
            let shift = u32::from(name.as_bytes()[6] - b'0');
            let wide = (u64::from(a) << shift) + u64::from(b);
            (wide as u32, Some(wide >> 32 != 0))
        }
        "s_pack_ll_b32_b16" => ((a & 0xFFFF) | (b << 16), None),
        "s_pack_lh_b32_b16" => ((a & 0xFFFF) | (b & 0xFFFF_0000), None),
        "s_pack_hh_b32_b16" => ((a >> 16) | (b & 0xFFFF_0000), None),
        _ => return None,
    })
}

fn wide(name: &str, inst: &Inst, wave: &mut Wave) -> Result<bool> {
    let src = |index: usize| -> Result<u64> { wave.read_scalar64(arg(inst, index)?, inst) };
    let nonzero = |v: u64| (v, Some(v != 0));
    let (value, scc) = match name {
        "s_mov_b64" => (src(1)?, None),
        "s_cmov_b64" => {
            if !wave.scc {
                return Ok(true);
            }
            (src(1)?, None)
        }
        "s_not_b64" => nonzero(!src(1)?),
        "s_and_b64" => nonzero(src(1)? & src(2)?),
        "s_or_b64" => nonzero(src(1)? | src(2)?),
        "s_xor_b64" => nonzero(src(1)? ^ src(2)?),
        "s_andn2_b64" => nonzero(src(1)? & !src(2)?),
        "s_orn2_b64" => nonzero(src(1)? | !src(2)?),
        "s_nand_b64" => nonzero(!(src(1)? & src(2)?)),
        "s_nor_b64" => nonzero(!(src(1)? | src(2)?)),
        "s_xnor_b64" => nonzero(!(src(1)? ^ src(2)?)),
        "s_cselect_b64" => (if wave.scc { src(1)? } else { src(2)? }, None),
        "s_lshl_b64" => nonzero(src(1)? << (wave.read_scalar(arg(inst, 2)?, inst)? & 63)),
        "s_lshr_b64" => nonzero(src(1)? >> (wave.read_scalar(arg(inst, 2)?, inst)? & 63)),
        "s_bcnt1_i32_b64" => {
            let count = src(1)?.count_ones();
            wave.write_scalar(arg(inst, 0)?, count, inst)?;
            wave.scc = count != 0;
            return Ok(true);
        }
        _ => return Ok(false),
    };
    wave.write_scalar64(arg(inst, 0)?, value, inst)?;
    if let Some(scc) = scc {
        wave.scc = scc;
    }
    Ok(true)
}
