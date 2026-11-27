//! Turns x86-64 machine code into C-like pseudocode for reading. The output is a reading aid, not
//! compilable source: types are guessed from access widths and register names stand for variables.

mod cfg;
mod expr;
mod lift;
mod simplify;
mod structure;
mod types;
mod vars;

use expr::{Expr, Loc, Printer, register_name, type_name};
use lift::{Function, Stmt};
use structure::Node;
use types::stack_name;

pub enum Symbol {
    Import(String),
    Function,
    Text(String),
    Data,
    Unknown,
}

pub trait Program {
    fn image_base(&self) -> u64;
    /// Bytes from `va` to the end of its section, if `va` is in code.
    fn code(&self, va: u64) -> Option<&[u8]>;
    /// Bytes from `va` to the end of its section, in any section.
    fn bytes(&self, va: u64) -> Option<&[u8]>;
    fn symbol(&self, va: u64) -> Symbol;
    /// Whether a jump from the function at `entry` to `target` stays inside that function.
    fn inside(&self, entry: u64, target: u64) -> bool;
    /// A name for the function or data at `va`, if the caller knows one.
    fn name(&self, _va: u64) -> Option<String> {
        None
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("there is no code at the address")]
    NoCode,
    #[error("the function is too large to decompile")]
    TooLarge,
}

/// Limits that keep one function from holding up a run over the whole executable.
#[derive(Clone, Debug)]
pub struct Options {
    /// Functions with more basic blocks than this become a stub.
    pub max_blocks: usize,
    /// Time after which simplification stops improving the function.
    pub max_time: std::time::Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_blocks: 4000,
            max_time: std::time::Duration::from_secs(5),
        }
    }
}

pub fn decompile(program: &dyn Program, entry: u64) -> Result<String, Error> {
    decompile_with(program, entry, &Options::default())
}

/// Like `decompile`, but with explicit limits. It never panics: an internal failure yields a stub.
pub fn decompile_with(
    program: &dyn Program,
    entry: u64,
    options: &Options,
) -> Result<String, Error> {
    let title = || {
        program
            .name(entry)
            .unwrap_or_else(|| format!("sub_{:08x}", entry.wrapping_sub(program.image_base())))
    };
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decompile_inner(program, entry, options)
    }));
    match run {
        Ok(result) => result,
        Err(_) => Ok(format!(
            "void {}() {{
    /* the decompiler failed on this function */
}}
",
            title()
        )),
    }
}

fn decompile_inner(program: &dyn Program, entry: u64, options: &Options) -> Result<String, Error> {
    let started = std::time::Instant::now();
    let deadline = started + options.max_time;
    let raw = cfg::discover(program, entry)?;
    if !raw.iter().any(|b| b.start == entry) {
        return Err(Error::NoCode);
    }
    if raw.len() > options.max_blocks {
        let title = program
            .name(entry)
            .unwrap_or_else(|| format!("sub_{:08x}", entry - program.image_base()));
        return Ok(format!(
            "void {title}() {{
    /* too large: {} blocks */
}}
",
            raw.len()
        ));
    }
    let mut function = lift::lift(program, entry, &raw);
    let registers = simplify::parameters(&mut function);
    let dump = |f: &Function, label: &str| {
        if std::env::var_os("LEONIDA_DECOMPILE_DUMP").is_some() {
            for (b, block) in f.blocks.iter().enumerate() {
                eprintln!("{label} #{b} {:x} succ {:?}", block.start, f.succ[b]);
                for s in &block.stmts {
                    eprintln!("    {s:?}");
                }
                eprintln!("    -> {:?}", block.exit);
            }
        }
    };
    dump(&function, "lifted");
    simplify::run(&mut function, Some(deadline));
    simplify::merge_conditions(&mut function);
    let vars = vars::rename(&mut function, options.max_blocks).unwrap_or_default();
    dump(&function, "simplified");
    let (nodes, _) = structure::structure(&function);
    let title = program
        .name(entry)
        .unwrap_or_else(|| format!("sub_{:08x}", entry - function.base));
    let count = parameter_count(&function, &vars, registers);
    let facts = types::infer(&function, &vars, count);
    Ok(Writer::new(&function, &vars, &facts).function(&title, &nodes))
}

