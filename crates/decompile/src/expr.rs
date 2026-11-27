use iced_x86::ConditionCode;

use crate::types::{Facts, stack_name, virtual_call};

pub const RAX: u8 = 0;
pub const RCX: u8 = 1;
pub const RDX: u8 = 2;
pub const RBX: u8 = 3;
pub const RSP: u8 = 4;
pub const RBP: u8 = 5;
pub const RSI: u8 = 6;
pub const RDI: u8 = 7;
pub const R8: u8 = 8;
pub const R9: u8 = 9;
pub const VECTOR: u8 = 16;
pub const TEMP: u8 = 48;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Loc {
    Reg(u8),
    /// A stack slot, by offset from the stack pointer at entry.
    Stack(i64),
    Flags,
    /// A variable that stands for one value a register held across a region.
    Var(u32),
}

impl Loc {
    pub fn is_vector(self) -> bool {
        matches!(self, Loc::Reg(r) if r >= VECTOR)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlagKind {
    /// `cmp a, b` or `sub a, b`.
    Compare,
    /// `test a, b`.
    Test,
    /// The result of `and`, `or` or `xor`, which clear CF and OF.
    Logic,
    /// The result of other arithmetic: only ZF and SF are modelled.
    Result,
    /// `add a, b`.
    Add,
    /// `comiss`/`ucomiss` and friends.
    Float,
    /// `bt a, b`.
    Bit,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Expr {
    Var(Loc, u8),
    /// The incoming value of a 1-based argument, read as this many bytes; true for an xmm argument.
    Param(u8, u8, bool),
    Const(u64, u8),
    Name(String),
    Text(String),
    AddrOf(Box<Expr>),
    Load(Box<Expr>, u8, bool),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Cast(&'static str, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Intrinsic(String, Vec<Expr>),
    Flags(FlagKind, Box<Expr>, Box<Expr>, u8),
    Condition(ConditionCode, Box<Expr>),
    Select(Box<Expr>, Box<Expr>, Box<Expr>),
}

pub fn bin(op: &'static str, a: Expr, b: Expr) -> Expr {
    Expr::Binary(op, Box::new(a), Box::new(b))
}

pub fn konst(value: u64, width: u8) -> Expr {
    Expr::Const(mask(value, width), width)
}

pub fn mask(value: u64, width: u8) -> u64 {
    if width >= 8 {
        value
    } else {
        value & ((1u64 << (width * 8)) - 1)
    }
}

pub fn signed_value(value: u64, width: u8) -> i64 {
    if width >= 8 {
        value as i64
    } else {
        let shift = 64 - u32::from(width) * 8;
        ((value << shift) as i64) >> shift
    }
}

impl Expr {
    pub fn visit(&self, f: &mut impl FnMut(&Expr)) {
        f(self);
        match self {
            Expr::AddrOf(a)
            | Expr::Load(a, ..)
            | Expr::Unary(_, a)
            | Expr::Cast(_, a)
            | Expr::Condition(_, a) => a.visit(f),
            Expr::Binary(_, a, b) | Expr::Flags(_, a, b, _) => {
                a.visit(f);
                b.visit(f);
            }
            Expr::Select(a, b, c) => {
                a.visit(f);
                b.visit(f);
                c.visit(f);
            }
            Expr::Call(target, args) => {
                target.visit(f);
                args.iter().for_each(|a| a.visit(f));
            }
            Expr::Intrinsic(_, args) => args.iter().for_each(|a| a.visit(f)),
            _ => {}
        }
    }

    pub fn map(self, f: &mut impl FnMut(Expr) -> Expr) -> Expr {
        let mapped = match self {
            Expr::AddrOf(a) => Expr::AddrOf(Box::new(a.map(f))),
            Expr::Load(a, w, float) => Expr::Load(Box::new(a.map(f)), w, float),
            Expr::Unary(op, a) => Expr::Unary(op, Box::new(a.map(f))),
            Expr::Cast(ty, a) => Expr::Cast(ty, Box::new(a.map(f))),
            Expr::Condition(cc, a) => Expr::Condition(cc, Box::new(a.map(f))),
            Expr::Binary(op, a, b) => Expr::Binary(op, Box::new(a.map(f)), Box::new(b.map(f))),
            Expr::Flags(kind, a, b, w) => {
                Expr::Flags(kind, Box::new(a.map(f)), Box::new(b.map(f)), w)
            }
            Expr::Select(a, b, c) => {
                Expr::Select(Box::new(a.map(f)), Box::new(b.map(f)), Box::new(c.map(f)))
            }
            Expr::Call(target, args) => Expr::Call(
                Box::new(target.map(f)),
                args.into_iter().map(|a| a.map(f)).collect(),
            ),
            Expr::Intrinsic(name, args) => {
                Expr::Intrinsic(name, args.into_iter().map(|a| a.map(f)).collect())
            }
            other => other,
        };
        f(mapped)
    }

    pub fn reads(&self) -> Vec<(Loc, u8)> {
        let mut found = Vec::new();
        self.visit(&mut |e| {
            if let Expr::Var(loc, w) = e {
                found.push((*loc, *w));
            }
        });
        found
    }

    pub fn has_load(&self) -> bool {
        let mut found = false;
        self.visit(&mut |e| found |= matches!(e, Expr::Load(..)));
        found
    }

    pub fn has_side_effects(&self) -> bool {
        let mut found = false;
        self.visit(&mut |e| {
            found |= match e {
                Expr::Call(..) => true,
                Expr::Intrinsic(name, _) => {
                    name.starts_with("_Interlocked") || name.starts_with("__fastfail")
                }
                _ => false,
            }
        });
        found
    }

    pub fn has_call(&self) -> bool {
        let mut found = false;
        self.visit(&mut |e| found |= matches!(e, Expr::Call(..)));
        found
    }
}

pub fn signed_type(width: u8) -> &'static str {
    match width {
        1 => "int8_t",
        2 => "int16_t",
        4 => "int32_t",
        _ => "int64_t",
    }
}

pub fn unsigned_type(width: u8) -> &'static str {
    match width {
        1 => "uint8_t",
        2 => "uint16_t",
        4 => "uint32_t",
        _ => "uint64_t",
    }
}

pub fn type_name(width: u8, float: bool) -> String {
    match (width, float) {
        (4, true) => "float".into(),
        (8, true) => "double".into(),
        (16, _) => "__m128".into(),
        (32, _) => "__m256".into(),
        (64, _) => "__m512".into(),
        (w, _) => unsigned_type(w).into(),
    }
}

fn signed(e: Expr, width: u8) -> Expr {
    match e {
        Expr::Const(..) => e,
        other => Expr::Cast(signed_type(width), Box::new(other)),
    }
}

/// The condition `cc` holds after flags were set as `kind` describes.
pub fn resolve(cc: ConditionCode, kind: FlagKind, a: Expr, b: Expr, width: u8) -> Expr {
    use ConditionCode as C;
    let zero = || Expr::Const(0, width);
    match kind {
        FlagKind::Compare => match cc {
            C::e => bin("==", a, b),
            C::ne => bin("!=", a, b),
            C::b => bin("<", a, b),
            C::ae => bin(">=", a, b),
            C::a => bin(">", a, b),
            C::be => bin("<=", a, b),
            C::l => bin("<", signed(a, width), signed(b, width)),
            C::ge => bin(">=", signed(a, width), signed(b, width)),
            C::g => bin(">", signed(a, width), signed(b, width)),
            C::le => bin("<=", signed(a, width), signed(b, width)),
            C::s => bin("<", signed(bin("-", a, b), width), zero()),
            C::ns => bin(">=", signed(bin("-", a, b), width), zero()),
            C::o => Expr::Intrinsic("overflows_sub".into(), vec![a, b]),
            C::no => negate(Expr::Intrinsic("overflows_sub".into(), vec![a, b])),
            other => Expr::Intrinsic(format!("{other:?}"), vec![a, b]),
        },
        FlagKind::Test | FlagKind::Logic => {
            let value = if kind == FlagKind::Logic || a == b {
                a
            } else {
                bin("&", a, b)
            };
            match cc {
                C::e | C::be => bin("==", value, zero()),
                C::ne | C::a => bin("!=", value, zero()),
                C::s | C::l => bin("<", signed(value, width), zero()),
                C::ns | C::ge => bin(">=", signed(value, width), zero()),
                C::le => bin("<=", signed(value, width), zero()),
                C::g => bin(">", signed(value, width), zero()),
                C::b | C::o => Expr::Const(0, 1),
                C::ae | C::no => Expr::Const(1, 1),
                other => Expr::Intrinsic(format!("parity_{other:?}"), vec![value]),
            }
        }
        FlagKind::Result => match cc {
            C::e => bin("==", a, zero()),
            C::ne => bin("!=", a, zero()),
            C::s => bin("<", signed(a, width), zero()),
            C::ns => bin(">=", signed(a, width), zero()),
            other => Expr::Intrinsic(format!("flag_{other:?}"), vec![a]),
        },
        FlagKind::Add => {
            let all_ones = matches!(&b, Expr::Const(c, _) if *c == mask(u64::MAX, width));
            let carry_limit = match &b {
                Expr::Const(c, _) if *c != 0 => Some(konst(c.wrapping_neg(), width)),
                _ => None,
            };
            if all_ones && matches!(cc, C::b | C::ae) {
                return bin(if cc == C::b { "!=" } else { "==" }, a, zero());
            }
            let sum = bin("+", a.clone(), b.clone());
            match (cc, carry_limit) {
                (C::e, _) => bin("==", sum, zero()),
                (C::ne, _) => bin("!=", sum, zero()),
                (C::s, _) => bin("<", signed(sum, width), zero()),
                (C::ns, _) => bin(">=", signed(sum, width), zero()),
                (C::b, Some(limit)) => bin(">=", a, limit),
                (C::ae, Some(limit)) => bin("<", a, limit),
                (other, _) => Expr::Intrinsic(format!("flag_{other:?}"), vec![a, b]),
            }
        }
        FlagKind::Float => match cc {
            C::e => bin("==", a, b),
            C::ne => bin("!=", a, b),
            C::b => bin("<", a, b),
            C::ae => bin(">=", a, b),
            C::a => bin(">", a, b),
            C::be => bin("<=", a, b),
            C::p => Expr::Intrinsic("unordered".into(), vec![a, b]),
            C::np => Expr::Intrinsic("ordered".into(), vec![a, b]),
            other => Expr::Intrinsic(format!("fcmp_{other:?}"), vec![a, b]),
        },
        FlagKind::Bit => {
            let bit = bin("&", bin(">>", a, b), Expr::Const(1, width));
            match cc {
                C::b => bin("!=", bit, zero()),
                C::ae => bin("==", bit, zero()),
                other => Expr::Intrinsic(format!("bt_{other:?}"), vec![bit]),
            }
        }
    }
}

pub fn negate(e: Expr) -> Expr {
    match e {
        Expr::Binary(op, a, b) => {
            let flipped = match op {
                "==" => "!=",
                "!=" => "==",
                "<" => ">=",
                ">=" => "<",
                ">" => "<=",
                "<=" => ">",
                "&&" => return bin("||", negate(*a), negate(*b)),
                "||" => return bin("&&", negate(*a), negate(*b)),
                _ => return Expr::Unary("!", Box::new(Expr::Binary(op, a, b))),
            };
            Expr::Binary(flipped, a, b)
        }
        Expr::Unary("!", a) => *a,
        Expr::Const(v, w) => Expr::Const(u64::from(v == 0), w),
        other => Expr::Unary("!", Box::new(other)),
    }
}

/// Folds the patterns lifting leaves behind into simpler forms.
pub fn simplify(e: Expr) -> Expr {
    e.map(&mut |e| match e {
        Expr::Condition(cc, flags) => match *flags {
            Expr::Flags(kind, a, b, w) => resolve(cc, kind, *a, *b, w),
            other => Expr::Condition(cc, Box::new(other)),
        },
        Expr::Load(addr, w, float) => match *addr {
            Expr::AddrOf(inner) if matches!(*inner, Expr::Var(Loc::Stack(_), _)) => match *inner {
                Expr::Var(loc, _) => Expr::Var(loc, w),
                _ => unreachable!(),
            },
            other => Expr::Load(Box::new(other), w, float),
        },
        Expr::AddrOf(inner) => match *inner {
            Expr::Load(addr, ..) => *addr,
            other => Expr::AddrOf(Box::new(other)),
        },
        Expr::Binary(op @ ("+" | "-"), a, b) => match (*a, *b) {
            (a, Expr::Const(0, _)) => a,
            (Expr::Const(x, w), Expr::Const(y, _)) => {
                konst(if op == "+" { x.wrapping_add(y) } else { x.wrapping_sub(y) }, w)
            }
            (Expr::AddrOf(inner), Expr::Const(c, w)) if matches!(*inner, Expr::Var(Loc::Stack(_), _)) => {
                let Expr::Var(Loc::Stack(o), vw) = *inner else { unreachable!() };
                let c = signed_value(c, w);
                Expr::AddrOf(Box::new(Expr::Var(Loc::Stack(if op == "+" { o + c } else { o - c }), vw)))
            }
            (Expr::Binary("+", x, y), Expr::Const(c, _))
                if op == "+" && matches!(*x, Expr::AddrOf(ref v) if matches!(**v, Expr::Var(Loc::Stack(_), _))) =>
            {
                let Expr::AddrOf(v) = *x else { unreachable!() };
                let Expr::Var(Loc::Stack(o), vw) = *v else { unreachable!() };
                let moved = Expr::AddrOf(Box::new(Expr::Var(Loc::Stack(o + c as i64), vw)));
                bin("+", moved, *y)
            }
            (Expr::Binary(inner @ ("+" | "-"), x, y), z) if inner != op && *y == z => *x,
            (Expr::Binary(inner @ ("+" | "-"), x, y), Expr::Const(c, w)) if matches!(*y, Expr::Const(..)) => {
                let Expr::Const(d, _) = *y else { unreachable!() };
                let d = if inner == "+" { d } else { d.wrapping_neg() };
                let c = if op == "+" { c } else { c.wrapping_neg() };
                let total = d.wrapping_add(c);
                if total == 0 { *x } else { bin("+", *x, konst(total, w)) }
            }
            (a, Expr::Const(c, w)) if (-0x10000..0).contains(&signed_value(c, w)) => {
                bin(if op == "+" { "-" } else { "+" }, a, konst(c.wrapping_neg(), w))
            }
            (a, b) => Expr::Binary(op, Box::new(a), Box::new(b)),
        },
        Expr::Binary(op @ ("&" | "|"), a, b) if is_bool(&a) && is_bool(&b) && !b.has_side_effects() => {
            Expr::Binary(if op == "&" { "&&" } else { "||" }, a, b)
        }
        Expr::Binary(op, a, b) if matches!(op, "==" | "!=") && is_bool(&a) && matches!(*b, Expr::Const(0, _)) => {
            if op == "!=" { *a } else { negate(*a) }
        }
        Expr::Binary(op, a, b) if matches!(op, "==" | "!=") => match (*a, *b) {
            (Expr::Binary("-", x, y), Expr::Const(0, _)) => Expr::Binary(op, x, y),
            (Expr::Binary("^", x, y), Expr::Const(0, _)) => Expr::Binary(op, x, y),
            (Expr::Binary(logic @ ("|" | "&"), x, y), Expr::Const(0, _)) if is_bool(&x) && is_bool(&y) => {
                let joined = bin(if logic == "|" { "||" } else { "&&" }, *x, *y);
                if op == "!=" { joined } else { negate(joined) }
            }
            (Expr::Binary(arith @ ("+" | "-"), x, y), Expr::Const(c, w)) if matches!(*y, Expr::Const(..)) => {
                let Expr::Const(d, _) = *y else { unreachable!() };
                let moved = if arith == "+" { c.wrapping_sub(d) } else { c.wrapping_add(d) };
                Expr::Binary(op, x, Box::new(konst(moved, w)))
            }
            (Expr::Binary("+", x, y), z) if *y == z => Expr::Binary(op, x, Box::new(Expr::Const(0, 8))),
            (a, b) => Expr::Binary(op, Box::new(a), Box::new(b)),
        },
        Expr::Unary("!", a) => negate(*a),
        Expr::Binary(">>", product, shift) => match (*product, *shift) {
            (Expr::Binary("*", x, m), Expr::Const(s, _))
                if is_u32(&x) && matches!(*m, Expr::Const(..)) && (32..64).contains(&s) =>
            {
                let Expr::Const(m, _) = *m else { unreachable!() };
                match magic_divisor(m, s as u32) {
                    Some(n) => bin("/", *x, Expr::Const(n, 4)),
                    None => bin(">>", bin("*", *x, Expr::Const(m, 8)), Expr::Const(s, 1)),
                }
            }
            (a, b) => bin(">>", a, b),
        },
        other => other,
    })
}

fn is_u32(e: &Expr) -> bool {
    match e {
        Expr::Param(_, 4, false) | Expr::Load(_, 4, false) => true,
        Expr::Var(loc, 4) => !loc.is_vector(),
        Expr::Cast("uint32_t", _) => true,
        _ => false,
    }
}

/// The `n` for which `x * m >> s` equals `x / n` for every 32-bit `x`.
fn magic_divisor(m: u64, s: u32) -> Option<u64> {
    if m == 0 || m >= 1 << 33 {
        return None;
    }
    let power = 1u128 << s;
    let floor = (power / u128::from(m)) as u64;
    [floor, floor + 1].into_iter().find(|&n| {
        n > 1 && {
            let n128 = u128::from(n);
            let ceil = power.div_ceil(n128);
            ceil == u128::from(m) && ceil * n128 - power <= 1u128 << (s - 32)
        }
    })
}

pub fn is_bool(e: &Expr) -> bool {
    match e {
        Expr::Binary(op, ..) => matches!(*op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||"),
        Expr::Unary("!", _) | Expr::Condition(..) => true,
        _ => false,
    }
}

fn precedence(e: &Expr) -> u8 {
    match e {
        Expr::AddrOf(_) | Expr::Unary(..) | Expr::Cast(..) => 2,
        Expr::Load(addr, ..) if !matches!(**addr, Expr::AddrOf(_)) => 2,
        Expr::Binary(op, ..) => match *op {
            "*" | "/" | "%" => 3,
            "+" | "-" => 4,
            "<<" | ">>" => 5,
            "<" | "<=" | ">" | ">=" => 6,
            "==" | "!=" => 7,
            "&" => 8,
            "^" => 9,
            "|" => 10,
            "&&" => 11,
            _ => 12,
        },
        Expr::Select(..) => 13,
        _ => 0,
    }
}

pub struct Printer<'a> {
    pub facts: &'a Facts,
}

impl Printer<'_> {
    pub fn expr(&self, e: &Expr) -> String {
        let mut out = String::new();
        self.write(e, &mut out, 14);
        out
    }

    /// Field accesses bind like postfix operators.
    fn prec(&self, e: &Expr) -> u8 {
        match e {
            Expr::Load(addr, ..)
                if !matches!(**addr, Expr::AddrOf(_)) && self.facts.field(addr).is_some() =>
            {
                1
            }
            _ => precedence(e),
        }
    }

    fn write(&self, e: &Expr, out: &mut String, outer: u8) {
        let p = self.prec(e);
        if p > outer {
            out.push('(');
            self.write(e, out, 14);
            out.push(')');
            return;
        }
        match e {
            Expr::Var(Loc::Reg(r), w) => out.push_str(&register_name(*r, *w)),
            Expr::Var(Loc::Stack(o), _) => out.push_str(&stack_name(*o)),
            Expr::Var(Loc::Flags, _) => out.push_str("flags"),
            Expr::Var(Loc::Var(n), w) => {
                let width = self.facts.var_width(*n);
                if *w < width && width <= 8 {
                    out.push_str(&format!(
                        "({}){}",
                        unsigned_type(*w),
                        self.facts.var_name(*n)
                    ));
                } else {
                    out.push_str(&self.facts.var_name(*n));
                }
            }
            Expr::Param(n, w, vector) => {
                let name = self.facts.param_name(*n);
                let width = self.facts.param_width(*n);
                if *w < width && !*vector {
                    out.push_str(&format!("({}){name}", unsigned_type(*w)));
                } else {
                    out.push_str(&name);
                }
            }
            Expr::Const(v, w) => out.push_str(&constant(*v, *w)),
            Expr::Name(name) => out.push_str(name),
            Expr::Text(text) => out.push_str(&format!("{text:?}")),
            Expr::AddrOf(a) => {
                out.push('&');
                self.write(a, out, 2);
            }
            Expr::Load(addr, w, float) => match &**addr {
                Expr::AddrOf(inner) => self.write(inner, out, outer),
                other => match self.facts.field(other) {
                    Some((s, base, offset)) => {
                        self.write(base, out, 1);
                        out.push_str("->");
                        out.push_str(&Facts::field_name(s, offset));
                    }
                    None => {
                        out.push_str(&format!("*({} *)", type_name(*w, *float)));
                        self.write(other, out, 2);
                    }
                },
            },
            Expr::Unary(op, a) => {
                out.push_str(op);
                self.write(a, out, 2);
            }
            Expr::Cast(ty, a) => {
                out.push_str(&format!("({ty})"));
                self.write(a, out, 2);
            }
            Expr::Binary(op, a, b) => {
                let mixed = |child: &Expr| {
                    let c = precedence(child);
                    matches!(child, Expr::Binary(..))
                        && ((8..=10).contains(&p) && (3..=5).contains(&c)
                            || p == 5 && (3..=4).contains(&c))
                };
                let left = if mixed(a) { 1 } else { p };
                let right = if mixed(b) { 1 } else { p - 1 };
                self.write(a, out, left);
                let negative = match (&**b, *op) {
                    (Expr::Const(v, w), "+")
                        if signed_value(*v, *w) < 0 && signed_value(*v, *w) > -0x10000 =>
                    {
                        Some(-signed_value(*v, *w))
                    }
                    _ => None,
                };
                match negative {
                    Some(n) => out.push_str(&format!(" - {}", constant(n as u64, 8))),
                    None => {
                        out.push_str(&format!(" {op} "));
                        self.write(b, out, right);
                    }
                }
            }
            Expr::Call(target, args) => {
                if let Some((receiver, slot)) = virtual_call(target, args) {
                    self.write(receiver, out, 1);
                    out.push_str(&format!("->vf{slot}"));
                    self.arguments(&args[1..], out);
                    return;
                }
                match &**target {
                    Expr::Name(_) | Expr::Var(..) | Expr::Param(..) => self.write(target, out, 0),
                    other => {
                        out.push('(');
                        self.write(other, out, 14);
                        out.push(')');
                    }
                }
                self.arguments(args, out);
            }
            Expr::Intrinsic(name, args) => {
                out.push_str(name);
                self.arguments(args, out);
            }
            Expr::Flags(kind, a, b, _) => {
                out.push_str(&format!("{kind:?}").to_lowercase());
                self.arguments(&[(**a).clone(), (**b).clone()], out);
            }
            Expr::Condition(cc, flags) => {
                out.push_str(&format!("cond_{cc:?}("));
                self.write(flags, out, 14);
                out.push(')');
            }
            Expr::Select(c, a, b) => {
                self.write(c, out, 12);
                out.push_str(" ? ");
                self.write(a, out, 12);
                out.push_str(" : ");
                self.write(b, out, 13);
            }
        }
    }

    fn arguments(&self, args: &[Expr], out: &mut String) {
        out.push('(');
        for (index, arg) in args.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            self.write(arg, out, 14);
        }
        out.push(')');
    }
}

fn constant(value: u64, width: u8) -> String {
    let s = signed_value(value, width);
    if (-0x10000..0).contains(&s) {
        format!("-0x{:x}", -s)
    } else if value < 10 {
        value.to_string()
    } else {
        format!("0x{value:x}")
    }
}

pub fn register_name(r: u8, width: u8) -> String {
    if r >= TEMP {
        return format!("tmp{}", r - TEMP);
    }
    if r >= VECTOR {
        let n = r - VECTOR;
        return match width {
            32 => format!("ymm{n}"),
            64 => format!("zmm{n}"),
            _ => format!("xmm{n}"),
        };
    }
    const LEGACY: [&str; 8] = ["ax", "cx", "dx", "bx", "sp", "bp", "si", "di"];
    if r < 8 {
        let base = LEGACY[r as usize];
        match width {
            8 => format!("r{base}"),
            4 => format!("e{base}"),
            2 => base.to_string(),
            _ if r < 4 => format!("{}l", &base[..1]),
            _ => format!("{base}l"),
        }
    } else {
        match width {
            8 => format!("r{r}"),
            4 => format!("r{r}d"),
            2 => format!("r{r}w"),
            _ => format!("r{r}b"),
        }
    }
}
