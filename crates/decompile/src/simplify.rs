use std::collections::HashSet;

use crate::expr::*;
use crate::lift::{Block, Exit, Function, Stmt, volatile};

pub fn partial(loc: Loc, w: u8) -> bool {
    match loc {
        Loc::Reg(r) if r < VECTOR => w < 4,
        Loc::Reg(_) => false,
        Loc::Stack(_) => w < 4,
        Loc::Flags | Loc::Var(_) => false,
    }
}

impl Stmt {
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Stmt::Def(_, _, e) | Stmt::Eval(e) => vec![e],
            Stmt::Store(a, _, _, v) => vec![a, v],
            _ => vec![],
        }
    }

    pub fn map_exprs(&mut self, f: &mut impl FnMut(Expr) -> Expr) {
        let take = |e: &mut Expr, f: &mut dyn FnMut(Expr) -> Expr| {
            let old = std::mem::replace(e, Expr::Const(0, 1));
            *e = f(old);
        };
        match self {
            Stmt::Def(_, _, e) | Stmt::Eval(e) => take(e, f),
            Stmt::Store(a, _, _, v) => {
                take(a, f);
                take(v, f);
            }
            _ => {}
        }
    }

    fn reads(&self) -> Vec<(Loc, u8)> {
        let mut reads: Vec<(Loc, u8)> = self.exprs().iter().flat_map(|e| e.reads()).collect();
        if let Stmt::Def(loc, w, _) = self
            && partial(*loc, *w)
        {
            reads.push((*loc, *w));
        }
        reads
    }

    fn calls(&self) -> bool {
        self.exprs().iter().any(|e| e.has_call())
    }

    pub fn writes(&self) -> Vec<Loc> {
        let mut writes = match self {
            Stmt::Def(loc, ..) => vec![*loc],
            Stmt::Clobber(locs) => locs.clone(),
            _ => vec![],
        };
        if self.calls() {
            writes.extend(volatile());
        }
        writes
    }

    fn full_writes(&self) -> Vec<Loc> {
        let mut writes = match self {
            Stmt::Def(loc, w, _) if !partial(*loc, *w) => vec![*loc],
            Stmt::Clobber(locs) => locs.clone(),
            _ => vec![],
        };
        if self.calls() {
            writes.extend(volatile());
        }
        writes
    }

    fn effects(&self, aliased: &HashSet<i64>) -> bool {
        match self {
            Stmt::Store(..) | Stmt::Eval(_) | Stmt::Asm(_) => true,
            Stmt::Def(Loc::Stack(o), ..) if aliased.contains(o) => true,
            Stmt::Def(_, _, e) => e.has_side_effects(),
            Stmt::Clobber(_) => false,
        }
    }
}

impl Exit {
    pub fn expr(&self) -> Option<&Expr> {
        match self {
            Exit::Branch(e, ..)
            | Exit::Switch(e, ..)
            | Exit::TailCall(e)
            | Exit::Return(Some(e)) => Some(e),
            _ => None,
        }
    }

    pub fn map_expr(&mut self, f: &mut impl FnMut(Expr) -> Expr) {
        match self {
            Exit::Branch(e, ..)
            | Exit::Switch(e, ..)
            | Exit::TailCall(e)
            | Exit::Return(Some(e)) => {
                let old = std::mem::replace(e, Expr::Const(0, 1));
                *e = f(old);
            }
            _ => {}
        }
    }

    fn reads(&self) -> Vec<(Loc, u8)> {
        self.expr().map(Expr::reads).unwrap_or_default()
    }
}

fn reads_memory(e: &Expr, aliased: &HashSet<i64>) -> bool {
    e.has_load()
        || e.reads()
            .iter()
            .any(|(l, _)| matches!(l, Loc::Stack(o) if aliased.contains(o)))
}

