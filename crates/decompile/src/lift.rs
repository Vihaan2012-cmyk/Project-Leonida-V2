use std::collections::{HashMap, HashSet, VecDeque};

use iced_x86::{
    ConditionCode, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess, OpKind,
    Register,
};

use crate::cfg::RawBlock;
use crate::expr::*;
use crate::{Program, Symbol};

#[derive(Clone, Debug)]
pub enum Stmt {
    Def(Loc, u8, Expr),
    Store(Expr, u8, bool, Expr),
    Eval(Expr),
    Clobber(Vec<Loc>),
    Asm(String),
}

#[derive(Clone, Debug)]
pub enum Exit {
    Switch(Expr, Vec<usize>, Option<usize>),
    Jump(usize),
    Branch(Expr, usize, usize),
    Return(Option<Expr>),
    TailCall(Expr),
    Stop,
}

pub struct Block {
    pub start: u64,
    pub stmts: Vec<Stmt>,
    pub exit: Exit,
}

pub struct Function {
    pub base: u64,
    pub entry: usize,
    pub blocks: Vec<Block>,
    pub succ: Vec<Vec<usize>>,
    pub pred: Vec<Vec<usize>>,
    /// The register that carries the return value at `ret` and its width in bytes.
    pub returns: Option<(Loc, u8)>,
    pub aliased: HashSet<i64>,
}

enum RawExit {
    Switch(Expr, Vec<u64>),
    Jump(u64),
    Branch(Expr, u64, u64),
    Return,
    TailCall(Expr),
    Stop,
}

enum Place {
    Stack(i64),
    Global(u64),
    Memory(Expr),
}

struct State {
    delta: i64,
    entry: bool,
    written: HashSet<u8>,
    since_call: Vec<(usize, Loc, u8)>,
    eax: Option<u64>,
}

const FLAGS: Loc = Loc::Flags;

fn callee_saved(r: u8) -> bool {
    matches!(r, RBX | RBP | RSI | RDI | 12..=15) || (VECTOR + 6..VECTOR + 16).contains(&r)
}

pub fn volatile() -> Vec<Loc> {
    let mut locs: Vec<Loc> = [RCX, RDX, R8, R9, 10, 11]
        .into_iter()
        .map(Loc::Reg)
        .collect();
    locs.extend((VECTOR..VECTOR + 6).map(Loc::Reg));
    locs.extend([Loc::Reg(RAX), FLAGS]);
    locs
}

fn reg(r: Register) -> Option<(u8, u8, bool)> {
    if r.is_gpr() {
        let high = matches!(r, Register::AH | Register::CH | Register::DH | Register::BH);
        Some((r.full_register().number() as u8, r.size() as u8, high))
    } else if r.is_xmm() || r.is_ymm() || r.is_zmm() {
        Some((VECTOR + r.number() as u8, r.size() as u8, false))
    } else {
        None
    }
}

fn op_width(i: &Instruction, op: u32) -> u8 {
    let w = match i.op_kind(op) {
        OpKind::Register => i.op_register(op).size(),
        OpKind::Memory => i.memory_size().size(),
        _ => 0,
    };
    if w == 0 {
        if op > 0 { op_width(i, 0) } else { 8 }
    } else {
        w.min(64) as u8
    }
}

/// Scalar float instructions compute the low lane only, so their register result is 4 or 8 bytes wide.
fn scalar_float_width(name: &str, i: &Instruction) -> Option<u8> {
    if i.op_count() == 0 || i.op0_kind() != OpKind::Register || !i.op0_register().is_xmm() {
        return None;
    }
    let name = name.to_ascii_lowercase();
    if name.starts_with('p') || name.starts_with("vp") || name.contains("broadcast") {
        return None;
    }
    match &name[name.len().saturating_sub(2)..] {
        "ss" => Some(4),
        "sd" => Some(8),
        _ => None,
    }
}

fn float_memory(i: &Instruction) -> bool {
    let size = i.memory_size();
    !size.is_packed() && format!("{size:?}").starts_with("Float")
}

fn sign_extend(e: Expr, from: u8, to: u8) -> Expr {
    Expr::Cast(
        signed_type(to),
        Box::new(Expr::Cast(signed_type(from), Box::new(e))),
    )
}

fn is_immediate(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate64
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
            | OpKind::Immediate8to64
            | OpKind::Immediate32to64
    )
}

fn is_move(m: Mnemonic) -> bool {
    use Mnemonic as M;
    matches!(
        m,
        M::Mov
            | M::Movaps
            | M::Movups
            | M::Movapd
            | M::Movupd
            | M::Movdqa
            | M::Movdqu
            | M::Vmovaps
            | M::Vmovups
            | M::Vmovapd
            | M::Vmovupd
            | M::Vmovdqa
            | M::Vmovdqu
            | M::Vmovdqa32
            | M::Vmovdqa64
            | M::Vmovdqu8
            | M::Vmovdqu16
            | M::Vmovdqu32
            | M::Vmovdqu64
            | M::Movd
            | M::Movq
            | M::Vmovd
            | M::Vmovq
            | M::Movss
            | M::Movsd
            | M::Vmovss
            | M::Vmovsd
            | M::Movnti
            | M::Movntps
            | M::Vmovntps
            | M::Movntdq
            | M::Vmovntdq
            | M::Lddqu
            | M::Vlddqu
    )
}

fn zero_idiom(i: &Instruction) -> bool {
    use Mnemonic as M;
    let same = |a: u32, b: u32| {
        i.op_kind(a) == OpKind::Register
            && i.op_kind(b) == OpKind::Register
            && i.op_register(a) == i.op_register(b)
    };
    matches!(
        i.mnemonic(),
        M::Xor
            | M::Sub
            | M::Pxor
            | M::Xorps
            | M::Xorpd
            | M::Vxorps
            | M::Vxorpd
            | M::Vpxor
            | M::Vpxord
            | M::Vpxorq
    ) && ((i.op_count() == 2 && same(0, 1)) || (i.op_count() == 3 && same(1, 2)))
}

