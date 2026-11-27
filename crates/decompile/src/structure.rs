use std::collections::HashSet;

use crate::cfg::{dominates, dominators};
use crate::expr::*;
use crate::lift::{Exit, Function, Stmt};

pub enum Node {
    Stmt(Stmt),
    Switch(Expr, Vec<(Vec<u64>, Vec<Node>)>, Option<Vec<Node>>),
    If(Expr, Vec<Node>, Vec<Node>),
    Loop(Vec<Node>),
    While(Expr, Vec<Node>),
    DoWhile(Vec<Node>, Expr),
    Break,
    Continue,
    Goto(usize),
    Label(usize),
    Return(Option<Expr>),
    Tail(Expr),
    Unreachable,
}

struct LoopInfo {
    body: Vec<bool>,
    follow: Option<usize>,
}

struct Structurer<'f> {
    f: &'f Function,
    ipdom: Vec<Option<usize>>,
    loops: Vec<Option<LoopInfo>>,
    active: Vec<usize>,
    /// How many switches enclose each active loop's body at the point being emitted.
    switches: Vec<usize>,
    emitted: Vec<bool>,
    gotos: HashSet<usize>,
}

fn post_dominators(f: &Function) -> Vec<Option<usize>> {
    let n = f.blocks.len();
    let exit = n;
    let mut succ = vec![Vec::new(); n + 1];
    let mut pred = vec![Vec::new(); n + 1];
    for (b, targets) in f.succ.iter().enumerate() {
        for &s in targets {
            succ[s].push(b);
            pred[b].push(s);
        }
        if targets.is_empty() && !matches!(f.blocks[b].exit, Exit::Stop) {
            succ[exit].push(b);
            pred[b].push(exit);
        }
    }
    let idom = dominators(n + 1, exit, &succ, &pred);
    idom.into_iter()
        .take(n)
        .map(|d| d.filter(|d| *d != exit))
        .collect()
}

fn find_loops(
    f: &Function,
    idom: &[Option<usize>],
    ipdom: &[Option<usize>],
) -> Vec<Option<LoopInfo>> {
    let n = f.blocks.len();
    let mut loops: Vec<Option<LoopInfo>> = (0..n).map(|_| None).collect();
    for u in 0..n {
        for &h in &f.succ[u] {
            if idom[u].is_none() || !dominates(idom, h, u) {
                continue;
            }
            let info = loops[h].get_or_insert_with(|| LoopInfo {
                body: vec![false; n],
                follow: None,
            });
            info.body[h] = true;
            let mut stack = vec![u];
            while let Some(x) = stack.pop() {
                if info.body[x] {
                    continue;
                }
                info.body[x] = true;
                stack.extend(f.pred[x].iter().copied());
            }
        }
    }
    for h in 0..n {
        let Some(info) = &mut loops[h] else { continue };
        let mut exits: Vec<(usize, usize)> = Vec::new();
        for x in (0..n).filter(|x| info.body[*x]) {
            for &s in &f.succ[x] {
                if !info.body[s] {
                    match exits.iter_mut().find(|(t, _)| *t == s) {
                        Some((_, count)) => *count += 1,
                        None => exits.push((s, 1)),
                    }
                }
            }
        }
        info.follow = match ipdom[h] {
            Some(p) if !info.body[p] => Some(p),
            _ => exits
                .iter()
                .max_by_key(|(t, count)| (*count, std::cmp::Reverse(f.blocks[*t].start)))
                .map(|(t, _)| *t),
        };
    }
    loops
}