fn arg_slot(loc: Loc) -> Option<u8> {
    match loc {
        Loc::Reg(RCX) => Some(0),
        Loc::Reg(RDX) => Some(1),
        Loc::Reg(R8) => Some(2),
        Loc::Reg(R9) => Some(3),
        Loc::Reg(r) if (VECTOR..VECTOR + 4).contains(&r) => Some(4 + r - VECTOR),
        _ => None,
    }
}

/// Reads of argument registers that no definition can reach become parameters.
pub fn parameters(f: &mut Function) -> u8 {
    let n = f.blocks.len();
    let generated: Vec<u8> = f
        .blocks
        .iter()
        .map(|b| {
            b.stmts
                .iter()
                .flat_map(Stmt::writes)
                .filter_map(arg_slot)
                .fold(0u8, |m, s| m | (1 << s))
        })
        .collect();
    let mut incoming = vec![0u8; n];
    let mut outgoing = vec![0u8; n];
    let mut changed = true;
    while changed {
        changed = false;
        for b in 0..n {
            let input = if b == f.entry {
                0
            } else {
                f.pred[b].iter().fold(0, |m, p| m | outgoing[*p])
            };
            let output = input | generated[b];
            if input != incoming[b] || output != outgoing[b] {
                incoming[b] = input;
                outgoing[b] = output;
                changed = true;
            }
        }
    }
    let mut count = 0u8;
    for (b, block) in f.blocks.iter_mut().enumerate() {
        let mut defined = incoming[b];
        let replace = |e: Expr, defined: u8, count: &mut u8| {
            e.map(&mut |x| match x {
                Expr::Var(loc, w) => match arg_slot(loc) {
                    Some(slot) if defined & (1 << slot) == 0 => {
                        let n = slot % 4 + 1;
                        *count = (*count).max(n);
                        Expr::Param(n, w, slot >= 4)
                    }
                    _ => Expr::Var(loc, w),
                },
                other => other,
            })
        };
        for stmt in &mut block.stmts {
            stmt.map_exprs(&mut |e| replace(e, defined, &mut count));
            for loc in stmt.writes() {
                if let Some(slot) = arg_slot(loc) {
                    defined |= 1 << slot;
                }
            }
        }
        block
            .exit
            .map_expr(&mut |e| replace(e, defined, &mut count));
    }
    count
}

fn liveness(f: &Function) -> Vec<HashSet<Loc>> {
    let n = f.blocks.len();
    let mut uses = vec![HashSet::new(); n];
    let mut defs = vec![HashSet::new(); n];
    for (b, block) in f.blocks.iter().enumerate() {
        for stmt in &block.stmts {
            for (loc, _) in stmt.reads() {
                if !defs[b].contains(&loc) {
                    uses[b].insert(loc);
                }
            }
            for loc in stmt.full_writes() {
                defs[b].insert(loc);
            }
        }
        for (loc, _) in block.exit.reads() {
            if !defs[b].contains(&loc) {
                uses[b].insert(loc);
            }
        }
    }
    let mut live_in: Vec<HashSet<Loc>> = uses.clone();
    let mut live_out: Vec<HashSet<Loc>> = vec![HashSet::new(); n];
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..n).rev() {
            let out: HashSet<Loc> = f.succ[b]
                .iter()
                .flat_map(|s| live_in[*s].iter().copied())
                .collect();
            if out.len() != live_out[b].len() {
                let mut input = uses[b].clone();
                input.extend(out.iter().filter(|l| !defs[b].contains(l)));
                live_in[b] = input;
                live_out[b] = out;
                changed = true;
            }
        }
    }
    live_out
}