pub struct Lifter<'p> {
    program: &'p dyn Program,
    base: u64,
    entry: u64,
    factory: InstructionInfoFactory,
    saves: HashMap<i64, u8>,
    frame: Option<i64>,
    /// The width of the vector register an instruction writes when it is a scalar float operation.
    scalar: Option<u8>,
}

impl<'p> Lifter<'p> {
    pub fn new(program: &'p dyn Program, entry: u64) -> Self {
        Self {
            program,
            base: program.image_base(),
            entry,
            factory: InstructionInfoFactory::new(),
            saves: HashMap::new(),
            frame: None,
            scalar: None,
        }
    }

    pub fn code_name(&self, va: u64) -> Expr {
        match self.program.symbol(va) {
            Symbol::Import(name) => Expr::Name(name),
            _ => Expr::Name(
                self.program
                    .name(va)
                    .unwrap_or_else(|| format!("sub_{:08x}", va.wrapping_sub(self.base))),
            ),
        }
    }

    fn global(&self, va: u64) -> Expr {
        Expr::Name(
            self.program
                .name(va)
                .unwrap_or_else(|| format!("g_{:08x}", va.wrapping_sub(self.base))),
        )
    }

    fn read_reg(&self, r: Register, st: &State, i: &Instruction) -> Expr {
        match r {
            Register::RIP => return konst(i.next_ip(), 8),
            Register::RSP | Register::ESP => {
                return Expr::AddrOf(Box::new(Expr::Var(Loc::Stack(st.delta), 8)));
            }
            Register::RBP if self.frame.is_some() => {
                return Expr::AddrOf(Box::new(Expr::Var(Loc::Stack(self.frame.unwrap_or(0)), 8)));
            }
            _ => {}
        }
        match reg(r) {
            Some((index, _, true)) => Expr::Cast(
                "u8",
                Box::new(bin(">>", Expr::Var(Loc::Reg(index), 2), Expr::Const(8, 1))),
            ),
            Some((index, w, false)) => Expr::Var(Loc::Reg(index), w),
            None => Expr::Name(format!("{r:?}").to_lowercase()),
        }
    }

    fn place(&self, i: &Instruction, st: &State) -> Place {
        if i.is_ip_rel_memory_operand() {
            return Place::Global(i.ip_rel_memory_address());
        }
        let disp = i.memory_displacement64();
        let base = i.memory_base();
        let index = i.memory_index();
        let segment = matches!(i.memory_segment(), Register::GS | Register::FS);
        if index == Register::None && !segment {
            if base == Register::RSP {
                return Place::Stack(st.delta + disp as i64);
            }
            if base == Register::RBP
                && let Some(frame) = self.frame
            {
                return Place::Stack(frame + disp as i64);
            }
            if base == Register::None {
                return Place::Global(disp);
            }
        }
        let mut address: Option<Expr> = segment.then(|| {
            Expr::Name(
                if i.memory_segment() == Register::GS {
                    "__gs"
                } else {
                    "__fs"
                }
                .into(),
            )
        });
        if base != Register::None {
            let value = self.read_reg(base, st, i);
            address = Some(match address {
                Some(a) => bin("+", a, value),
                None => value,
            });
        }
        if index != Register::None {
            let mut value = self.read_reg(index, st, i);
            let scale = i.memory_index_scale();
            if scale > 1 {
                value = bin("*", value, Expr::Const(u64::from(scale), 8));
            }
            address = Some(match address {
                Some(a) => bin("+", a, value),
                None => value,
            });
        }
        let address = match address {
            None => konst(disp, 8),
            Some(a) if disp == 0 => a,
            Some(a) => bin("+", a, konst(disp, 8)),
        };
        Place::Memory(simplify(address))
    }

    fn load(&self, i: &Instruction, st: &State) -> Expr {
        let w = (i.memory_size().size().clamp(1, 64)) as u8;
        if i.memory_segment() == Register::GS
            && i.memory_base() == Register::None
            && i.memory_index() == Register::None
        {
            let name = match w {
                1 => "__readgsbyte",
                2 => "__readgsword",
                4 => "__readgsdword",
                _ => "__readgsqword",
            };
            return Expr::Intrinsic(name.into(), vec![konst(i.memory_displacement64(), 4)]);
        }
        let float = float_memory(i);
        match self.place(i, st) {
            Place::Stack(o) => Expr::Var(Loc::Stack(o), w),
            Place::Global(va) => match self.program.symbol(va) {
                Symbol::Import(name) => Expr::Name(name),
                _ => Expr::Load(Box::new(Expr::AddrOf(Box::new(self.global(va)))), w, float),
            },
            Place::Memory(e) => Expr::Load(Box::new(e), w, float),
        }
    }

    fn address(&self, i: &Instruction, st: &State) -> Expr {
        match self.place(i, st) {
            Place::Stack(o) => Expr::AddrOf(Box::new(Expr::Var(Loc::Stack(o), 8))),
            Place::Global(va) => match self.program.symbol(va) {
                Symbol::Text(text) => Expr::Text(text),
                Symbol::Function => self.code_name(va),
                Symbol::Import(name) => Expr::AddrOf(Box::new(Expr::Name(name))),
                _ => Expr::AddrOf(Box::new(self.global(va))),
            },
            Place::Memory(e) => e,
        }
    }

