use crate::ir::{
    ATTRIBUTES, Bin, Cmp, Expr, Function, Reg, Stmt, Terminator, Tri, Un, attribute_slot,
};

const VCC: usize = 106;
const M0: usize = 124;
const EXEC: usize = 126;

pub const BLOCK_LIMIT: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("memory access at 0x{0:x} is outside the memory")]
    OutOfBounds(u64),
    #[error("LDS access at 0x{0:x} is outside the LDS")]
    LdsOutOfBounds(u64),
    #[error("ran more than {BLOCK_LIMIT} blocks")]
    TooLong,
    #[error("images are not modelled by the interpreter")]
    Image,
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub sgpr: [u32; 128],
    pub vgpr: Vec<[u32; 64]>,
    pub scc: bool,
    pub lds: Vec<u8>,
    pub memory: Vec<u8>,
    pub flush_denormals: bool,
    pub canonical_nan: Option<u32>,
    pub exports: Vec<u32>,
    pub messages: Vec<(u32, u32)>,
    pub code_base: u64,
    pub attributes: Vec<[u32; 64]>,
}

pub const EXPORT_WORDS: usize = 64 * 4 * 64;

pub fn export_slot(target: u8, component: usize, lane: usize) -> usize {
    (usize::from(target) * 4 + component) * 64 + lane
}

impl Machine {
    pub fn new(wave64: bool, memory: usize) -> Machine {
        let mut machine = Machine {
            sgpr: [0; 128],
            vgpr: vec![[0; 64]; 256],
            scc: false,
            lds: vec![0; 64 * 1024],
            memory: vec![0; memory],
            flush_denormals: false,
            canonical_nan: None,
            exports: vec![0; EXPORT_WORDS],
            messages: Vec::new(),
            code_base: 0,
            attributes: vec![[0; 64]; ATTRIBUTES * 12],
        };
        machine.sgpr[EXEC] = u32::MAX;
        if wave64 {
            machine.sgpr[EXEC + 1] = u32::MAX;
        }
        machine
    }

    fn pair(&self, slot: usize) -> u64 {
        u64::from(self.sgpr[slot]) | (u64::from(self.sgpr[slot + 1]) << 32)
    }

    fn set_pair(&mut self, slot: usize, value: u64) {
        self.sgpr[slot] = value as u32;
        self.sgpr[slot + 1] = (value >> 32) as u32;
    }
}