fn remove_dead(block: &mut Block, live_out: &HashSet<Loc>, aliased: &HashSet<i64>) -> bool {
    let mut live = live_out.clone();
    live.extend(block.exit.reads().into_iter().map(|(l, _)| l));
    let mut changed = false;
    let mut k = block.stmts.len();
    while k > 0 {
        k -= 1;
        if let Stmt::Def(loc, _, e) = &block.stmts[k] {
            let pinned = matches!(loc, Loc::Stack(o) if aliased.contains(o));
            if !live.contains(loc) && !pinned {
                changed = true;
                if e.has_side_effects() {
                    let (loc, e) = (*loc, e.clone());
                    block.stmts[k] = Stmt::Eval(e);
                    block.stmts.insert(k + 1, Stmt::Clobber(vec![loc]));
                } else {
                    block.stmts.remove(k);
                    continue;
                }
            }
        }
        let stmt = &block.stmts[k];
        for loc in stmt.full_writes() {
            live.remove(&loc);
        }
        live.extend(stmt.reads().into_iter().map(|(l, _)| l));
    }
    changed
}

fn natural_width(e: &Expr) -> Option<u8> {
    match e {
        Expr::Load(_, w, _) | Expr::Var(_, w) => Some(*w),
        Expr::Cast(ty, _) => match *ty {
            "uint8_t" | "int8_t" => Some(1),
            "uint16_t" | "int16_t" => Some(2),
            "uint32_t" | "int32_t" | "float" => Some(4),
            _ => Some(8),
        },
        Expr::Binary(op, ..)
            if matches!(*op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||") =>
        {
            Some(1)
        }
        Expr::Condition(..) => Some(1),
        _ => None,
    }
}

fn adapt(e: &Expr, def_w: u8, read_w: u8, loc: Loc) -> Option<Expr> {
    if read_w == def_w || loc == Loc::Flags || loc.is_vector() {
        return Some(e.clone());
    }
    if read_w < def_w {
        return Some(match e {
            Expr::Call(..) => e.clone(),
            Expr::Const(v, _) => konst(*v, read_w),
            _ if natural_width(e).is_some_and(|w| w <= read_w) => e.clone(),
            _ => Expr::Cast(unsigned_type(read_w.min(4)), Box::new(e.clone())),
        });
    }
    (def_w == 4).then(|| e.clone())
}

fn cheap(e: &Expr, aliased: &HashSet<i64>) -> bool {
    match e {
        Expr::Const(..) | Expr::Param(..) | Expr::Name(_) | Expr::Text(_) => true,
        Expr::Var(Loc::Stack(o), _) => !aliased.contains(o),
        Expr::Var(..) => true,
        Expr::AddrOf(inner) => matches!(**inner, Expr::Var(Loc::Stack(_), _) | Expr::Name(_)),
        _ => false,
    }
}

fn intact(stmts: &[Stmt], from: usize, to: usize, e: &Expr, aliased: &HashSet<i64>) -> bool {
    let reads: Vec<Loc> = e.reads().into_iter().map(|(l, _)| l).collect();
    let memory = reads_memory(e, aliased);
    let effects = e.has_side_effects();
    for stmt in stmts.iter().take(to.min(stmts.len())).skip(from + 1) {
        if stmt.writes().iter().any(|l| reads.contains(l)) {
            return false;
        }
        let stmt_effects = stmt.effects(aliased);
        if memory && stmt_effects {
            return false;
        }
        if effects && (stmt_effects || stmt.exprs().iter().any(|x| reads_memory(x, aliased))) {
            return false;
        }
    }
    true
}

fn substitute(e: Expr, loc: Loc, value: &Expr, def_w: u8) -> Expr {
    e.map(&mut |x| match x {
        Expr::Var(l, w) if l == loc => adapt(value, def_w, w, loc).unwrap_or(Expr::Var(l, w)),
        other => other,
    })
}

fn condition_code(e: &Expr) -> Option<iced_x86::ConditionCode> {
    let mut found = None;
    e.visit(&mut |x| {
        if let Expr::Condition(cc, flags) = x
            && matches!(**flags, Expr::Var(Loc::Flags, _))
        {
            found = Some(*cc);
        }
    });
    found
}

fn flag_reads(e: &Expr) -> usize {
    let mut count = 0;
    e.visit(&mut |x| count += usize::from(matches!(x, Expr::Var(Loc::Flags, _))));
    count
}

/// After `r32 = x`, a read of the whole 64-bit register sees the same zero-extended value.
fn narrow_reads(block: &mut Block) {
    for i in 0..block.stmts.len() {
        let Stmt::Def(Loc::Reg(r), 4, _) = block.stmts[i] else {
            continue;
        };
        if r >= VECTOR {
            continue;
        }
        let loc = Loc::Reg(r);
        let narrow = |e: Expr| {
            e.map(&mut |x| match x {
                Expr::Var(l, 8) if l == loc => Expr::Var(l, 4),
                other => other,
            })
        };
        let mut j = i + 1;
        while j < block.stmts.len() {
            block.stmts[j].map_exprs(&mut |e| narrow(e));
            if block.stmts[j].writes().contains(&loc) {
                break;
            }
            j += 1;
        }
        if j == block.stmts.len() {
            block.exit.map_expr(&mut |e| narrow(e));
        }
    }
}

/// Replaces a comparison that cannot reach its only test with a boolean computed where it was made.
fn materialize(block: &mut Block, i: usize, e: &Expr, temps: &mut u8) -> bool {
    if *temps >= 200 || !matches!(e, Expr::Flags(..)) {
        return false;
    }
    let len = block.stmts.len();
    let Some(j) = (i + 1..=len).find(|&j| {
        let reads = if j == len {
            block.exit.reads()
        } else {
            block.stmts[j].reads()
        };
        reads.iter().any(|(l, _)| *l == Loc::Flags)
    }) else {
        return false;
    };
    let exprs: Vec<Expr> = if j == len {
        block.exit.expr().cloned().into_iter().collect()
    } else {
        block.stmts[j].exprs().into_iter().cloned().collect()
    };
    let codes: Vec<_> = exprs.iter().filter_map(condition_code).collect();
    let count: usize = exprs.iter().map(flag_reads).sum();
    let ([cc], 1) = (codes.as_slice(), count) else {
        return false;
    };
    let cc = *cc;
    let temp = Loc::Reg(TEMP + *temps);
    *temps += 1;
    block.stmts[i] = Stmt::Def(temp, 1, simplify(Expr::Condition(cc, Box::new(e.clone()))));
    let replace = |x: Expr| {
        x.map(&mut |y| match y {
            Expr::Condition(c, flags) if c == cc && matches!(*flags, Expr::Var(Loc::Flags, _)) => {
                Expr::Var(temp, 1)
            }
            other => other,
        })
    };
    if j == len {
        block.exit.map_expr(&mut |x| replace(x));
    } else {
        block.stmts[j].map_exprs(&mut |x| replace(x));
    }
    true
}

fn propagate(
    block: &mut Block,
    live_out: &HashSet<Loc>,
    aliased: &HashSet<i64>,
    flags_only: bool,
    temps: &mut u8,
) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < block.stmts.len() {
        let Stmt::Def(loc, w, e) = &block.stmts[i] else {
            i += 1;
            continue;
        };
        let (loc, w, e) = (*loc, *w, e.clone());
        if flags_only && loc != Loc::Flags {
            i += 1;
            continue;
        }
        if matches!(loc, Loc::Stack(o) if aliased.contains(&o)) {
            i += 1;
            continue;
        }
        let copyable =
            loc == Loc::Flags || (cheap(&e, aliased) && !e.reads().iter().any(|(l, _)| *l == loc));
        let len = block.stmts.len();
        let mut uses = Vec::new();
        let mut occurrences = 0;
        let mut redefined = false;
        let mut blocked = false;
        for j in i + 1..=len {
            let reads = if j == len {
                block.exit.reads()
            } else {
                block.stmts[j].reads()
            };
            let count = reads.iter().filter(|(l, _)| *l == loc).count();
            if count > 0 {
                let real = if j == len {
                    count
                } else {
                    block.stmts[j]
                        .exprs()
                        .iter()
                        .flat_map(|x| x.reads())
                        .filter(|(l, _)| *l == loc)
                        .count()
                };
                if real == 0 {
                    blocked = true;
                    break;
                }
                let fits = reads
                    .iter()
                    .filter(|(l, _)| *l == loc)
                    .all(|(_, rw)| adapt(&e, w, *rw, loc).is_some());
                if !fits || !intact(&block.stmts, i, j, &e, aliased) {
                    blocked = true;
                    break;
                }
                uses.push(j);
                occurrences += count;
            }
            if j < len && block.stmts[j].writes().contains(&loc) {
                redefined = true;
                break;
            }
        }
        if blocked && loc == Loc::Flags && uses.is_empty() && materialize(block, i, &e, temps) {
            changed = true;
            i += 1;
            continue;
        }
        if occurrences == 0 {
            i += 1;
            continue;
        }
        let removable = !blocked && (redefined || !live_out.contains(&loc));
        if !copyable && !(occurrences == 1 && removable) {
            i += 1;
            continue;
        }
        for &j in &uses {
            if j == len {
                block
                    .exit
                    .map_expr(&mut |x| simplify(substitute(x, loc, &e, w)));
            } else {
                block.stmts[j].map_exprs(&mut |x| simplify(substitute(x, loc, &e, w)));
            }
        }
        changed = true;
        if removable {
            block.stmts.remove(i);
        } else {
            i += 1;
        }
    }
    changed
}