/// Register arguments that were read plus the stack arguments the code touches.
fn parameter_count(f: &Function, vars: &[vars::VarInfo], registers: u8) -> u8 {
    let mut count = registers;
    for v in vars {
        count = count.max(v.param);
    }
    for block in &f.blocks {
        let mut note = |e: &Expr| {
            e.visit(&mut |x| match x {
                Expr::Var(Loc::Stack(o), _) => {
                    if let Some(n) = types::stack_param(*o) {
                        count = count.max(n);
                    }
                }
                Expr::Param(n, ..) => count = count.max(*n),
                _ => {}
            })
        };
        block.stmts.iter().flat_map(Stmt::exprs).for_each(&mut note);
        if let Some(e) = block.exit.expr() {
            note(e);
        }
    }
    count
}

struct Writer<'f> {
    f: &'f Function,
    vars: &'f [vars::VarInfo],
    facts: &'f types::Facts,
    text: String,
    depth: usize,
}

impl<'f> Writer<'f> {
    fn new(f: &'f Function, vars: &'f [vars::VarInfo], facts: &'f types::Facts) -> Self {
        Self {
            f,
            vars,
            facts,
            text: String::new(),
            depth: 0,
        }
    }

    fn expr(&self, e: &Expr) -> String {
        Printer { facts: self.facts }.expr(e)
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.depth {
            self.text.push_str("    ");
        }
        self.text.push_str(text);
        self.text.push('\n');
    }

    fn label(&self, b: usize) -> String {
        format!("L_{:x}", self.f.blocks[b].start - self.f.base)
    }

    fn function(mut self, title: &str, nodes: &[Node]) -> String {
        let params: Vec<String> = self
            .facts
            .params
            .iter()
            .map(|p| {
                let space = if p.decl.ends_with('*') { "" } else { " " };
                format!("{}{space}{}", p.decl, p.name)
            })
            .collect();
        let returns = match self.f.returns {
            None => "void".to_string(),
            Some((loc, w)) => type_name(w, loc.is_vector() && matches!(w, 4 | 8)),
        };
        for definition in self.facts.definitions() {
            self.text.push_str(&definition);
            self.text.push('\n');
        }
        self.line(&format!("{returns} {title}({}) {{", params.join(", ")));
        self.depth = 1;
        self.declarations();
        self.nodes(nodes);
        self.depth = 0;
        self.line("}");
        self.text
    }

    fn declarations(&mut self) {
        let mut any = false;
        for (offset, decl) in &self.facts.locals {
            let name = stack_name(*offset);
            self.line(&format!("{};", decl.replace("{}", &name)));
            any = true;
        }
        let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
        for (n, ty) in self.facts.vars.iter().enumerate() {
            if self.vars[n].param != 0 {
                continue;
            }
            let name = self.facts.var_name(n as u32);
            match groups.iter_mut().find(|(d, _)| *d == ty.decl) {
                Some((_, names)) => names.push(name),
                None => groups.push((&ty.decl, vec![name])),
            }
        }
        for (decl, names) in groups {
            let (base, stars) = match decl.strip_suffix('*') {
                Some(base) => (base.trim_end(), "*"),
                None => (decl, ""),
            };
            let names: Vec<String> = names.iter().map(|n| format!("{stars}{n}")).collect();
            self.line(&format!("{base} {};", names.join(", ")));
            any = true;
        }
        if any {
            self.text.push('\n');
        }
    }

    fn stmt(&self, stmt: &Stmt) -> Option<String> {
        let assign = |lhs: String, value: &Expr, target: &Expr| {
            if let Expr::Binary(op, a, b) = value
                && **a == *target
                && matches!(*op, "+" | "-" | "*" | "&" | "|" | "^" | "<<" | ">>")
            {
                return format!("{lhs} {op}= {};", self.expr(b));
            }
            format!("{lhs} = {};", self.expr(value))
        };
        Some(match stmt {
            Stmt::Def(Loc::Flags, _, e) => format!("flags = {};", self.expr(e)),
            Stmt::Def(loc, w, e) => {
                let target = Expr::Var(*loc, *w);
                let lhs = match loc {
                    Loc::Reg(r) => register_name(*r, *w),
                    Loc::Var(n) => self.facts.var_name(*n),
                    _ => self.expr(&target),
                };
                assign(lhs, e, &target)
            }
            Stmt::Store(address, w, float, value) => {
                let target = Expr::Load(Box::new(address.clone()), *w, *float);
                assign(self.expr(&target), value, &target)
            }
            Stmt::Eval(e) => format!("{};", self.expr(e)),
            Stmt::Asm(text) => format!("asm(\"{text}\");"),
            Stmt::Clobber(_) => return None,
        })
    }

