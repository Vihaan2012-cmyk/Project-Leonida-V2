//! Guesses a C type for each variable, argument and stack slot from how it is defined and used.

use std::collections::{BTreeMap, HashSet};

use crate::expr::{Expr, Loc, type_name, unsigned_type};
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
}

pub struct Facts {
    pub vars: Vec<Typed>,
    /// Indexed by argument number minus one, stack arguments included.
    pub params: Vec<Typed>,
    /// Stack slots below the return address: (offset, declaration with name placeholder `{}`).
    pub locals: Vec<(i64, String)>,
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

fn pointer_type() -> Typed {
    Typed {
        decl: "void *".into(),
        width: 8,
    }
}

fn integer_type(width: u8) -> Typed {
    Typed {
        decl: unsigned_type(width.clamp(1, 8)).into(),
        width: width.clamp(1, 8),
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
        }
    }

    fn slot(&mut self, o: i64, w: u8) {
        let entry = self.slots.entry(o).or_insert(0);
        *entry = (*entry).max(w);
    }

    fn address(&mut self, addr: &Expr) {
        if let Some(s) = base_subject(addr) {
            self.deref.insert(s);
        }
    }

    fn expr(&mut self, e: &Expr) {
        e.visit(&mut |x| match x {
            Expr::Load(addr, ..) => self.address(addr),
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
                Stmt::Store(addr, ..) => scan.address(addr),
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
        ..
    } = scan;

    let var_types: Vec<Typed> = vars
        .iter()
        .enumerate()
        .map(|(v, info)| {
            if info.vector {
                vector_type(info.width, info.scalar_only)
            } else if deref.contains(&Subject::Var(v as u32)) {
                pointer_type()
            } else {
                integer_type(info.width)
            }
        })
        .collect();

    let mut param_types = Vec::new();
    for n in 1..=params {
        let i = usize::from(n);
        let slot_width = slots
            .get(&(0x28 + 8 * (i64::from(n) - 5)))
            .copied()
            .filter(|_| n >= 5);
        param_types.push(if n >= 5 {
            integer_type(slot_width.unwrap_or(8))
        } else if param_vector[i] != 0 && param_width[i] == 0 {
            vector_type(param_vector[i], true)
        } else if deref.contains(&Subject::Param(n)) {
            pointer_type()
        } else {
            integer_type(if param_width[i] == 0 {
                8
            } else {
                param_width[i]
            })
        });
    }

    let mut locals: Vec<(i64, String)> = Vec::new();
    let frame: Vec<i64> = slots
        .keys()
        .copied()
        .filter(|o| *o < 0x28 && stack_param(*o).is_none())
        .chain(addressed.iter().copied().filter(|o| *o < 0x28))
        .collect::<std::collections::BTreeSet<_>>()
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
    }
}
