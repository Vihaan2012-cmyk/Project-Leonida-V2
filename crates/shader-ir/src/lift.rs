use std::collections::BTreeSet;

use rdna::{Arg, Inst, Operand};

use crate::ir::{Bin, Block, Cmp, Expr, Function, Reg, Stmt, Terminator, Tmp, Tri, Un};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiftError {
    #[error("invalid instruction words")]
    Invalid,
    #[error("unsupported instruction at 0x{pc:x}: {text}")]
    Unsupported { pc: usize, text: String },
    #[error("branch at 0x{pc:x} leaves the program")]
    BadBranch { pc: usize },
}

pub fn lift(code: &[u8], wave64: bool) -> Result<Function, LiftError> {
    let decoded = rdna::decode_all(code, !wave64, 0);
    if decoded.invalid > 0 {
        return Err(LiftError::Invalid);
    }
    let insts = decoded.insts;
    let mut leaders = BTreeSet::from([0usize]);
    for (index, inst) in insts.iter().enumerate() {
        if let Some(target) = branch_target(inst) {
            leaders.insert(target);
            if let Some(next) = insts.get(index + 1) {
                leaders.insert(next.pc);
            }
        } else if is_end(inst)
            && let Some(next) = insts.get(index + 1)
        {
            leaders.insert(next.pc);
        }
    }
    let leaders: Vec<usize> = leaders
        .into_iter()
        .filter(|pc| insts.iter().any(|i| i.pc == *pc))
        .collect();
    let block_of = |pc: usize, at: usize| {
        leaders
            .iter()
            .position(|l| *l == pc)
            .ok_or(LiftError::BadBranch { pc: at })
    };
    let mut lifter = Lifter {
        wave64,
        stmts: Vec::new(),
        next: 0,
        overrides: Vec::new(),
        destination: None,
        write_when: None,
    };
    let mut blocks = Vec::new();
    for (id, start) in leaders.iter().enumerate() {
        let end = leaders.get(id + 1).copied().unwrap_or(usize::MAX);
        let body: Vec<&Inst> = insts
            .iter()
            .filter(|i| i.pc >= *start && i.pc < end)
            .collect();
        let mut terminator = match leaders.get(id + 1) {
            Some(_) => Terminator::Jump(id + 1),
            None => Terminator::End,
        };
        for inst in body {
            if is_end(inst) {
                terminator = Terminator::End;
                break;
            }
            if let Some(target) = branch_target(inst) {
                let taken = block_of(target, inst.pc)?;
                terminator = match lifter.condition(inst)? {
                    None => Terminator::Jump(taken),
                    Some(cond) => Terminator::Branch(cond, taken, id + 1),
                };
                break;
            }
            lifter.inst(inst)?;
        }
        blocks.push(Block {
            pc: *start,
            stmts: std::mem::take(&mut lifter.stmts),
            terminator,
        });
    }
    let falls_off = blocks.iter().any(|block| match block.terminator {
        Terminator::Jump(target) => target == leaders.len(),
        Terminator::Branch(_, taken, fallthrough) => {
            taken == leaders.len() || fallthrough == leaders.len()
        }
        Terminator::End => false,
    });
    if falls_off {
        blocks.push(Block {
            pc: code.len(),
            stmts: Vec::new(),
            terminator: Terminator::End,
        });
    }
    Ok(Function {
        wave64,
        blocks,
        temps: lifter.next,
    })
}

#[derive(Debug, Clone, Default)]
pub struct Coverage {
    pub instructions: usize,
    pub invalid_words: usize,
    pub unsupported: Vec<String>,
}

pub fn coverage(code: &[u8], wave64: bool) -> Coverage {
    let decoded = rdna::decode_all(code, !wave64, 0);
    let mut unsupported = Vec::new();
    for inst in &decoded.insts {
        let mut lifter = Lifter {
            wave64,
            stmts: Vec::new(),
            next: 0,
            overrides: Vec::new(),
            destination: None,
            write_when: None,
        };
        let supported = if is_end(inst) {
            true
        } else if branch_target(inst).is_some() {
            lifter.condition(inst).is_ok()
        } else {
            lifter.inst(inst).is_ok()
        };
        if !supported {
            let name = inst.name.as_str();
            let base = name
                .strip_suffix("_e32")
                .or_else(|| name.strip_suffix("_e64"))
                .unwrap_or(name);
            unsupported.push(base.to_string());
        }
    }
    Coverage {
        instructions: decoded.insts.len(),
        invalid_words: decoded.invalid,
        unsupported,
    }
}

fn is_end(inst: &Inst) -> bool {
    inst.name.starts_with("s_endpgm")
}

fn branch_target(inst: &Inst) -> Option<usize> {
    if inst.name != "s_branch" && !inst.name.starts_with("s_cbranch_") {
        return None;
    }
    match inst.args.first()?.operand {
        Operand::Int(offset) => Some((inst.pc as i64 + 4 + 4 * offset) as usize),
        _ => None,
    }
}

struct Lifter {
    wave64: bool,
    stmts: Vec<Stmt>,
    next: Tmp,
    overrides: Vec<(*const Arg, Tmp)>,
    destination: Option<(Select, Select)>,
    write_when: Option<Tmp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Select {
    Byte(u32),
    Word(u32),
    Dword,
    Pad,
    Sext,
    Preserve,
}

fn selection(inst: &Inst, key: &str, default: Select) -> Option<Select> {
    let Some(text) = inst
        .mods
        .iter()
        .find_map(|m| m.strip_prefix(key)?.strip_prefix(':'))
    else {
        return Some(default);
    };
    Some(match text {
        "BYTE_0" => Select::Byte(0),
        "BYTE_1" => Select::Byte(1),
        "BYTE_2" => Select::Byte(2),
        "BYTE_3" => Select::Byte(3),
        "WORD_0" => Select::Word(0),
        "WORD_1" => Select::Word(1),
        "DWORD" => Select::Dword,
        "UNUSED_PAD" => Select::Pad,
        "UNUSED_SEXT" => Select::Sext,
        "UNUSED_PRESERVE" => Select::Preserve,
        _ => return None,
    })
}

fn field(select: Select) -> Option<(u32, u32)> {
    match select {
        Select::Byte(n) => Some((8 * n, 8)),
        Select::Word(n) => Some((16 * n, 16)),
        _ => None,
    }
}

fn unsupported(inst: &Inst) -> LiftError {
    LiftError::Unsupported {
        pc: inst.pc,
        text: inst.text(),
    }
}

impl Lifter {
    fn value(&mut self, expr: Expr) -> Tmp {
        let tmp = self.next;
        self.next += 1;
        self.stmts.push(Stmt::Let(tmp, expr));
        tmp
    }

    fn c(&mut self, value: u32) -> Tmp {
        self.value(Expr::Const(value))
    }

    fn c64(&mut self, value: u64) -> Tmp {
        self.value(Expr::Const64(value))
    }

    fn un(&mut self, op: Un, a: Tmp) -> Tmp {
        self.value(Expr::Un(op, a))
    }

    fn bin(&mut self, op: Bin, a: Tmp, b: Tmp) -> Tmp {
        self.value(Expr::Bin(op, a, b))
    }

    fn cmp(&mut self, op: Cmp, a: Tmp, b: Tmp) -> Tmp {
        self.value(Expr::Cmp(op, a, b))
    }

    fn full_mask(&mut self) -> Tmp {
        self.c64(if self.wave64 { u64::MAX } else { 0xFFFF_FFFF })
    }

    fn scalar(&mut self, arg: &Arg, inst: &Inst) -> Result<Tmp, LiftError> {
        Ok(match &arg.operand {
            Operand::Sgpr { reg, .. } => self.value(Expr::Get(Reg::S(*reg))),
            Operand::Int(value) => self.c(*value as u32),
            Operand::Literal(value) => self.c(*value),
            Operand::Float(_) => self.c(arg.operand.float_bits().unwrap_or(0)),
            Operand::Named(name) => match name.as_str() {
                "vcc_lo" | "vcc_hi" | "exec_lo" | "exec_hi" => {
                    let reg = if name.starts_with("vcc") {
                        Reg::Vcc
                    } else {
                        Reg::Exec
                    };
                    let wide = self.value(Expr::Get(reg));
                    self.un(
                        if name.ends_with("_hi") {
                            Un::Hi
                        } else {
                            Un::Lo
                        },
                        wide,
                    )
                }
                "m0" => self.value(Expr::Get(Reg::M0)),
                "null" => self.c(0),
                "src_scc" => {
                    let scc = self.value(Expr::Get(Reg::Scc));
                    self.un(Un::BoolToU32, scc)
                }
                "src_execz" | "src_vccz" => {
                    let reg = if name == "src_execz" {
                        Reg::Exec
                    } else {
                        Reg::Vcc
                    };
                    let mask = self.mask_reg(reg);
                    let zero = self.c64(0);
                    let is_zero = self.cmp(Cmp::Eq64, mask, zero);
                    self.un(Un::BoolToU32, is_zero)
                }
                _ => return Err(unsupported(inst)),
            },
            _ => return Err(unsupported(inst)),
        })
    }

    fn mask_reg(&mut self, reg: Reg) -> Tmp {
        let wide = self.value(Expr::Get(reg));
        let full = self.full_mask();
        self.bin(Bin::And64, wide, full)
    }

    fn scalar64(&mut self, arg: &Arg, inst: &Inst) -> Result<Tmp, LiftError> {
        Ok(match &arg.operand {
            Operand::Sgpr { reg, count } if *count >= 2 => self.value(Expr::GetPair(*reg)),
            Operand::Named(name) if name == "vcc" => self.value(Expr::Get(Reg::Vcc)),
            Operand::Named(name) if name == "exec" => self.value(Expr::Get(Reg::Exec)),
            Operand::Int(value) => self.c64(*value as u64),
            Operand::Literal(value) => self.c64(u64::from(*value)),
            _ => {
                let low = self.scalar(arg, inst)?;
                self.un(Un::Zext, low)
            }
        })
    }

    fn mask(&mut self, arg: &Arg, inst: &Inst) -> Result<Tmp, LiftError> {
        let value = if self.wave64 {
            self.scalar64(arg, inst)?
        } else {
            self.scalar(arg, inst)?
        };
        let value = if self.wave64 {
            value
        } else {
            self.un(Un::Zext, value)
        };
        Ok(value)
    }

    fn lane(&mut self, arg: &Arg, inst: &Inst) -> Result<Tmp, LiftError> {
        if let Some((_, value)) = self.overrides.iter().find(|(p, _)| std::ptr::eq(*p, arg)) {
            return Ok(*value);
        }
        match &arg.operand {
            Operand::Vgpr { reg, .. } => Ok(self.value(Expr::Get(Reg::V(*reg)))),
            _ => self.scalar(arg, inst),
        }
    }

    fn flane(&mut self, arg: &Arg, inst: &Inst) -> Result<Tmp, LiftError> {
        let mut value = self.lane(arg, inst)?;
        if arg.abs {
            value = self.un(Un::FAbs, value);
        }
        if arg.neg {
            value = self.un(Un::FNeg, value);
        }
        Ok(value)
    }

