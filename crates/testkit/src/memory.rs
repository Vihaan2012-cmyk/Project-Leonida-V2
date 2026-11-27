use rdna::{Arg, Inst, Operand};

use crate::state::{Memory, Wave, arg, bad, sgpr_slot};
use crate::{ExecError, Result};

fn offset_mod(inst: &Inst, key: &str) -> i64 {
    inst.mods
        .iter()
        .find_map(|m| m.strip_prefix(key).and_then(|rest| rest.strip_prefix(':')))
        .and_then(|text| match text.strip_prefix("0x") {
            Some(hex) => i64::from_str_radix(hex, 16).ok(),
            None => match text.strip_prefix("-0x") {
                Some(hex) => i64::from_str_radix(hex, 16).ok().map(|v| -v),
                None => text.parse().ok(),
            },
        })
        .unwrap_or(0)
}

fn has(inst: &Inst, flag: &str) -> bool {
    inst.mods.iter().any(|m| m == flag)
}

fn count(arg: &Arg) -> usize {
    match arg.operand {
        Operand::Vgpr { count, .. } | Operand::Sgpr { count, .. } => usize::from(count),
        _ => 1,
    }
}

fn vreg(arg: &Arg, inst: &Inst) -> Result<usize> {
    match arg.operand {
        Operand::Vgpr { reg, .. } => Ok(usize::from(reg)),
        _ => Err(bad(inst)),
    }
}

pub fn execute(name: &str, inst: &Inst, wave: &mut Wave, memory: &mut Memory) -> Result<bool> {
    if name.starts_with("ds_") {
        lds(name, inst, wave)?;
        return Ok(true);
    }
    if name.starts_with("buffer_load_dword") || name.starts_with("buffer_store_dword") {
        buffer(name, inst, wave, memory)?;
        return Ok(true);
    }
    if name.starts_with("tbuffer_load_format_") || name.starts_with("buffer_load_format_") {
        typed(name, inst, wave, memory)?;
        return Ok(true);
    }
    if name.starts_with("tbuffer_store_format_") || name.starts_with("buffer_store_format_") {
        typed_store(name, inst, wave, memory)?;
        return Ok(true);
    }
    if let Some(op) = name.strip_prefix("buffer_atomic_") {
        buffer_atomic(op, inst, wave, memory)?;
        return Ok(true);
    }
    if name.starts_with("s_load_dword") || name.starts_with("s_buffer_load_dword") {
        scalar_load(name, inst, wave, memory)?;
        return Ok(true);
    }
    if name.starts_with("global_load_dword") || name.starts_with("global_store_dword") {
        global(name, inst, wave, memory)?;
        return Ok(true);
    }
    if matches!(
        name,
        "global_load_ubyte" | "global_load_sbyte" | "global_load_ushort" | "global_load_sshort"
    ) {
        global_small(name, inst, wave, memory)?;
        return Ok(true);
    }
    Ok(false)
}

fn lds_read(wave: &Wave, address: u64) -> Result<u32> {
    let at = address as usize;
    let bytes = wave
        .lds
        .get(at..at + 4)
        .ok_or(ExecError::OutOfBounds { address })?;
    Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
}

