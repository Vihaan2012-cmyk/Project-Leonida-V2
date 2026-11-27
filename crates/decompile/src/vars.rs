//! Gives each value a register holds across a region a variable of its own.
//!
//! A definition of a register and every use it can reach form one web; the webs become `v1`, `v2`, ...
//! Registers whose value may come from before the function starts keep their register name.

use std::collections::{HashMap, VecDeque};

use crate::expr::{Expr, Loc, R8, R9, RCX, RDX, VECTOR};
use crate::lift::{Function, Stmt};
use crate::simplify::partial;

/// What the renamer learned about one variable.
#[derive(Clone, Copy, Debug)]
pub struct VarInfo {
    /// The widest definition, in bytes.
    pub width: u8,
    /// The 1-based argument whose incoming value it carries, or 0.
    pub param: u8,
}

struct Union {
    parent: Vec<u32>,
    undefined: Vec<bool>,
    /// 1-based argument number of an incoming argument register, or 0.
    param: Vec<u8>,
}

impl Union {
    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let up = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = up;
            x = up;
        }
        x
    }

    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[b as usize] = a;
            let (pa, pb) = (self.param[a as usize], self.param[b as usize]);
            self.undefined[a as usize] |=
                self.undefined[b as usize] || (pa != 0 && pb != 0 && pa != pb);
            self.param[a as usize] = pa.max(pb);
        }
    }

    fn add(&mut self, undefined: bool, param: u8) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(id);
        self.undefined.push(undefined);
        self.param.push(param);
        id
    }
}

#[derive(Clone, Copy)]
enum Src {
    Def(u32),
    /// The value live on entry to the block.
    In,
}

#[derive(Default)]
struct Site {
    def: Option<u32>,
    /// For a partial write, the value the untouched bytes keep.
    prev: Option<Src>,
    uses: Vec<(Loc, Src)>,
}

/// The 1-based argument an incoming register carries under the Windows x64 convention, or 0.
fn argument(loc: Loc) -> u8 {
    match loc {
        Loc::Reg(RCX) => 1,
        Loc::Reg(RDX) => 2,
        Loc::Reg(R8) => 3,
        Loc::Reg(R9) => 4,
        Loc::Reg(r) if (VECTOR..VECTOR + 4).contains(&r) => r - VECTOR + 1,
        _ => 0,
    }
}

fn is_register(loc: Loc) -> bool {
    matches!(loc, Loc::Reg(_))
}

fn register_reads(e: &Expr, out: &mut Vec<Loc>) {
    e.visit(&mut |x| {
        if let Expr::Var(loc, _) = x
            && is_register(*loc)
            && !out.contains(loc)
        {
            out.push(*loc);
        }
    });
}

struct Walk {
    defs: Vec<Loc>,
    entry: HashMap<Loc, u32>,
    union: Union,
}

impl Walk {
    fn entry_of(&mut self, loc: Loc) -> u32 {
        if let Some(id) = self.entry.get(&loc) {
            return *id;
        }
        let param = argument(loc);
        let id = self.union.add(param == 0, param);
        self.defs.push(loc);
        self.entry.insert(loc, id);
        id
    }

    fn new_def(&mut self, loc: Loc) -> u32 {
        self.defs.push(loc);
        self.union.add(false, 0)
    }
}