    fn set_scalar(&mut self, arg: &Arg, value: Tmp, inst: &Inst) -> Result<(), LiftError> {
        match &arg.operand {
            Operand::Sgpr { reg, .. } => self.stmts.push(Stmt::Set(Reg::S(*reg), value)),
            Operand::Named(name) => match name.as_str() {
                "m0" => self.stmts.push(Stmt::Set(Reg::M0, value)),
                "null" => {}
                "vcc_lo" | "vcc_hi" | "exec_lo" | "exec_hi" => {
                    let reg = if name.starts_with("vcc") {
                        Reg::Vcc
                    } else {
                        Reg::Exec
                    };
                    let old = self.value(Expr::Get(reg));
                    let combined = if name.ends_with("_hi") {
                        let low = self.un(Un::Lo, old);
                        self.bin(Bin::Pack64, low, value)
                    } else {
                        let high = self.un(Un::Hi, old);
                        self.bin(Bin::Pack64, value, high)
                    };
                    self.stmts.push(Stmt::Set(reg, combined));
                }
                _ => return Err(unsupported(inst)),
            },
            _ => return Err(unsupported(inst)),
        }
        Ok(())
    }

    fn set_scalar64(&mut self, arg: &Arg, value: Tmp, inst: &Inst) -> Result<(), LiftError> {
        match &arg.operand {
            Operand::Sgpr { reg, count } if *count >= 2 => {
                self.stmts.push(Stmt::SetPair(*reg, value))
            }
            Operand::Named(name) if name == "vcc" => self.stmts.push(Stmt::Set(Reg::Vcc, value)),
            Operand::Named(name) if name == "exec" => self.stmts.push(Stmt::Set(Reg::Exec, value)),
            _ => {
                let low = self.un(Un::Lo, value);
                self.set_scalar(arg, low, inst)?;
            }
        }
        Ok(())
    }

    fn set_mask(&mut self, arg: &Arg, mask: Tmp, inst: &Inst) -> Result<(), LiftError> {
        if self.wave64 {
            self.set_scalar64(arg, mask, inst)
        } else {
            let low = self.un(Un::Lo, mask);
            self.set_scalar(arg, low, inst)
        }
    }

    fn set_exec(&mut self, mask: Tmp) {
        if self.wave64 {
            self.stmts.push(Stmt::Set(Reg::Exec, mask));
        } else {
            let old = self.value(Expr::Get(Reg::Exec));
            let high = self.un(Un::Hi, old);
            let low = self.un(Un::Lo, mask);
            let combined = self.bin(Bin::Pack64, low, high);
            self.stmts.push(Stmt::Set(Reg::Exec, combined));
        }
    }

    fn set_lane(&mut self, arg: &Arg, value: Tmp, inst: &Inst) -> Result<(), LiftError> {
        match &arg.operand {
            Operand::Vgpr { reg, .. } => {
                let value = match self.destination {
                    Some((select, unused)) => self.place(*reg, value, select, unused),
                    None => value,
                };
                let value = match self.write_when {
                    Some(when) => {
                        let old = self.value(Expr::Get(Reg::V(*reg)));
                        self.value(Expr::Select(when, value, old))
                    }
                    None => value,
                };
                self.stmts.push(Stmt::SetLane(*reg, value));
                Ok(())
            }
            _ => Err(unsupported(inst)),
        }
    }

    fn half_source(
        &mut self,
        arg: &Arg,
        high: bool,
        modifiers: bool,
        inst: &Inst,
    ) -> Result<Tmp, LiftError> {
        let raw = match &arg.operand {
            Operand::Float(_) => {
                let bits = arg.operand.float_bits().unwrap_or(0);
                return Ok(self.half_modifiers(self_constant_half(bits), arg, modifiers));
            }
            _ => self.lane(arg, inst)?,
        };
        let raw = if high {
            let sixteen = self.c(16);
            self.bin(Bin::ShrU, raw, sixteen)
        } else {
            raw
        };
        let mask = self.c(0xFFFF);
        let half = self.bin(Bin::And, raw, mask);
        Ok(self.half_modifiers(half.into(), arg, modifiers))
    }

    fn half_modifiers(&mut self, half: Half, arg: &Arg, modifiers: bool) -> Tmp {
        let mut half = match half {
            Half::Tmp(tmp) => tmp,
            Half::Bits(bits) => self.c(bits),
        };
        if modifiers && arg.abs {
            let mask = self.c(0x7FFF);
            half = self.bin(Bin::And, half, mask);
        }
        if modifiers && arg.neg {
            let sign = self.c(0x8000);
            half = self.bin(Bin::Xor, half, sign);
        }
        half
    }

    fn keep_half(&mut self, target: u8, half: Tmp, high: bool) -> Tmp {
        let old = self.value(Expr::Get(Reg::V(target)));
        let keep = self.c(if high { 0xFFFF } else { 0xFFFF_0000 });
        let kept = self.bin(Bin::And, old, keep);
        let placed = if high {
            let sixteen = self.c(16);
            self.bin(Bin::Shl, half, sixteen)
        } else {
            half
        };
        self.bin(Bin::Or, kept, placed)
    }

    fn extract(&mut self, value: Tmp, select: Select, sext: bool) -> Tmp {
        let Some((shift, width)) = field(select) else {
            return value;
        };
        if sext {
            let up = self.c(32 - width - shift);
            let raised = self.bin(Bin::Shl, value, up);
            let down = self.c(32 - width);
            self.bin(Bin::ShrS, raised, down)
        } else {
            let amount = self.c(shift);
            let shifted = self.bin(Bin::ShrU, value, amount);
            let mask = self.c(u32::MAX >> (32 - width));
            self.bin(Bin::And, shifted, mask)
        }
    }

    fn place(&mut self, reg: u8, value: Tmp, select: Select, unused: Select) -> Tmp {
        let Some((shift, width)) = field(select) else {
            return value;
        };
        let mask = self.c(u32::MAX >> (32 - width));
        let low = self.bin(Bin::And, value, mask);
        let amount = self.c(shift);
        let placed = self.bin(Bin::Shl, low, amount);
        match unused {
            Select::Sext => {
                let up = self.c(32 - width);
                let raised = self.bin(Bin::Shl, value, up);
                let extended = self.bin(Bin::ShrS, raised, up);
                self.bin(Bin::Shl, extended, amount)
            }
            Select::Preserve => {
                let old = self.value(Expr::Get(Reg::V(reg)));
                let hole = self.c(!((u32::MAX >> (32 - width)) << shift));
                let kept = self.bin(Bin::And, old, hole);
                self.bin(Bin::Or, kept, placed)
            }
            _ => placed,
        }
    }

    fn sdwa(&mut self, name: &str, inst: &Inst) -> Result<(), LiftError> {
        let compare = name.starts_with("v_cmp");
        let sources: Vec<usize> = if name.starts_with("v_cmpx") {
            vec![0, 1]
        } else {
            (1..inst.args.len()).collect()
        };
        for (n, index) in sources.into_iter().enumerate() {
            let arg = inst.args.get(index).ok_or_else(|| unsupported(inst))?;
            let select = selection(inst, &format!("src{n}_sel"), Select::Dword)
                .ok_or_else(|| unsupported(inst))?;
            let raw = match &arg.operand {
                Operand::Vgpr { reg, .. } => self.value(Expr::Get(Reg::V(*reg))),
                _ => self.scalar(arg, inst)?,
            };
            let value = self.extract(raw, select, arg.sext);
            self.overrides.push((arg as *const Arg, value));
        }
        if !compare {
            let select =
                selection(inst, "dst_sel", Select::Dword).ok_or_else(|| unsupported(inst))?;
            let unused =
                selection(inst, "dst_unused", Select::Preserve).ok_or_else(|| unsupported(inst))?;
            self.destination = Some((select, unused));
        }
        let result = if compare {
            self.vector_compare(name, inst)
        } else if self.vector_op(name, inst)? {
            Ok(())
        } else {
            Err(unsupported(inst))
        };
        self.overrides.clear();
        self.destination = None;
        result
    }

    fn lane_active(&mut self, lane: Tmp) -> Tmp {
        let exec = self.mask_reg(Reg::Exec);
        self.bin(Bin::LaneBit, exec, lane)
    }

    fn dpp(&mut self, name: &str, inst: &Inst) -> Result<(), LiftError> {
        if name.starts_with("v_cmp") {
            return Err(unsupported(inst));
        }
        let source_arg = inst.args.get(1).ok_or_else(|| unsupported(inst))?;
        let Operand::Vgpr { reg, .. } = source_arg.operand else {
            return Err(unsupported(inst));
        };
        let raw = self.value(Expr::Get(Reg::V(reg)));
        let lane = self.value(Expr::LaneId);
        let text = |key: &str| {
            inst.mods
                .iter()
                .find_map(|m| m.strip_prefix(key)?.strip_prefix(':'))
        };
        let flag = |key: &str| inst.mods.iter().any(|m| m == &format!("{key}:1"));
        let number = |value: &str| match value.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => value.parse().ok(),
        };
        let fetch_inactive = flag("fi");
        let (from, in_row, bound) = if let Some(list) = text("dpp8") {
            let lanes: Vec<u32> = list
                .trim_matches(['[', ']'])
                .split(',')
                .map(|l| l.trim().parse().unwrap_or(0))
                .collect();
            if lanes.len() != 8 {
                return Err(unsupported(inst));
            }
            let packed = lanes
                .iter()
                .enumerate()
                .fold(0u32, |p, (i, l)| p | ((l & 7) << (3 * i)));
            let seven = self.c(7);
            let group_lane = self.bin(Bin::And, lane, seven);
            let three = self.c(3);
            let shift = self.value(Expr::Bin(Bin::Mul, group_lane, three));
            let pattern = self.c(packed);
            let select = self.bin(Bin::ShrU, pattern, shift);
            let select = self.bin(Bin::And, select, seven);
            let group = self.c(!7);
            let base = self.bin(Bin::And, lane, group);
            let from = self.bin(Bin::Or, base, select);
            let yes = self.cmp(Cmp::True, lane, lane);
            (from, yes, true)
        } else {
            let fifteen = self.c(15);
            let r = self.bin(Bin::And, lane, fifteen);
            let yes = self.cmp(Cmp::True, lane, lane);
            let (within, inside) = if let Some(list) = text("quad_perm") {
                let quad: Vec<u32> = list
                    .trim_matches(['[', ']'])
                    .split(',')
                    .map(|l| l.trim().parse().unwrap_or(0))
                    .collect();
                let packed = quad
                    .iter()
                    .enumerate()
                    .fold(0u32, |p, (i, l)| p | ((l & 3) << (2 * i)));
                let three = self.c(3);
                let quad_lane = self.bin(Bin::And, r, three);
                let one = self.c(1);
                let shift = self.bin(Bin::Shl, quad_lane, one);
                let pattern = self.c(packed);
                let select = self.bin(Bin::ShrU, pattern, shift);
                let select = self.bin(Bin::And, select, three);
                let quad = self.c(12);
                let quad_base = self.bin(Bin::And, r, quad);
                (self.bin(Bin::Or, quad_base, select), yes)
            } else if inst.mods.iter().any(|m| m == "row_mirror") {
                (self.bin(Bin::Sub, fifteen, r), yes)
            } else if inst.mods.iter().any(|m| m == "row_half_mirror") {
                let eight = self.c(8);
                let half = self.bin(Bin::And, r, eight);
                let seven = self.c(7);
                let low = self.bin(Bin::And, r, seven);
                let flipped = self.bin(Bin::Sub, seven, low);
                (self.bin(Bin::Or, half, flipped), yes)
            } else if let Some(n) = text("row_shl").and_then(number) {
                let n = self.c(n);
                let s = self.bin(Bin::Add, r, n);
                let sixteen = self.c(16);
                let inside = self.cmp(Cmp::LtU, s, sixteen);
                (self.bin(Bin::And, s, fifteen), inside)
            } else if let Some(n) = text("row_shr").and_then(number) {
                let n = self.c(n);
                let s = self.bin(Bin::Sub, r, n);
                let inside = self.cmp(Cmp::GeU, r, n);
                (self.bin(Bin::And, s, fifteen), inside)
            } else if let Some(n) = text("row_ror").and_then(number) {
                let n = self.c(n);
                let s = self.bin(Bin::Sub, r, n);
                (self.bin(Bin::And, s, fifteen), yes)
            } else if let Some(n) = text("row_share").and_then(number) {
                (self.c(n & 15), yes)
            } else if let Some(n) = text("row_xmask").and_then(number) {
                let n = self.c(n & 15);
                (self.bin(Bin::Xor, r, n), yes)
            } else {
                return Err(unsupported(inst));
            };
            let row = self.c(!15);
            let base = self.bin(Bin::And, lane, row);
            (self.bin(Bin::Or, base, within), inside, flag("bound_ctrl"))
        };
        let source_ok = if fetch_inactive {
            in_row
        } else {
            let active = self.lane_active(from);
            self.value(Expr::Select(in_row, active, in_row))
        };
        let value = self.value(Expr::Shuffle(raw, from));
        let zero = self.c(0);
        let value = self.value(Expr::Select(source_ok, value, zero));
        let mut write = if bound {
            self.cmp(Cmp::True, lane, lane)
        } else {
            source_ok
        };
        if text("dpp8").is_none() {
            for (key, shift) in [("row_mask", 4), ("bank_mask", 2)] {
                let mask = text(key).and_then(number).unwrap_or(0xF);
                let amount = self.c(shift);
                let index = self.bin(Bin::ShrU, lane, amount);
                let three = self.c(3);
                let index = self.bin(Bin::And, index, three);
                let bits = self.c(mask);
                let bit = self.bin(Bin::ShrU, bits, index);
                let one = self.c(1);
                let bit = self.bin(Bin::And, bit, one);
                let enabled = self.cmp(Cmp::Ne, bit, zero);
                write = self.value(Expr::Select(enabled, write, enabled));
            }
        }
        self.overrides.push((source_arg as *const Arg, value));
        self.write_when = Some(write);
        let result = if self.vector_op(name, inst)? {
            Ok(())
        } else {
            Err(unsupported(inst))
        };
        self.overrides.clear();
        self.write_when = None;
        result
    }