/// Gives a block that tests flags set by its only predecessor its own copy of that comparison.
fn copy_flags(f: &mut Function) {
    for _ in 0..2 {
        for b in 0..f.blocks.len() {
            if f.pred[b].len() != 1 || f.pred[b][0] == b {
                continue;
            }
            let block = &f.blocks[b];
            let mut needs = false;
            for stmt in &block.stmts {
                if stmt.reads().iter().any(|(l, _)| *l == Loc::Flags) {
                    needs = true;
                    break;
                }
                if stmt.writes().contains(&Loc::Flags) {
                    break;
                }
            }
            if !needs && !block.stmts.iter().any(|s| s.writes().contains(&Loc::Flags)) {
                needs = block.exit.reads().iter().any(|(l, _)| *l == Loc::Flags);
            }
            if !needs {
                continue;
            }
            let p = f.pred[b][0];
            let stmts = &f.blocks[p].stmts;
            let Some(k) = stmts.iter().rposition(|s| s.writes().contains(&Loc::Flags)) else {
                continue;
            };
            let Stmt::Def(Loc::Flags, _, e) = &stmts[k] else {
                continue;
            };
            if !matches!(e, Expr::Flags(..)) || !intact(stmts, k, stmts.len(), e, &f.aliased) {
                continue;
            }
            let copy = Stmt::Def(Loc::Flags, 1, e.clone());
            f.blocks[b].stmts.insert(0, copy);
        }
    }
}