    /// `init; while (c) { body; step }` as the parts of a `for`, when the loop is driven by one variable.
    fn for_parts(&self, nodes: &[Node], k: usize) -> Option<(String, String, String)> {
        let (Node::Stmt(init @ Stmt::Def(loc, ..)), Some(Node::While(c, body))) =
            (&nodes[k], nodes.get(k + 1))
        else {
            return None;
        };
        let Some(Node::Stmt(step @ Stmt::Def(step_loc, ..))) = body.last() else {
            return None;
        };
        if step_loc != loc
            || !c.reads().iter().any(|(l, _)| l == loc)
            || body[..body.len() - 1]
                .iter()
                .any(|n| structure::has_continue(std::slice::from_ref(n)))
            || matches!(loc, Loc::Flags)
        {
            return None;
        }
        let text = |s: &Stmt| self.stmt(s).map(|t| t.trim_end_matches(';').to_string());
        Some((text(init)?, self.expr(c), text(step)?))
    }

    fn nodes(&mut self, nodes: &[Node]) {
        let mut k = 0;
        while k < nodes.len() {
            let node = &nodes[k];
            k += 1;
            if let Some((init, cond, step)) = self.for_parts(nodes, k - 1) {
                let Some(Node::While(_, body)) = nodes.get(k) else {
                    unreachable!()
                };
                k += 1;
                self.line(&format!("for ({init}; {cond}; {step}) {{"));
                self.depth += 1;
                self.nodes(&body[..body.len() - 1]);
                self.depth -= 1;
                self.line("}");
                continue;
            }
            match node {
                Node::Stmt(stmt) => {
                    if let Some(text) = self.stmt(stmt) {
                        self.line(&text);
                    }
                }
                Node::If(c, then, els) => {
                    let mut header = format!("if ({}) {{", self.expr(c));
                    let (mut then, mut els) = (then, els);
                    loop {
                        self.line(&header);
                        self.depth += 1;
                        self.nodes(then);
                        self.depth -= 1;
                        match els.as_slice() {
                            [] => {
                                self.line("}");
                                break;
                            }
                            [Node::If(c2, t2, e2)] => {
                                header = format!("}} else if ({}) {{", self.expr(c2));
                                then = t2;
                                els = e2;
                            }
                            _ => {
                                self.line("} else {");
                                self.depth += 1;
                                self.nodes(els);
                                self.depth -= 1;
                                self.line("}");
                                break;
                            }
                        }
                    }
                }
                Node::Loop(body) => self.block("while (true) {", body, "}"),
                Node::While(c, body) => {
                    let header = format!("while ({}) {{", self.expr(c));
                    self.block(&header, body, "}");
                }
                Node::DoWhile(body, c) => {
                    let footer = format!("}} while ({});", self.expr(c));
                    self.block("do {", body, &footer);
                }
                Node::Switch(value, cases, default) => {
                    let header = format!("switch ({}) {{", self.expr(value));
                    self.line(&header);
                    for (values, body) in cases {
                        for v in values {
                            self.line(&format!("case {v}:"));
                        }
                        self.case(body);
                    }
                    if let Some(body) = default {
                        self.line("default:");
                        self.case(body);
                    }
                    self.line("}");
                }
                Node::Break => self.line("break;"),
                Node::Continue => self.line("continue;"),
                Node::Goto(b) => {
                    let text = format!("goto {};", self.label(*b));
                    self.line(&text);
                }
                Node::Label(b) => {
                    let text = format!("{}:", self.label(*b));
                    self.depth -= 1;
                    self.line(&text);
                    self.depth += 1;
                }
                Node::Return(None) => self.line("return;"),
                Node::Return(Some(e)) | Node::Tail(e) => {
                    let text = format!("return {};", self.expr(e));
                    self.line(&text);
                }
                Node::Unreachable => self.line("unreachable();"),
            }
        }
    }

    fn case(&mut self, body: &[Node]) {
        self.depth += 1;
        self.nodes(body);
        if !structure::ends_in_jump(body) {
            self.line("break;");
        }
        self.depth -= 1;
    }

    fn block(&mut self, header: &str, body: &[Node], footer: &str) {
        self.line(header);
        self.depth += 1;
        self.nodes(body);
        self.depth -= 1;
        self.line(footer);
    }
}

#[cfg(test)]
mod tests;