    fn perm(&mut self, a: Tmp, b: Tmp, c: Tmp) -> Tmp {
        let data = self.bin(Bin::Pack64, b, a);
        let mut result = self.c(0);
        let (byte, zero, ones) = (self.c(0xFF), self.c(0), self.c(0xFF));
        for i in 0..4 {
            let shift = self.c(8 * i);
            let selector = self.bin(Bin::ShrU, c, shift);
            let selector = self.bin(Bin::And, selector, byte);
            let seven = self.c(7);
            let index = self.bin(Bin::And, selector, seven);
            let three = self.c(3);
            let bits = self.bin(Bin::Shl, index, three);
            let bits = self.un(Un::Zext, bits);
            let moved = self.bin(Bin::ShrU64, data, bits);
            let moved = self.un(Un::Lo, moved);
            let plain = self.bin(Bin::And, moved, byte);
            let pair = self.bin(Bin::And, selector, three);
            let four = self.c(4);
            let top = self.bin(Bin::Shl, pair, four);
            let fifteen = self.c(15);
            let top = self.bin(Bin::Add, top, fifteen);
            let top = self.un(Un::Zext, top);
            let sign = self.bin(Bin::ShrU64, data, top);
            let sign = self.un(Un::Lo, sign);
            let one = self.c(1);
            let sign = self.bin(Bin::And, sign, one);
            let negative = self.cmp(Cmp::Ne, sign, zero);
            let sign = self.value(Expr::Select(negative, ones, zero));
            let eight = self.c(8);
            let high = self.cmp(Cmp::GeU, selector, eight);
            let value = self.value(Expr::Select(high, sign, plain));
            let twelve = self.c(12);
            let blank = self.cmp(Cmp::Eq, selector, twelve);
            let value = self.value(Expr::Select(blank, zero, value));
            let thirteen = self.c(13);
            let full = self.cmp(Cmp::GeU, selector, thirteen);
            let value = self.value(Expr::Select(full, ones, value));
            let value = self.bin(Bin::Shl, value, shift);
            result = self.bin(Bin::Or, result, value);
        }
        result
    }

    fn condition(&mut self, inst: &Inst) -> Result<Option<Tmp>, LiftError> {
        let zero = |me: &mut Self, reg: Reg| {
            let mask = me.mask_reg(reg);
            let zero = me.c64(0);
            me.cmp(Cmp::Eq64, mask, zero)
        };
        Ok(Some(match inst.name.as_str() {
            "s_branch" => return Ok(None),
            "s_cbranch_scc1" => self.value(Expr::Get(Reg::Scc)),
            "s_cbranch_scc0" => {
                let scc = self.value(Expr::Get(Reg::Scc));
                let as_int = self.un(Un::BoolToU32, scc);
                let zero = self.c(0);
                self.cmp(Cmp::Eq, as_int, zero)
            }
            "s_cbranch_vccz" => zero(self, Reg::Vcc),
            "s_cbranch_execz" => zero(self, Reg::Exec),
            "s_cbranch_vccnz" | "s_cbranch_execnz" => {
                let reg = if inst.name.contains("vcc") {
                    Reg::Vcc
                } else {
                    Reg::Exec
                };
                let mask = self.mask_reg(reg);
                let zero = self.c64(0);
                self.cmp(Cmp::Ne64, mask, zero)
            }
            _ => return Err(unsupported(inst)),
        }))
    }

    fn inst(&mut self, inst: &Inst) -> Result<(), LiftError> {
        let name = inst.name.as_str();
        let base = name
            .strip_suffix("_e32")
            .or_else(|| name.strip_suffix("_e64"))
            .unwrap_or(name);
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        if matches!(
            base,
            "s_nop"
                | "s_waitcnt"
                | "s_barrier"
                | "v_nop"
                | "s_waitcnt_vscnt"
                | "s_clause"
                | "s_inst_prefetch"
                | "s_code_end"
        ) {
            return Ok(());
        }
        if base.starts_with("s_setreg") && harmless_setreg(inst) {
            return Ok(());
        }
        if base == "exp" {
            let Operand::Named(target) = &arg(0)?.operand else {
                return Err(unsupported(inst));
            };
            let target = export_target(target).ok_or_else(|| unsupported(inst))?;
            let compressed = inst.mods.iter().any(|m| m == "compr");
            let sources: Vec<usize> = if compressed {
                vec![1, 3]
            } else {
                vec![1, 2, 3, 4]
            };
            let mut values = [None; 4];
            for (component, index) in sources.into_iter().enumerate() {
                if let Operand::Vgpr { reg, .. } = arg(index)?.operand {
                    values[component] = Some(self.value(Expr::Get(Reg::V(reg))));
                }
            }
            self.stmts.push(Stmt::Export { target, values });
            return Ok(());
        }
        if base == "s_sendmsg" {
            let Operand::Named(text) = &arg(0)?.operand else {
                return Err(unsupported(inst));
            };
            let code = rdna::parse_sendmsg(text).ok_or_else(|| unsupported(inst))?;
            let m0 = self.value(Expr::Get(Reg::M0));
            self.stmts.push(Stmt::Message(code, m0));
            return Ok(());
        }
        if let Some(op) = base
            .strip_suffix("_wrexec_b32")
            .or_else(|| base.strip_suffix("_wrexec_b64"))
        {
            let source = self.mask(arg(1)?, inst)?;
            let old = self.mask_reg(Reg::Exec);
            let new = match op {
                "s_andn1" => {
                    let inverted = self.un(Un::Not64, source);
                    self.bin(Bin::And64, inverted, old)
                }
                "s_andn2" => {
                    let inverted = self.un(Un::Not64, old);
                    self.bin(Bin::And64, source, inverted)
                }
                _ => return Err(unsupported(inst)),
            };
            let full = self.full_mask();
            let new = self.bin(Bin::And64, new, full);
            self.set_exec(new);
            self.set_mask(arg(0)?, new, inst)?;
            let zero = self.c64(0);
            let scc = self.cmp(Cmp::Ne64, new, zero);
            self.stmts.push(Stmt::Set(Reg::Scc, scc));
            return Ok(());
        }
        if let Some(op) = base
            .strip_prefix("s_cmp_")
            .or_else(|| base.strip_prefix("s_cmpk_"))
        {
            let a = self.scalar(arg(0)?, inst)?;
            let b = self.scalar(arg(1)?, inst)?;
            let cmp = int_compare(op).ok_or_else(|| unsupported(inst))?;
            let result = self.cmp(cmp, a, b);
            self.stmts.push(Stmt::Set(Reg::Scc, result));
            return Ok(());
        }
        if let Some(op) = base
            .strip_suffix("_saveexec_b32")
            .or_else(|| base.strip_suffix("_saveexec_b64"))
        {
            let source = self.mask(arg(1)?, inst)?;
            let old = self.mask_reg(Reg::Exec);
            let new = match op {
                "s_and" => self.bin(Bin::And64, source, old),
                "s_or" => self.bin(Bin::Or64, source, old),
                "s_xor" => self.bin(Bin::Xor64, source, old),
                "s_andn2" => {
                    let inverted = self.un(Un::Not64, old);
                    self.bin(Bin::And64, source, inverted)
                }
                _ => return Err(unsupported(inst)),
            };
            let full = self.full_mask();
            let new = self.bin(Bin::And64, new, full);
            self.set_mask(arg(0)?, old, inst)?;
            self.set_exec(new);
            let zero = self.c64(0);
            let scc = self.cmp(Cmp::Ne64, new, zero);
            self.stmts.push(Stmt::Set(Reg::Scc, scc));
            return Ok(());
        }
        if base.starts_with("s_") && self.scalar_op(base, inst)? {
            return Ok(());
        }
        if let Some(op) = base.strip_suffix("_dpp") {
            return self.dpp(op, inst);
        }
        if let Some(op) = base.strip_suffix("_sdwa") {
            return self.sdwa(op, inst);
        }
        if base.starts_with("v_cmp") {
            return self.vector_compare(base, inst);
        }
        if base.starts_with("v_") && self.vector_op(base, inst)? {
            return Ok(());
        }
        if self.memory_op(base, inst)? {
            return Ok(());
        }
        Err(unsupported(inst))
    }

    fn flag(&mut self, flag: Flag, value: Tmp) {
        let scc = match flag {
            Flag::Keep => return,
            Flag::NonZero => {
                let zero = self.c(0);
                self.cmp(Cmp::Ne, value, zero)
            }
            Flag::NonZero64 => {
                let zero = self.c64(0);
                self.cmp(Cmp::Ne64, value, zero)
            }
            Flag::Set(tmp) => tmp,
        };
        self.stmts.push(Stmt::Set(Reg::Scc, scc));
    }

    fn sign_bit(&mut self, value: Tmp) -> Tmp {
        let zero = self.c(0);
        self.cmp(Cmp::LtS, value, zero)
    }

    fn high_nonzero(&mut self, wide: Tmp) -> Tmp {
        let high = self.un(Un::Hi, wide);
        let zero = self.c(0);
        self.cmp(Cmp::Ne, high, zero)
    }

