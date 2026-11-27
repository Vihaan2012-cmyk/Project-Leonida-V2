//! Guesses a C type for each variable, argument and stack slot from how it is defined and used, and
//! recovers structures from pointers read at several constant offsets.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::expr::{Expr, Loc, signed_value, type_name, unsigned_type};
use crate::lift::{Function, Stmt};
use crate::vars::VarInfo;

/// A value that other expressions dereference.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Subject {
    Var(u32),
    Param(u8),
}

pub struct Typed {
    /// The declared type, e.g. `uint32_t` or `void *`.
    pub decl: String,
    /// The width in bytes of an integer, or 8 for pointers.
    pub width: u8,
    pub name: String,
}

pub struct Field {
    pub decl: String,
}

/// A structure whose fields are the constant offsets one pointer is accessed at.
pub struct Struct {
    pub name: String,
    pub vtable: bool,
    pub fields: BTreeMap<i64, Field>,
}

pub struct Facts {
    pub vars: Vec<Typed>,
    /// Indexed by argument number minus one, stack arguments included.
    pub params: Vec<Typed>,
    /// Stack slots below the return address: (offset, declaration with name placeholder `{}`).
    pub locals: Vec<(i64, String)>,
    /// By the key of the pointer expression, see `root_key`.
    pub structs: HashMap<String, Struct>,
}

impl Facts {
    pub fn var_name(&self, n: u32) -> String {
        self.vars
            .get(n as usize)
            .map_or_else(|| format!("v{}", n + 1), |v| v.name.clone())
    }

    pub fn var_width(&self, n: u32) -> u8 {
        self.vars.get(n as usize).map_or(8, |v| v.width)
    }

    pub fn param_name(&self, n: u8) -> String {
        self.params
            .get(usize::from(n).wrapping_sub(1))
            .map_or_else(|| format!("a{n}"), |p| p.name.clone())
    }

    pub fn param_width(&self, n: u8) -> u8 {
        self.params
            .get(usize::from(n).wrapping_sub(1))
            .map_or(8, |p| p.width)
    }

    /// The structure and field offset `addr` reads, when `addr` is a structure pointer plus a constant.
    pub fn field<'a>(&self, addr: &'a Expr) -> Option<(&Struct, &'a Expr, i64)> {
        let (base, offset) = base_offset(addr);
        if offset < 0 {
            return None;
        }
        let s = self.structs.get(&root_key(base)?)?;
        Some((s, base, offset))
    }

    pub fn field_name(s: &Struct, offset: i64) -> String {
        if offset == 0 && s.vtable {
            "vtable".into()
        } else {
            format!("f_{offset:x}")
        }
    }

    /// Structure definitions in the order they should be written.
    pub fn definitions(&self) -> Vec<String> {
        let mut all: Vec<&Struct> = self.structs.values().collect();
        all.sort_by(|a, b| {
            let number = |s: &Struct| s.name[1..].parse::<u32>().unwrap_or(0);
            number(a).cmp(&number(b))
        });
        all.into_iter()
            .map(|s| {
                let mut text = format!("struct {} {{\n", s.name);
                for (offset, field) in &s.fields {
                    let name = Self::field_name(s, *offset);
                    let space = if field.decl.ends_with('*') { "" } else { " " };
                    text.push_str(&format!("    {}{space}{name};\n", field.decl));
                }
                text.push_str("};\n");
                text
            })
            .collect()
    }
}

/// The name a stack slot gets: locals count down from the return address, arguments up.
pub fn stack_name(o: i64) -> String {
    if o < 0 {
        format!("v_{:x}", -o)
    } else if let Some(n) = stack_param(o) {
        format!("a{n}")
    } else {
        format!("home_{o:x}")
    }
}

/// The register or stack argument number a stack slot above the shadow space holds.
pub fn stack_param(o: i64) -> Option<u8> {
    (o >= 0x28 && o % 8 == 0 && o < 0x28 + 8 * 60).then(|| (5 + (o - 0x28) / 8) as u8)
}

/// The value an address expression is computed from.
pub fn base_subject(addr: &Expr) -> Option<Subject> {
    match addr {
        Expr::Var(Loc::Var(v), 8) => Some(Subject::Var(*v)),
        Expr::Param(n, 8, false) => Some(Subject::Param(*n)),
        Expr::Binary("+" | "-", a, b) => base_subject(a).or_else(|| base_subject(b)),
        _ => None,
    }
}

/// Splits `base + constant`.
pub fn base_offset(addr: &Expr) -> (&Expr, i64) {
    match addr {
        Expr::Binary(op @ ("+" | "-"), a, b) => match &**b {
            Expr::Const(c, w) => {
                let c = signed_value(*c, *w);
                (a, if *op == "+" { c } else { c.wrapping_neg() })
            }
            _ => (addr, 0),
        },
        _ => (addr, 0),
    }
}