    fn read(&self, i: &Instruction, op: u32, st: &State) -> Expr {
        let kind = i.op_kind(op);
        match kind {
            OpKind::Register => self.read_reg(i.op_register(op), st, i),
            OpKind::Memory => self.load(i, st),
            OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64 => {
                self.code_name(i.near_branch_target())
            }
            k if is_immediate(k) => konst(i.immediate(op), op_width(i, 0)),
            other => Expr::Name(format!("{other:?}")),
        }
    }

    fn write(&self, i: &Instruction, op: u32, value: Expr, st: &mut State, out: &mut Vec<Stmt>) {
        match i.op_kind(op) {
            OpKind::Register => match reg(i.op_register(op)) {
                Some((RSP, ..)) => {}
                Some((index, w, high)) => {
                    let w = match self.scalar {
                        Some(scalar) if index >= VECTOR => scalar,
                        _ if high => 1,
                        _ => w,
                    };
                    out.push(Stmt::Def(Loc::Reg(index), w, value));
                    note(st, Loc::Reg(index), w);
                }
                None => out.push(Stmt::Asm(
                    format!("{:?} = {value:?}", i.op_register(op)).to_lowercase(),
                )),
            },
            OpKind::Memory => {
                let w = (i.memory_size().size().clamp(1, 64)) as u8;
                let float = float_memory(i);
                match self.place(i, st) {
                    Place::Stack(o) => {
                        out.push(Stmt::Def(Loc::Stack(o), w, value));
                        note(st, Loc::Stack(o), w);
                    }
                    Place::Global(va) => out.push(Stmt::Store(
                        Expr::AddrOf(Box::new(self.global(va))),
                        w,
                        float,
                        value,
                    )),
                    Place::Memory(e) => out.push(Stmt::Store(e, w, float, value)),
                }
            }
            _ => out.push(Stmt::Eval(value)),
        }
    }

    fn call(&self, i: &Instruction, st: &mut State, out: &mut Vec<Stmt>) {
        let target = match i.op0_kind() {
            OpKind::NearBranch64 | OpKind::NearBranch32 => self.code_name(i.near_branch_target()),
            OpKind::Register => self.read_reg(i.op0_register(), st, i),
            OpKind::Memory => self.load(i, st),
            _ => Expr::Name("far_call".into()),
        };
        let mut args = arguments(st);
        if args.is_empty() && matches!(&target, Expr::Load(address, ..) if address.has_load()) {
            args.push(Expr::Var(Loc::Reg(RCX), 8));
        }
        out.push(Stmt::Def(
            Loc::Reg(RAX),
            8,
            Expr::Call(Box::new(target), args),
        ));
    }

    fn flags(&self, kind: FlagKind, a: Expr, b: Expr, w: u8, out: &mut Vec<Stmt>) {
        out.push(Stmt::Def(
            FLAGS,
            1,
            Expr::Flags(kind, Box::new(a), Box::new(b), w),
        ));
    }