    fn add_overflow(&mut self, x: Tmp, y: Tmp, sum: Tmp) -> Tmp {
        let a = self.bin(Bin::Xor, x, sum);
        let b = self.bin(Bin::Xor, y, sum);
        let both = self.bin(Bin::And, a, b);
        self.sign_bit(both)
    }

    fn scalar_op(&mut self, name: &str, inst: &Inst) -> Result<bool, LiftError> {
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        if name == "s_getpc_b64" {
            let base = self.value(Expr::CodeBase);
            let next = self.c64(inst.pc as u64 + 4);
            let address = self.bin(Bin::Add64, base, next);
            self.set_scalar64(arg(0)?, address, inst)?;
            return Ok(true);
        }
        if name == "s_bcnt1_i32_b64" {
            let a = self.scalar64(arg(1)?, inst)?;
            let count = self.un(Un::Popcount64, a);
            self.set_scalar(arg(0)?, count, inst)?;
            self.flag(Flag::NonZero, count);
            return Ok(true);
        }
        if name.ends_with("_b64") {
            return self.scalar_wide(name, inst);
        }
        let unary = matches!(
            name,
            "s_mov_b32"
                | "s_movk_i32"
                | "s_cmov_b32"
                | "s_cmovk_i32"
                | "s_not_b32"
                | "s_brev_b32"
                | "s_bcnt1_i32_b32"
                | "s_ff1_i32_b32"
                | "s_sext_i32_i8"
                | "s_sext_i32_i16"
                | "s_addk_i32"
                | "s_mulk_i32"
                | "s_flbit_i32_b32"
                | "s_bitset1_b32"
        );
        if !unary && !binary_scalar(name) {
            return Ok(false);
        }
        let x = self.scalar(arg(1)?, inst)?;
        let (value, flag) = if unary {
            match name {
                "s_mov_b32" | "s_movk_i32" => (x, Flag::Keep),
                "s_cmov_b32" | "s_cmovk_i32" => {
                    let old = self.scalar(arg(0)?, inst)?;
                    let scc = self.value(Expr::Get(Reg::Scc));
                    (self.value(Expr::Select(scc, x, old)), Flag::Keep)
                }
                "s_not_b32" => (self.un(Un::Not, x), Flag::NonZero),
                "s_brev_b32" => (self.un(Un::Bfrev, x), Flag::Keep),
                "s_bcnt1_i32_b32" => (self.un(Un::Popcount, x), Flag::NonZero),
                "s_ff1_i32_b32" => (self.un(Un::Ffbl, x), Flag::Keep),
                "s_flbit_i32_b32" => (self.un(Un::Ffbh, x), Flag::Keep),
                "s_bitset1_b32" => {
                    let old = self.scalar(arg(0)?, inst)?;
                    let one = self.c(1);
                    let bit = self.bin(Bin::Shl, one, x);
                    (self.bin(Bin::Or, old, bit), Flag::Keep)
                }
                "s_sext_i32_i8" | "s_sext_i32_i16" => {
                    let shift = self.c(if name.ends_with("i8") { 24 } else { 16 });
                    let up = self.bin(Bin::Shl, x, shift);
                    (self.bin(Bin::ShrS, up, shift), Flag::Keep)
                }
                "s_addk_i32" => {
                    let old = self.scalar(arg(0)?, inst)?;
                    let sum = self.bin(Bin::Add, old, x);
                    let overflow = self.add_overflow(old, x, sum);
                    (sum, Flag::Set(overflow))
                }
                _ => {
                    let old = self.scalar(arg(0)?, inst)?;
                    (self.bin(Bin::Mul, old, x), Flag::Keep)
                }
            }
        } else {
            let y = self.scalar(arg(2)?, inst)?;
            self.scalar_binary(name, x, y)
        };
        self.set_scalar(arg(0)?, value, inst)?;
        self.flag(flag, value);
        Ok(true)
    }

    fn scalar_binary(&mut self, name: &str, x: Tmp, y: Tmp) -> (Tmp, Flag) {
        match name {
            "s_add_u32" | "s_addc_u32" | "s_sub_u32" | "s_subb_u32" => {
                let (wx, wy) = (self.un(Un::Zext, x), self.un(Un::Zext, y));
                let op = if name.starts_with("s_add") {
                    Bin::Add64
                } else {
                    Bin::Sub64
                };
                let mut wide = self.bin(op, wx, wy);
                if matches!(name, "s_addc_u32" | "s_subb_u32") {
                    let scc = self.value(Expr::Get(Reg::Scc));
                    let carry = self.un(Un::BoolToU32, scc);
                    let carry = self.un(Un::Zext, carry);
                    wide = self.bin(op, wide, carry);
                }
                let carry = self.high_nonzero(wide);
                (self.un(Un::Lo, wide), Flag::Set(carry))
            }
            "s_add_i32" => {
                let sum = self.bin(Bin::Add, x, y);
                let overflow = self.add_overflow(x, y, sum);
                (sum, Flag::Set(overflow))
            }
            "s_sub_i32" => {
                let difference = self.bin(Bin::Sub, x, y);
                let sources = self.bin(Bin::Xor, x, y);
                let changed = self.bin(Bin::Xor, x, difference);
                let both = self.bin(Bin::And, sources, changed);
                let overflow = self.sign_bit(both);
                (difference, Flag::Set(overflow))
            }
            "s_min_u32" | "s_max_u32" | "s_min_i32" | "s_max_i32" => {
                let (op, cmp) = match name {
                    "s_min_u32" => (Bin::MinU, Cmp::LtU),
                    "s_max_u32" => (Bin::MaxU, Cmp::GtU),
                    "s_min_i32" => (Bin::MinS, Cmp::LtS),
                    _ => (Bin::MaxS, Cmp::GtS),
                };
                let flag = self.cmp(cmp, x, y);
                (self.bin(op, x, y), Flag::Set(flag))
            }
            "s_cselect_b32" => {
                let scc = self.value(Expr::Get(Reg::Scc));
                (self.value(Expr::Select(scc, x, y)), Flag::Keep)
            }
            "s_bfe_u32" => {
                let field = self.c(31);
                let offset = self.bin(Bin::And, y, field);
                let sixteen = self.c(16);
                let width = self.bin(Bin::ShrU, y, sixteen);
                let seven = self.c(0x7F);
                let width = self.bin(Bin::And, width, seven);
                let narrow = self.value(Expr::Tri(Tri::BfeU, x, offset, width));
                let whole = self.bin(Bin::ShrU, x, offset);
                let limit = self.c(32);
                let wide = self.cmp(Cmp::GeU, width, limit);
                (self.value(Expr::Select(wide, whole, narrow)), Flag::NonZero)
            }
            "s_lshl1_add_u32" | "s_lshl2_add_u32" | "s_lshl3_add_u32" | "s_lshl4_add_u32" => {
                let shift = self.c64(u64::from(name.as_bytes()[6] - b'0'));
                let wx = self.un(Un::Zext, x);
                let shifted = self.bin(Bin::Shl64, wx, shift);
                let wy = self.un(Un::Zext, y);
                let wide = self.bin(Bin::Add64, shifted, wy);
                let carry = self.high_nonzero(wide);
                (self.un(Un::Lo, wide), Flag::Set(carry))
            }
            "s_pack_ll_b32_b16" | "s_pack_lh_b32_b16" | "s_pack_hh_b32_b16" => {
                let low_mask = self.c(0xFFFF);
                let high_mask = self.c(0xFFFF_0000);
                let sixteen = self.c(16);
                let low = if name == "s_pack_hh_b32_b16" {
                    self.bin(Bin::ShrU, x, sixteen)
                } else {
                    self.bin(Bin::And, x, low_mask)
                };
                let high = if name == "s_pack_ll_b32_b16" {
                    self.bin(Bin::Shl, y, sixteen)
                } else {
                    self.bin(Bin::And, y, high_mask)
                };
                (self.bin(Bin::Or, low, high), Flag::Keep)
            }
            "s_mul_i32" => (self.bin(Bin::Mul, x, y), Flag::Keep),
            "s_mul_hi_u32" => (self.bin(Bin::MulHiU, x, y), Flag::Keep),
            "s_bfe_i32" => {
                let field = self.c(31);
                let offset = self.bin(Bin::And, y, field);
                let sixteen = self.c(16);
                let width = self.bin(Bin::ShrU, y, sixteen);
                let seven = self.c(0x7F);
                let width = self.bin(Bin::And, width, seven);
                let limit = self.c(32);
                let width = self.bin(Bin::MinU, width, limit);
                let spare = self.bin(Bin::Sub, limit, width);
                let shifted = self.bin(Bin::ShrU, x, offset);
                let up = self.bin(Bin::Shl, shifted, spare);
                let value = self.bin(Bin::ShrS, up, spare);
                let zero = self.c(0);
                let empty = self.cmp(Cmp::Eq, width, zero);
                (self.value(Expr::Select(empty, zero, value)), Flag::NonZero)
            }
            _ => {
                let (op, invert_result, invert_second) =
                    logic_scalar(name).expect("checked by caller");
                let y = if invert_second {
                    self.un(Un::Not, y)
                } else {
                    y
                };
                let mut value = self.bin(op, x, y);
                if invert_result {
                    value = self.un(Un::Not, value);
                }
                (value, Flag::NonZero)
            }
        }
    }

    fn scalar_wide(&mut self, name: &str, inst: &Inst) -> Result<bool, LiftError> {
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        let known = matches!(
            name,
            "s_mov_b64"
                | "s_cmov_b64"
                | "s_not_b64"
                | "s_lshl_b64"
                | "s_lshr_b64"
                | "s_cselect_b64"
                | "s_and_b64"
                | "s_or_b64"
                | "s_xor_b64"
                | "s_andn2_b64"
                | "s_orn2_b64"
                | "s_nand_b64"
                | "s_nor_b64"
                | "s_xnor_b64"
        );
        if !known {
            return Ok(false);
        }
        let a = self.scalar64(arg(1)?, inst)?;
        let (value, flag) = match name {
            "s_mov_b64" => (a, Flag::Keep),
            "s_cmov_b64" => {
                let old = self.scalar64(arg(0)?, inst)?;
                let scc = self.value(Expr::Get(Reg::Scc));
                (self.value(Expr::Select(scc, a, old)), Flag::Keep)
            }
            "s_not_b64" => (self.un(Un::Not64, a), Flag::NonZero64),
            "s_lshl_b64" | "s_lshr_b64" => {
                let amount = self.scalar(arg(2)?, inst)?;
                let amount = self.un(Un::Zext, amount);
                let op = if name == "s_lshl_b64" {
                    Bin::Shl64
                } else {
                    Bin::ShrU64
                };
                (self.bin(op, a, amount), Flag::NonZero64)
            }
            "s_cselect_b64" => {
                let b = self.scalar64(arg(2)?, inst)?;
                let scc = self.value(Expr::Get(Reg::Scc));
                (self.value(Expr::Select(scc, a, b)), Flag::Keep)
            }
            _ => {
                let b = self.scalar64(arg(2)?, inst)?;
                let b = if matches!(name, "s_andn2_b64" | "s_orn2_b64") {
                    self.un(Un::Not64, b)
                } else {
                    b
                };
                let op = match name {
                    "s_and_b64" | "s_andn2_b64" | "s_nand_b64" => Bin::And64,
                    "s_or_b64" | "s_orn2_b64" | "s_nor_b64" => Bin::Or64,
                    _ => Bin::Xor64,
                };
                let mut value = self.bin(op, a, b);
                if matches!(name, "s_nand_b64" | "s_nor_b64" | "s_xnor_b64") {
                    value = self.un(Un::Not64, value);
                }
                (value, Flag::NonZero64)
            }
        };
        self.set_scalar64(arg(0)?, value, inst)?;
        self.flag(flag, value);
        Ok(true)
    }