/// A comparison made before `x = x ± y` and tested after it is rewritten in terms of the new `x`.
fn rebase_flags(f: &mut Function) {
    for block in &mut f.blocks {
        let mut i = 0;
        while i + 1 < block.stmts.len() {
            let (Stmt::Def(Loc::Flags, _, flags), Stmt::Def(loc, w, update)) =
                (&block.stmts[i], &block.stmts[i + 1])
            else {
                i += 1;
                continue;
            };
            let (loc, w) = (*loc, *w);
            let inverse = match update {
                Expr::Binary(op @ ("+" | "-"), x, y)
                    if matches!(**x, Expr::Var(l, _) if l == loc)
                        && !y.reads().iter().any(|(l, _)| *l == loc) =>
                {
                    Some(bin(
                        if *op == "+" { "-" } else { "+" },
                        Expr::Var(loc, w),
                        (**y).clone(),
                    ))
                }
                _ => None,
            };
            match inverse {
                Some(old)
                    if flags.reads().iter().any(|(l, _)| *l == loc)
                        && flags.reads().iter().all(|(l, rw)| *l != loc || *rw == w)
                        && !partial(loc, w) =>
                {
                    let rebased = simplify(flags.clone().map(&mut |x| match x {
                        Expr::Var(l, rw) if l == loc && rw == w => old.clone(),
                        other => other,
                    }));
                    block.stmts.swap(i, i + 1);
                    block.stmts[i + 1] = Stmt::Def(Loc::Flags, 1, rebased);
                    i += 2;
                }
                _ => i += 1,
            }
        }
    }
}