    fn instruction(&mut self, i: &Instruction, st: &mut State, out: &mut Vec<Stmt>) {
        use Mnemonic as M;
        let m = i.mnemonic();
        let op0_reg =
            (i.op_count() > 0 && i.op0_kind() == OpKind::Register).then(|| i.op0_register());
        let op0_full = op0_reg.and_then(reg).map(|(r, ..)| r);
        let is_rsp = op0_full == Some(RSP);
        let w = op_width(i, 0);
        let last = i.op_count().saturating_sub(1);
        let name = format!("{m:?}");
        self.scalar = scalar_float_width(&name, i);
        match m {
            M::Nop
            | M::Endbr64
            | M::Lfence
            | M::Mfence
            | M::Sfence
            | M::Prefetcht0
            | M::Prefetcht1
            | M::Prefetcht2
            | M::Prefetchnta
            | M::Prefetchw
            | M::Vzeroupper => {}
            M::Push => {
                st.delta -= 8;
                if st.entry
                    && let Some(r) = op0_full
                    && callee_saved(r)
                    && !st.written.contains(&r)
                {
                    return;
                }
                let value = self.read(i, 0, st);
                out.push(Stmt::Def(Loc::Stack(st.delta), 8, value));
            }
            M::Pop => {
                if let Some(r) = op0_full
                    && callee_saved(r)
                {
                    st.delta += 8;
                    return;
                }
                let value = Expr::Var(Loc::Stack(st.delta), 8);
                st.delta += 8;
                self.write(i, 0, value, st, out);
            }
            M::Sub | M::Add if is_rsp => {
                let amount = if is_immediate(i.op1_kind()) {
                    i.immediate(1) as i64
                } else {
                    st.eax.unwrap_or(0) as i64
                };
                st.delta += if m == M::Sub { -amount } else { amount };
            }
            M::And | M::Lea | M::Mov if is_rsp => {}
            M::Lea if op0_full == Some(RBP) && st.entry && self.frame.is_none() => {
                if let Place::Stack(o) = self.place(i, st) {
                    self.frame = Some(o);
                } else {
                    let value = self.address(i, st);
                    self.write(i, 0, value, st, out);
                }
            }
            M::Mov
                if op0_full == Some(RBP)
                    && st.entry
                    && self.frame.is_none()
                    && i.op1_kind() == OpKind::Register
                    && i.op1_register() == Register::RSP =>
            {
                self.frame = Some(st.delta);
            }
            _ if zero_idiom(i) => {
                self.write(i, 0, Expr::Const(0, w), st, out);
                if op0_full.is_some_and(|r| r < VECTOR) {
                    self.flags(
                        FlagKind::Logic,
                        Expr::Const(0, w),
                        Expr::Const(0, w),
                        w,
                        out,
                    );
                }
            }
            _ if is_move(m) && i.op0_kind() != OpKind::MemoryESRDI && i.op_count() == 2 => {
                if i.op0_kind() == OpKind::Memory
                    && i.op1_kind() == OpKind::Register
                    && let Place::Stack(o) = self.place(i, st)
                    && let Some((r, ..)) = reg(i.op1_register())
                    && st.entry
                    && callee_saved(r)
                    && !st.written.contains(&r)
                {
                    self.saves.insert(o, r);
                    return;
                }
                if i.op1_kind() == OpKind::Memory
                    && let Some(r) = op0_full
                    && let Place::Stack(o) = self.place(i, st)
                    && self.saves.get(&o) == Some(&r)
                {
                    return;
                }
                if op0_full == Some(RAX) && is_immediate(i.op1_kind()) {
                    st.eax = Some(i.immediate(1));
                }
                let value = self.read(i, 1, st);
                self.write(i, 0, value, st, out);
            }
            M::Movzx => {
                let value = self.read(i, 1, st);
                self.write(i, 0, value, st, out);
            }
            M::Movsx | M::Movsxd => {
                let value = sign_extend(self.read(i, 1, st), op_width(i, 1), w);
                self.write(i, 0, value, st, out);
            }
            M::Lea => {
                let value = self.address(i, st);
                self.write(i, 0, value, st, out);
            }
            M::Add
            | M::Sub
            | M::And
            | M::Or
            | M::Xor
            | M::Shl
            | M::Sal
            | M::Shr
            | M::Sar
            | M::Rol
            | M::Ror => {
                let a = self.read(i, 0, st);
                let b = self.read(i, 1, st);
                match m {
                    M::Sub => self.flags(FlagKind::Compare, a.clone(), b.clone(), w, out),
                    M::Add => self.flags(FlagKind::Add, a.clone(), b.clone(), w, out),
                    _ => {}
                }
                let result = match m {
                    M::Add => bin("+", a, b),
                    M::Sub => bin("-", a, b),
                    M::And => bin("&", a, b),
                    M::Or => bin("|", a, b),
                    M::Xor => bin("^", a, b),
                    M::Shl | M::Sal => bin("<<", a, b),
                    M::Shr => bin(">>", a, b),
                    M::Sar => bin(">>", Expr::Cast(signed_type(w), Box::new(a)), b),
                    M::Rol => Expr::Intrinsic("rotl".into(), vec![a, b]),
                    _ => Expr::Intrinsic("rotr".into(), vec![a, b]),
                };
                self.write(i, 0, result, st, out);
                if !matches!(m, M::Sub | M::Add) {
                    let kind = if matches!(m, M::And | M::Or | M::Xor) {
                        FlagKind::Logic
                    } else {
                        FlagKind::Result
                    };
                    let value = self.read(i, 0, st);
                    self.flags(kind, value, Expr::Const(0, w), w, out);
                }
            }
            M::Adc | M::Sbb => {
                let args = vec![
                    self.read(i, 0, st),
                    self.read(i, 1, st),
                    Expr::Var(FLAGS, 1),
                ];
                self.write(i, 0, Expr::Intrinsic(name.to_lowercase(), args), st, out);
                out.push(Stmt::Def(FLAGS, 1, Expr::Intrinsic("flags".into(), vec![])));
            }
            M::Andn => {
                let value = bin(
                    "&",
                    Expr::Unary("~", Box::new(self.read(i, 1, st))),
                    self.read(i, 2, st),
                );
                self.write(i, 0, value, st, out);
                let result = self.read(i, 0, st);
                self.flags(FlagKind::Logic, result, Expr::Const(0, w), w, out);
            }
            M::Shlx | M::Shrx | M::Sarx => {
                let a = self.read(i, 1, st);
                let a = if m == M::Sarx {
                    Expr::Cast(signed_type(w), Box::new(a))
                } else {
                    a
                };
                let value = bin(
                    if m == M::Shlx { "<<" } else { ">>" },
                    a,
                    self.read(i, 2, st),
                );
                self.write(i, 0, value, st, out);
            }
            M::Tzcnt | M::Lzcnt | M::Popcnt => {
                let value = Expr::Intrinsic(name.to_lowercase(), vec![self.read(i, 1, st)]);
                self.write(i, 0, value, st, out);
                let result = self.read(i, 0, st);
                self.flags(FlagKind::Result, result, Expr::Const(0, w), w, out);
            }
            M::Inc | M::Dec | M::Neg | M::Not => {
                let a = self.read(i, 0, st);
                let result = match m {
                    M::Inc => bin("+", a, Expr::Const(1, w)),
                    M::Dec => bin("-", a, Expr::Const(1, w)),
                    M::Neg => Expr::Unary("-", Box::new(a)),
                    _ => Expr::Unary("~", Box::new(a)),
                };
                self.write(i, 0, result, st, out);
                if m != M::Not {
                    let value = self.read(i, 0, st);
                    self.flags(FlagKind::Result, value, Expr::Const(0, w), w, out);
                }
            }
            M::Imul if i.op_count() >= 2 => {
                let (a, b) = if i.op_count() == 3 {
                    (self.read(i, 1, st), self.read(i, 2, st))
                } else {
                    (self.read(i, 0, st), self.read(i, 1, st))
                };
                self.write(i, 0, bin("*", a, b), st, out);
                let value = self.read(i, 0, st);
                self.flags(FlagKind::Result, value, Expr::Const(0, w), w, out);
            }
            M::Mul | M::Imul => {
                let source = self.read(i, 0, st);
                let rax = Expr::Var(Loc::Reg(RAX), w);
                if w > 1 {
                    let high = if m == M::Mul { "mulhi" } else { "imulhi" };
                    out.push(Stmt::Def(
                        Loc::Reg(RDX),
                        w,
                        Expr::Intrinsic(high.into(), vec![rax.clone(), source.clone()]),
                    ));
                }
                out.push(Stmt::Def(Loc::Reg(RAX), w.max(2), bin("*", rax, source)));
                out.push(Stmt::Def(FLAGS, 1, Expr::Intrinsic("flags".into(), vec![])));
            }
            M::Div | M::Idiv => {
                let divisor = self.read(i, 0, st);
                let rax = Expr::Var(Loc::Reg(RAX), w);
                let (n, d) = if m == M::Idiv {
                    (
                        Expr::Cast(signed_type(w), Box::new(rax)),
                        Expr::Cast(signed_type(w), Box::new(divisor)),
                    )
                } else {
                    (rax, divisor)
                };
                if w > 1 {
                    out.push(Stmt::Def(Loc::Reg(RDX), w, bin("%", n.clone(), d.clone())));
                }
                out.push(Stmt::Def(Loc::Reg(RAX), w, bin("/", n, d)));
            }
            M::Cmpxchg if i.op0_kind() == OpKind::Memory => {
                let address = self.address(i, st);
                let replacement = self.read(i, 1, st);
                let expected = Loc::Reg(TEMP);
                out.push(Stmt::Def(expected, w, Expr::Var(Loc::Reg(RAX), w)));
                let exchange = Expr::Intrinsic(
                    "_InterlockedCompareExchange".into(),
                    vec![address, replacement, Expr::Var(expected, w)],
                );
                out.push(Stmt::Def(Loc::Reg(RAX), w, exchange));
                self.flags(
                    FlagKind::Compare,
                    Expr::Var(Loc::Reg(RAX), w),
                    Expr::Var(expected, w),
                    w,
                    out,
                );
            }
            M::Xadd | M::Xchg
                if i.op0_kind() == OpKind::Memory && i.op1_kind() == OpKind::Register =>
            {
                let address = self.address(i, st);
                let value = self.read(i, 1, st);
                let name = if m == M::Xadd {
                    "_InterlockedExchangeAdd"
                } else {
                    "_InterlockedExchange"
                };
                let result = Expr::Intrinsic(name.into(), vec![address, value]);
                if let Some((r, rw, _)) = reg(i.op1_register()) {
                    out.push(Stmt::Def(Loc::Reg(r), rw, result));
                }
                if m == M::Xadd {
                    out.push(Stmt::Def(FLAGS, 1, Expr::Intrinsic("flags".into(), vec![])));
                }
            }
            M::Xchg
                if i.op_count() == 2
                    && i.op0_kind() == OpKind::Register
                    && i.op0_register() == i.op1_register() => {}
            M::Cmp => {
                let (a, b) = (self.read(i, 0, st), self.read(i, 1, st));
                self.flags(FlagKind::Compare, a, b, w, out);
            }
            M::Test => {
                let (a, b) = (self.read(i, 0, st), self.read(i, 1, st));
                self.flags(FlagKind::Test, a, b, w, out);
            }
            M::Bt => {
                let (a, b) = (self.read(i, 0, st), self.read(i, 1, st));
                self.flags(FlagKind::Bit, a, b, w, out);
            }
            M::Comiss
            | M::Ucomiss
            | M::Comisd
            | M::Ucomisd
            | M::Vcomiss
            | M::Vucomiss
            | M::Vcomisd
            | M::Vucomisd => {
                let (a, b) = (self.read(i, 0, st), self.read(i, 1, st));
                let width = if name.ends_with("ss") { 4 } else { 8 };
                self.flags(FlagKind::Float, a, b, width, out);
            }
            M::Cdqe => out.push(Stmt::Def(
                Loc::Reg(RAX),
                8,
                sign_extend(Expr::Var(Loc::Reg(RAX), 4), 4, 8),
            )),
            M::Cwde => out.push(Stmt::Def(
                Loc::Reg(RAX),
                4,
                sign_extend(Expr::Var(Loc::Reg(RAX), 2), 2, 4),
            )),
            M::Cdq => out.push(Stmt::Def(
                Loc::Reg(RDX),
                4,
                bin(
                    ">>",
                    Expr::Cast("i32", Box::new(Expr::Var(Loc::Reg(RAX), 4))),
                    Expr::Const(31, 1),
                ),
            )),
            M::Cqo => out.push(Stmt::Def(
                Loc::Reg(RDX),
                8,
                bin(
                    ">>",
                    Expr::Cast("i64", Box::new(Expr::Var(Loc::Reg(RAX), 8))),
                    Expr::Const(63, 1),
                ),
            )),
            M::Addss | M::Addsd | M::Vaddss | M::Vaddsd => self.scalar(i, "+", st, out),
            M::Subss | M::Subsd | M::Vsubss | M::Vsubsd => self.scalar(i, "-", st, out),
            M::Mulss | M::Mulsd | M::Vmulss | M::Vmulsd => self.scalar(i, "*", st, out),
            M::Divss | M::Divsd | M::Vdivss | M::Vdivsd => self.scalar(i, "/", st, out),
            M::Cvtsi2ss | M::Vcvtsi2ss | M::Cvtsd2ss | M::Vcvtsd2ss => {
                let value = Expr::Cast("f32", Box::new(self.read(i, last, st)));
                self.write(i, 0, value, st, out);
            }
            M::Cvtsi2sd | M::Vcvtsi2sd | M::Cvtss2sd | M::Vcvtss2sd => {
                let value = Expr::Cast("f64", Box::new(self.read(i, last, st)));
                self.write(i, 0, value, st, out);
            }
            M::Cvttss2si | M::Cvttsd2si | M::Vcvttss2si | M::Vcvttsd2si => {
                let value = Expr::Cast(signed_type(w), Box::new(self.read(i, last, st)));
                self.write(i, 0, value, st, out);
            }
            M::Movsb
            | M::Movsw
            | M::Movsd
            | M::Movsq
            | M::Stosb
            | M::Stosw
            | M::Stosd
            | M::Stosq
                if i.has_rep_prefix() =>
            {
                let size = match m {
                    M::Movsb | M::Stosb => 1,
                    M::Movsw | M::Stosw => 2,
                    M::Movsd | M::Stosd => 4,
                    _ => 8,
                };
                let mut count = Expr::Var(Loc::Reg(RCX), 8);
                if size > 1 {
                    count = bin("*", count, Expr::Const(size, 8));
                }
                let destination = Expr::Var(Loc::Reg(RDI), 8);
                let call = if name.starts_with("Movs") {
                    Expr::Intrinsic(
                        "memcpy".into(),
                        vec![destination, Expr::Var(Loc::Reg(RSI), 8), count],
                    )
                } else {
                    Expr::Intrinsic(
                        "memset".into(),
                        vec![destination, Expr::Var(Loc::Reg(RAX), size as u8), count],
                    )
                };
                out.push(Stmt::Eval(call));
                out.push(Stmt::Clobber(vec![
                    Loc::Reg(RCX),
                    Loc::Reg(RDI),
                    Loc::Reg(RSI),
                ]));
            }
            _ if i.condition_code() != ConditionCode::None && name.starts_with("Set") => {
                let value = Expr::Condition(i.condition_code(), Box::new(Expr::Var(FLAGS, 1)));
                self.write(i, 0, value, st, out);
            }
            _ if i.condition_code() != ConditionCode::None && name.starts_with("Cmov") => {
                let condition = Expr::Condition(i.condition_code(), Box::new(Expr::Var(FLAGS, 1)));
                let value = Expr::Select(
                    Box::new(condition),
                    Box::new(self.read(i, 1, st)),
                    Box::new(self.read(i, 0, st)),
                );
                self.write(i, 0, value, st, out);
            }
            _ => self.generic(i, st, out),
        }
    }