    fn vector_compare(&mut self, name: &str, inst: &Inst) -> Result<(), LiftError> {
        let exec_form = name.starts_with("v_cmpx_");
        let body = name
            .trim_start_matches("v_cmpx_")
            .trim_start_matches("v_cmp_");
        let (op, ty) = body.rsplit_once('_').ok_or_else(|| unsupported(inst))?;
        let cmp = match ty {
            "f32" => float_compare(op),
            "u32" => int_compare(&format!("{op}_u32")),
            "i32" => int_compare(&format!("{op}_i32")),
            _ => None,
        }
        .ok_or_else(|| unsupported(inst))?;
        let first = usize::from(!exec_form);
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        let (a, b) = if ty == "f32" {
            (
                self.flane(arg(first)?, inst)?,
                self.flane(arg(first + 1)?, inst)?,
            )
        } else {
            (
                self.lane(arg(first)?, inst)?,
                self.lane(arg(first + 1)?, inst)?,
            )
        };
        let outcome = self.cmp(cmp, a, b);
        let mask = self.value(Expr::Ballot(outcome));
        if exec_form {
            self.set_exec(mask);
            Ok(())
        } else {
            self.set_mask(arg(0)?, mask, inst)
        }
    }

    fn vector_op(&mut self, name: &str, inst: &Inst) -> Result<bool, LiftError> {
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        let value = match name {
            "v_mov_b32" => self.lane(arg(1)?, inst)?,
            "v_movrels_b32" => {
                let (base, _) = vgprs(arg(1)?, inst)?;
                let index = self.value(Expr::Get(Reg::M0));
                self.value(Expr::GetRelative(base, index))
            }
            "v_permlane16_b32" | "v_permlanex16_b32" => {
                let (target, _) = vgprs(arg(0)?, inst)?;
                let source = self.lane(arg(1)?, inst)?;
                let low = self.scalar(arg(2)?, inst)?;
                let high = self.scalar(arg(3)?, inst)?;
                let flags = op_sel(inst, "op_sel");
                let lane = self.value(Expr::LaneId);
                let fifteen = self.c(15);
                let row_lane = self.bin(Bin::And, lane, fifteen);
                let eight = self.c(8);
                let upper = self.cmp(Cmp::GeU, row_lane, eight);
                let word = self.value(Expr::Select(upper, high, low));
                let seven = self.c(7);
                let nibble = self.bin(Bin::And, row_lane, seven);
                let two = self.c(2);
                let shift = self.bin(Bin::Shl, nibble, two);
                let shifted = self.bin(Bin::ShrU, word, shift);
                let select = self.bin(Bin::And, shifted, fifteen);
                let row = self.c(!15);
                let mut row_base = self.bin(Bin::And, lane, row);
                if name == "v_permlanex16_b32" {
                    let sixteen = self.c(16);
                    row_base = self.bin(Bin::Xor, row_base, sixteen);
                }
                let from = self.bin(Bin::Or, row_base, select);
                let value = self.value(Expr::Shuffle(source, from));
                if flags[0] {
                    value
                } else {
                    let active = self.lane_active(from);
                    let fallback = if flags[1] {
                        self.c(0)
                    } else {
                        self.value(Expr::Get(Reg::V(target)))
                    };
                    self.value(Expr::Select(active, value, fallback))
                }
            }
            "v_cndmask_b32" => {
                let condition = if inst.args.len() > 3 {
                    self.mask(arg(3)?, inst)?
                } else {
                    self.mask_reg(Reg::Vcc)
                };
                let lane = self.value(Expr::LaneId);
                let bit = self.bin(Bin::LaneBit, condition, lane);
                let a = self.lane(arg(1)?, inst)?;
                let b = self.lane(arg(2)?, inst)?;
                self.value(Expr::Select(bit, b, a))
            }
            "v_readfirstlane_b32" | "v_readlane_b32" => {
                let source = self.lane(arg(1)?, inst)?;
                let lane = if name == "v_readlane_b32" {
                    let selected = self.scalar(arg(2)?, inst)?;
                    let wrap = self.c(if self.wave64 { 63 } else { 31 });
                    self.bin(Bin::And, selected, wrap)
                } else {
                    self.value(Expr::FirstActiveLane)
                };
                let value = self.value(Expr::ReadLane(source, lane));
                self.set_scalar(arg(0)?, value, inst)?;
                return Ok(true);
            }
            "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
                let mask = self.lane(arg(1)?, inst)?;
                let lane = self.value(Expr::LaneId);
                let base = self.c(if name.contains("_hi_") { 32 } else { 0 });
                let one = self.c(1);
                let zero = self.c(0);
                let shifted = self.bin(Bin::Sub, lane, base);
                let at_least_32 = self.c(32);
                let above = self.cmp(Cmp::GeU, shifted, at_least_32);
                let below_base = self.cmp(Cmp::LtU, lane, base);
                let bit = self.bin(Bin::Shl, one, shifted);
                let partial = self.bin(Bin::Sub, bit, one);
                let partial = self.value(Expr::Select(above, mask, partial));
                let partial = self.bin(Bin::And, partial, mask);
                let partial = self.value(Expr::Select(below_base, zero, partial));
                let count = self.un(Un::Popcount, partial);
                let add = self.lane(arg(2)?, inst)?;
                self.bin(Bin::Add, count, add)
            }
            "v_cvt_f32_f16" => {
                let half = self.half_source(arg(1)?, false, true, inst)?;
                let value = self.un(Un::F16ToF32, half);
                self.output_modifiers(value, inst)
            }
            "v_cvt_f16_f32" => {
                if has_output_modifiers(inst) {
                    return Err(unsupported(inst));
                }
                let (target, _) = vgprs(arg(0)?, inst)?;
                let single = self.flane(arg(1)?, inst)?;
                let half = self.un(Un::F32ToF16, single);
                if self.destination.is_some() {
                    half
                } else {
                    self.keep_half(target, half, false)
                }
            }
            "v_pack_b32_f16" => {
                if has_output_modifiers(inst) {
                    return Err(unsupported(inst));
                }
                let select = op_sel(inst, "op_sel");
                let low = self.half_source(arg(1)?, select[0], true, inst)?;
                let high = self.half_source(arg(2)?, select[1], true, inst)?;
                let sixteen = self.c(16);
                let high = self.bin(Bin::Shl, high, sixteen);
                self.bin(Bin::Or, low, high)
            }
            "v_fma_mix_f32" | "v_fma_mixlo_f16" | "v_fma_mixhi_f16" => {
                let halves = op_sel(inst, "op_sel_hi");
                let select = op_sel(inst, "op_sel");
                let mut sources = Vec::new();
                for i in 0..3 {
                    let source = arg(i + 1)?;
                    let mut value = if halves[i] {
                        let half = self.half_source(source, select[i], false, inst)?;
                        self.un(Un::F16ToF32, half)
                    } else {
                        self.lane(source, inst)?
                    };
                    if source.abs {
                        value = self.un(Un::FAbs, value);
                    }
                    if source.neg {
                        value = self.un(Un::FNeg, value);
                    }
                    sources.push(value);
                }
                let value = self.value(Expr::Tri(Tri::FFma, sources[0], sources[1], sources[2]));
                let value = self.output_modifiers(value, inst);
                if name == "v_fma_mix_f32" {
                    value
                } else {
                    let (target, _) = vgprs(arg(0)?, inst)?;
                    let half = self.un(Un::F32ToF16, value);
                    self.keep_half(target, half, name == "v_fma_mixhi_f16")
                }
            }
            "v_fmaak_f32" | "v_madak_f32" | "v_fmamk_f32" | "v_madmk_f32" => {
                let a = self.flane(arg(1)?, inst)?;
                let b = self.flane(arg(2)?, inst)?;
                let c = self.flane(arg(3)?, inst)?;
                let op = if name.starts_with("v_fma") {
                    Tri::FFma
                } else {
                    Tri::FMad
                };
                let value = self.value(Expr::Tri(op, a, b, c));
                self.output_modifiers(value, inst)
            }
            "v_mad_legacy_f32" => {
                let a = self.flane(arg(1)?, inst)?;
                let b = self.flane(arg(2)?, inst)?;
                let c = self.flane(arg(3)?, inst)?;
                let zero = self.c(0);
                let a_zero = self.cmp(Cmp::FEq, a, zero);
                let b_zero = self.cmp(Cmp::FEq, b, zero);
                let either = self.value(Expr::Select(a_zero, a_zero, b_zero));
                let product = self.bin(Bin::FMul, a, b);
                let product = self.value(Expr::Select(either, zero, product));
                let value = self.bin(Bin::FAdd, product, c);
                self.output_modifiers(value, inst)
            }
            "v_med3_f32" => {
                let a = self.flane(arg(1)?, inst)?;
                let b = self.flane(arg(2)?, inst)?;
                let c = self.flane(arg(3)?, inst)?;
                let low = self.bin(Bin::FMin, a, b);
                let high = self.bin(Bin::FMax, a, b);
                let capped = self.bin(Bin::FMin, high, c);
                let median = self.bin(Bin::FMax, low, capped);
                let smallest = self.bin(Bin::FMin, low, c);
                let unordered = self.cmp(Cmp::FU, a, b);
                let c_nan = self.cmp(Cmp::FU, c, c);
                let any_nan = self.value(Expr::Select(unordered, unordered, c_nan));
                let value = self.value(Expr::Select(any_nan, smallest, median));
                self.output_modifiers(value, inst)
            }
            "v_cvt_pk_u8_f32" => {
                let x = self.flane(arg(1)?, inst)?;
                let rounded = self.un(Un::FRndne, x);
                let converted = self.un(Un::CvtU32F32, rounded);
                let limit = self.c(255);
                let byte = self.bin(Bin::MinU, converted, limit);
                let selector = self.lane(arg(2)?, inst)?;
                let three = self.c(3);
                let selector = self.bin(Bin::And, selector, three);
                let shift = self.bin(Bin::Shl, selector, three);
                let old = self.lane(arg(3)?, inst)?;
                let mask = self.c(0xFF);
                let mask = self.bin(Bin::Shl, mask, shift);
                let keep = self.un(Un::Not, mask);
                let kept = self.bin(Bin::And, old, keep);
                let placed = self.bin(Bin::Shl, byte, shift);
                self.bin(Bin::Or, kept, placed)
            }
            "v_lshlrev_b64" => {
                let (target, _) = vgprs(arg(0)?, inst)?;
                let shift = self.lane(arg(1)?, inst)?;
                let shift = self.un(Un::Zext, shift);
                let value = match arg(2)?.operand {
                    Operand::Vgpr { reg, .. } => {
                        let low = self.value(Expr::Get(Reg::V(reg)));
                        let high = self.value(Expr::Get(Reg::V(reg + 1)));
                        self.bin(Bin::Pack64, low, high)
                    }
                    _ => self.scalar64(arg(2)?, inst)?,
                };
                let shifted = self.bin(Bin::Shl64, value, shift);
                let low = self.un(Un::Lo, shifted);
                let high = self.un(Un::Hi, shifted);
                self.stmts.push(Stmt::SetLane(target, low));
                self.stmts.push(Stmt::SetLane(target + 1, high));
                return Ok(true);
            }
            "v_fmac_f32" | "v_mac_f32" => {
                let a = self.flane(arg(1)?, inst)?;
                let b = self.flane(arg(2)?, inst)?;
                let c = self.flane(arg(0)?, inst)?;
                let op = if name == "v_fmac_f32" {
                    Tri::FFma
                } else {
                    Tri::FMad
                };
                let value = self.value(Expr::Tri(op, a, b, c));
                self.output_modifiers(value, inst)
            }
            "v_mul_u32_u24" | "v_mul_i32_i24" => {
                let a = self.lane(arg(1)?, inst)?;
                let b = self.lane(arg(2)?, inst)?;
                let (x, y) = if name == "v_mul_u32_u24" {
                    let mask = self.c(0xFF_FFFF);
                    (self.bin(Bin::And, a, mask), self.bin(Bin::And, b, mask))
                } else {
                    (self.sext(a, 8), self.sext(b, 8))
                };
                self.bin(Bin::Mul, x, y)
            }
            "v_xnor_b32" => {
                let a = self.lane(arg(1)?, inst)?;
                let b = self.lane(arg(2)?, inst)?;
                let x = self.bin(Bin::Xor, a, b);
                self.un(Un::Not, x)
            }
            "v_bfm_b32" => {
                let a = self.lane(arg(1)?, inst)?;
                let b = self.lane(arg(2)?, inst)?;
                let one = self.c(1);
                let bit = self.bin(Bin::Shl, one, a);
                let ones = self.bin(Bin::Sub, bit, one);
                self.bin(Bin::Shl, ones, b)
            }
            "v_bcnt_u32_b32" => {
                let a = self.lane(arg(1)?, inst)?;
                let b = self.lane(arg(2)?, inst)?;
                let count = self.un(Un::Popcount, a);
                self.bin(Bin::Add, count, b)
            }
            "v_mul_legacy_f32" => {
                let a = self.flane(arg(1)?, inst)?;
                let b = self.flane(arg(2)?, inst)?;
                let zero = self.c(0);
                let a_zero = self.cmp(Cmp::FEq, a, zero);
                let b_zero = self.cmp(Cmp::FEq, b, zero);
                let either = self.value(Expr::Select(a_zero, a_zero, b_zero));
                let product = self.bin(Bin::FMul, a, b);
                let value = self.value(Expr::Select(either, zero, product));
                self.output_modifiers(value, inst)
            }
            "v_cvt_f32_ubyte0" | "v_cvt_f32_ubyte1" | "v_cvt_f32_ubyte2" | "v_cvt_f32_ubyte3" => {
                let a = self.lane(arg(1)?, inst)?;
                let shift = self.c(8 * u32::from(name.as_bytes()[name.len() - 1] - b'0'));
                let shifted = self.bin(Bin::ShrU, a, shift);
                let mask = self.c(0xFF);
                let byte = self.bin(Bin::And, shifted, mask);
                let value = self.un(Un::CvtF32U32, byte);
                self.output_modifiers(value, inst)
            }
            "v_writelane_b32" => {
                let Operand::Vgpr { reg, .. } = arg(0)?.operand else {
                    return Err(unsupported(inst));
                };
                let value = self.scalar(arg(1)?, inst)?;
                let lane = self.scalar(arg(2)?, inst)?;
                self.stmts.push(Stmt::WriteLane(reg, lane, value));
                return Ok(true);
            }
            "v_add_co_u32" | "v_sub_co_u32" | "v_subrev_co_u32" | "v_add_co_ci_u32"
            | "v_sub_co_ci_u32" | "v_subrev_co_ci_u32" => {
                let carry_in = if name.contains("_ci_") {
                    let mask = self.mask(arg(inst.args.len() - 1)?, inst)?;
                    let lane = self.value(Expr::LaneId);
                    let bit = self.bin(Bin::LaneBit, mask, lane);
                    let one = self.c64(1);
                    let zero = self.c64(0);
                    self.value(Expr::Select(bit, one, zero))
                } else {
                    self.c64(0)
                };
                let a = self.lane(arg(2)?, inst)?;
                let b = self.lane(arg(3)?, inst)?;
                let (wa, wb) = (self.un(Un::Zext, a), self.un(Un::Zext, b));
                let wide = if name.starts_with("v_add") {
                    let sum = self.bin(Bin::Add64, wa, wb);
                    self.bin(Bin::Add64, sum, carry_in)
                } else {
                    let (x, y) = if name.starts_with("v_subrev") {
                        (wb, wa)
                    } else {
                        (wa, wb)
                    };
                    let difference = self.bin(Bin::Sub64, x, y);
                    self.bin(Bin::Sub64, difference, carry_in)
                };
                let out = self.high_nonzero(wide);
                let value = self.un(Un::Lo, wide);
                self.set_lane(arg(0)?, value, inst)?;
                let mask = self.value(Expr::Ballot(out));
                self.set_mask(arg(1)?, mask, inst)?;
                return Ok(true);
            }
            _ => {
                if let Some(op) = unary(name) {
                    let float_in = matches!(op, Un::CvtU32F32 | Un::CvtI32F32) || is_float_un(op);
                    let a = if float_in {
                        self.flane(arg(1)?, inst)?
                    } else {
                        self.lane(arg(1)?, inst)?
                    };
                    let value = self.un(op, a);
                    if is_float_un(op) || matches!(op, Un::CvtF32U32 | Un::CvtF32I32) {
                        self.output_modifiers(value, inst)
                    } else {
                        value
                    }
                } else if let Some((op, float, reversed)) = binary(name) {
                    let (x, y) = if float {
                        (self.flane(arg(1)?, inst)?, self.flane(arg(2)?, inst)?)
                    } else {
                        (self.lane(arg(1)?, inst)?, self.lane(arg(2)?, inst)?)
                    };
                    let (x, y) = if reversed { (y, x) } else { (x, y) };
                    let value = self.bin(op, x, y);
                    if float {
                        self.output_modifiers(value, inst)
                    } else {
                        value
                    }
                } else if let Some(sequence) = ternary(name) {
                    let float = matches!(
                        sequence,
                        Ternary::Fma | Ternary::Mad | Ternary::Min3F | Ternary::Max3F
                    );
                    let mut values = Vec::new();
                    for index in 1..4 {
                        values.push(if float {
                            self.flane(arg(index)?, inst)?
                        } else {
                            self.lane(arg(index)?, inst)?
                        });
                    }
                    let value = self.ternary(sequence, values[0], values[1], values[2]);
                    if float {
                        self.output_modifiers(value, inst)
                    } else {
                        value
                    }
                } else {
                    return Ok(false);
                }
            }
        };
        self.set_lane(arg(0)?, value, inst)?;
        Ok(true)
    }

    fn ternary(&mut self, op: Ternary, a: Tmp, b: Tmp, c: Tmp) -> Tmp {
        match op {
            Ternary::Fma => self.value(Expr::Tri(Tri::FFma, a, b, c)),
            Ternary::Mad => self.value(Expr::Tri(Tri::FMad, a, b, c)),
            Ternary::Add3 => {
                let ab = self.bin(Bin::Add, a, b);
                self.bin(Bin::Add, ab, c)
            }
            Ternary::LshlAdd => {
                let shifted = self.bin(Bin::Shl, a, b);
                self.bin(Bin::Add, shifted, c)
            }
            Ternary::MadU24 => {
                let mask = self.c(0xFF_FFFF);
                let x = self.bin(Bin::And, a, mask);
                let y = self.bin(Bin::And, b, mask);
                let product = self.bin(Bin::Mul, x, y);
                self.bin(Bin::Add, product, c)
            }
            Ternary::BfeU => self.value(Expr::Tri(Tri::BfeU, a, b, c)),
            Ternary::AndOr => {
                let ab = self.bin(Bin::And, a, b);
                self.bin(Bin::Or, ab, c)
            }
            Ternary::Or3 => {
                let ab = self.bin(Bin::Or, a, b);
                self.bin(Bin::Or, ab, c)
            }
            Ternary::Xor3 => {
                let ab = self.bin(Bin::Xor, a, b);
                self.bin(Bin::Xor, ab, c)
            }
            Ternary::AddLshl => {
                let sum = self.bin(Bin::Add, a, b);
                self.bin(Bin::Shl, sum, c)
            }
            Ternary::LshlOr => {
                let shifted = self.bin(Bin::Shl, a, b);
                self.bin(Bin::Or, shifted, c)
            }
            Ternary::Xad => {
                let x = self.bin(Bin::Xor, a, b);
                self.bin(Bin::Add, x, c)
            }
            Ternary::Bfi => {
                let selected = self.bin(Bin::And, a, b);
                let inverse = self.un(Un::Not, a);
                let rest = self.bin(Bin::And, inverse, c);
                self.bin(Bin::Or, selected, rest)
            }
            Ternary::AlignBit | Ternary::AlignByte => {
                let wide = self.bin(Bin::Pack64, b, a);
                let amount = if matches!(op, Ternary::AlignBit) {
                    let mask = self.c(31);
                    self.bin(Bin::And, c, mask)
                } else {
                    let mask = self.c(3);
                    let bytes = self.bin(Bin::And, c, mask);
                    let three = self.c(3);
                    self.bin(Bin::Shl, bytes, three)
                };
                let amount = self.un(Un::Zext, amount);
                let shifted = self.bin(Bin::ShrU64, wide, amount);
                self.un(Un::Lo, shifted)
            }
            Ternary::Min3(op) | Ternary::Max3(op) => {
                let ab = self.bin(op, a, b);
                self.bin(op, ab, c)
            }
            Ternary::Min3F => {
                let ab = self.bin(Bin::FMin, a, b);
                self.bin(Bin::FMin, ab, c)
            }
            Ternary::Max3F => {
                let ab = self.bin(Bin::FMax, a, b);
                self.bin(Bin::FMax, ab, c)
            }
            Ternary::Med3 { min, max } => {
                let low = self.bin(min, a, b);
                let high = self.bin(max, a, b);
                let capped = self.bin(min, high, c);
                self.bin(max, low, capped)
            }
            Ternary::Perm => self.perm(a, b, c),
            Ternary::BfeI => {
                let mask = self.c(31);
                let width = self.bin(Bin::And, c, mask);
                let thirty_two = self.c(32);
                let spare = self.bin(Bin::Sub, thirty_two, width);
                let field = self.bin(Bin::ShrU, a, b);
                let up = self.bin(Bin::Shl, field, spare);
                let value = self.bin(Bin::ShrS, up, spare);
                let zero = self.c(0);
                let empty = self.cmp(Cmp::Eq, width, zero);
                self.value(Expr::Select(empty, zero, value))
            }
        }
    }

    fn sext(&mut self, value: Tmp, spare: u32) -> Tmp {
        let shift = self.c(spare);
        let up = self.bin(Bin::Shl, value, shift);
        self.bin(Bin::ShrS, up, shift)
    }

    fn output_modifiers(&mut self, mut value: Tmp, inst: &Inst) -> Tmp {
        for modifier in &inst.mods {
            let factor = match modifier.as_str() {
                "mul:2" => 2.0f32,
                "mul:4" => 4.0,
                "div:2" => 0.5,
                _ => continue,
            };
            let constant = self.c(factor.to_bits());
            value = self.bin(Bin::FMul, value, constant);
        }
        if inst.mods.iter().any(|m| m == "clamp") {
            value = self.un(Un::FClamp01, value);
        }
        value
    }

    fn buffer_address(&mut self, inst: &Inst) -> Result<(Tmp, Tmp), LiftError> {
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        let has = |flag: &str| inst.mods.iter().any(|m| m == flag);
        let offset = Self::immediate(inst, "offset");
        let (base, words) = self.descriptor(arg(2)?, inst)?;
        let zero = self.c(0);
        let (index, voffset) = match (has("idxen"), has("offen")) {
            (false, false) => (zero, zero),
            (true, false) => (self.lane(arg(1)?, inst)?, zero),
            (false, true) => (zero, self.lane(arg(1)?, inst)?),
            (true, true) => {
                let (reg, _) = vgprs(arg(1)?, inst)?;
                let index = self.value(Expr::Get(Reg::V(reg)));
                let voffset = self.value(Expr::Get(Reg::V(reg + 1)));
                (index, voffset)
            }
        };
        let immediate = self.c(offset as u32);
        let local = self.bin(Bin::Add, voffset, immediate);
        let records = words[2];
        let sixteen = self.c(16);
        let stride = self.bin(Bin::ShrU, words[1], sixteen);
        let field = self.c(0x3FFF);
        let stride = self.bin(Bin::And, stride, field);
        let raw = self.cmp(Cmp::Eq, stride, zero);
        let within = self.cmp(Cmp::LtU, local, records);
        let indexed = self.cmp(Cmp::LtU, index, records);
        let in_range = self.value(Expr::Select(raw, within, indexed));
        let soffset = self.scalar(arg(3)?, inst)?;
        let total = self.bin(Bin::Add, local, soffset);
        let total = self.un(Un::Zext, total);
        let low = self.bin(Bin::Mul, stride, index);
        let high = self.bin(Bin::MulHiU, stride, index);
        let element = self.bin(Bin::Pack64, low, high);
        let start = self.bin(Bin::Add64, base, element);
        let start = self.bin(Bin::Add64, start, total);
        Ok((start, in_range))
    }

    fn descriptor(&mut self, arg: &Arg, inst: &Inst) -> Result<(Tmp, [Tmp; 4]), LiftError> {
        let Operand::Sgpr { reg, .. } = arg.operand else {
            return Err(unsupported(inst));
        };
        let words = [0, 1, 2, 3].map(|i| self.value(Expr::Get(Reg::S(reg + i))));
        let mask = self.c(0xFFFF);
        let high = self.bin(Bin::And, words[1], mask);
        let base = self.bin(Bin::Pack64, words[0], high);
        Ok((base, words))
    }

    fn immediate(inst: &Inst, key: &str) -> i64 {
        inst.mods
            .iter()
            .find_map(|m| m.strip_prefix(key).and_then(|r| r.strip_prefix(':')))
            .and_then(|text| {
                let (negative, digits) =
                    text.strip_prefix('-').map_or((false, text), |d| (true, d));
                let value = match digits.strip_prefix("0x") {
                    Some(hex) => i64::from_str_radix(hex, 16).ok()?,
                    None => digits.parse().ok()?,
                };
                Some(if negative { -value } else { value })
            })
            .unwrap_or(0)
    }

    fn memory_op(&mut self, name: &str, inst: &Inst) -> Result<bool, LiftError> {
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        let offset = Self::immediate(inst, "offset");
        match name {
            "ds_swizzle_b32" => {
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
                .ok_or_else(|| unsupported(inst))?;
                let source = self.lane(arg(1)?, inst)?;
                let lane = self.value(Expr::LaneId);
                let from = if offset & 0x8000 != 0 {
                    let three = self.c(3);
                    let quad_lane = self.bin(Bin::And, lane, three);
                    let one = self.c(1);
                    let shift = self.bin(Bin::Shl, quad_lane, one);
                    let pattern = self.c(offset & 0xFF);
                    let select = self.bin(Bin::ShrU, pattern, shift);
                    let select = self.bin(Bin::And, select, three);
                    let quad = self.c(!3);
                    let quad_base = self.bin(Bin::And, lane, quad);
                    self.bin(Bin::Or, quad_base, select)
                } else {
                    let low = self.c(31);
                    let group_lane = self.bin(Bin::And, lane, low);
                    let and_mask = self.c(offset & 0x1F);
                    let or_mask = self.c((offset >> 5) & 0x1F);
                    let xor_mask = self.c((offset >> 10) & 0x1F);
                    let picked = self.bin(Bin::And, group_lane, and_mask);
                    let picked = self.bin(Bin::Or, picked, or_mask);
                    let picked = self.bin(Bin::Xor, picked, xor_mask);
                    let group = self.c(32);
                    let group_base = self.bin(Bin::And, lane, group);
                    self.bin(Bin::Or, group_base, picked)
                };
                let value = self.value(Expr::Shuffle(source, from));
                let active = self.lane_active(from);
                let zero = self.c(0);
                let value = self.value(Expr::Select(active, value, zero));
                self.set_lane(arg(0)?, value, inst)?;
                Ok(true)
            }
            "ds_write2_b32" | "ds_write2st64_b32" | "ds_read2_b32" | "ds_read2st64_b32" => {
                let read = name.starts_with("ds_read");
                let scale = if name.contains("st64") { 256 } else { 4 };
                let first = Self::immediate(inst, "offset0") as u32;
                let second = Self::immediate(inst, "offset1") as u32;
                let base = self.lane(arg(usize::from(read))?, inst)?;
                let mut addresses = Vec::new();
                for slot in [first, second] {
                    let immediate = self.c(slot * scale);
                    addresses.push(self.bin(Bin::Add, base, immediate));
                }
                if read {
                    let (target, _) = vgprs(arg(0)?, inst)?;
                    let low = self.value(Expr::LdsLoad(addresses[0]));
                    let high = self.value(Expr::LdsLoad(addresses[1]));
                    self.stmts.push(Stmt::SetLane(target, low));
                    self.stmts.push(Stmt::SetLane(target + 1, high));
                } else {
                    let low = self.lane(arg(1)?, inst)?;
                    let high = self.lane(arg(2)?, inst)?;
                    self.stmts.push(Stmt::LdsStore(addresses[0], low));
                    self.stmts.push(Stmt::LdsStore(addresses[1], high));
                }
                Ok(true)
            }
            "ds_write_b32" | "ds_read_b32" => {
                let address_arg = if name == "ds_read_b32" {
                    arg(1)?
                } else {
                    arg(0)?
                };
                let base = self.lane(address_arg, inst)?;
                let immediate = self.c(offset as u32);
                let address = self.bin(Bin::Add, base, immediate);
                if name == "ds_read_b32" {
                    let value = self.value(Expr::LdsLoad(address));
                    self.set_lane(arg(0)?, value, inst)?;
                } else {
                    let data = self.lane(arg(1)?, inst)?;
                    self.stmts.push(Stmt::LdsStore(address, data));
                }
                Ok(true)
            }
            _ if name.starts_with("buffer_load_dword")
                || name.starts_with("buffer_store_dword") =>
            {
                let (data, count) = vgprs(arg(0)?, inst)?;
                let (start, in_range) = self.buffer_address(inst)?;
                let zero = self.c(0);
                for dword in 0..count {
                    let step = self.c64(4 * u64::from(dword));
                    let address = self.bin(Bin::Add64, start, step);
                    if name.starts_with("buffer_load") {
                        let loaded = self.value(Expr::Load {
                            when: Some(in_range),
                            address,
                        });
                        let value = self.value(Expr::Select(in_range, loaded, zero));
                        self.stmts.push(Stmt::SetLane(data + dword, value));
                    } else {
                        let value = self.value(Expr::Get(Reg::V(data + dword)));
                        self.stmts.push(Stmt::Store {
                            when: Some(in_range),
                            address,
                            value,
                        });
                    }
                }
                Ok(true)
            }
            _ if name.starts_with("tbuffer_load_format_")
                || name.starts_with("buffer_load_format_") =>
            {
                let suffix = name.rsplit('_').next().unwrap_or("");
                let count = match suffix {
                    "x" => 1,
                    "xy" => 2,
                    "xyz" => 3,
                    "xyzw" => 4,
                    _ => return Err(unsupported(inst)),
                };
                let (data, _) = vgprs(arg(0)?, inst)?;
                let (address, in_range) = self.buffer_address(inst)?;
                let (format, select) = if name.starts_with("tbuffer") {
                    let code = rdna::format::instruction_format(&inst.mods)
                        .ok_or_else(|| unsupported(inst))?;
                    (self.c(code), None)
                } else {
                    let (_, words) = self.descriptor(arg(2)?, inst)?;
                    let twelve = self.c(12);
                    let format = self.bin(Bin::ShrU, words[3], twelve);
                    let mask = self.c(0x7F);
                    (self.bin(Bin::And, format, mask), Some(words[3]))
                };
                let mut values = Vec::new();
                for component in 0..count {
                    values.push(self.value(Expr::TypedLoad {
                        format,
                        address,
                        in_range,
                        select,
                        component,
                    }));
                }
                for (component, value) in values.into_iter().enumerate() {
                    self.stmts
                        .push(Stmt::SetLane(data + component as u8, value));
                }
                Ok(true)
            }
            _ if name.starts_with("s_load_dword") || name.starts_with("s_buffer_load_dword") => {
                let Operand::Sgpr { reg: target, count } = arg(0)?.operand else {
                    return Err(unsupported(inst));
                };
                let base = if name.starts_with("s_buffer") {
                    self.descriptor(arg(1)?, inst)?.0
                } else {
                    self.scalar64(arg(1)?, inst)?
                };
                let displacement = match inst.args.get(2).map(|a| &a.operand) {
                    Some(Operand::Int(value)) => self.c64(*value as u64),
                    Some(Operand::Named(null)) if null == "null" => self.c64(0),
                    None => self.c64(0),
                    Some(_) => {
                        let register = self.scalar(arg(2)?, inst)?;
                        let register = self.un(Un::Zext, register);
                        let extra = self.c64(offset as u64);
                        self.bin(Bin::Add64, register, extra)
                    }
                };
                let start = self.bin(Bin::Add64, base, displacement);
                for dword in 0..count {
                    let step = self.c64(4 * u64::from(dword));
                    let address = self.bin(Bin::Add64, start, step);
                    let value = self.value(Expr::ScalarLoad(address));
                    self.stmts.push(Stmt::Set(Reg::S(target + dword), value));
                }
                Ok(true)
            }
            _ if name.starts_with("global_load_dword")
                || name.starts_with("global_store_dword") =>
            {
                let store = name.starts_with("global_store");
                let (data, address_arg, saddr) = if store {
                    (arg(1)?, arg(0)?, arg(2)?)
                } else {
                    (arg(0)?, arg(1)?, arg(2)?)
                };
                let (data, count) = vgprs(data, inst)?;
                let (areg, _) = vgprs(address_arg, inst)?;
                let low = self.value(Expr::Get(Reg::V(areg)));
                let base = match &saddr.operand {
                    Operand::Named(off) if off == "off" => {
                        let high = self.value(Expr::Get(Reg::V(areg + 1)));
                        self.bin(Bin::Pack64, low, high)
                    }
                    _ => {
                        let base = self.scalar64(saddr, inst)?;
                        let low = self.un(Un::Zext, low);
                        self.bin(Bin::Add64, base, low)
                    }
                };
                let displacement = self.c64(offset as u64);
                let start = self.bin(Bin::Add64, base, displacement);
                for dword in 0..count {
                    let step = self.c64(4 * u64::from(dword));
                    let address = self.bin(Bin::Add64, start, step);
                    if store {
                        let value = self.value(Expr::Get(Reg::V(data + dword)));
                        self.stmts.push(Stmt::Store {
                            when: None,
                            address,
                            value,
                        });
                    } else {
                        let value = self.value(Expr::Load {
                            when: None,
                            address,
                        });
                        self.stmts.push(Stmt::SetLane(data + dword, value));
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

pub fn export_target(name: &str) -> Option<u8> {
    let number = |prefix: &str| name.strip_prefix(prefix).and_then(|n| n.parse::<u8>().ok());
    Some(match name {
        "mrtz" => 8,
        "null" => 9,
        "prim" => 20,
        _ => {
            if let Some(n) = number("param") {
                32 + n
            } else if let Some(n) = number("pos") {
                12 + n
            } else if let Some(n) = number("mrt") {
                n
            } else {
                number("tgt")?
            }
        }
    })
}

fn harmless_setreg(inst: &Inst) -> bool {
    let text = inst.text();
    if text.contains("HW_REG_FLAT_SCR") {
        return true;
    }
    let Some(Operand::Named(reg)) = inst.args.first().map(|a| &a.operand) else {
        return false;
    };
    let Some(code) = rdna::parse_hwreg(reg) else {
        return false;
    };
    let value = match inst.args.get(1).map(|a| &a.operand) {
        Some(Operand::Int(value)) => *value,
        Some(Operand::Literal(value)) => i64::from(*value),
        _ => return false,
    };
    let offset = (code >> 6) & 31;
    let size = ((code >> 11) & 31) + 1;
    code & 63 == 1 && offset + size <= 4 && value == 0
}

enum Half {
    Tmp(Tmp),
    Bits(u32),
}

impl From<Tmp> for Half {
    fn from(tmp: Tmp) -> Half {
        Half::Tmp(tmp)
    }
}

fn self_constant_half(bits: u32) -> Half {
    Half::Bits(rdna::format::f32_to_f16(bits))
}

fn has_output_modifiers(inst: &Inst) -> bool {
    inst.mods
        .iter()
        .any(|m| m == "clamp" || m.starts_with("mul:") || m.starts_with("div:"))
}

fn op_sel(inst: &Inst, key: &str) -> [bool; 4] {
    let mut bits = [false; 4];
    let list = inst
        .mods
        .iter()
        .find_map(|m| m.strip_prefix(key)?.strip_prefix(":["))
        .and_then(|rest| rest.strip_suffix(']'));
    if let Some(list) = list {
        for (slot, value) in bits.iter_mut().zip(list.split(',')) {
            *slot = value.trim() == "1";
        }
    }
    bits
}

fn vgprs(arg: &Arg, inst: &Inst) -> Result<(u8, u8), LiftError> {
    match arg.operand {
        Operand::Vgpr { reg, count } => Ok((reg, count)),
        _ => Err(unsupported(inst)),
    }
}

fn logic_scalar(name: &str) -> Option<(Bin, bool, bool)> {
    Some(match name {
        "s_and_b32" => (Bin::And, false, false),
        "s_or_b32" => (Bin::Or, false, false),
        "s_xor_b32" => (Bin::Xor, false, false),
        "s_andn2_b32" => (Bin::And, false, true),
        "s_orn2_b32" => (Bin::Or, false, true),
        "s_nand_b32" => (Bin::And, true, false),
        "s_nor_b32" => (Bin::Or, true, false),
        "s_xnor_b32" => (Bin::Xor, true, false),
        "s_lshl_b32" => (Bin::Shl, false, false),
        "s_lshr_b32" => (Bin::ShrU, false, false),
        "s_ashr_i32" => (Bin::ShrS, false, false),
        _ => return None,
    })
}

fn binary_scalar(name: &str) -> bool {
    logic_scalar(name).is_some()
        || matches!(
            name,
            "s_add_u32"
                | "s_addc_u32"
                | "s_sub_u32"
                | "s_subb_u32"
                | "s_add_i32"
                | "s_sub_i32"
                | "s_min_u32"
                | "s_max_u32"
                | "s_min_i32"
                | "s_max_i32"
                | "s_cselect_b32"
                | "s_bfe_u32"
                | "s_lshl1_add_u32"
                | "s_lshl2_add_u32"
                | "s_lshl3_add_u32"
                | "s_lshl4_add_u32"
                | "s_pack_ll_b32_b16"
                | "s_pack_lh_b32_b16"
                | "s_pack_hh_b32_b16"
                | "s_mul_i32"
                | "s_mul_hi_u32"
                | "s_bfe_i32"
        )
}

#[derive(Clone, Copy)]
enum Flag {
    Keep,
    NonZero,
    NonZero64,
    Set(Tmp),
}

#[derive(Clone, Copy)]
enum Ternary {
    Fma,
    Mad,
    Add3,
    LshlAdd,
    MadU24,
    BfeU,
    AndOr,
    Or3,
    Xor3,
    AddLshl,
    LshlOr,
    Xad,
    Bfi,
    AlignBit,
    AlignByte,
    Min3(Bin),
    Max3(Bin),
    Min3F,
    Max3F,
    Med3 { min: Bin, max: Bin },
    BfeI,
    Perm,
}

fn ternary(name: &str) -> Option<Ternary> {
    Some(match name {
        "v_xor3_b32" => Ternary::Xor3,
        "v_add_lshl_u32" => Ternary::AddLshl,
        "v_lshl_or_b32" => Ternary::LshlOr,
        "v_xad_u32" => Ternary::Xad,
        "v_bfi_b32" => Ternary::Bfi,
        "v_alignbit_b32" => Ternary::AlignBit,
        "v_alignbyte_b32" => Ternary::AlignByte,
        "v_min3_u32" => Ternary::Min3(Bin::MinU),
        "v_max3_u32" => Ternary::Max3(Bin::MaxU),
        "v_min3_i32" => Ternary::Min3(Bin::MinS),
        "v_max3_i32" => Ternary::Max3(Bin::MaxS),
        "v_min3_f32" => Ternary::Min3F,
        "v_max3_f32" => Ternary::Max3F,
        "v_med3_u32" => Ternary::Med3 {
            min: Bin::MinU,
            max: Bin::MaxU,
        },
        "v_med3_i32" => Ternary::Med3 {
            min: Bin::MinS,
            max: Bin::MaxS,
        },
        "v_bfe_i32" => Ternary::BfeI,
        "v_perm_b32" => Ternary::Perm,
        "v_fma_f32" => Ternary::Fma,
        "v_mad_f32" => Ternary::Mad,
        "v_add3_u32" => Ternary::Add3,
        "v_lshl_add_u32" => Ternary::LshlAdd,
        "v_mad_u32_u24" => Ternary::MadU24,
        "v_bfe_u32" => Ternary::BfeU,
        "v_and_or_b32" => Ternary::AndOr,
        "v_or3_b32" => Ternary::Or3,
        _ => return None,
    })
}

fn unary(name: &str) -> Option<Un> {
    Some(match name {
        "v_mov_b32" => return None,
        "v_not_b32" => Un::Not,
        "v_bfrev_b32" => Un::Bfrev,
        "v_ffbl_b32" => Un::Ffbl,
        "v_ffbh_u32" => Un::Ffbh,
        "v_cvt_f32_u32" => Un::CvtF32U32,
        "v_cvt_f32_i32" => Un::CvtF32I32,
        "v_cvt_u32_f32" => Un::CvtU32F32,
        "v_cvt_i32_f32" => Un::CvtI32F32,
        "v_rcp_f32" | "v_rcp_iflag_f32" => Un::FRcp,
        "v_sin_f32" => Un::FSin,
        "v_cos_f32" => Un::FCos,
        "v_rsq_f32" => Un::FRsq,
        "v_sqrt_f32" => Un::FSqrt,
        "v_exp_f32" => Un::FExp2,
        "v_log_f32" => Un::FLog2,
        "v_floor_f32" => Un::FFloor,
        "v_ceil_f32" => Un::FCeil,
        "v_trunc_f32" => Un::FTrunc,
        "v_rndne_f32" => Un::FRndne,
        "v_fract_f32" => Un::FFract,
        _ => return None,
    })
}

fn is_float_un(op: Un) -> bool {
    matches!(
        op,
        Un::FRcp
            | Un::FRsq
            | Un::FSqrt
            | Un::FExp2
            | Un::FLog2
            | Un::FFloor
            | Un::FCeil
            | Un::FTrunc
            | Un::FRndne
            | Un::FFract
            | Un::FSin
            | Un::FCos
    )
}

fn binary(name: &str) -> Option<(Bin, bool, bool)> {
    Some(match name {
        "v_add_nc_u32" => (Bin::Add, false, false),
        "v_sub_nc_u32" => (Bin::Sub, false, false),
        "v_subrev_nc_u32" => (Bin::Sub, false, true),
        "v_mul_lo_u32" => (Bin::Mul, false, false),
        "v_mul_hi_u32" => (Bin::MulHiU, false, false),
        "v_and_b32" => (Bin::And, false, false),
        "v_or_b32" => (Bin::Or, false, false),
        "v_xor_b32" => (Bin::Xor, false, false),
        "v_lshlrev_b32" => (Bin::Shl, false, true),
        "v_lshrrev_b32" => (Bin::ShrU, false, true),
        "v_ashrrev_i32" => (Bin::ShrS, false, true),
        "v_lshl_b32" => (Bin::Shl, false, false),
        "v_lshr_b32" => (Bin::ShrU, false, false),
        "v_min_u32" => (Bin::MinU, false, false),
        "v_max_u32" => (Bin::MaxU, false, false),
        "v_min_i32" => (Bin::MinS, false, false),
        "v_max_i32" => (Bin::MaxS, false, false),
        "v_add_f32" => (Bin::FAdd, true, false),
        "v_sub_f32" => (Bin::FSub, true, false),
        "v_subrev_f32" => (Bin::FSub, true, true),
        "v_mul_f32" => (Bin::FMul, true, false),
        "v_min_f32" => (Bin::FMin, true, false),
        "v_max_f32" => (Bin::FMax, true, false),
        _ => return None,
    })
}

fn int_compare(op: &str) -> Option<Cmp> {
    Some(match op {
        "eq_u32" | "eq_i32" => Cmp::Eq,
        "lg_u32" | "lg_i32" | "ne_u32" | "ne_i32" => Cmp::Ne,
        "lt_u32" => Cmp::LtU,
        "le_u32" => Cmp::LeU,
        "gt_u32" => Cmp::GtU,
        "ge_u32" => Cmp::GeU,
        "lt_i32" => Cmp::LtS,
        "le_i32" => Cmp::LeS,
        "gt_i32" => Cmp::GtS,
        "ge_i32" => Cmp::GeS,
        "f_u32" | "f_i32" => Cmp::False,
        "t_u32" | "t_i32" => Cmp::True,
        _ => return None,
    })
}

fn float_compare(op: &str) -> Option<Cmp> {
    Some(match op {
        "f" => Cmp::False,
        "lt" => Cmp::FLt,
        "eq" => Cmp::FEq,
        "le" => Cmp::FLe,
        "gt" => Cmp::FGt,
        "lg" => Cmp::FLg,
        "ge" => Cmp::FGe,
        "o" => Cmp::FO,
        "u" => Cmp::FU,
        "nge" => Cmp::FNge,
        "nlg" => Cmp::FNlg,
        "ngt" => Cmp::FNgt,
        "nle" => Cmp::FNle,
        "neq" => Cmp::FNeq,
        "nlt" => Cmp::FNlt,
        "t" | "tru" => Cmp::True,
        "class" => Cmp::Class,
        _ => return None,
    })
}