impl Structurer<'_> {
    /// A short run of blocks ending in a return, copied to each place that jumps to it.
    fn tail(&mut self, start: usize) -> Option<Vec<Node>> {
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut visited = Vec::new();
        let mut b = start;
        let exit = loop {
            let block = &self.f.blocks[b];
            stmts.extend(
                block
                    .stmts
                    .iter()
                    .filter(|s| !matches!(s, Stmt::Clobber(_)))
                    .cloned(),
            );
            visited.push(b);
            if stmts.len() > 2 || visited.len() > 3 {
                return None;
            }
            match &block.exit {
                Exit::Return(e) => break Node::Return(e.clone()),
                Exit::TailCall(e) => break Node::Tail(e.clone()),
                Exit::Jump(t) if !visited.contains(t) && self.loops[*t].is_none() => b = *t,
                _ => return None,
            }
        };
        let exit = match (stmts.last(), exit) {
            (Some(Stmt::Def(loc, w, value)), Node::Return(Some(Expr::Var(l, rw))))
                if *loc == l
                    && (*w == rw || matches!(value, Expr::Const(..)))
                    && !value.has_side_effects() =>
            {
                let value = match value {
                    Expr::Const(v, _) => konst(*v, rw),
                    other => other.clone(),
                };
                stmts.pop();
                Node::Return(Some(value))
            }
            (_, exit) => exit,
        };
        for v in visited {
            self.emitted[v] = true;
        }
        let mut nodes: Vec<Node> = stmts.into_iter().map(Node::Stmt).collect();
        nodes.push(exit);
        Some(nodes)
    }

    fn join(&self, b: usize) -> Option<usize> {
        let j = self.ipdom[b]?;
        match self.active.last() {
            Some(&h) if !self.loops[h].as_ref().is_some_and(|l| l.body[j]) => None,
            _ => Some(j),
        }
    }

    fn seq(&mut self, mut b: usize, stop: Option<usize>, out: &mut Vec<Node>, mut entering: bool) {
        loop {
            if !entering {
                if Some(b) == stop {
                    return;
                }
                if let Some(&h) = self.active.last() {
                    if h == b {
                        out.push(Node::Continue);
                        return;
                    }
                    if self.loops[h].as_ref().and_then(|l| l.follow) == Some(b)
                        && self.switches.last() == Some(&0)
                    {
                        out.push(Node::Break);
                        return;
                    }
                }
                if let Some(tail) = self.tail(b) {
                    out.extend(tail);
                    return;
                }
                let outer = self
                    .active
                    .iter()
                    .any(|&h| h == b || self.loops[h].as_ref().and_then(|l| l.follow) == Some(b));
                if outer || self.emitted[b] {
                    self.gotos.insert(b);
                    out.push(Node::Goto(b));
                    return;
                }
                if self.loops[b].is_some() {
                    let follow = self.loops[b].as_ref().and_then(|l| l.follow);
                    self.active.push(b);
                    self.switches.push(0);
                    let mut body = Vec::new();
                    self.seq(b, None, &mut body, true);
                    self.switches.pop();
                    self.active.pop();
                    out.push(Node::Loop(body));
                    match follow {
                        Some(next) => {
                            b = next;
                            continue;
                        }
                        None => return,
                    }
                }
            }
            entering = false;
            self.emitted[b] = true;
            out.push(Node::Label(b));
            let block = &self.f.blocks[b];
            out.extend(
                block
                    .stmts
                    .iter()
                    .filter(|s| !matches!(s, Stmt::Clobber(_)))
                    .cloned()
                    .map(Node::Stmt),
            );
            match &block.exit {
                Exit::Switch(value, cases, default) => {
                    let (value, cases, default) = (value.clone(), cases.clone(), *default);
                    let join = self.join(b);
                    if let Some(depth) = self.switches.last_mut() {
                        *depth += 1;
                    }
                    let mut arms: Vec<(Vec<u64>, Vec<Node>)> = Vec::new();
                    let mut placed: Vec<usize> = Vec::new();
                    for (value, &target) in cases.iter().enumerate() {
                        if Some(target) == default {
                            continue;
                        }
                        if let Some(index) = placed.iter().position(|t| *t == target) {
                            arms[index].0.push(value as u64);
                            continue;
                        }
                        let mut body = Vec::new();
                        self.seq(target, join, &mut body, false);
                        placed.push(target);
                        arms.push((vec![value as u64], body));
                    }
                    let default = default.map(|d| {
                        let mut body = Vec::new();
                        self.seq(d, join, &mut body, false);
                        body
                    });
                    if let Some(depth) = self.switches.last_mut() {
                        *depth -= 1;
                    }
                    out.push(Node::Switch(value, arms, default));
                    match join {
                        Some(j) => b = j,
                        None => return,
                    }
                }
                Exit::Jump(t) => b = *t,
                Exit::Return(e) => {
                    out.push(Node::Return(e.clone()));
                    return;
                }
                Exit::TailCall(e) => {
                    out.push(Node::Tail(e.clone()));
                    return;
                }
                Exit::Stop => {
                    out.push(Node::Unreachable);
                    return;
                }
                Exit::Branch(c, t, e) => {
                    let (c, t, e) = (c.clone(), *t, *e);
                    let join = self.join(b);
                    let mut then = Vec::new();
                    self.seq(t, join, &mut then, false);
                    let mut els = Vec::new();
                    self.seq(e, join, &mut els, false);
                    out.push(Node::If(c, then, els));
                    match join {
                        Some(j) => b = j,
                        None => return,
                    }
                }
            }
        }
    }
}

pub fn structure(f: &Function) -> (Vec<Node>, HashSet<usize>) {
    let n = f.blocks.len();
    let idom = dominators(n, f.entry, &f.succ, &f.pred);
    let ipdom = post_dominators(f);
    let loops = find_loops(f, &idom, &ipdom);
    if std::env::var_os("LEONIDA_DECOMPILE_DUMP").is_some() {
        for b in 0..n {
            eprintln!(
                "#{b} {:x} succ {:?} idom {:?} ipdom {:?} loop {:?}",
                f.blocks[b].start,
                f.succ[b],
                idom[b],
                ipdom[b],
                loops[b]
                    .as_ref()
                    .map(|l| (l.follow, (0..n).filter(|x| l.body[*x]).collect::<Vec<_>>()))
            );
        }
    }
    let mut s = Structurer {
        f,
        ipdom,
        loops,
        active: Vec::new(),
        switches: vec![0],
        emitted: vec![false; n],
        gotos: HashSet::new(),
    };
    let mut out = Vec::new();
    s.seq(f.entry, None, &mut out, false);
    while let Some(b) = (0..n).find(|b| !s.emitted[*b] && idom[*b].is_some()) {
        s.active.clear();
        s.seq(b, None, &mut out, false);
    }
    let gotos = s.gotos;
    (tidy(out, &gotos), gotos)
}