/// A name for a pointer that is stable across the function: an argument, a variable, or a pointer
/// stored in a field of one of those.
pub fn root_key(e: &Expr) -> Option<String> {
    match e {
        Expr::Var(Loc::Var(v), 8) => Some(format!("v{v}")),
        Expr::Param(n, 8, false) => Some(format!("a{n}")),
        Expr::Load(addr, 8, false) => {
            let (base, offset) = base_offset(addr);
            (0..0x10_0000).contains(&offset).then_some(())?;
            Some(format!("{}.{offset:x}", root_key(base)?))
        }
        _ => None,
    }
}

/// `(*(recv->vtable + 8 * slot))(recv, ...)`: the receiver and the slot.
pub fn virtual_call<'a>(target: &'a Expr, args: &'a [Expr]) -> Option<(&'a Expr, i64)> {
    let Expr::Load(slot_address, 8, false) = target else {
        return None;
    };
    let (table, offset) = base_offset(slot_address);
    let Expr::Load(table_address, 8, false) = table else {
        return None;
    };
    let (receiver, zero) = base_offset(table_address);
    (zero == 0 && offset >= 0 && offset % 8 == 0 && args.first() == Some(receiver))
        .then_some((receiver, offset / 8))
}

fn pointer_type() -> Typed {
    Typed {
        decl: "void *".into(),
        width: 8,
        name: String::new(),
    }
}

fn integer_type(width: u8) -> Typed {
    Typed {
        decl: unsigned_type(width.clamp(1, 8)).into(),
        width: width.clamp(1, 8),
        name: String::new(),
    }
}

fn vector_type(width: u8, scalar_only: bool) -> Typed {
    Typed {
        decl: if scalar_only && matches!(width, 4 | 8) {
            type_name(width, true)
        } else {
            type_name(width.clamp(16, 64), false)
        },
        width,
        name: String::new(),
    }
}

fn field_type(width: u8, float: bool) -> String {
    if width >= 16 || (float && matches!(width, 4 | 8)) {
        type_name(width.min(64), float)
    } else {
        unsigned_type(width.clamp(1, 8)).into()
    }
}

struct Scan {
    deref: HashSet<Subject>,
    param_width: [u8; 64],
    param_vector: [u8; 64],
    slots: BTreeMap<i64, u8>,
    addressed: HashSet<i64>,
    /// Slots whose address is being taken: the pre-order walk then meets the slot itself, which is no access.
    taking: Vec<i64>,
    accesses: HashMap<String, BTreeMap<i64, (u8, bool)>>,
    receivers: HashSet<String>,
    /// Field keys whose value a variable holds, by the variable's key.
    loaded_into: HashMap<String, String>,
}

impl Scan {
    fn new() -> Self {
        Self {
            deref: HashSet::new(),
            param_width: [0; 64],
            param_vector: [0; 64],
            slots: BTreeMap::new(),
            addressed: HashSet::new(),
            taking: Vec::new(),
            accesses: HashMap::new(),
            receivers: HashSet::new(),
            loaded_into: HashMap::new(),
        }
    }

    fn slot(&mut self, o: i64, w: u8) {
        let entry = self.slots.entry(o).or_insert(0);
        *entry = (*entry).max(w);
    }