fn bare_branch(f: &Function, b: usize) -> Option<(Expr, usize, usize)> {
    let block = &f.blocks[b];
    if block.stmts.iter().any(|s| !matches!(s, Stmt::Clobber(_))) || f.pred[b].len() != 1 {
        return None;
    }
    match &block.exit {
        Exit::Branch(c, t, e) if c.reads().iter().all(|(l, _)| *l != Loc::Flags) => {
            Some((c.clone(), *t, *e))
        }
        _ => None,
    }
}

/// Folds a branch whose arm is an empty block that branches again to a shared target into `a || b` or `a && b`.
pub fn merge_conditions(f: &mut Function) {
    let mut changed = true;
    while changed {
        changed = false;
        for b in 0..f.blocks.len() {
            let Exit::Branch(c1, t, e) = f.blocks[b].exit.clone() else {
                continue;
            };
            let mut merged = None;
            if e != b
                && e != t
                && let Some((c2, t2, e2)) = bare_branch(f, e)
            {
                if t2 == t {
                    merged = Some((bin("||", c1.clone(), c2), t, e2, e));
                } else if e2 == t {
                    merged = Some((bin("||", c1.clone(), negate(c2)), t, t2, e));
                }
            }
            if merged.is_none()
                && t != b
                && t != e
                && let Some((c2, t2, e2)) = bare_branch(f, t)
            {
                if e2 == e {
                    merged = Some((bin("&&", c1, c2), t2, e, t));
                } else if t2 == e {
                    merged = Some((bin("&&", c1, negate(c2)), e2, e, t));
                }
            }
            let Some((condition, taken, fallthrough, removed)) = merged else {
                continue;
            };
            f.blocks[b].exit = Exit::Branch(simplify(condition), taken, fallthrough);
            f.blocks[removed].exit = Exit::Stop;
            for s in std::mem::take(&mut f.succ[removed]) {
                f.pred[s].retain(|p| *p != removed);
            }
            f.pred[removed].clear();
            f.succ[b] = if taken == fallthrough {
                vec![taken]
            } else {
                vec![taken, fallthrough]
            };
            for s in [taken, fallthrough] {
                if !f.pred[s].contains(&b) {
                    f.pred[s].push(b);
                }
            }
            changed = true;
        }
    }
}

pub fn run(f: &mut Function, deadline: Option<std::time::Instant>) {
    rebase_flags(f);
    copy_flags(f);
    let mut temps = 1u8;
    for _ in 0..64 {
        if deadline.is_some_and(|d| std::time::Instant::now() > d) {
            break;
        }
        let mut changed = false;
        let live = liveness(f);
        for (b, block) in f.blocks.iter_mut().enumerate() {
            changed |= remove_dead(block, &live[b], &f.aliased);
        }
        let live = liveness(f);
        for (b, block) in f.blocks.iter_mut().enumerate() {
            changed |= propagate(block, &live[b], &f.aliased, true, &mut temps);
            changed |= propagate(block, &live[b], &f.aliased, false, &mut temps);
            narrow_reads(block);
        }
        if !changed {
            break;
        }
    }
    for block in &mut f.blocks {
        for stmt in &mut block.stmts {
            stmt.map_exprs(&mut simplify);
        }
        block.exit.map_expr(&mut simplify);
    }
}