/// Replaces register locations with variables. Returns the variables, or `None` when the function is too
/// large for the analysis and was left alone.
pub fn rename(f: &mut Function, max_blocks: usize) -> Option<Vec<VarInfo>> {
    let n = f.blocks.len();
    if n > max_blocks {
        return None;
    }
    let mut walk = Walk {
        defs: Vec::new(),
        entry: HashMap::new(),
        union: Union {
            parent: Vec::new(),
            undefined: Vec::new(),
            param: Vec::new(),
        },
    };
    let mut sites: Vec<Vec<Site>> = Vec::with_capacity(n);
    // Per register: the last definition in each block that defines it, and the blocks that read it first.
    let mut last: HashMap<Loc, Vec<(usize, u32)>> = HashMap::new();
    let mut exposed: HashMap<Loc, Vec<usize>> = HashMap::new();
    for (b, block) in f.blocks.iter().enumerate() {
        let mut current: HashMap<Loc, u32> = HashMap::new();
        let mut block_sites = Vec::with_capacity(block.stmts.len() + 1);
        let mut source = |loc: Loc, current: &HashMap<Loc, u32>| -> Src {
            match current.get(&loc) {
                Some(d) => Src::Def(*d),
                None => {
                    let blocks = exposed.entry(loc).or_default();
                    if blocks.last() != Some(&b) {
                        blocks.push(b);
                    }
                    Src::In
                }
            }
        };
        for stmt in &block.stmts {
            let mut site = Site::default();
            let mut reads = Vec::new();
            for e in stmt.exprs() {
                register_reads(e, &mut reads);
            }
            let partial_def = match stmt {
                Stmt::Def(loc, w, _) if is_register(*loc) && partial(*loc, *w) => Some(*loc),
                _ => None,
            };
            for loc in reads {
                let src = source(loc, &current);
                site.uses.push((loc, src));
            }
            if let Some(loc) = partial_def {
                site.prev = Some(source(loc, &current));
            }
            let own = match stmt {
                Stmt::Def(loc, ..) if is_register(*loc) => Some(*loc),
                _ => None,
            };
            for loc in stmt.writes() {
                if is_register(loc) && Some(loc) != own {
                    let undefined = walk.entry_of(loc);
                    current.insert(loc, undefined);
                }
            }
            if let Some(loc) = own {
                let id = walk.new_def(loc);
                site.def = Some(id);
                current.insert(loc, id);
            }
            block_sites.push(site);
        }
        let mut site = Site::default();
        let mut reads = Vec::new();
        if let Some(e) = block.exit.expr() {
            register_reads(e, &mut reads);
        }
        for loc in reads {
            let src = source(loc, &current);
            site.uses.push((loc, src));
        }
        block_sites.push(site);
        for (loc, id) in current {
            last.entry(loc).or_default().push((b, id));
        }
        sites.push(block_sites);
    }

    // The definitions reaching the start of each block that reads a register before writing it.
    let mut block_in: HashMap<(usize, Loc), u32> = HashMap::new();
    let mut locs: Vec<Loc> = exposed.keys().copied().collect();
    locs.sort();
    for loc in locs {
        let entry = walk.entry_of(loc);
        let mut ids: Vec<u32> = walk
            .defs
            .iter()
            .enumerate()
            .filter(|(_, l)| **l == loc)
            .map(|(i, _)| i as u32)
            .collect();
        ids.sort_unstable();
        let words = ids.len().div_ceil(64);
        let local = |id: u32| {
            ids.binary_search(&id)
                .expect("a definition of this register")
        };
        let mut generated: Vec<Option<usize>> = vec![None; n];
        for &(b, id) in last.get(&loc).map(Vec::as_slice).unwrap_or(&[]) {
            generated[b] = Some(local(id));
        }
        let mut input = vec![0u64; n * words];
        let mut output = vec![0u64; n * words];
        let entry_bit = local(entry);
        input[f.entry * words + entry_bit / 64] |= 1 << (entry_bit % 64);
        let mut queue: VecDeque<usize> = (0..n).collect();
        let mut queued = vec![true; n];
        while let Some(b) = queue.pop_front() {
            queued[b] = false;
            for &p in &f.pred[b] {
                for w in 0..words {
                    input[b * words + w] |= output[p * words + w];
                }
            }
            let mut next = input[b * words..(b + 1) * words].to_vec();
            if let Some(g) = generated[b] {
                next.fill(0);
                next[g / 64] |= 1 << (g % 64);
            }
            if next != output[b * words..(b + 1) * words] {
                output[b * words..(b + 1) * words].copy_from_slice(&next);
                for &s in &f.succ[b] {
                    if !queued[s] {
                        queued[s] = true;
                        queue.push_back(s);
                    }
                }
            }
        }
        for &b in exposed.get(&loc).map(Vec::as_slice).unwrap_or(&[]) {
            if block_in.contains_key(&(b, loc)) {
                continue;
            }
            let mut first = None;
            for (k, &id) in ids.iter().enumerate() {
                if input[b * words + k / 64] >> (k % 64) & 1 == 1 {
                    match first {
                        None => first = Some(id),
                        Some(f) => walk.union.join(f, id),
                    }
                }
            }
            block_in.insert((b, loc), first.unwrap_or(entry));
        }
    }

    let resolve = |src: Src, b: usize, loc: Loc, block_in: &HashMap<(usize, Loc), u32>| match src {
        Src::Def(d) => d,
        Src::In => block_in[&(b, loc)],
    };
    for (b, block_sites) in sites.iter().enumerate() {
        for site in block_sites {
            let (Some(def), Some(prev)) = (site.def, site.prev) else {
                continue;
            };
            let loc = walk.defs[def as usize];
            let prev = resolve(prev, b, loc, &block_in);
            let root = walk.union.find(prev);
            if !walk.union.undefined[root as usize] {
                walk.union.join(prev, def);
            }
        }
    }

    // Webs that only ever copy an argument which nothing modifies stand for that argument.
    let mut defs_of: HashMap<u32, (usize, Option<(usize, usize)>)> = HashMap::new();
    let mut mutated = [false; 9];
    for (b, block) in f.blocks.iter().enumerate() {
        for (k, (stmt, site)) in block.stmts.iter().zip(&sites[b]).enumerate() {
            let (Some(def), Stmt::Def(Loc::Reg(_), ..)) = (site.def, stmt) else {
                continue;
            };
            let root = walk.union.find(def);
            let entry = defs_of.entry(root).or_insert((0, None));
            entry.0 += 1;
            entry.1 = Some((b, k));
            let param = walk.union.param[root as usize];
            if param != 0 {
                mutated[usize::from(param)] = true;
            }
        }
    }
    let mut aliases: HashMap<u32, u8> = HashMap::new();
    for (root, (count, site)) in &defs_of {
        let Some((b, k)) = site else { continue };
        if *count == 1
            && !walk.union.undefined[*root as usize]
            && walk.union.param[*root as usize] == 0
            && let Stmt::Def(_, 8, Expr::Param(n)) = &f.blocks[*b].stmts[*k]
            && !mutated[usize::from(*n)]
        {
            aliases.insert(*root, *n);
        }
    }

    // Names and widths.
    let mut names: HashMap<u32, Target> = HashMap::new();
    let mut vars: Vec<VarInfo> = Vec::new();
    for (b, block) in f.blocks.iter().enumerate() {
        for (stmt, site) in block.stmts.iter().zip(&sites[b]) {
            let (Some(def), Stmt::Def(Loc::Reg(_), w, _)) = (site.def, stmt) else {
                continue;
            };
            let root = walk.union.find(def);
            if walk.union.undefined[root as usize] {
                continue;
            }
            if let Some(n) = aliases.get(&root) {
                names.insert(root, Target::Param(*n));
                continue;
            }
            let target = *names.entry(root).or_insert_with(|| {
                vars.push(VarInfo {
                    width: 0,
                    param: walk.union.param[root as usize],
                });
                Target::Var(vars.len() as u32 - 1)
            });
            if let Target::Var(index) = target {
                let info = &mut vars[index as usize];
                info.width = info.width.max(*w);
            }
        }
    }

    // Rewrites.
    for (b, block) in f.blocks.iter_mut().enumerate() {
        let block_sites = &sites[b];
        let stmt_count = block.stmts.len();
        for (k, stmt) in block.stmts.iter_mut().enumerate() {
            let site = &block_sites[k];
            let mut mapping: Vec<(Loc, Target)> = Vec::new();
            for (loc, src) in &site.uses {
                let d = resolve(*src, b, *loc, &block_in);
                let root = walk.union.find(d);
                if let Some(t) = names.get(&root) {
                    mapping.push((*loc, *t));
                }
            }
            stmt.map_exprs(&mut |e| rewrite(e, &mapping));
            if let (Some(def), Stmt::Def(loc, ..)) = (site.def, &mut *stmt) {
                let root = walk.union.find(def);
                match names.get(&root) {
                    Some(Target::Var(v)) => *loc = Loc::Var(*v),
                    Some(Target::Param(_)) => *stmt = Stmt::Clobber(Vec::new()),
                    None => {}
                }
            }
        }
        let site = &block_sites[stmt_count];
        let mut mapping: Vec<(Loc, Target)> = Vec::new();
        for (loc, src) in &site.uses {
            let d = resolve(*src, b, *loc, &block_in);
            let root = walk.union.find(d);
            if let Some(t) = names.get(&root) {
                mapping.push((*loc, *t));
            }
        }
        block.exit.map_expr(&mut |e| rewrite(e, &mapping));
    }
    Some(vars)
}

#[derive(Clone, Copy)]
enum Target {
    Var(u32),
    /// The incoming value of this 1-based argument.
    Param(u8),
}

fn rewrite(e: Expr, mapping: &[(Loc, Target)]) -> Expr {
    if mapping.is_empty() {
        return e;
    }
    e.map(&mut |x| match x {
        Expr::Var(loc, w) => match mapping.iter().find(|(l, _)| *l == loc) {
            Some((_, Target::Var(v))) => Expr::Var(Loc::Var(*v), w),
            Some((_, Target::Param(n))) if w >= 8 => Expr::Param(*n),
            Some((_, Target::Param(n))) => Expr::Cast(
                match w {
                    1 => "u8",
                    2 => "u16",
                    _ => "u32",
                },
                Box::new(Expr::Param(*n)),
            ),
            None => Expr::Var(loc, w),
        },
        other => other,
    })
}