    fn scalar(&self, i: &Instruction, op: &'static str, st: &mut State, out: &mut Vec<Stmt>) {
        let (a, b) = if i.op_count() == 3 {
            (self.read(i, 1, st), self.read(i, 2, st))
        } else {
            (self.read(i, 0, st), self.read(i, 1, st))
        };
        self.write(i, 0, bin(op, a, b), st, out);
    }

    fn generic(&mut self, i: &Instruction, st: &mut State, out: &mut Vec<Stmt>) {
        let access = if i.op_count() > 0 {
            self.factory.info(i).op_access(0)
        } else {
            OpAccess::None
        };
        let writes = matches!(
            access,
            OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite | OpAccess::ReadCondWrite
        );
        let reads_destination = matches!(access, OpAccess::ReadWrite | OpAccess::ReadCondWrite);
        let args = (0..i.op_count())
            .filter(|&op| op > 0 || !writes || reads_destination)
            .map(|op| self.read(i, op, st))
            .collect();
        let value = Expr::Intrinsic(format!("{:?}", i.mnemonic()).to_lowercase(), args);
        if writes && matches!(i.op0_kind(), OpKind::Register | OpKind::Memory) {
            self.write(i, 0, value, st, out);
        } else {
            out.push(Stmt::Eval(value));
        }
        if i.rflags_modified() != 0 {
            out.push(Stmt::Def(FLAGS, 1, Expr::Intrinsic("flags".into(), vec![])));
        }
    }