    fn address(&mut self, addr: &Expr, width: u8, float: bool) {
        if let Some(s) = base_subject(addr) {
            self.deref.insert(s);
        }
        let (base, offset) = base_offset(addr);
        if (0..0x10_0000).contains(&offset)
            && let Some(key) = root_key(base)
        {
            let field = self
                .accesses
                .entry(key)
                .or_default()
                .entry(offset)
                .or_insert((width, float));
            if width > field.0 {
                *field = (width, float);
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        e.visit(&mut |x| match x {
            Expr::Load(addr, w, float) => self.address(addr, *w, *float),
            Expr::Call(target, args) => {
                if let Some((receiver, _)) = virtual_call(target, args)
                    && let Some(key) = root_key(receiver)
                {
                    self.receivers.insert(key);
                }
            }
            Expr::Param(n, w, vector) if usize::from(*n) < 64 => {
                let table = if *vector {
                    &mut self.param_vector
                } else {
                    &mut self.param_width
                };
                table[usize::from(*n)] = table[usize::from(*n)].max(*w);
            }
            Expr::Var(Loc::Stack(o), w) => match self.taking.iter().position(|t| t == o) {
                Some(k) => {
                    self.taking.swap_remove(k);
                }
                None => self.slot(*o, *w),
            },
            Expr::AddrOf(inner) => {
                if let Expr::Var(Loc::Stack(o), _) = **inner {
                    self.addressed.insert(o);
                    self.taking.push(o);
                }
            }
            _ => {}
        })
    }
}

pub fn infer(f: &Function, vars: &[VarInfo], params: u8) -> Facts {
    let mut scan = Scan::new();
    for block in &f.blocks {
        for stmt in &block.stmts {
            for e in stmt.exprs() {
                scan.expr(e);
            }
            match stmt {
                Stmt::Def(Loc::Stack(o), w, _) => scan.slot(*o, *w),
                Stmt::Def(Loc::Var(v), 8, value @ Expr::Load(_, 8, false)) => {
                    if let Some(key) = root_key(value) {
                        scan.loaded_into.entry(key).or_insert(format!("v{v}"));
                    }
                }
                Stmt::Store(addr, w, float, _) => scan.address(addr, *w, *float),
                _ => {}
            }
        }
        if let Some(e) = block.exit.expr() {
            scan.expr(e);
        }
    }
    let Scan {
        deref,
        param_width,
        param_vector,
        slots,
        addressed,
        accesses,
        receivers,
        loaded_into,
        ..
    } = scan;

    let mut keys: BTreeSet<String> = accesses
        .iter()
        .filter(|(_, fields)| fields.len() >= 2)
        .map(|(k, _)| k.clone())
        .collect();
    keys.extend(receivers.iter().cloned());
    let mut structs: HashMap<String, Struct> = HashMap::new();
    for (k, key) in keys.iter().enumerate() {
        structs.insert(
            key.clone(),
            Struct {
                name: format!("S{}", k + 1),
                vtable: receivers.contains(key),
                fields: BTreeMap::new(),
            },
        );
    }
    let names: HashMap<String, String> = structs
        .iter()
        .map(|(k, s)| (k.clone(), s.name.clone()))
        .collect();
    for (key, s) in &mut structs {
        let seen = accesses.get(key);
        if s.vtable {
            s.fields.insert(
                0,
                Field {
                    decl: "void **".into(),
                },
            );
        }
        for (&offset, &(width, float)) in seen.into_iter().flatten() {
            if offset == 0 && s.vtable {
                continue;
            }
            let child = format!("{key}.{offset:x}");
            let held = loaded_into.get(&child);
            let decl = match names
                .get(&child)
                .or_else(|| held.and_then(|v| names.get(v)))
            {
                Some(name) => format!("struct {name} *"),
                None if accesses.contains_key(&child)
                    || held.is_some_and(|v| {
                        v.strip_prefix('v')
                            .and_then(|n| n.parse().ok())
                            .is_some_and(|n| deref.contains(&Subject::Var(n)))
                    }) =>
                {
                    "void *".into()
                }
                None => field_type(width, float),
            };
            s.fields.insert(offset, Field { decl });
        }
    }
    let pointer_to = |key: &str| -> Typed {
        match names.get(key) {
            Some(name) => Typed {
                decl: format!("struct {name} *"),
                width: 8,
                name: String::new(),
            },
            None => pointer_type(),
        }
    };

    let var_types: Vec<Typed> = vars
        .iter()
        .enumerate()
        .map(|(v, info)| {
            let mut ty = if info.vector {
                vector_type(info.width, info.scalar_only)
            } else if names.contains_key(&format!("v{v}")) {
                pointer_to(&format!("v{v}"))
            } else if deref.contains(&Subject::Var(v as u32)) {
                pointer_type()
            } else {
                integer_type(info.width)
            };
            ty.name = if info.param != 0 {
                format!("a{}", info.param)
            } else {
                format!("v{}", v + 1)
            };
            ty
        })
        .collect();

    let mut param_types = Vec::new();
    for n in 1..=params {
        let i = usize::from(n);
        let slot_width = slots
            .get(&(0x28 + 8 * (i64::from(n) - 5)))
            .copied()
            .filter(|_| n >= 5);
        let mut ty = if n >= 5 {
            integer_type(slot_width.unwrap_or(8))
        } else if param_vector[i] != 0 && param_width[i] == 0 {
            vector_type(param_vector[i], true)
        } else if names.contains_key(&format!("a{n}")) {
            pointer_to(&format!("a{n}"))
        } else if deref.contains(&Subject::Param(n)) {
            pointer_type()
        } else {
            integer_type(if param_width[i] == 0 {
                8
            } else {
                param_width[i]
            })
        };
        ty.name = if n == 1 && receivers.contains("a1") {
            "this".into()
        } else {
            format!("a{n}")
        };
        param_types.push(ty);
    }

    let mut locals: Vec<(i64, String)> = Vec::new();
    let frame: Vec<i64> = slots
        .keys()
        .copied()
        .filter(|o| *o < 0x28 && stack_param(*o).is_none())
        .chain(addressed.iter().copied().filter(|o| *o < 0x28))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for (k, &o) in frame.iter().enumerate() {
        let width = slots.get(&o).copied().unwrap_or(0);
        let next = frame
            .get(k + 1)
            .copied()
            .unwrap_or(if o < 0 { 0 } else { 0x28 });
        let span = (next - o).clamp(1, 0x1000);
        let decl = if addressed.contains(&o) && (width == 0 || span > i64::from(width)) {
            format!("uint8_t {{}}[0x{span:x}]")
        } else if width >= 16 {
            format!("{} {{}}", type_name(width.min(64), false))
        } else {
            format!("{} {{}}", unsigned_type(width.clamp(1, 8)))
        };
        locals.push((o, decl));
    }
    Facts {
        vars: var_types,
        params: param_types,
        locals,
        structs,
    }
}