fn word(bytes: &[u8], address: u64) -> Option<u32> {
    let at = usize::try_from(address).ok()?;
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn put(bytes: &mut [u8], address: u64, value: u32) -> Option<()> {
    let at = usize::try_from(address).ok()?;
    bytes
        .get_mut(at..at.checked_add(4)?)?
        .copy_from_slice(&value.to_le_bytes());
    Some(())
}

fn f(bits: u64) -> f32 {
    f32::from_bits(bits as u32)
}

#[derive(Clone, Copy)]
struct Mode {
    flush: bool,
    nan: Option<u32>,
}

fn settle(value: f32, mode: Mode) -> f32 {
    match mode.nan {
        Some(bits) if value.is_nan() => f32::from_bits(bits),
        _ if mode.flush && value.is_subnormal() => 0.0f32.copysign(value),
        _ => value,
    }
}

fn load(bits: u64, mode: Mode) -> f32 {
    settle(f(bits), mode)
}

fn store(value: f32, mode: Mode) -> u64 {
    u64::from(settle(value, mode).to_bits())
}

pub fn float_class(bits: u32) -> u32 {
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

fn unary(op: Un, a: u64, mode: Mode) -> u64 {
    let f = |x: u64| load(x, mode);
    let bits = |v: f32| store(v, mode);
    let x = a as u32;
    match op {
        Un::Not => u64::from(!x),
        Un::F16ToF32 => u64::from(rdna::format::f16_to_f32(x & 0xFFFF, true)),
        Un::F32ToF16(round) => u64::from(rdna::format::f32_to_f16_round(x, round)),
        Un::FCanonical => bits(f(a)),
        Un::Bfrev => u64::from(x.reverse_bits()),
        Un::Ffbl => u64::from(if x == 0 { u32::MAX } else { x.trailing_zeros() }),
        Un::Ffbh => u64::from(if x == 0 { u32::MAX } else { x.leading_zeros() }),
        Un::Popcount => u64::from(x.count_ones()),
        Un::Popcount64 => u64::from(a.count_ones()),
        Un::Not64 => !a,
        Un::Lo | Un::Zext => u64::from(x),
        Un::Hi => a >> 32,
        Un::BoolToU32 => u64::from(a != 0),
        Un::CvtF32U32 => bits(x as f32),
        Un::CvtF32I32 => bits(x as i32 as f32),
        Un::CvtU32F32 => u64::from(f(a) as u32),
        Un::CvtI32F32 => u64::from(f(a) as i32 as u32),
        Un::FNeg => u64::from(x ^ 0x8000_0000),
        Un::FAbs => u64::from(x & 0x7FFF_FFFF),
        Un::FClamp01 => bits(if f(a) > 0.0 { f(a).min(1.0) } else { 0.0 }),
        Un::FRcp => bits(nan_rule(1.0 / f(a), &[f(a)])),
        Un::FRsq => bits(nan_rule(1.0 / f(a).sqrt(), &[f(a)])),
        Un::FSqrt => bits(nan_rule(f(a).sqrt(), &[f(a)])),
        Un::FExp2 => bits(nan_rule(f(a).exp2(), &[f(a)])),
        Un::FLog2 => bits(nan_rule(f(a).log2(), &[f(a)])),
        Un::FFloor => bits(nan_rule(f(a).floor(), &[f(a)])),
        Un::FCeil => bits(nan_rule(f(a).ceil(), &[f(a)])),
        Un::FTrunc => bits(nan_rule(f(a).trunc(), &[f(a)])),
        Un::FRndne => bits(nan_rule(f(a).round_ties_even(), &[f(a)])),
        Un::FFract => bits(nan_rule(f(a) - f(a).floor(), &[f(a)])),
        Un::FSin => bits(nan_rule((f(a) * std::f32::consts::TAU).sin(), &[f(a)])),
        Un::FCos => bits(nan_rule((f(a) * std::f32::consts::TAU).cos(), &[f(a)])),
    }
}

fn binary(op: Bin, a: u64, b: u64, mode: Mode) -> u64 {
    let f = |x: u64| load(x, mode);
    let bits = |v: f32| store(v, mode);
    let (x, y) = (a as u32, b as u32);
    u64::from(match op {
        Bin::Add => x.wrapping_add(y),
        Bin::Sub => x.wrapping_sub(y),
        Bin::Mul => x.wrapping_mul(y),
        Bin::MulHiU => ((u64::from(x) * u64::from(y)) >> 32) as u32,
        Bin::And => x & y,
        Bin::Or => x | y,
        Bin::Xor => x ^ y,
        Bin::Shl => x << (y & 31),
        Bin::ShrU => x >> (y & 31),
        Bin::ShrS => ((x as i32) >> (y & 31)) as u32,
        Bin::MinU => x.min(y),
        Bin::MaxU => x.max(y),
        Bin::MinS => (x as i32).min(y as i32) as u32,
        Bin::MaxS => (x as i32).max(y as i32) as u32,
        Bin::FAdd => return bits(nan_rule(f(a) + f(b), &[f(a), f(b)])),
        Bin::FSub => return bits(nan_rule(f(a) - f(b), &[f(a), f(b)])),
        Bin::FMul => return bits(nan_rule(f(a) * f(b), &[f(a), f(b)])),
        Bin::FMin => return bits(nan_rule(fmin(f(a), f(b)), &[f(a), f(b)])),
        Bin::FMax => return bits(nan_rule(fmax(f(a), f(b)), &[f(a), f(b)])),
        Bin::Add64 => return a.wrapping_add(b),
        Bin::Sub64 => return a.wrapping_sub(b),
        Bin::And64 => return a & b,
        Bin::Or64 => return a | b,
        Bin::Xor64 => return a ^ b,
        Bin::Shl64 => return a << (b & 63),
        Bin::ShrU64 => return a >> (b & 63),
        Bin::Pack64 => return u64::from(x) | (u64::from(y) << 32),
        Bin::LaneBit => return (a >> (b & 63)) & 1,
    })
}

fn ternary(op: Tri, a: u64, b: u64, c: u64, mode: Mode) -> u64 {
    let f = |x: u64| load(x, mode);
    let bits = |v: f32| store(v, mode);
    match op {
        Tri::FFma => bits(nan_rule(f(a).mul_add(f(b), f(c)), &[f(a), f(b), f(c)])),
        Tri::FMad => {
            let product = f(bits(nan_rule(f(a) * f(b), &[f(a), f(b)])));
            bits(nan_rule(product + f(c), &[product, f(c)]))
        }
        Tri::BfeU => {
            let width = c as u32 & 31;
            if width == 0 {
                0
            } else {
                u64::from((a as u32 >> (b as u32 & 31)) & (u32::MAX >> (32 - width)))
            }
        }
    }
}

fn compare(op: Cmp, a: u64, b: u64, mode: Mode) -> bool {
    let f = |x: u64| load(x, mode);
    use std::cmp::Ordering::{Equal, Greater, Less};
    let (x, y) = (a as u32, b as u32);
    let ord = f(a).partial_cmp(&f(b));
    let (lt, eq, gt) = (ord == Some(Less), ord == Some(Equal), ord == Some(Greater));
    match op {
        Cmp::Eq => x == y,
        Cmp::Ne => x != y,
        Cmp::LtU => x < y,
        Cmp::LeU => x <= y,
        Cmp::GtU => x > y,
        Cmp::GeU => x >= y,
        Cmp::LtS => (x as i32) < (y as i32),
        Cmp::LeS => (x as i32) <= (y as i32),
        Cmp::GtS => (x as i32) > (y as i32),
        Cmp::GeS => (x as i32) >= (y as i32),
        Cmp::Eq64 => a == b,
        Cmp::Ne64 => a != b,
        Cmp::FLt => lt,
        Cmp::FEq => eq,
        Cmp::FLe => lt || eq,
        Cmp::FGt => gt,
        Cmp::FLg => lt || gt,
        Cmp::FGe => gt || eq,
        Cmp::FO => ord.is_some(),
        Cmp::FU => ord.is_none(),
        Cmp::FNge => !(gt || eq),
        Cmp::FNlg => !(lt || gt),
        Cmp::FNgt => !gt,
        Cmp::FNle => !(lt || eq),
        Cmp::FNeq => !eq,
        Cmp::FNlt => !lt,
        Cmp::False => false,
        Cmp::True => true,
        Cmp::Class => (y >> float_class(x)) & 1 != 0,
    }
}

struct Run<'a> {
    machine: &'a mut Machine,
    lanes: usize,
    values: Vec<[u64; 64]>,
    varying: Vec<bool>,
}

impl Run<'_> {
    fn mode(&self) -> Mode {
        Mode {
            flush: self.machine.flush_denormals,
            nan: self.machine.canonical_nan,
        }
    }

    fn exec(&self) -> u64 {
        let mask = if self.lanes == 64 {
            u64::MAX
        } else {
            0xFFFF_FFFF
        };
        self.machine.pair(EXEC) & mask
    }

    fn active(&self, lane: usize) -> bool {
        self.exec() >> lane & 1 != 0
    }

    fn get(&self, reg: Reg, lane: usize) -> u64 {
        let m = &self.machine;
        match reg {
            Reg::S(r) => u64::from(m.sgpr[usize::from(r)]),
            Reg::V(r) => u64::from(m.vgpr[usize::from(r)][lane]),
            Reg::Exec => m.pair(EXEC),
            Reg::Vcc => m.pair(VCC),
            Reg::Scc => u64::from(m.scc),
            Reg::M0 => u64::from(m.sgpr[M0]),
        }
    }

    fn eval(&self, expr: &Expr, lane: usize) -> Result<u64, RunError> {
        let v = |t: &u32| self.values[*t as usize][lane];
        Ok(match expr {
            Expr::Const(c) => u64::from(*c),
            Expr::Const64(c) => *c,
            Expr::Get(reg) => self.get(*reg, lane),
            Expr::GetPair(r) => self.machine.pair(usize::from(*r)),
            Expr::Un(op, a) => unary(*op, v(a), self.mode()),
            Expr::Bin(op, a, b) => binary(*op, v(a), v(b), self.mode()),
            Expr::Tri(op, a, b, c) => ternary(*op, v(a), v(b), v(c), self.mode()),
            Expr::Cmp(op, a, b) => u64::from(compare(*op, v(a), v(b), self.mode())),
            Expr::Select(c, a, b) => {
                if v(c) != 0 {
                    v(a)
                } else {
                    v(b)
                }
            }
            Expr::LaneId => lane as u64,
            Expr::CodeBase => self.machine.code_base,
            Expr::Active => self.exec(),
            Expr::Ballot(t) => (0..self.lanes)
                .filter(|l| self.active(*l) && self.values[*t as usize][*l] != 0)
                .fold(0, |mask, l| mask | 1 << l),
            Expr::ReadLane(source, which) | Expr::Shuffle(source, which) => {
                self.values[*source as usize][v(which) as usize % self.lanes]
            }
            Expr::GetRelative(base, index) => {
                let at = usize::from(*base) + v(index) as u32 as usize;
                let at = if at < 256 { at } else { 0 };
                u64::from(self.machine.vgpr[at][lane])
            }
            Expr::FirstActiveLane => {
                let exec = self.exec();
                if exec == 0 {
                    0
                } else {
                    u64::from(exec.trailing_zeros())
                }
            }
            Expr::LdsLoad(address) => {
                if !self.active(lane) {
                    return Ok(0);
                }
                let address = v(address) & 0xFFFF_FFFF;
                u64::from(
                    word(&self.machine.lds, address).ok_or(RunError::LdsOutOfBounds(address))?,
                )
            }
            Expr::Load { when, address } => {
                if !self.active(lane) || when.is_some_and(|w| v(&w) == 0) {
                    return Ok(0);
                }
                let address = v(address);
                u64::from(
                    word(&self.machine.memory, address).ok_or(RunError::OutOfBounds(address))?,
                )
            }
            Expr::TypedLoad {
                format,
                address,
                in_range,
                select,
                component,
            } => {
                if !self.active(lane) {
                    return Ok(0);
                }
                let shift = 3 * u32::from(*component);
                let selector =
                    select.map_or(4 + u32::from(*component), |s| (v(&s) as u32 >> shift) & 7);
                let address = v(address);
                let mut failure = None;
                let value =
                    rdna::format::select(v(format) as u32, selector, v(in_range) != 0, |layout| {
                        let start = usize::try_from(address).unwrap_or(usize::MAX);
                        match self
                            .machine
                            .memory
                            .get(start..start.saturating_add(layout.bytes()))
                        {
                            Some(bytes) => layout.decode(bytes),
                            None => {
                                failure = Some(RunError::OutOfBounds(address));
                                [0; 4]
                            }
                        }
                    });
                if let Some(error) = failure {
                    return Err(error);
                }
                u64::from(value)
            }
            Expr::ScalarLoad(address) => {
                let address = v(address);
                u64::from(
                    word(&self.machine.memory, address).ok_or(RunError::OutOfBounds(address))?,
                )
            }
            Expr::Image(_) => return Err(RunError::Image),
            Expr::Attribute {
                attr,
                channel,
                vertex,
            } => u64::from(self.machine.attributes[attribute_slot(*attr, *channel, *vertex)][lane]),
        })
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), RunError> {
        match stmt {
            Stmt::Let(tmp, expr) => {
                let slot = *tmp as usize;
                if self.varying[slot] {
                    for lane in 0..self.lanes {
                        self.values[slot][lane] = self.eval(expr, lane)?;
                    }
                } else {
                    let value = self.eval(expr, 0)?;
                    self.values[slot] = [value; 64];
                }
            }
            Stmt::Set(reg, tmp) => {
                let value = self.values[*tmp as usize][0];
                let m = &mut *self.machine;
                match reg {
                    Reg::S(r) => m.sgpr[usize::from(*r)] = value as u32,
                    Reg::V(r) => m.vgpr[usize::from(*r)] = [value as u32; 64],
                    Reg::Exec => m.set_pair(EXEC, value),
                    Reg::Vcc => m.set_pair(VCC, value),
                    Reg::Scc => m.scc = value != 0,
                    Reg::M0 => m.sgpr[M0] = value as u32,
                }
            }
            Stmt::SetPair(r, tmp) => {
                let value = self.values[*tmp as usize][0];
                self.machine.set_pair(usize::from(*r), value);
            }
            Stmt::SetLane(r, tmp) => {
                for lane in 0..self.lanes {
                    if self.active(lane) {
                        self.machine.vgpr[usize::from(*r)][lane] =
                            self.values[*tmp as usize][lane] as u32;
                    }
                }
            }
            Stmt::WriteLane(r, lane, value) => {
                let lane = self.values[*lane as usize][0] as usize % self.lanes;
                self.machine.vgpr[usize::from(*r)][lane] = self.values[*value as usize][0] as u32;
            }
            Stmt::Export { target, values } => {
                for lane in 0..self.lanes {
                    if self.active(lane) {
                        for (component, value) in values.iter().enumerate() {
                            if let Some(value) = value {
                                let word = self.values[*value as usize][lane] as u32;
                                self.machine.exports[export_slot(*target, component, lane)] = word;
                            }
                        }
                    }
                }
            }
            Stmt::FetchVertexInputs => {}
            Stmt::Message(code, m0) => {
                let m0 = self.values[*m0 as usize][0] as u32;
                self.machine.messages.push((*code, m0));
            }
            Stmt::LdsStore(address, value) => {
                for lane in 0..self.lanes {
                    if self.active(lane) {
                        let address = self.values[*address as usize][lane] & 0xFFFF_FFFF;
                        let value = self.values[*value as usize][lane] as u32;
                        put(&mut self.machine.lds, address, value)
                            .ok_or(RunError::LdsOutOfBounds(address))?;
                    }
                }
            }
            Stmt::ImageStore { .. } => return Err(RunError::Image),
            Stmt::Store {
                when,
                address,
                value,
            } => {
                for lane in 0..self.lanes {
                    let enabled = when.is_none_or(|w| self.values[w as usize][lane] != 0);
                    if self.active(lane) && enabled {
                        let address = self.values[*address as usize][lane];
                        let value = self.values[*value as usize][lane] as u32;
                        put(&mut self.machine.memory, address, value)
                            .ok_or(RunError::OutOfBounds(address))?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn run(function: &Function, machine: &mut Machine) -> Result<usize, RunError> {
    let mut run = Run {
        machine,
        lanes: if function.wave64 { 64 } else { 32 },
        values: vec![[0; 64]; function.temps as usize],
        varying: function.varying(),
    };
    let mut block = 0;
    let mut count = 0;
    while let Some(current) = function.blocks.get(block) {
        count += 1;
        if count > BLOCK_LIMIT {
            return Err(RunError::TooLong);
        }
        for stmt in &current.stmts {
            run.stmt(stmt)?;
        }
        block = match current.terminator {
            Terminator::Jump(next) => next,
            Terminator::Branch(condition, taken, fallthrough) => {
                if run.values[condition as usize][0] != 0 {
                    taken
                } else {
                    fallthrough
                }
            }
            Terminator::End => break,
        };
    }
    Ok(count)
}