    fn block(&mut self, raw: &RawBlock, delta: i64, entry: bool) -> (Vec<Stmt>, RawExit, i64) {
        let mut st = State {
            delta,
            entry,
            written: HashSet::new(),
            since_call: Vec::new(),
            eax: None,
        };
        let mut out = Vec::new();
        if raw.external {
            return (
                out,
                RawExit::TailCall(Expr::Call(Box::new(self.code_name(raw.start)), Vec::new())),
                delta,
            );
        }
        let mut exit = None;
        for (index, i) in raw.instructions.iter().enumerate() {
            match i.flow_control() {
                FlowControl::Call | FlowControl::IndirectCall => {
                    let probe = raw.instructions.get(index + 1).is_some_and(|next| {
                        next.mnemonic() == Mnemonic::Sub
                            && next.op0_kind() == OpKind::Register
                            && next.op0_register() == Register::RSP
                            && next.op1_kind() == OpKind::Register
                    });
                    if !probe {
                        self.call(i, &mut st, &mut out);
                    }
                }
                FlowControl::UnconditionalBranch => {
                    let target = i.near_branch_target();
                    exit = Some(if target == 0 {
                        RawExit::Stop
                    } else if self.program.inside(self.entry, target) {
                        RawExit::Jump(target)
                    } else {
                        RawExit::TailCall(Expr::Call(
                            Box::new(self.code_name(target)),
                            arguments(&mut st),
                        ))
                    });
                }
                FlowControl::IndirectBranch if raw.switch.is_some() => {
                    let switch = raw.switch.as_ref().expect("checked");
                    let index = self.read_reg(switch.index, &st, i);
                    exit = Some(RawExit::Switch(index, switch.cases.clone()));
                }
                FlowControl::IndirectBranch => {
                    let target = self.read(i, 0, &st);
                    let mut args = arguments(&mut st);
                    if args.is_empty() && !matches!(target, Expr::Name(_)) {
                        args.push(Expr::Var(Loc::Reg(RCX), 8));
                    }
                    exit = Some(RawExit::TailCall(Expr::Call(Box::new(target), args)));
                }
                FlowControl::ConditionalBranch => {
                    let cc = i.condition_code();
                    let condition = if cc == ConditionCode::None {
                        Expr::Intrinsic(
                            format!("{:?}", i.mnemonic()).to_lowercase(),
                            vec![Expr::Var(Loc::Reg(RCX), 8)],
                        )
                    } else {
                        Expr::Condition(cc, Box::new(Expr::Var(FLAGS, 1)))
                    };
                    exit = Some(RawExit::Branch(
                        condition,
                        i.near_branch_target(),
                        i.next_ip(),
                    ));
                }
                FlowControl::Return => exit = Some(RawExit::Return),
                FlowControl::Interrupt | FlowControl::Exception => {
                    if i.mnemonic() == Mnemonic::Int && i.immediate(0) == 0x29 {
                        out.push(Stmt::Eval(Expr::Intrinsic(
                            "__fastfail".into(),
                            vec![Expr::Var(Loc::Reg(RCX), 4)],
                        )));
                    }
                    exit = Some(RawExit::Stop);
                }
                _ if raw
                    .switch
                    .as_ref()
                    .is_some_and(|s| s.lookup.contains(&i.ip())) => {}
                _ => self.instruction(i, &mut st, &mut out),
            }
        }
        let exit = exit.unwrap_or_else(|| {
            RawExit::Jump(
                raw.instructions
                    .last()
                    .map_or(raw.start, Instruction::next_ip),
            )
        });
        (out, exit, st.delta)
    }
}

