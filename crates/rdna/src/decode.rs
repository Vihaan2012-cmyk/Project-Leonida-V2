use crate::formats::FORMATS;
use crate::inst::{Arg, Inst, Operand};
use crate::opcodes as op;
use crate::table::{Field, OpInfo, contains, lookup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid {
    pub pc: usize,
    pub len: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Decoded {
    pub insts: Vec<Inst>,
    pub invalid: usize,
}

const INLINE_FLOATS: [(u32, &str); 9] = [
    (240, "0.5"),
    (241, "-0.5"),
    (242, "1.0"),
    (243, "-1.0"),
    (244, "2.0"),
    (245, "-2.0"),
    (246, "4.0"),
    (247, "-4.0"),
    (248, "0.15915494"),
];
const VOP2_MANDATORY_LITERAL: [u32; 6] = [0x20, 0x21, 0x2C, 0x2D, 0x37, 0x38];
const MIMG_DIMS: [&str; 8] = [
    "SQ_RSRC_IMG_1D",
    "SQ_RSRC_IMG_2D",
    "SQ_RSRC_IMG_3D",
    "SQ_RSRC_IMG_CUBE",
    "SQ_RSRC_IMG_1D_ARRAY",
    "SQ_RSRC_IMG_2D_ARRAY",
    "SQ_RSRC_IMG_2D_MSAA",
    "SQ_RSRC_IMG_2D_MSAA_ARRAY",
];
const MIMG_DIM_COORDS: [usize; 8] = [1, 2, 3, 3, 2, 3, 3, 4];
const MIMG_GRAD_COORDS: [usize; 8] = [1, 2, 3, 2, 1, 2, 2, 2];
const SDWA_SELS: [&str; 8] = [
    "BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD", "DWORD",
];
const SDWA_UNUSED: [&str; 4] = ["UNUSED_PAD", "UNUSED_SEXT", "UNUSED_PRESERVE", "UNUSED_PAD"];
const SOPP_BRANCHES: [&str; 11] = [
    "s_branch",
    "s_cbranch_scc0",
    "s_cbranch_scc1",
    "s_cbranch_vccz",
    "s_cbranch_vccnz",
    "s_cbranch_execz",
    "s_cbranch_execnz",
    "s_cbranch_cdbgsys",
    "s_cbranch_cdbguser",
    "s_cbranch_cdbgsys_or_user",
    "s_cbranch_cdbgsys_and_user",
];
const SOPP_NO_OPERAND: [&str; 8] = [
    "s_endpgm",
    "s_barrier",
    "s_icache_inv",
    "s_ttracedata",
    "s_wakeup",
    "s_endpgm_saved",
    "s_code_end",
    "s_endpgm_ordered_ps_done",
];
const SOPK_SIGNED: [&str; 10] = [
    "s_movk_i32",
    "s_cmovk_i32",
    "s_cmpk_eq_i32",
    "s_cmpk_lg_i32",
    "s_cmpk_gt_i32",
    "s_cmpk_ge_i32",
    "s_cmpk_lt_i32",
    "s_cmpk_le_i32",
    "s_addk_i32",
    "s_mulk_i32",
];

pub fn decode_all(code: &[u8], wave32: bool, code_start: usize) -> Decoded {
    let mut decoded = Decoded::default();
    let mut pc = 0;
    while pc < code.len() {
        match decode_one(code, pc, wave32) {
            Ok(mut inst) => {
                pc += inst.bytes.len();
                inst.pc += code_start;
                decoded.insts.push(inst);
            }
            Err(invalid) => {
                pc += invalid.len.max(4);
                decoded.invalid += 1;
            }
        }
    }
    decoded
}

pub fn disassemble(code: &[u8], wave32: bool, code_start: usize) -> (String, usize) {
    let decoded = decode_all(code, wave32, code_start);
    let mut text = String::new();
    for inst in &decoded.insts {
        text.push_str(&inst.listing_line());
        text.push('\n');
    }
    (text, decoded.invalid)
}

pub fn decode_one(code: &[u8], pc: usize, wave32: bool) -> Result<Inst, Invalid> {
    let invalid = Invalid { pc, len: 4 };
    let words = Words { code, pc };
    let Some(w0) = words.get(0) else {
        return Err(Invalid {
            pc,
            len: code.len().saturating_sub(pc),
        });
    };
    let finish = |name: String, args: Vec<Arg>, mods: Vec<String>, len: usize| {
        Ok(Inst {
            name,
            args,
            mods,
            pc,
            bytes: code[pc..pc + len].to_vec(),
        })
    };

    if w0 >> 31 == 0 {
        return vector_32(&words, w0, wave32).map_or(Err(invalid), |(name, args, mods, len)| {
            finish(name, args, mods, len)
        });
    }
    if w0 >> 30 == 0b10 {
        return scalar(&words, w0, wave32).map_or(Err(invalid), |(name, args, mods, len)| {
            finish(name, args, mods, len)
        });
    }
    let hi6 = w0 >> 26;
    if hi6 == 0x32 {
        return vintrp(w0).map_or(Err(invalid), |(name, args, mods, len)| {
            finish(name, args, mods, len)
        });
    }
    let Some(w1) = words.get(1) else {
        return Err(invalid);
    };
    let decoded = match hi6 {
        0x33 => vop3p(&words, w0, w1, wave32),
        0x35 => vop3(&words, w0, w1, wave32),
        0x36 => ds(w0, w1, wave32),
        0x37 => flat(w0, w1, wave32),
        0x38 => mubuf(w0, w1, wave32),
        0x3A => mtbuf(w0, w1, wave32),
        0x3C => mimg(&words, w0, w1),
        0x3D => smem(w0, w1, wave32),
        0x3E => Some(export(w0, w1)),
        _ => None,
    };
    decoded.map_or(Err(invalid), |(name, args, mods, len)| {
        finish(name, args, mods, len)
    })
}

type Decode = Option<(String, Vec<Arg>, Vec<String>, usize)>;

struct Words<'a> {
    code: &'a [u8],
    pc: usize,
}

impl Words<'_> {
    fn get(&self, index: usize) -> Option<u32> {
        let at = self.pc + 4 * index;
        let bytes = self.code.get(at..at + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    fn has(&self, len: usize) -> bool {
        self.pc + len <= self.code.len()
    }
}

fn s16(value: u32) -> i64 {
    i64::from(value as u16 as i16)
}

fn sign_extend(value: u32, bits: u32) -> i64 {
    let value = i64::from(value & ((1 << bits) - 1));
    if value & (1 << (bits - 1)) != 0 {
        value - (1 << bits)
    } else {
        value
    }
}

fn width(regs: i8, wave32: bool) -> u32 {
    match regs {
        -1 if wave32 => 1,
        -1 => 2,
        n => n.max(0) as u32,
    }
}

fn count(n: u32) -> u8 {
    n.clamp(1, 255) as u8
}

fn sreg(code: u32, n: u32) -> Arg {
    match code {
        0..=105 if n <= 1 => Arg::new(Operand::Sgpr {
            reg: code as u8,
            count: 1,
        }),
        0..=105 if code + n <= 106 => Arg::new(Operand::Sgpr {
            reg: code as u8,
            count: count(n),
        }),
        0..=105 if code + n <= 108 => {
            let items = (code..code + n)
                .map(|reg| match reg {
                    0..=105 => format!("s{reg}"),
                    106 => "vcc_lo".to_string(),
                    _ => "vcc_hi".to_string(),
                })
                .collect();
            Arg::new(Operand::ScalarList(items))
        }
        0..=105 => Arg::new(Operand::Sgpr {
            reg: code as u8,
            count: count(n),
        }),
        106 => Arg::named(if n == 2 { "vcc" } else { "vcc_lo" }),
        107 => Arg::named("vcc_hi"),
        108..=123 => {
            let lo = code - 108;
            if n <= 1 {
                Arg::named(format!("ttmp{lo}"))
            } else {
                Arg::named(format!("ttmp[{lo}:{}]", lo + n - 1))
            }
        }
        124 => Arg::named("m0"),
        125 => Arg::named("null"),
        126 => Arg::named(if n == 2 { "exec" } else { "exec_lo" }),
        127 => Arg::named("exec_hi"),
        _ => Arg::named(format!("sreg_{code}")),
    }
}

fn vreg(reg: u32, n: u32) -> Arg {
    Arg::new(Operand::Vgpr {
        reg: (reg & 0xFF) as u8,
        count: count(n),
    })
}

fn src(code: u32, n: u32, literal: Option<u32>, neg: bool, abs: bool) -> Arg {
    let mut arg = match code {
        0..=127 => sreg(code, n),
        128..=192 => Arg::new(Operand::Int(i64::from(code) - 128)),
        193..=208 => Arg::new(Operand::Int(-(i64::from(code) - 192))),
        240..=248 => Arg::new(Operand::Float(INLINE_FLOATS[(code - 240) as usize].1)),
        251 => Arg::named("src_vccz"),
        252 => Arg::named("src_execz"),
        253 => Arg::named("src_scc"),
        254 => Arg::named("src_lds_direct"),
        255 => Arg::new(Operand::Literal(literal.unwrap_or(0))),
        256..=511 => vreg(code - 256, n),
        _ => Arg::named(format!("src_{code}")),
    };
    arg.neg |= neg;
    arg.abs |= abs;
    arg
}

fn plain_src(code: u32, n: u32, literal: Option<u32>) -> Arg {
    src(code, n, literal, false, false)
}

fn vcc(wave32: bool) -> Arg {
    Arg::named(if wave32 { "vcc_lo" } else { "vcc" })
}

fn off() -> Arg {
    Arg::named("off")
}

fn bit(word: u32, shift: u32) -> bool {
    (word >> shift) & 1 != 0
}

fn omod(value: u32) -> Option<&'static str> {
    match value {
        1 => Some("mul:2"),
        2 => Some("mul:4"),
        3 => Some("div:2"),
        _ => None,
    }
}

fn dpp16_ctrl(ctrl: u32) -> String {
    match ctrl {
        0..=0xFF => {
            let q: Vec<String> = (0..4)
                .map(|i| ((ctrl >> (2 * i)) & 3).to_string())
                .collect();
            format!("quad_perm:[{}]", q.join(","))
        }
        0x101..=0x10F => format!("row_shl:{}", ctrl - 0x100),
        0x111..=0x11F => format!("row_shr:{}", ctrl - 0x110),
        0x121..=0x12F => format!("row_ror:{}", ctrl - 0x120),
        0x140 => "row_mirror".to_string(),
        0x141 => "row_half_mirror".to_string(),
        0x150..=0x15F => format!("row_share:{}", ctrl - 0x150),
        0x160..=0x16F => format!("row_xmask:{}", ctrl - 0x160),
        _ => format!("dpp_ctrl:0x{ctrl:x}"),
    }
}

fn dpp8_lanes(w1: u32) -> String {
    let lanes: Vec<String> = (0..8)
        .map(|i| ((w1 >> (8 + 3 * i)) & 7).to_string())
        .collect();
    format!("dpp8:[{}]", lanes.join(","))
}

fn dpp16_mods(w1: u32, mods: &mut Vec<String>) {
    mods.push(dpp16_ctrl((w1 >> 8) & 0x1FF));
    mods.push(format!("row_mask:0x{:x}", (w1 >> 28) & 0xF));
    mods.push(format!("bank_mask:0x{:x}", (w1 >> 24) & 0xF));
    if bit(w1, 19) {
        mods.push("bound_ctrl:1".to_string());
    }
    if bit(w1, 18) {
        mods.push("fi:1".to_string());
    }
}

fn sdwa_src(reg: u32, is_sgpr: bool, sext: bool, neg: bool, abs: bool) -> Arg {
    let mut arg = if is_sgpr {
        plain_src(reg, 1, None)
    } else {
        vreg(reg, 1)
    };
    arg.sext = sext;
    arg.neg = neg;
    arg.abs = abs;
    arg
}

fn sdwa_dst_mods(w1: u32, mods: &mut Vec<String>) {
    if bit(w1, 13) {
        mods.push("clamp".to_string());
    }
    if let Some(text) = omod((w1 >> 14) & 3) {
        mods.push(text.to_string());
    }
    mods.push(format!("dst_sel:{}", SDWA_SELS[((w1 >> 8) & 7) as usize]));
    mods.push(format!(
        "dst_unused:{}",
        SDWA_UNUSED[((w1 >> 11) & 3) as usize]
    ));
    mods.push(format!("src0_sel:{}", SDWA_SELS[((w1 >> 16) & 7) as usize]));
}

fn vector_32(words: &Words<'_>, w0: u32, wave32: bool) -> Decode {
    let hi7 = w0 >> 25;
    let src0 = w0 & 0x1FF;
    let is_dpp16 = src0 == 250;
    let is_dpp8 = src0 == 233 || src0 == 234;
    let is_sdwa = src0 == 249;
    let extended = is_dpp16 || is_dpp8 || is_sdwa;

    match hi7 {
        0x3F => {
            let info = lookup(op::VOP1, (w0 >> 9) & 0xFF)?;
            let vdst = (w0 >> 17) & 0xFF;
            let mut mods = Vec::new();
            if extended {
                let w1 = words.get(1)?;
                let src0_reg = w1 & 0xFF;
                let (suffix, s0) = if is_dpp16 {
                    let mut s0 = vreg(src0_reg, 1);
                    s0.neg = bit(w1, 20);
                    s0.abs = bit(w1, 21);
                    dpp16_mods(w1, &mut mods);
                    ("_dpp", s0)
                } else if is_dpp8 {
                    mods.push(dpp8_lanes(w1));
                    if src0 == 234 {
                        mods.push("fi:1".to_string());
                    }
                    ("_dpp", vreg(src0_reg, 1))
                } else {
                    let s0 = sdwa_src(src0_reg, bit(w1, 23), bit(w1, 19), bit(w1, 20), bit(w1, 21));
                    sdwa_dst_mods(w1, &mut mods);
                    ("_sdwa", s0)
                };
                let mut args = Vec::new();
                for spec in info.spec {
                    match spec.field {
                        Field::Vdst => args.push(vreg(vdst, width(spec.regs, wave32))),
                        Field::Src0 => args.push(s0.clone()),
                        _ => {}
                    }
                }
                return Some((format!("{}{suffix}", info.name), args, mods, 8));
            }
            let (literal, len) = literal_at(words, src0 == 255, 1)?;
            let bare = info.spec.is_empty()
                || matches!(
                    info.name,
                    "v_nop" | "v_clrexcp" | "v_pipeflush" | "v_readfirstlane_b32"
                );
            let name = if bare {
                info.name.to_string()
            } else {
                format!("{}_e32", info.name)
            };
            let mut args = Vec::new();
            for spec in info.spec {
                let w = width(spec.regs, wave32);
                match spec.field {
                    Field::Vdst if info.name == "v_readfirstlane_b32" => {
                        args.push(sreg(vdst & 0x7F, 1))
                    }
                    Field::Vdst => args.push(vreg(vdst, w)),
                    Field::Src0 => args.push(plain_src(src0, w, literal)),
                    _ => {}
                }
            }
            Some((name, args, mods, len))
        }
        0x3E => {
            let info = lookup(op::VOPC, (w0 >> 17) & 0xFF)?;
            let vsrc1 = (w0 >> 9) & 0xFF;
            let cmpx = info.name.starts_with("v_cmpx_");
            if is_sdwa {
                let w1 = words.get(1)?;
                let s0 = sdwa_src(
                    w1 & 0xFF,
                    bit(w1, 23),
                    bit(w1, 19),
                    bit(w1, 20),
                    bit(w1, 21),
                );
                let s1 = sdwa_src(vsrc1, bit(w1, 31), bit(w1, 27), bit(w1, 28), bit(w1, 29));
                let mods = vec![
                    format!("src0_sel:{}", SDWA_SELS[((w1 >> 16) & 7) as usize]),
                    format!("src1_sel:{}", SDWA_SELS[((w1 >> 24) & 7) as usize]),
                ];
                let dst = if bit(w1, 15) {
                    sreg((w1 >> 8) & 0x7F, if wave32 { 1 } else { 2 })
                } else {
                    vcc(wave32)
                };
                let args = if cmpx {
                    vec![s0, s1]
                } else {
                    vec![dst, s0, s1]
                };
                return Some((format!("{}_sdwa", info.name), args, mods, 8));
            }
            let (literal, len) = literal_at(words, src0 == 255, 1)?;
            let mut args = Vec::new();
            for spec in info.spec {
                let w = width(spec.regs, wave32);
                match spec.field {
                    Field::Vcc if !cmpx => args.push(vcc(wave32)),
                    Field::Src0 => args.push(plain_src(src0, w, literal)),
                    Field::Vsrc1 => args.push(vreg(vsrc1, w)),
                    _ => {}
                }
            }
            Some((format!("{}_e32", info.name), args, Vec::new(), len))
        }
        _ => {
            let opcode = (w0 >> 25) & 0x3F;
            let info = lookup(op::VOP2, opcode)?;
            let vdst = (w0 >> 17) & 0xFF;
            let vsrc1 = (w0 >> 9) & 0xFF;
            let mut mods = Vec::new();
            if extended {
                let w1 = words.get(1)?;
                let src0_reg = w1 & 0xFF;
                let (suffix, s0, s1) = if is_dpp16 {
                    let mut s0 = vreg(src0_reg, 1);
                    s0.neg = bit(w1, 20);
                    s0.abs = bit(w1, 21);
                    let mut s1 = vreg(vsrc1, 1);
                    s1.neg = bit(w1, 22);
                    s1.abs = bit(w1, 23);
                    dpp16_mods(w1, &mut mods);
                    ("_dpp", s0, s1)
                } else if is_dpp8 {
                    mods.push(dpp8_lanes(w1));
                    if src0 == 234 {
                        mods.push("fi:1".to_string());
                    }
                    ("_dpp", vreg(src0_reg, 1), vreg(vsrc1, 1))
                } else {
                    let s0 = sdwa_src(src0_reg, bit(w1, 23), bit(w1, 19), bit(w1, 20), bit(w1, 21));
                    let s1 = sdwa_src(vsrc1, bit(w1, 31), bit(w1, 27), bit(w1, 28), bit(w1, 29));
                    sdwa_dst_mods(w1, &mut mods);
                    mods.push(format!("src1_sel:{}", SDWA_SELS[((w1 >> 24) & 7) as usize]));
                    ("_sdwa", s0, s1)
                };
                let mut args = Vec::new();
                for spec in info.spec {
                    match spec.field {
                        Field::Vdst => args.push(vreg(vdst, width(spec.regs, wave32))),
                        Field::Vcc => args.push(vcc(wave32)),
                        Field::Src0 => args.push(s0.clone()),
                        Field::Vsrc1 => args.push(s1.clone()),
                        _ => {}
                    }
                }
                return Some((format!("{}{suffix}", info.name), args, mods, 8));
            }
            let mandatory = VOP2_MANDATORY_LITERAL.contains(&opcode);
            let (literal, len) = literal_at(words, src0 == 255 || mandatory, 1)?;
            let name = if mandatory {
                info.name.to_string()
            } else {
                format!("{}_e32", info.name)
            };
            let mut args = Vec::new();
            for spec in info.spec {
                let w = width(spec.regs, wave32);
                match spec.field {
                    Field::Vdst => args.push(vreg(vdst, w)),
                    Field::Vcc => args.push(vcc(wave32)),
                    Field::Src0 => args.push(plain_src(src0, w, literal)),
                    Field::Vsrc1 => args.push(vreg(vsrc1, w)),
                    Field::Simm32 | Field::Literal => {
                        args.push(Arg::new(Operand::Literal(literal.unwrap_or(0))))
                    }
                    _ => {}
                }
            }
            Some((name, args, mods, len))
        }
    }
}

fn literal_at(words: &Words<'_>, wanted: bool, index: usize) -> Option<(Option<u32>, usize)> {
    if !wanted {
        return Some((None, 4 * index));
    }
    Some((Some(words.get(index)?), 4 * index + 4))
}

fn scalar(words: &Words<'_>, w0: u32, wave32: bool) -> Decode {
    let hi9 = w0 >> 23;
    match hi9 {
        0x17D => {
            let info = lookup(op::SOP1, (w0 >> 8) & 0xFF)?;
            let sdst = (w0 >> 16) & 0x7F;
            let ssrc0 = w0 & 0xFF;
            let (literal, len) = literal_at(words, ssrc0 == 255, 1)?;
            let mut args = Vec::new();
            for spec in info.spec {
                let w = width(spec.regs, wave32);
                match spec.field {
                    Field::Sdst => args.push(sreg(sdst, w)),
                    Field::Ssrc0 => args.push(plain_src(ssrc0, w, literal)),
                    _ => {}
                }
            }
            Some((info.name.to_string(), args, Vec::new(), len))
        }
        0x17E => {
            let info = lookup(op::SOPC, (w0 >> 16) & 0x7F)?;
            let ssrc0 = w0 & 0xFF;
            let ssrc1 = (w0 >> 8) & 0xFF;
            let (literal, len) = literal_at(words, ssrc0 == 255 || ssrc1 == 255, 1)?;
            let mut args = Vec::new();
            for spec in info.spec {
                let w = width(spec.regs, wave32);
                match spec.field {
                    Field::Ssrc0 => args.push(plain_src(ssrc0, w, literal)),
                    Field::Ssrc1 => args.push(plain_src(ssrc1, w, literal)),
                    _ => {}
                }
            }
            Some((info.name.to_string(), args, Vec::new(), len))
        }
        0x17F => {
            let info = lookup(op::SOPP, (w0 >> 16) & 0x7F)?;
            let simm16 = w0 & 0xFFFF;
            let name = info.name;
            let args = if SOPP_NO_OPERAND.contains(&name) {
                Vec::new()
            } else if name == "s_waitcnt" {
                waitcnt(simm16)
            } else if name == "s_sendmsg" || name == "s_sendmsghalt" {
                vec![Arg::named(sendmsg(simm16))]
            } else if SOPP_BRANCHES.contains(&name) {
                vec![Arg::new(Operand::Int(s16(simm16)))]
            } else {
                vec![Arg::new(Operand::Int(i64::from(simm16)))]
            };
            Some((name.to_string(), args, Vec::new(), 4))
        }
        _ if w0 >> 28 == 0xB => {
            let opcode = (w0 >> 23) & 0x1F;
            let info = lookup(op::SOPK, opcode)?;
            let sdst = (w0 >> 16) & 0x7F;
            let simm16 = w0 & 0xFFFF;
            let name = info.name;
            let (literal, len) = literal_at(words, opcode == 0x15, 1)?;
            let args = match name {
                "s_setreg_b32" => vec![Arg::named(hwreg(simm16)), sreg(sdst, 1)],
                "s_getreg_b32" => vec![sreg(sdst, 1), Arg::named(hwreg(simm16))],
                "s_setreg_imm32_b32" => vec![
                    Arg::named(hwreg(simm16)),
                    Arg::new(Operand::Int(i64::from(literal.unwrap_or(0)))),
                ],
                "s_waitcnt_vscnt" | "s_waitcnt_vmcnt" | "s_waitcnt_expcnt"
                | "s_waitcnt_lgkmcnt" => {
                    vec![sreg(sdst, 1), Arg::new(Operand::Int(i64::from(simm16)))]
                }
                "s_call_b64" => vec![sreg(sdst, 2), Arg::new(Operand::Int(s16(simm16)))],
                _ => {
                    let mut args = Vec::new();
                    for spec in info.spec {
                        match spec.field {
                            Field::Sdst => args.push(sreg(sdst, width(spec.regs, wave32))),
                            Field::Simm16 => {
                                let value = if SOPK_SIGNED.contains(&name) {
                                    s16(simm16)
                                } else {
                                    i64::from(simm16)
                                };
                                args.push(Arg::new(Operand::Int(value)));
                            }
                            _ => {}
                        }
                    }
                    args
                }
            };
            let len = if matches!(name, "s_setreg_b32" | "s_getreg_b32") {
                4
            } else {
                len
            };
            let len = if matches!(
                name,
                "s_waitcnt_vscnt"
                    | "s_waitcnt_vmcnt"
                    | "s_waitcnt_expcnt"
                    | "s_waitcnt_lgkmcnt"
                    | "s_call_b64"
            ) {
                4
            } else {
                len
            };
            Some((name.to_string(), args, Vec::new(), len))
        }
        _ => {
            let info = lookup(op::SOP2, (w0 >> 23) & 0x7F)?;
            let sdst = (w0 >> 16) & 0x7F;
            let ssrc1 = (w0 >> 8) & 0xFF;
            let ssrc0 = w0 & 0xFF;
            let (literal, len) = literal_at(words, ssrc0 == 255 || ssrc1 == 255, 1)?;
            let mut args = Vec::new();
            for spec in info.spec {
                let w = width(spec.regs, wave32);
                match spec.field {
                    Field::Sdst => args.push(sreg(sdst, w)),
                    Field::Ssrc0 => args.push(plain_src(ssrc0, w, literal)),
                    Field::Ssrc1 => args.push(plain_src(ssrc1, w, literal)),
                    _ => {}
                }
            }
            Some((info.name.to_string(), args, Vec::new(), len))
        }
    }
}

fn hwreg(simm16: u32) -> String {
    let id = simm16 & 0x3F;
    let offset = (simm16 >> 6) & 0x1F;
    let size = ((simm16 >> 11) & 0x1F) + 1;
    let name = match id {
        1 => "HW_REG_MODE".to_string(),
        2 => "HW_REG_STATUS".to_string(),
        3 => "HW_REG_TRAPSTS".to_string(),
        4 => "HW_REG_HW_ID1".to_string(),
        5 => "HW_REG_GPR_ALLOC".to_string(),
        6 => "HW_REG_LDS_ALLOC".to_string(),
        7 => "HW_REG_IB_STS".to_string(),
        15 => "HW_REG_SH_MEM_BASES".to_string(),
        16 => "HW_REG_TBA_LO".to_string(),
        17 => "HW_REG_TBA_HI".to_string(),
        18 => "HW_REG_TMA_LO".to_string(),
        19 => "HW_REG_TMA_HI".to_string(),
        20 => "HW_REG_FLAT_SCR_LO".to_string(),
        21 => "HW_REG_FLAT_SCR_HI".to_string(),
        22 => "HW_REG_XNACK_MASK".to_string(),
        23 => "HW_REG_HW_ID2".to_string(),
        24 => "HW_REG_POPS_PACKER".to_string(),
        26 => "HW_REG_SHADER_CYCLES".to_string(),
        other => other.to_string(),
    };
    if offset == 0 && size == 32 {
        format!("hwreg({name})")
    } else {
        format!("hwreg({name}, {offset}, {size})")
    }
}

fn waitcnt(simm16: u32) -> Vec<Arg> {
    let vmcnt = (simm16 & 0xF) | (((simm16 >> 14) & 3) << 4);
    let expcnt = (simm16 >> 4) & 7;
    let lgkmcnt = (simm16 >> 8) & 0x3F;
    let mut parts = Vec::new();
    if vmcnt != 63 {
        parts.push(Arg::named(format!("vmcnt({vmcnt})")));
    }
    if expcnt != 7 {
        parts.push(Arg::named(format!("expcnt({expcnt})")));
    }
    if lgkmcnt != 63 {
        parts.push(Arg::named(format!("lgkmcnt({lgkmcnt})")));
    }
    if parts.is_empty() {
        parts.push(Arg::new(Operand::Int(i64::from(simm16))));
    }
    parts
}

fn sendmsg(simm16: u32) -> String {
    let id = simm16 & 0xF;
    let op = (simm16 >> 4) & 7;
    let stream = (simm16 >> 8) & 3;
    let name = match id {
        1 => "MSG_INTERRUPT",
        2 => "MSG_GS",
        3 => "MSG_GS_DONE",
        4 => "MSG_SAVEWAVE",
        5 => "MSG_STALL_WAVE_GEN",
        6 => "MSG_HALT_WAVES",
        7 => "MSG_ORDERED_PS_DONE",
        8 => "MSG_EARLY_PRIM_DEALLOC",
        9 => "MSG_GS_ALLOC_REQ",
        10 => "MSG_GET_DOORBELL",
        11 => "MSG_GET_DDID",
        15 => "MSG_SYSMSG",
        _ => return format!("sendmsg({simm16})"),
    };
    if id == 2 || id == 3 {
        let op_name = ["GS_OP_NOP", "GS_OP_CUT", "GS_OP_EMIT", "GS_OP_EMIT_CUT"]
            .get(op as usize)
            .map_or_else(|| op.to_string(), |name| (*name).to_string());
        if op == 0 {
            return format!("sendmsg({name}|{op_name})");
        }
        return format!("sendmsg({name}|{op_name}|{stream})");
    }
    if op != 0 {
        return format!("sendmsg({name}|{op})");
    }
    format!("sendmsg({name})")
}

fn interp_param(value: u32) -> String {
    match value & 3 {
        0 => "p10".to_string(),
        1 => "p20".to_string(),
        2 => "p0".to_string(),
        _ => format!("p{value}"),
    }
}

fn vintrp(w0: u32) -> Decode {
    let info = lookup(op::VINTRP, (w0 >> 16) & 3)?;
    let vdst = (w0 >> 18) & 0xFF;
    let attr = Arg::new(Operand::Attr {
        attr: ((w0 >> 10) & 0x3F) as u8,
        chan: ((w0 >> 8) & 3) as u8,
    });
    let vsrc = w0 & 0xFF;
    let middle = if info.name == "v_interp_mov_f32" {
        Arg::named(interp_param(vsrc))
    } else {
        vreg(vsrc, 1)
    };
    Some((
        info.name.to_string(),
        vec![vreg(vdst, 1), middle, attr],
        Vec::new(),
        4,
    ))
}

fn source_count(info: &OpInfo) -> usize {
    info.spec
        .iter()
        .filter(|spec| matches!(spec.field, Field::Src0 | Field::Src1 | Field::Src2))
        .count()
}

fn bits_list(value: u32, count: usize) -> Vec<u32> {
    (0..count).map(|i| (value >> i) & 1).collect()
}

fn join_bits(bits: &[u32]) -> String {
    let parts: Vec<String> = bits.iter().map(u32::to_string).collect();
    parts.join(",")
}

fn vop3p(words: &Words<'_>, w0: u32, w1: u32, wave32: bool) -> Decode {
    let info = lookup(op::VOP3P, (w0 >> 16) & 0x7F)?;
    let vdst = w0 & 0xFF;
    let neg_hi = (w0 >> 8) & 7;
    let op_sel = (w0 >> 11) & 7;
    let op_sel_hi = ((w1 >> 27) & 3) | (((w0 >> 14) & 1) << 2);
    let neg_lo = (w1 >> 29) & 7;
    let codes = [w1 & 0x1FF, (w1 >> 9) & 0x1FF, (w1 >> 18) & 0x1FF];
    let nsrc = source_count(info);
    let (literal, len) = literal_at(words, codes[..nsrc].contains(&255), 2)?;
    let is_mix = info.name.starts_with("v_fma_mix");
    let mut args = Vec::new();
    for spec in info.spec {
        let w = width(spec.regs, wave32);
        let index = match spec.field {
            Field::Vdst => {
                args.push(vreg(vdst, w));
                continue;
            }
            Field::Src0 => 0,
            Field::Src1 => 1,
            Field::Src2 => 2,
            _ => continue,
        };
        let neg = is_mix && neg_lo & (1 << index) != 0;
        let abs = is_mix && neg_hi & (1 << index) != 0;
        args.push(src(codes[index], w, literal, neg, abs));
    }
    let mut mods = Vec::new();
    let sel = bits_list(op_sel, nsrc);
    let sel_hi = bits_list(op_sel_hi, nsrc);
    let lo = bits_list(neg_lo, nsrc);
    let hi = bits_list(neg_hi, nsrc);
    if sel.contains(&1) {
        mods.push(format!("op_sel:[{}]", join_bits(&sel)));
    }
    let default_hi = vec![u32::from(!is_mix); nsrc];
    if sel_hi != default_hi {
        mods.push(format!("op_sel_hi:[{}]", join_bits(&sel_hi)));
    }
    if !is_mix && lo.contains(&1) {
        mods.push(format!("neg_lo:[{}]", join_bits(&lo)));
    }
    if !is_mix && hi.contains(&1) {
        mods.push(format!("neg_hi:[{}]", join_bits(&hi)));
    }
    if bit(w0, 15) {
        mods.push("clamp".to_string());
    }
    Some((info.name.to_string(), args, mods, len))
}

fn vop3(words: &Words<'_>, w0: u32, w1: u32, wave32: bool) -> Decode {
    let opcode = (w0 >> 16) & 0x3FF;
    let vdst = w0 & 0xFF;
    let codes = [w1 & 0x1FF, (w1 >> 9) & 0x1FF, (w1 >> 18) & 0x1FF];
    let neg = (w1 >> 29) & 7;
    let is_vop3b = contains(op::VOP3B, opcode);
    let info = if is_vop3b {
        lookup(op::VOP3B, opcode)?
    } else {
        lookup(op::VOP3, opcode)?
    };
    let name = if opcode == 0x140 {
        "v_mad_legacy_f32"
    } else {
        info.name
    };
    let sdst = (w0 >> 8) & 0x7F;
    let abs_bits = if is_vop3b { 0 } else { (w0 >> 8) & 7 };
    let op_sel = if is_vop3b { 0 } else { (w0 >> 11) & 0xF };
    let nsrc = source_count(info);
    let (literal, len) = literal_at(words, codes[..nsrc].contains(&255), 2)?;
    let has_e32 = (opcode < 0x100 && contains(op::VOPC, opcode))
        || ((0x100..0x140).contains(&opcode) && contains(op::VOP2, opcode - 0x100))
        || ((0x180..0x200).contains(&opcode) && contains(op::VOP1, opcode - 0x180));
    let mut op_name = if has_e32 && !name.starts_with("v_mad_") && !name.starts_with("v_fma_") {
        format!("{name}_e64")
    } else {
        name.to_string()
    };
    if opcode == 0x106 || opcode == 0x11F {
        op_name = format!("{name}_e64");
    }
    let is_interp = matches!(opcode, 0x200 | 0x201 | 0x202 | 0x342 | 0x343 | 0x35A);
    let mut args = Vec::new();
    let mut mods = Vec::new();
    for spec in info.spec {
        let w = width(spec.regs, wave32);
        match spec.field {
            Field::Vdst if opcode < 0x100 => {
                if !name.starts_with("v_cmpx_") {
                    args.push(sreg(vdst & 0x7F, if wave32 { 1 } else { 2 }));
                }
            }
            Field::Vdst if matches!(name, "v_readlane_b32" | "v_readfirstlane_b32") => {
                args.push(sreg(vdst & 0x7F, 1));
            }
            Field::Vdst => args.push(vreg(vdst, w)),
            Field::Sdst => args.push(sreg(sdst, if wave32 { 1 } else { 2 })),
            Field::Src0 if is_interp => {
                args.push(Arg::new(Operand::Attr {
                    attr: (codes[0] & 0x3F) as u8,
                    chan: ((codes[0] >> 6) & 3) as u8,
                }));
                if matches!(opcode, 0x342 | 0x343 | 0x35A) && (codes[0] >> 8) & 1 != 0 {
                    mods.push("high".to_string());
                }
            }
            Field::Src0 => args.push(src(codes[0], w, literal, neg & 1 != 0, abs_bits & 1 != 0)),
            Field::Src1 if opcode == 0x202 => args.push(Arg::named(interp_param(codes[1]))),
            Field::Src1 => args.push(src(codes[1], w, literal, neg & 2 != 0, abs_bits & 2 != 0)),
            Field::Src2 => args.push(src(codes[2], w, literal, neg & 4 != 0, abs_bits & 4 != 0)),
            _ => {}
        }
    }
    if op_sel != 0 {
        let mut bits = bits_list(op_sel, nsrc);
        if opcode >= 0x100 {
            bits.push((op_sel >> 3) & 1);
        }
        mods.push(format!("op_sel:[{}]", join_bits(&bits)));
    }
    if bit(w0, 15) {
        mods.push("clamp".to_string());
    }
    if let Some(text) = omod((w1 >> 27) & 3) {
        mods.push(text.to_string());
    }
    Some((op_name, args, mods, len))
}

fn ds(w0: u32, w1: u32, wave32: bool) -> Decode {
    let info = lookup(op::DS, (w0 >> 18) & 0xFF)?;
    let offset0 = w0 & 0xFF;
    let offset1 = (w0 >> 8) & 0xFF;
    let mut args = Vec::new();
    for spec in info.spec {
        let w = width(spec.regs, wave32);
        match spec.field {
            Field::Vdst => args.push(vreg((w1 >> 24) & 0xFF, w)),
            Field::Addr => args.push(vreg(w1 & 0xFF, w)),
            Field::Data0 => args.push(vreg((w1 >> 8) & 0xFF, w)),
            Field::Data1 => args.push(vreg((w1 >> 16) & 0xFF, w)),
            _ => {}
        }
    }
    let mut mods = Vec::new();
    if info.name == "ds_swizzle_b32" {
        mods.push(ds_swizzle(offset0 | (offset1 << 8)));
    } else if info.name.contains("2_") || info.name.contains("2st64_") {
        if offset0 != 0 {
            mods.push(format!("offset0:{offset0}"));
        }
        if offset1 != 0 {
            mods.push(format!("offset1:{offset1}"));
        }
    } else if offset0 | (offset1 << 8) != 0 {
        mods.push(format!("offset:{}", offset0 | (offset1 << 8)));
    }
    if bit(w0, 17) {
        mods.push("gds".to_string());
    }
    Some((info.name.to_string(), args, mods, 8))
}

fn ds_swizzle(offset: u32) -> String {
    if offset & 0x8000 == 0 {
        let and_mask = offset & 0x1F;
        let or_mask = (offset >> 5) & 0x1F;
        let xor_mask = (offset >> 10) & 0x1F;
        if and_mask == 0x1F && or_mask == 0 {
            if matches!(xor_mask, 1 | 2 | 4 | 8 | 16) {
                return format!("offset:swizzle(SWAP,{xor_mask})");
            }
            if matches!(xor_mask, 3 | 7 | 15 | 31) {
                return format!("offset:swizzle(REVERSE,{})", xor_mask + 1);
            }
        }
        if xor_mask == 0
            && or_mask & and_mask == 0
            && matches!(0x1F ^ and_mask, 1 | 3 | 7 | 15 | 31)
        {
            return format!(
                "offset:swizzle(BROADCAST,{},{or_mask})",
                (0x1F ^ and_mask) + 1
            );
        }
        let mut chars = String::new();
        for bit_index in (0..5).rev() {
            let bits = (
                (and_mask >> bit_index) & 1,
                (or_mask >> bit_index) & 1,
                (xor_mask >> bit_index) & 1,
            );
            chars.push(match bits {
                (0, 0, 0) => '0',
                (0, 1, 0) => '1',
                (1, 0, 0) => 'p',
                (1, 0, 1) => 'i',
                _ => return format!("offset:{offset}"),
            });
        }
        return format!("offset:swizzle(BITMASK_PERM,\"{chars}\")");
    }
    if offset & 0xFF00 == 0x8000 {
        let p: Vec<String> = (0..4)
            .map(|i| ((offset >> (2 * i)) & 3).to_string())
            .collect();
        return format!("offset:swizzle(QUAD_PERM,{})", p.join(","));
    }
    format!("offset:{offset}")
}

fn flat(w0: u32, w1: u32, wave32: bool) -> Decode {
    let segment = (w0 >> 14) & 3;
    let table = match segment {
        0 => op::FLAT,
        1 => op::SCRATCH,
        2 => op::GLOBAL,
        _ => return None,
    };
    let info = lookup(table, (w0 >> 18) & 0x7F)?;
    let glc = bit(w0, 16);
    let lds = bit(w0, 13);
    let saddr = (w1 >> 16) & 0x7F;
    let saddr_off = saddr == 0x7D || saddr == 0x7F;
    let atomic = info.name.contains("_atomic_");
    let mut args = Vec::new();
    for spec in info.spec {
        let w = width(spec.regs, wave32);
        match spec.field {
            Field::Vdst => {
                if !((atomic && !glc) || (lds && segment != 0)) {
                    args.push(vreg((w1 >> 24) & 0xFF, w));
                }
            }
            Field::Addr if segment == 1 && saddr != 0x7D => args.push(off()),
            Field::Addr => {
                let address_regs = if segment == 0 || (segment == 2 && saddr_off) {
                    2
                } else {
                    1
                };
                args.push(vreg(w1 & 0xFF, address_regs));
            }
            Field::Data => args.push(vreg((w1 >> 8) & 0xFF, w)),
            Field::Saddr if segment != 0 => {
                if saddr_off {
                    args.push(off());
                } else {
                    args.push(sreg(saddr, if segment == 2 { 2 } else { 1 }));
                }
            }
            _ => {}
        }
    }
    let mut mods = Vec::new();
    let offset = if segment == 0 {
        i64::from(w0 & 0xFFF)
    } else {
        sign_extend(w0, 12)
    };
    if offset != 0 {
        mods.push(format!("offset:{offset}"));
    }
    for (set, text) in [
        (glc, "glc"),
        (bit(w0, 17), "slc"),
        (bit(w0, 12), "dlc"),
        (lds, "lds"),
    ] {
        if set {
            mods.push(text.to_string());
        }
    }
    Some((info.name.to_string(), args, mods, 8))
}

fn buffer_args(info: &OpInfo, w0: u32, w1: u32, wave32: bool) -> Vec<Arg> {
    let offen = bit(w0, 12);
    let idxen = bit(w0, 13);
    let mut args = Vec::new();
    for spec in info.spec {
        match spec.field {
            Field::Vdata => args.push(vreg((w1 >> 8) & 0xFF, width(spec.regs, wave32))),
            Field::Vaddr if !offen && !idxen => args.push(off()),
            Field::Vaddr => args.push(vreg(w1 & 0xFF, if offen && idxen { 2 } else { 1 })),
            Field::Srsrc => args.push(sreg(((w1 >> 16) & 0x1F) * 4, 4)),
            Field::Soffset => args.push(plain_src((w1 >> 24) & 0xFF, 1, None)),
            _ => {}
        }
    }
    args
}

fn mubuf(w0: u32, w1: u32, wave32: bool) -> Decode {
    let info = lookup(op::MUBUF, (w0 >> 18) & 0xFF)?;
    let args = buffer_args(info, w0, w1, wave32);
    let mut mods = Vec::new();
    if bit(w0, 13) {
        mods.push("idxen".to_string());
    }
    if bit(w0, 12) {
        mods.push("offen".to_string());
    }
    if w0 & 0xFFF != 0 {
        mods.push(format!("offset:{}", w0 & 0xFFF));
    }
    for (set, text) in [
        (bit(w0, 14), "glc"),
        (bit(w1, 22), "slc"),
        (bit(w0, 15), "dlc"),
        (bit(w0, 16), "lds"),
        (bit(w1, 23), "tfe"),
    ] {
        if set {
            mods.push(text.to_string());
        }
    }
    Some((info.name.to_string(), args, mods, 8))
}

fn mtbuf(w0: u32, w1: u32, wave32: bool) -> Decode {
    let opcode = ((w0 >> 16) & 7) | (((w1 >> 21) & 1) << 3);
    let info = lookup(op::MTBUF, opcode)?;
    let args = buffer_args(info, w0, w1, wave32);
    let mut mods = Vec::new();
    let format = (w0 >> 19) & 0x7F;
    if format != 1 {
        match FORMATS.iter().find(|(code, _)| u32::from(*code) == format) {
            Some((_, name)) => mods.push(format!(
                "format:[{}]",
                name.replace("GFX10_FORMAT_", "BUF_FMT_")
            )),
            None => mods.push(format!("format:{format}")),
        }
    }
    if bit(w0, 13) {
        mods.push("idxen".to_string());
    }
    if bit(w0, 12) {
        mods.push("offen".to_string());
    }
    if w0 & 0xFFF != 0 {
        mods.push(format!("offset:{}", w0 & 0xFFF));
    }
    for (set, text) in [
        (bit(w0, 14), "glc"),
        (bit(w1, 22), "slc"),
        (bit(w0, 15), "dlc"),
        (bit(w1, 23), "tfe"),
    ] {
        if set {
            mods.push(text.to_string());
        }
    }
    Some((info.name.to_string(), args, mods, 8))
}

fn mimg_address_count(name: &str, dim: usize, a16: bool) -> usize {
    match name {
        "image_bvh_intersect_ray" => return 11,
        "image_bvh64_intersect_ray" => return 12,
        "image_get_resinfo" => return 1,
        "image_get_lod" => return MIMG_DIM_COORDS[dim],
        _ => {}
    }
    let base = MIMG_DIM_COORDS[dim];
    if name.starts_with("image_atomic_") {
        return if a16 { base.div_ceil(2) } else { base };
    }
    let has_offset = name.ends_with("_o") || name.contains("_o_");
    let mut base_name = name.strip_suffix("_o").unwrap_or(name);
    let has_clamp = base_name.ends_with("_cl");
    if has_clamp {
        base_name = &base_name[..base_name.len() - 3];
    }
    let fixed_family = base_name.starts_with("image_atomic") || base_name.starts_with("image_get");
    let mut fixed32 = usize::from(has_offset);
    if base_name.contains("_b") && !base_name.starts_with("image_bvh") {
        fixed32 += 1;
    }
    if base_name.contains("_c") && !fixed_family {
        fixed32 += 1;
    }
    if base_name.contains("_d") && !fixed_family {
        fixed32 += 2 * MIMG_GRAD_COORDS[dim];
    }
    let mut packable = base;
    let load_store = ["image_load", "image_store", "image_atomic", "image_get"]
        .iter()
        .any(|prefix| base_name.starts_with(prefix));
    if base_name.contains("_l") && !base_name.contains("_lz") && !load_store {
        packable += 1;
    }
    if has_clamp {
        packable += 1;
    }
    if base_name == "image_load_mip" || base_name == "image_store_mip" {
        packable += 1;
    }
    if a16 {
        fixed32 + base.div_ceil(2) + (packable - base)
    } else {
        fixed32 + packable
    }
}

fn mimg_data_count(name: &str, dmask: u32, d16: bool, tfe: bool) -> usize {
    if name == "image_bvh_intersect_ray" || name == "image_bvh64_intersect_ray" {
        return 4;
    }
    if name.starts_with("image_atomic_") {
        if matches!(dmask, 1 | 3 | 15) {
            return dmask.count_ones() as usize;
        }
        let mut n = if name.contains("cmpswap") { 2 } else { 1 };
        if name.ends_with("_x2") {
            n *= 2;
        }
        return n;
    }
    let mut n = if name.starts_with("image_gather4") {
        4
    } else {
        ((dmask & 0xF).count_ones() as usize).max(1)
    };
    if d16 {
        n = n.div_ceil(2);
    }
    n + usize::from(tfe)
}

fn mimg(words: &Words<'_>, w0: u32, w1: u32) -> Decode {
    let opcode = ((w0 >> 18) & 0x7F) | ((w0 & 1) << 7);
    let info = lookup(op::MIMG, opcode)?;
    let name = info.name;
    let nsa_dwords = ((w0 >> 1) & 3) as usize;
    let dim = ((w0 >> 3) & 7) as usize;
    let dmask = (w0 >> 8) & 0xF;
    let (unorm, glc, r128, tfe, lwe) = (
        bit(w0, 12),
        bit(w0, 13),
        bit(w0, 15),
        bit(w0, 16),
        bit(w0, 17),
    );
    let (slc, a16, d16, dlc) = (bit(w0, 25), bit(w1, 30), bit(w1, 31), bit(w0, 7));
    let len = 8 + 4 * nsa_dwords;
    if !words.has(len) {
        return None;
    }
    let address_count = mimg_address_count(name, dim, a16);
    let vaddr0 = w1 & 0xFF;
    let vaddr = if nsa_dwords > 0 {
        let extra = &words.code[words.pc + 8..words.pc + len];
        let mut regs = vec![vaddr0 as u8];
        regs.extend(extra.iter().take(address_count.saturating_sub(1)));
        Arg::new(Operand::VgprList(regs))
    } else {
        vreg(vaddr0, address_count as u32)
    };
    let bvh = name.starts_with("image_bvh");
    let data_count = mimg_data_count(name, dmask, d16, tfe);
    let rsrc_regs = if r128 || bvh { 4 } else { 8 };
    let mut args = Vec::new();
    for spec in info.spec {
        match spec.field {
            Field::Vdata => args.push(vreg((w1 >> 8) & 0xFF, data_count as u32)),
            Field::Vaddr => args.push(vaddr.clone()),
            Field::Srsrc => args.push(sreg(((w1 >> 16) & 0x1F) * 4, rsrc_regs)),
            Field::Ssamp => args.push(sreg(((w1 >> 21) & 0x1F) * 4, 4)),
            _ => {}
        }
    }
    let mut mods = Vec::new();
    if dmask != 0 && !bvh {
        mods.push(format!("dmask:0x{dmask:x}"));
    }
    if !bvh {
        mods.push(format!("dim:{}", MIMG_DIMS[dim]));
        if unorm {
            mods.push("unorm".to_string());
        }
    }
    for (set, text) in [
        (glc, "glc"),
        (slc, "slc"),
        (r128 && !bvh, "r128"),
        (tfe, "tfe"),
        (lwe, "lwe"),
        (a16, "a16"),
        (d16, "d16"),
        (dlc, "dlc"),
    ] {
        if set {
            mods.push(text.to_string());
        }
    }
    Some((name.to_string(), args, mods, len))
}

fn smem(w0: u32, w1: u32, wave32: bool) -> Decode {
    let info = lookup(op::SMEM, (w0 >> 18) & 0xFF)?;
    let sbase = (w0 & 0x3F) * 2;
    let sdata = (w0 >> 6) & 0x7F;
    let soffset = (w1 >> 25) & 0x7F;
    let mut args = Vec::new();
    let mut mods = Vec::new();
    for spec in info.spec {
        let w = width(spec.regs, wave32);
        match spec.field {
            Field::Sdata => args.push(sreg(sdata, w)),
            Field::Sbase => args.push(sreg(sbase, w)),
            _ => {}
        }
    }
    if info.spec.iter().any(|spec| spec.field == Field::Soffset) {
        let immediate = if info.name.contains("buffer") {
            i64::from(w1 & 0x1FFFFF)
        } else {
            sign_extend(w1, 21)
        };
        if soffset == 125 {
            if immediate == 0 {
                args.push(Arg::named("null"));
            } else {
                args.push(Arg::new(Operand::Int(immediate)));
            }
        } else {
            args.push(sreg(soffset, 1));
            if immediate > 0 {
                mods.push(format!("offset:0x{immediate:x}"));
            } else if immediate < 0 {
                mods.push(format!("offset:-0x{:x}", -immediate));
            }
        }
    }
    if bit(w0, 16) {
        mods.push("glc".to_string());
    }
    if bit(w0, 14) {
        mods.push("dlc".to_string());
    }
    Some((info.name.to_string(), args, mods, 8))
}

fn export(w0: u32, w1: u32) -> (String, Vec<Arg>, Vec<String>, usize) {
    let enable = w0 & 0xF;
    let target = (w0 >> 4) & 0x3F;
    let compressed = bit(w0, 10);
    let target_name = match target {
        0..=7 => format!("mrt{target}"),
        8 => "mrtz".to_string(),
        9 => "null".to_string(),
        12..=16 => format!("pos{}", target - 12),
        20 => "prim".to_string(),
        32..=63 => format!("param{}", target - 32),
        _ => format!("tgt{target}"),
    };
    let sources: Vec<u32> = (0..4).map(|i| (w1 >> (8 * i)) & 0xFF).collect();
    let mut args = vec![Arg::named(target_name)];
    if compressed {
        for (pair, source) in sources.iter().take(2).enumerate() {
            let operand = if (enable >> (2 * pair)) & 3 != 0 {
                vreg(*source, 1)
            } else {
                off()
            };
            args.push(operand.clone());
            args.push(operand);
        }
    } else {
        for (i, source) in sources.iter().enumerate() {
            args.push(if (enable >> i) & 1 != 0 {
                vreg(*source, 1)
            } else {
                off()
            });
        }
    }
    let mut mods = Vec::new();
    for (set, text) in [
        (bit(w0, 11), "done"),
        (compressed, "compr"),
        (bit(w0, 12), "vm"),
    ] {
        if set {
            mods.push(text.to_string());
        }
    }
    ("exp".to_string(), args, mods, 8)
}
