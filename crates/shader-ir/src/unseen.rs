//! Lane writes that may ignore exec, because no lane they leave alone is ever looked at again.

use std::collections::HashMap;

use crate::ir::{Bin, Expr, Function, Reg, Stmt, Terminator, Tmp, Un};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Regs([u64; 4]);

impl Regs {
    const ALL: Regs = Regs([u64::MAX; 4]);

    fn add(&mut self, reg: u8) {
        self.0[usize::from(reg / 64)] |= 1 << (reg % 64);
    }

    fn has(&self, reg: u8) -> bool {
        self.0[usize::from(reg / 64)] >> (reg % 64) & 1 != 0
    }

    fn join(&mut self, other: Regs) {
        for (mine, theirs) in self.0.iter_mut().zip(other.0) {
            *mine |= theirs;
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Fact {
    /// VGPRs read in some lanes.
    reads: Regs,
    /// VGPRs whose inactive lanes may be read.
    crossing: Regs,
    /// Turns on lanes that may have been off.
    widens: bool,
}

fn facts(function: &Function, stmts: &[Stmt]) -> Vec<Fact> {
    let mut sources: HashMap<Tmp, Regs> = HashMap::new();
    let mut masks: HashMap<Tmp, u32> = HashMap::new();
    let mut version = 0;
    let mut out = Vec::with_capacity(stmts.len());
    for stmt in stmts {
        let mut fact = Fact::default();
        match stmt {
            Stmt::Let(tmp, expr) => {
                match expr {
                    Expr::Get(Reg::V(reg)) => fact.reads.add(*reg),
                    Expr::GetRelative(base, _) => {
                        for reg in *base..=255 {
                            fact.reads.add(reg);
                        }
                        fact.reads.add(0);
                    }
                    _ => {}
                }
                let source_of = |t: &Tmp| sources.get(t).copied().unwrap_or(Regs::ALL);
                let mut from = fact.reads;
                for operand in expr.operands() {
                    from.join(source_of(&operand));
                }
                match expr {
                    Expr::ReadLane(source, _) | Expr::Shuffle(source, _) => {
                        fact.crossing = source_of(source)
                    }
                    Expr::Image(image) if image.op.implicit() => {
                        for operand in &image.coordinates {
                            fact.crossing.join(source_of(operand));
                        }
                    }
                    _ => {}
                }
                sources.insert(*tmp, from);
                let current = |t: &Tmp| masks.get(t) == Some(&version);
                let within = match expr {
                    Expr::Get(Reg::Exec) | Expr::Ballot(_) => true,
                    Expr::Const(0) | Expr::Const64(0) => true,
                    Expr::Bin(Bin::And | Bin::And64, a, b) => current(a) || current(b),
                    Expr::Un(Un::Lo | Un::Zext, a) => current(a),
                    Expr::Bin(Bin::Pack64, low, _) => !function.wave64 && current(low),
                    Expr::Select(_, a, b) => current(a) && current(b),
                    _ => false,
                };
                if within {
                    masks.insert(*tmp, version);
                }
            }
            Stmt::Set(Reg::Exec, tmp) => {
                fact.widens = masks.get(tmp) != Some(&version);
                version += 1;
            }
            _ => {}
        }
        out.push(fact);
    }
    out
}

fn successors(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Jump(target) => vec![*target],
        Terminator::Branch(_, taken, fallthrough) => vec![*taken, *fallthrough],
        Terminator::End => Vec::new(),
    }
}

/// For each block and statement, whether that `SetLane` may write every lane rather than only the active
/// ones. A lane it skips is off and stays off until something turns lanes back on, so this holds when its
/// register is not read after any such point and no read across lanes (`ReadLane`, `Shuffle`, the
/// derivatives of an implicit-level image sample) sees it. `end_reads_all` makes the end of the program
/// read every register in every lane, as a harness that saves the registers does.
pub fn free_writes(function: &Function, end_reads_all: bool) -> Vec<Vec<bool>> {
    let blocks = &function.blocks;
    let facts: Vec<Vec<Fact>> = blocks.iter().map(|b| facts(function, &b.stmts)).collect();
    let next: Vec<Vec<usize>> = blocks.iter().map(|b| successors(&b.terminator)).collect();
    let end = |b: usize| {
        if end_reads_all && blocks[b].terminator == Terminator::End {
            Regs::ALL
        } else {
            Regs::default()
        }
    };
    let mut read_in = vec![Regs::default(); blocks.len()];
    let mut seen_in = vec![Regs::default(); blocks.len()];
    let out_of = |sets: &[Regs], b: usize| {
        let mut out = end(b);
        for s in &next[b] {
            if let Some(set) = sets.get(*s) {
                out.join(*set);
            }
        }
        out
    };
    let walk = |b: usize, read_in: &[Regs], seen_in: &[Regs], record: Option<&mut Vec<bool>>| {
        let mut read = out_of(read_in, b);
        let mut seen = out_of(seen_in, b);
        let mut free = vec![false; facts[b].len()];
        for (i, fact) in facts[b].iter().enumerate().rev() {
            if fact.widens {
                seen.join(read);
            }
            seen.join(fact.crossing);
            if let Stmt::SetLane(reg, _) = blocks[b].stmts[i] {
                free[i] = !seen.has(reg);
            }
            read.join(fact.reads);
        }
        if let Some(record) = record {
            *record = free;
        }
        (read, seen)
    };
    loop {
        let mut changed = false;
        for b in (0..blocks.len()).rev() {
            let (read, seen) = walk(b, &read_in, &seen_in, None);
            if read != read_in[b] || seen != seen_in[b] {
                read_in[b] = read;
                seen_in[b] = seen;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (0..blocks.len())
        .map(|b| {
            let mut free = Vec::new();
            walk(b, &read_in, &seen_in, Some(&mut free));
            free
        })
        .collect()
}