fn note(st: &mut State, loc: Loc, w: u8) {
    if let Loc::Reg(r) = loc {
        st.written.insert(r);
    }
    let slot = match loc {
        Loc::Reg(RCX) => Some(0),
        Loc::Reg(RDX) => Some(1),
        Loc::Reg(R8) => Some(2),
        Loc::Reg(R9) => Some(3),
        Loc::Reg(r) if (VECTOR..VECTOR + 4).contains(&r) => Some((r - VECTOR) as usize),
        Loc::Stack(o) => {
            let relative = o - st.delta;
            ((0x20..0x20 + 8 * 16).contains(&relative) && relative % 8 == 0)
                .then(|| 4 + ((relative - 0x20) / 8) as usize)
        }
        _ => None,
    };
    if let Some(slot) = slot {
        st.since_call.retain(|(s, ..)| *s != slot);
        st.since_call.push((slot, loc, w));
    }
}

fn arguments(st: &mut State) -> Vec<Expr> {
    let has = |slot: usize| st.since_call.iter().any(|(s, ..)| *s == slot);
    let stack = (4..).take_while(|s| has(*s)).count();
    let count = if stack > 0 {
        4 + stack
    } else {
        st.since_call
            .iter()
            .map(|(s, ..)| s + 1)
            .filter(|s| *s <= 4)
            .max()
            .unwrap_or(0)
    };
    let args = (0..count)
        .map(
            |slot| match st.since_call.iter().find(|(s, ..)| *s == slot) {
                Some((_, loc, w)) => Expr::Var(*loc, *w),
                None if slot < 4 => Expr::Var(Loc::Reg([RCX, RDX, R8, R9][slot]), 8),
                None => Expr::Var(Loc::Stack(st.delta + 0x20 + 8 * (slot as i64 - 4)), 8),
            },
        )
        .collect();
    st.since_call.clear();
    args
}

pub fn lift(program: &dyn Program, entry: u64, raw: &[RawBlock]) -> Function {
    let index: HashMap<u64, usize> = raw.iter().enumerate().map(|(i, b)| (b.start, i)).collect();
    let entry_index = index[&entry];
    let mut lifter = Lifter::new(program, entry);
    let mut lifted: Vec<Option<(Vec<Stmt>, RawExit)>> = (0..raw.len()).map(|_| None).collect();
    let mut queue = VecDeque::from([(entry_index, 0i64)]);
    let mut seen = vec![false; raw.len()];
    seen[entry_index] = true;
    loop {
        while let Some((b, delta)) = queue.pop_front() {
            let (stmts, exit, out) = lifter.block(&raw[b], delta, b == entry_index);
            let targets: Vec<u64> = match &exit {
                RawExit::Switch(_, cases) => cases.clone(),
                RawExit::Jump(t) => vec![*t],
                RawExit::Branch(_, t, f) => vec![*t, *f],
                _ => vec![],
            };
            for target in targets {
                if let Some(&s) = index.get(&target)
                    && !seen[s]
                {
                    seen[s] = true;
                    queue.push_back((s, out));
                }
            }
            lifted[b] = Some((stmts, exit));
        }
        match seen.iter().position(|s| !s) {
            Some(b) => {
                seen[b] = true;
                queue.push_back((b, 0));
            }
            None => break,
        }
    }
    let mut blocks = Vec::with_capacity(raw.len());
    for (b, slot) in lifted.into_iter().enumerate() {
        let (stmts, exit) = slot.expect("every block lifted");
        let target = |t: u64| index.get(&t).copied();
        let exit = match exit {
            RawExit::Switch(e, cases) => {
                match cases.iter().map(|t| target(*t)).collect::<Option<Vec<_>>>() {
                    Some(cases) => Exit::Switch(e, cases, None),
                    None => Exit::Stop,
                }
            }
            RawExit::Jump(t) => match target(t) {
                Some(s) => Exit::Jump(s),
                None => Exit::TailCall(Expr::Call(Box::new(lifter.code_name(t)), Vec::new())),
            },
            RawExit::Branch(c, t, f) => match (target(t), target(f)) {
                (Some(t), Some(f)) => Exit::Branch(c, t, f),
                _ => Exit::Stop,
            },
            RawExit::Return => Exit::Return(None),
            RawExit::TailCall(e) => Exit::TailCall(e),
            RawExit::Stop => Exit::Stop,
        };
        blocks.push(Block {
            start: raw[b].start,
            stmts,
            exit,
        });
    }
    absorb_defaults(&mut blocks);
    let mut succ = vec![Vec::new(); blocks.len()];
    let mut pred = vec![Vec::new(); blocks.len()];
    for (b, block) in blocks.iter().enumerate() {
        for t in block.exit.targets() {
            succ[b].push(t);
            pred[t].push(b);
        }
    }
    let returns = return_register(&blocks, &pred);
    if let Some((loc, w)) = returns {
        for block in &mut blocks {
            if let Exit::Return(value) = &mut block.exit {
                *value = Some(Expr::Var(loc, w));
            }
        }
    }
    let aliased = aliased_slots(&blocks);
    Function {
        base: program.image_base(),
        entry: entry_index,
        blocks,
        succ,
        pred,
        returns,
        aliased,
    }
}