fn lds_write(wave: &mut Wave, address: u64, value: u32) -> Result<()> {
    let at = address as usize;
    let bytes = wave
        .lds
        .get_mut(at..at + 4)
        .ok_or(ExecError::OutOfBounds { address })?;
    bytes.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn lds_pair(name: &str, inst: &Inst, wave: &mut Wave) -> Result<()> {
    let read = name.starts_with("ds_read");
    let scale = if name.contains("st64") { 256 } else { 4 };
    let first = offset_mod(inst, "offset0") as u64 * scale;
    let second = offset_mod(inst, "offset1") as u64 * scale;
    let address_arg = arg(inst, usize::from(read))?;
    let lanes = wave.active_lanes();
    let bases: Vec<u64> = lanes
        .iter()
        .map(|lane| wave.read_lane(address_arg, *lane, inst).map(u64::from))
        .collect::<Result<_>>()?;
    if read {
        let reg = vreg(arg(inst, 0)?, inst)?;
        let mut values = Vec::new();
        for base in &bases {
            values.push((
                lds_read(wave, (base + first) & 0xFFFF_FFFF)?,
                lds_read(wave, (base + second) & 0xFFFF_FFFF)?,
            ));
        }
        for (lane, (low, high)) in lanes.iter().zip(values) {
            wave.vgpr[reg][*lane] = low;
            wave.vgpr[reg + 1][*lane] = high;
        }
    } else {
        for (index, offset) in [(1, first), (2, second)] {
            for (lane, base) in lanes.iter().zip(&bases) {
                let data = wave.read_lane(arg(inst, index)?, *lane, inst)?;
                lds_write(wave, (base + offset) & 0xFFFF_FFFF, data)?;
            }
        }
    }
    Ok(())
}

fn swizzle(inst: &Inst, wave: &mut Wave) -> Result<()> {
    let text = inst
        .mods
        .iter()
        .find_map(|m| m.strip_prefix("offset:"))
        .unwrap_or("0");
    let offset = if text.starts_with("swizzle(") {
        rdna::parse_swizzle(text)
    } else {
        text.parse().ok()
    }
    .ok_or_else(|| bad(inst))? as usize;
    let target = vreg(arg(inst, 0)?, inst)?;
    let source = wave.vgpr[vreg(arg(inst, 1)?, inst)?];
    for lane in wave.active_lanes() {
        let from = if offset & 0x8000 != 0 {
            (lane & !3) | ((offset >> (2 * (lane & 3))) & 3)
        } else {
            let (and, or, xor) = (offset & 0x1F, (offset >> 5) & 0x1F, (offset >> 10) & 0x1F);
            (lane & 32) | ((((lane & 31) & and) | or) ^ xor)
        };
        wave.vgpr[target][lane] = if wave.active(from) { source[from] } else { 0 };
    }
    Ok(())
}

fn lds(name: &str, inst: &Inst, wave: &mut Wave) -> Result<()> {
    if name == "ds_swizzle_b32" {
        return swizzle(inst, wave);
    }
    if name == "ds_bpermute_b32" {
        let offset = offset_mod(inst, "offset") as u32;
        let target = vreg(arg(inst, 0)?, inst)?;
        let source = wave.vgpr[vreg(arg(inst, 2)?, inst)?];
        let lanes = if wave.wave64 { 64 } else { 32 };
        let mut values = Vec::new();
        for lane in wave.active_lanes() {
            let address = wave
                .read_lane(arg(inst, 1)?, lane, inst)?
                .wrapping_add(offset);
            let from = (address >> 2) as usize % lanes;
            values.push((lane, if wave.active(from) { source[from] } else { 0 }));
        }
        for (lane, value) in values {
            wave.vgpr[target][lane] = value;
        }
        return Ok(());
    }
    if name.starts_with("ds_read2") || name.starts_with("ds_write2") {
        return lds_pair(name, inst, wave);
    }
    let offset = offset_mod(inst, "offset") as u64;
    let returns = name.contains("_rtn_") || name.starts_with("ds_read");
    let first = usize::from(returns);
    for lane in wave.active_lanes() {
        let address = u64::from(wave.read_lane(arg(inst, first)?, lane, inst)?) + offset;
        let address = if name.starts_with("ds_read") || name.starts_with("ds_write") {
            address
        } else {
            address & !3
        };
        match name {
            "ds_write_b32" => {
                let data = wave.read_lane(arg(inst, first + 1)?, lane, inst)?;
                lds_write(wave, address, data)?;
            }
            "ds_write_b16" | "ds_write_b16_d16_hi" => {
                let data = wave.read_lane(arg(inst, first + 1)?, lane, inst)?;
                let half = if name.ends_with("_hi") {
                    data >> 16
                } else {
                    data
                } as u16;
                let at = address as usize;
                wave.lds
                    .get_mut(at..at + 2)
                    .ok_or(ExecError::OutOfBounds { address })?
                    .copy_from_slice(&half.to_le_bytes());
            }
            "ds_read_u16" | "ds_read_u16_d16" | "ds_read_u16_d16_hi" => {
                let at = address as usize;
                let bytes = wave
                    .lds
                    .get(at..at + 2)
                    .ok_or(ExecError::OutOfBounds { address })?;
                let half = u32::from(u16::from_le_bytes(bytes.try_into().expect("2 bytes")));
                let old = wave.read_lane(arg(inst, 0)?, lane, inst)?;
                let value = match name {
                    "ds_read_u16" => half,
                    "ds_read_u16_d16" => (old & 0xFFFF_0000) | half,
                    _ => (old & 0xFFFF) | (half << 16),
                };
                wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
            }
            "ds_read_b32" => {
                let value = lds_read(wave, address)?;
                wave.write_lane(arg(inst, 0)?, lane, value, inst)?;
            }
            _ => {
                let op = name.trim_start_matches("ds_").replace("_rtn", "");
                let data = wave.read_lane(arg(inst, first + 1)?, lane, inst)?;
                let old = lds_read(wave, address)?;
                let new = match op.as_str() {
                    "add_u32" => old.wrapping_add(data),
                    "sub_u32" => old.wrapping_sub(data),
                    "min_u32" => old.min(data),
                    "max_u32" => old.max(data),
                    "min_i32" => (old as i32).min(data as i32) as u32,
                    "max_i32" => (old as i32).max(data as i32) as u32,
                    "cmpst_b32" => {
                        let source = wave.read_lane(arg(inst, first + 2)?, lane, inst)?;
                        if old == data { source } else { old }
                    }
                    "and_b32" => old & data,
                    "or_b32" => old | data,
                    "xor_b32" => old ^ data,
                    "wrxchg_rtn_b32" | "wrxchg_b32" => data,
                    _ => {
                        return Err(ExecError::Unsupported {
                            pc: inst.pc,
                            text: inst.text(),
                        });
                    }
                };
                lds_write(wave, address, new)?;
                if returns {
                    wave.write_lane(arg(inst, 0)?, lane, old, inst)?;
                }
            }
        }
    }
    Ok(())
}

struct Descriptor {
    base: u64,
    stride: u64,
    records: u64,
}

fn descriptor(wave: &Wave, arg: &Arg, inst: &Inst) -> Result<Descriptor> {
    let slot = sgpr_slot(arg).ok_or_else(|| bad(inst))?;
    let s = &wave.sgpr[slot..slot + 4];
    Ok(Descriptor {
        base: u64::from(s[0]) | (u64::from(s[1] & 0xFFFF) << 32),
        stride: u64::from((s[1] >> 16) & 0x3FFF),
        records: u64::from(s[2]),
    })
}

fn buffer(name: &str, inst: &Inst, wave: &mut Wave, memory: &mut Memory) -> Result<()> {
    let store = name.starts_with("buffer_store");
    let data = arg(inst, 0)?;
    let dwords = count(data);
    let rsrc = descriptor(wave, arg(inst, 2)?, inst)?;
    let soffset = u64::from(wave.read_scalar(arg(inst, 3)?, inst)?);
    let immediate = offset_mod(inst, "offset") as u64;
    let (offen, idxen) = (has(inst, "offen"), has(inst, "idxen"));
    let address_arg = arg(inst, 1)?;
    for lane in wave.active_lanes() {
        let (index, voffset) = match (idxen, offen) {
            (false, false) => (0, 0),
            (true, false) => (u64::from(wave.read_lane(address_arg, lane, inst)?), 0),
            (false, true) => (0, u64::from(wave.read_lane(address_arg, lane, inst)?)),
            (true, true) => {
                let reg = vreg(address_arg, inst)?;
                (
                    u64::from(wave.vgpr[reg][lane]),
                    u64::from(wave.vgpr[reg + 1][lane]),
                )
            }
        };
        let offset = voffset + immediate;
        let in_range = if rsrc.stride == 0 {
            offset < rsrc.records
        } else {
            index < rsrc.records
        };
        let start = rsrc.base + soffset + rsrc.stride * index + offset;
        let reg = vreg(data, inst)?;
        for dword in 0..dwords {
            let address = start + 4 * dword as u64;
            if store {
                if in_range {
                    memory.write(address, wave.vgpr[reg + dword][lane])?;
                }
            } else {
                wave.vgpr[reg + dword][lane] = if in_range { memory.read(address)? } else { 0 };
            }
        }
    }
    Ok(())
}

fn typed(name: &str, inst: &Inst, wave: &mut Wave, memory: &Memory) -> Result<()> {
    let count = match name.rsplit('_').next() {
        Some("x") => 1,
        Some("xy") => 2,
        Some("xyz") => 3,
        Some("xyzw") => 4,
        _ => return Err(bad(inst)),
    };
    let reg = vreg(arg(inst, 0)?, inst)?;
    let rsrc = descriptor(wave, arg(inst, 2)?, inst)?;
    let slot = sgpr_slot(arg(inst, 2)?).ok_or_else(|| bad(inst))?;
    let word3 = wave.sgpr[slot + 3];
    let (format, select) = if name.starts_with("tbuffer") {
        let code = rdna::format::instruction_format(&inst.mods).ok_or_else(|| bad(inst))?;
        (code, None)
    } else {
        ((word3 >> 12) & 0x7F, Some(word3))
    };
    let soffset = u64::from(wave.read_scalar(arg(inst, 3)?, inst)?);
    let immediate = offset_mod(inst, "offset") as u64;
    let (offen, idxen) = (has(inst, "offen"), has(inst, "idxen"));
    let address_arg = arg(inst, 1)?;
    for lane in wave.active_lanes() {
        let (index, voffset) = match (idxen, offen) {
            (false, false) => (0, 0),
            (true, false) => (u64::from(wave.read_lane(address_arg, lane, inst)?), 0),
            (false, true) => (0, u64::from(wave.read_lane(address_arg, lane, inst)?)),
            (true, true) => {
                let first = vreg(address_arg, inst)?;
                (
                    u64::from(wave.vgpr[first][lane]),
                    u64::from(wave.vgpr[first + 1][lane]),
                )
            }
        };
        let offset = (voffset + immediate) & 0xFFFF_FFFF;
        let in_range = if rsrc.stride == 0 {
            offset < rsrc.records
        } else {
            index < rsrc.records
        };
        let start = rsrc.base + rsrc.stride * index + ((offset + soffset) & 0xFFFF_FFFF);
        for component in 0..count {
            let selector = select.map_or(4 + component as u32, |w| (w >> (3 * component)) & 7);
            let mut failure = None;
            let value = rdna::format::select(format, selector, in_range, |layout| {
                let at = start as usize;
                match memory.bytes.get(at..at + layout.bytes()) {
                    Some(bytes) => layout.decode(bytes),
                    None => {
                        failure = Some(crate::ExecError::OutOfBounds { address: start });
                        [0; 4]
                    }
                }
            });
            if let Some(error) = failure {
                return Err(error);
            }
            wave.vgpr[reg + component][lane] = value;
        }
    }
    Ok(())
}

fn buffer_lane(
    inst: &Inst,
    wave: &Wave,
    rsrc: &Descriptor,
    lane: usize,
    typed: bool,
) -> Result<(u64, bool)> {
    let soffset = u64::from(wave.read_scalar(arg(inst, 3)?, inst)?);
    let immediate = offset_mod(inst, "offset") as u64;
    let (offen, idxen) = (has(inst, "offen"), has(inst, "idxen"));
    let address_arg = arg(inst, 1)?;
    let (index, voffset) = match (idxen, offen) {
        (false, false) => (0, 0),
        (true, false) => (u64::from(wave.read_lane(address_arg, lane, inst)?), 0),
        (false, true) => (0, u64::from(wave.read_lane(address_arg, lane, inst)?)),
        (true, true) => {
            let first = vreg(address_arg, inst)?;
            (
                u64::from(wave.vgpr[first][lane]),
                u64::from(wave.vgpr[first + 1][lane]),
            )
        }
    };
    let offset = voffset + immediate;
    let offset = if typed { offset & 0xFFFF_FFFF } else { offset };
    let in_range = if rsrc.stride == 0 {
        offset < rsrc.records
    } else {
        index < rsrc.records
    };
    let start = if typed {
        rsrc.base + rsrc.stride * index + ((offset + soffset) & 0xFFFF_FFFF)
    } else {
        rsrc.base + soffset + rsrc.stride * index + offset
    };
    Ok((start, in_range))
}

fn typed_store(name: &str, inst: &Inst, wave: &mut Wave, memory: &mut Memory) -> Result<()> {
    let d16 = name.contains("_d16_");
    let count = match name.rsplit('_').next() {
        Some("x") => 1,
        Some("xy") => 2,
        Some("xyz") => 3,
        Some("xyzw") => 4,
        _ => return Err(bad(inst)),
    };
    let reg = vreg(arg(inst, 0)?, inst)?;
    let rsrc = descriptor(wave, arg(inst, 2)?, inst)?;
    let slot = sgpr_slot(arg(inst, 2)?).ok_or_else(|| bad(inst))?;
    let format = if name.starts_with("tbuffer") {
        rdna::format::instruction_format(&inst.mods).ok_or_else(|| bad(inst))?
    } else {
        (wave.sgpr[slot + 3] >> 12) & 0x7F
    };
    let Some(layout) = rdna::format::buffer_format(format) else {
        return Ok(());
    };
    for lane in wave.active_lanes() {
        let (start, in_range) = buffer_lane(inst, wave, &rsrc, lane, true)?;
        if !in_range {
            continue;
        }
        let mut values = [0u32; 4];
        for (component, value) in values.iter_mut().enumerate().take(count) {
            *value = if d16 {
                let word = wave.vgpr[reg + component / 2][lane];
                let half = (word >> (16 * (component % 2))) & 0xFFFF;
                match layout.number {
                    rdna::format::Number::Uint => half,
                    rdna::format::Number::Sint => half as u16 as i16 as i32 as u32,
                    _ => rdna::format::f16_to_f32(half, true),
                }
            } else {
                wave.vgpr[reg + component][lane]
            };
        }
        let bytes = layout.encode_element(values);
        let at = start as usize;
        memory
            .bytes
            .get_mut(at..at + bytes.len())
            .ok_or(ExecError::OutOfBounds { address: start })?
            .copy_from_slice(&bytes);
    }
    Ok(())
}

fn buffer_atomic(op: &str, inst: &Inst, wave: &mut Wave, memory: &mut Memory) -> Result<()> {
    let reg = vreg(arg(inst, 0)?, inst)?;
    let rsrc = descriptor(wave, arg(inst, 2)?, inst)?;
    let returns = has(inst, "glc");
    for lane in wave.active_lanes() {
        let (start, in_range) = buffer_lane(inst, wave, &rsrc, lane, false)?;
        if !in_range {
            if returns {
                wave.vgpr[reg][lane] = 0;
            }
            continue;
        }
        let address = start & !3;
        let old = memory.read(address)?;
        let data = wave.vgpr[reg][lane];
        let new = match op {
            "swap" => data,
            "cmpswap" => {
                if old == wave.vgpr[reg + 1][lane] {
                    data
                } else {
                    old
                }
            }
            "add" => old.wrapping_add(data),
            "sub" => old.wrapping_sub(data),
            "smin" => (old as i32).min(data as i32) as u32,
            "umin" => old.min(data),
            "smax" => (old as i32).max(data as i32) as u32,
            "umax" => old.max(data),
            "and" => old & data,
            "or" => old | data,
            "xor" => old ^ data,
            _ => {
                return Err(ExecError::Unsupported {
                    pc: inst.pc,
                    text: inst.text(),
                });
            }
        };
        memory.write(address, new)?;
        if returns {
            wave.vgpr[reg][lane] = old;
        }
    }
    Ok(())
}

fn scalar_load(name: &str, inst: &Inst, wave: &mut Wave, memory: &Memory) -> Result<()> {
    let target = arg(inst, 0)?;
    let slot = sgpr_slot(target).ok_or_else(|| bad(inst))?;
    let base_arg = arg(inst, 1)?;
    let base = if name.starts_with("s_buffer") {
        descriptor(wave, base_arg, inst)?.base
    } else {
        wave.read_scalar64(base_arg, inst)?
    };
    let offset = match inst.args.get(2).map(|a| &a.operand) {
        Some(Operand::Int(value)) => *value as u64,
        Some(Operand::Named(name)) if name == "null" => 0,
        Some(_) => u64::from(wave.read_scalar(arg(inst, 2)?, inst)?)
            .wrapping_add(offset_mod(inst, "offset") as u64),
        None => 0,
    };
    for dword in 0..count(target) {
        wave.sgpr[slot + dword] = memory.read(base.wrapping_add(offset) + 4 * dword as u64)?;
    }
    Ok(())
}

fn global_small(name: &str, inst: &Inst, wave: &mut Wave, memory: &mut Memory) -> Result<()> {
    let immediate = offset_mod(inst, "offset");
    let saddr = arg(inst, 2)?;
    let scalar_base = match saddr.operand {
        Operand::Named(ref off) if off == "off" => None,
        _ => Some(wave.read_scalar64(saddr, inst)?),
    };
    let reg = vreg(arg(inst, 0)?, inst)?;
    let address_reg = vreg(arg(inst, 1)?, inst)?;
    let bits = if name.ends_with("short") { 16 } else { 8 };
    for lane in wave.active_lanes() {
        let address = match scalar_base {
            Some(base) => base + u64::from(wave.vgpr[address_reg][lane]),
            None => {
                u64::from(wave.vgpr[address_reg][lane])
                    | (u64::from(wave.vgpr[address_reg + 1][lane]) << 32)
            }
        }
        .wrapping_add(immediate as u64);
        let word = memory.read(address & !3)?;
        let raised = (word >> (8 * (address & 3))) << (32 - bits);
        wave.vgpr[reg][lane] = if name.contains("_s") {
            ((raised as i32) >> (32 - bits)) as u32
        } else {
            raised >> (32 - bits)
        };
    }
    Ok(())
}

fn global(name: &str, inst: &Inst, wave: &mut Wave, memory: &mut Memory) -> Result<()> {
    let store = name.starts_with("global_store");
    let (data, address_arg, saddr) = if store {
        (arg(inst, 1)?, arg(inst, 0)?, arg(inst, 2)?)
    } else {
        (arg(inst, 0)?, arg(inst, 1)?, arg(inst, 2)?)
    };
    let immediate = offset_mod(inst, "offset");
    let scalar_base = match saddr.operand {
        Operand::Named(ref off) if off == "off" => None,
        _ => Some(wave.read_scalar64(saddr, inst)?),
    };
    let reg = vreg(data, inst)?;
    let address_reg = vreg(address_arg, inst)?;
    for lane in wave.active_lanes() {
        let start = match scalar_base {
            Some(base) => base + u64::from(wave.vgpr[address_reg][lane]),
            None => {
                u64::from(wave.vgpr[address_reg][lane])
                    | (u64::from(wave.vgpr[address_reg + 1][lane]) << 32)
            }
        }
        .wrapping_add(immediate as u64);
        for dword in 0..count(data) {
            let address = start + 4 * dword as u64;
            if store {
                memory.write(address, wave.vgpr[reg + dword][lane])?;
            } else {
                wave.vgpr[reg + dword][lane] = memory.read(address)?;
            }
        }
    }
    Ok(())
}