/// Whether the guard `guard` is the loop condition `again` evaluated with the variable the preceding statement set.
fn guards_after_init(previous: Option<&Node>, guard: &Expr, again: &Expr) -> bool {
    let Some(Node::Stmt(Stmt::Def(loc, _, value))) = previous else {
        return false;
    };
    if value.has_load() || value.has_side_effects() {
        return false;
    }
    let substituted = simplify(again.clone().map(&mut |x| match x {
        Expr::Var(l, _) if l == *loc => value.clone(),
        other => other,
    }));
    substituted == *guard
}

fn has_break(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| match n {
        Node::Break => true,
        Node::If(_, a, b) => has_break(a) || has_break(b),
        _ => false,
    })
}

pub fn ends_in_jump(nodes: &[Node]) -> bool {
    if let Some(Node::Loop(body)) = nodes.last() {
        return !has_break(body);
    }
    matches!(
        nodes.last(),
        Some(
            Node::Return(_)
                | Node::Tail(_)
                | Node::Break
                | Node::Continue
                | Node::Goto(_)
                | Node::Unreachable
        )
    )
}

pub fn has_continue(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| match n {
        Node::Continue => true,
        Node::If(_, a, b) => has_continue(a) || has_continue(b),
        _ => false,
    })
}

fn tidy(nodes: Vec<Node>, gotos: &HashSet<usize>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for node in nodes {
        let reachable = matches!(&node, Node::Label(b) if gotos.contains(b));
        if ends_in_jump(&out) && !reachable {
            continue;
        }
        match node {
            Node::Label(b) if !gotos.contains(&b) => {}
            Node::If(c, then, els) => {
                let (mut c, mut then, mut els) = (c, tidy(then, gotos), tidy(els, gotos));
                if then.is_empty() && els.is_empty() {
                    if c.has_side_effects() {
                        out.push(Node::Stmt(Stmt::Eval(c)));
                    }
                    continue;
                }
                if then.is_empty() {
                    c = negate(c);
                    std::mem::swap(&mut then, &mut els);
                }
                if els.is_empty()
                    && let [Node::DoWhile(_, again)] = then.as_slice()
                    && !c.has_side_effects()
                    && (*again == c || guards_after_init(out.last(), &c, again))
                {
                    let Some(Node::DoWhile(body, again)) = then.pop() else {
                        unreachable!()
                    };
                    out.push(Node::While(again, body));
                    continue;
                }
                if !els.is_empty() && ends_in_jump(&then) {
                    out.push(Node::If(c, then, Vec::new()));
                    out.extend(els);
                } else if !els.is_empty()
                    && ends_in_jump(&els)
                    && !ends_in_jump(&then)
                    && els.len() < then.len()
                {
                    out.push(Node::If(negate(c), els, Vec::new()));
                    out.extend(then);
                } else {
                    out.push(Node::If(c, then, els));
                }
            }
            Node::Switch(value, arms, default) => {
                let arms = arms
                    .into_iter()
                    .map(|(values, body)| (values, tidy(body, gotos)))
                    .collect();
                out.push(Node::Switch(value, arms, default.map(|d| tidy(d, gotos))));
            }
            Node::Loop(body) => {
                let mut body = tidy(body, gotos);
                if matches!(body.last(), Some(Node::Continue)) {
                    body.pop();
                }
                let leading_break = matches!(body.first(), Some(Node::If(_, t, e)) if e.is_empty() && matches!(t.as_slice(), [Node::Break]));
                let trailing_break = matches!(body.last(), Some(Node::If(_, t, e)) if e.is_empty() && matches!(t.as_slice(), [Node::Break]));
                let continue_then_break = body.len() >= 2
                    && matches!(body.last(), Some(Node::Break))
                    && matches!(&body[body.len() - 2], Node::If(_, t, e) if e.is_empty() && matches!(t.as_slice(), [Node::Continue]))
                    && !has_continue(&body[..body.len() - 2]);
                if continue_then_break {
                    body.pop();
                    let Some(Node::If(c, ..)) = body.pop() else {
                        unreachable!()
                    };
                    out.push(if body.is_empty() {
                        Node::While(c, body)
                    } else {
                        Node::DoWhile(body, c)
                    });
                } else if leading_break {
                    let Node::If(c, ..) = body.remove(0) else {
                        unreachable!()
                    };
                    out.push(Node::While(negate(c), body));
                } else if trailing_break && !has_continue(&body) {
                    let Some(Node::If(c, ..)) = body.pop() else {
                        unreachable!()
                    };
                    out.push(Node::DoWhile(body, negate(c)));
                } else {
                    out.push(Node::Loop(body));
                }
            }
            other => out.push(other),
        }
    }
    out
}