impl Exit {
    pub fn targets(&self) -> Vec<usize> {
        let mut targets = match self {
            Exit::Jump(t) => vec![*t],
            Exit::Branch(_, t, f) => vec![*t, *f],
            Exit::Switch(_, cases, default) => cases.iter().copied().chain(*default).collect(),
            _ => vec![],
        };
        let mut seen = HashSet::new();
        targets.retain(|t| seen.insert(*t));
        targets
    }
}

/// Folds the `ja default` guarding a jump table into the switch it guards.
fn absorb_defaults(blocks: &mut [Block]) {
    for s in 0..blocks.len() {
        if !matches!(blocks[s].exit, Exit::Switch(_, _, None)) || !blocks[s].stmts.is_empty() {
            continue;
        }
        let preds: Vec<usize> = (0..blocks.len())
            .filter(|p| blocks[*p].exit.targets().contains(&s))
            .collect();
        let [p] = preds.as_slice() else { continue };
        let Exit::Branch(Expr::Condition(ConditionCode::a, _), default, next) = blocks[*p].exit
        else {
            continue;
        };
        if next != s || default == s {
            continue;
        }
        let Exit::Switch(index, cases, None) = std::mem::replace(&mut blocks[s].exit, Exit::Stop)
        else {
            unreachable!()
        };
        blocks[*p].exit = Exit::Switch(index, cases, Some(default));
    }
}

const XMM0: Loc = Loc::Reg(VECTOR);

/// The last result register a block defines, and the width of that definition (0 for a clobber).
fn last_result(block: &Block) -> Option<(Loc, u8)> {
    block.stmts.iter().rev().find_map(|s| match s {
        Stmt::Def(loc @ Loc::Reg(RAX), w, _) => Some((*loc, *w)),
        Stmt::Def(loc @ Loc::Reg(VECTOR), w, _) => Some((*loc, *w)),
        Stmt::Clobber(locs) => [Loc::Reg(RAX), XMM0]
            .into_iter()
            .find(|l| locs.contains(l))
            .map(|l| (l, 0)),
        _ => None,
    })
}

fn return_register(blocks: &[Block], pred: &[Vec<usize>]) -> Option<(Loc, u8)> {
    let mut width: Option<(Loc, u8)> = None;
    for (b, block) in blocks.iter().enumerate() {
        if !matches!(block.exit, Exit::Return(_)) {
            continue;
        }
        let mut found: Vec<(Loc, u8)> = last_result(block).into_iter().collect();
        if found.is_empty() {
            for &p in &pred[b] {
                found.extend(last_result(&blocks[p]));
            }
        }
        for (loc, w) in found.into_iter().filter(|(_, w)| *w > 0) {
            width = Some(match width {
                Some((current, cw)) if current == loc => (loc, cw.min(w)),
                Some((current, cw)) if current == Loc::Reg(RAX) => (current, cw),
                _ => (loc, w),
            });
        }
    }
    width
}

/// Stack slots that may be reached through a pointer or overlap another access, and so behave like memory.
fn aliased_slots(blocks: &[Block]) -> HashSet<i64> {
    let mut taken: Option<i64> = None;
    let mut accesses: HashMap<i64, HashSet<u8>> = HashMap::new();
    let mut note = |e: &Expr| {
        e.visit(&mut |x| match x {
            Expr::AddrOf(inner) => {
                if let Expr::Var(Loc::Stack(o), _) = **inner {
                    taken = Some(taken.map_or(o, |t| t.min(o)));
                }
            }
            Expr::Var(Loc::Stack(o), w) => {
                accesses.entry(*o).or_default().insert(*w);
            }
            _ => {}
        })
    };
    for block in blocks {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Def(loc, w, e) => {
                    if let Loc::Stack(o) = loc {
                        note(&Expr::Var(Loc::Stack(*o), *w));
                    }
                    note(e);
                }
                Stmt::Store(a, _, _, v) => {
                    note(a);
                    note(v);
                }
                Stmt::Eval(e) => note(e),
                _ => {}
            }
        }
        match &block.exit {
            Exit::Branch(e, ..)
            | Exit::Switch(e, ..)
            | Exit::TailCall(e)
            | Exit::Return(Some(e)) => note(e),
            _ => {}
        }
    }
    let mut aliased = HashSet::new();
    let offsets: Vec<(i64, u8)> = accesses
        .iter()
        .map(|(o, widths)| (*o, widths.iter().copied().max().unwrap_or(8)))
        .collect();
    for &(o, w) in &offsets {
        let overlaps = offsets
            .iter()
            .any(|&(p, _)| p != o && p > o && p < o + i64::from(w));
        if overlaps || taken.is_some_and(|t| t <= o) {
            aliased.insert(o);
            for &(p, _) in &offsets {
                if p > o && p < o + i64::from(w) {
                    aliased.insert(p);
                }
            }
        }
    }
    aliased
}
