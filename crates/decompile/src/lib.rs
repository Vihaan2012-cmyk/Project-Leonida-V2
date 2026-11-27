//! Turns x86-64 machine code into C-like pseudocode for reading. The output is a reading aid, not
//! compilable source: types are guessed from access widths and register names stand for variables.

mod cfg;
mod expr;
mod lift;
mod simplify;
mod structure;

use expr::{Expr, Loc, Printer, register_name};
use lift::{Function, Stmt};
use structure::Node;

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

pub fn decompile(program: &dyn Program, entry: u64) -> Result<String, Error> {
    let raw = cfg::discover(program, entry)?;
    if !raw.iter().any(|b| b.start == entry) {
        return Err(Error::NoCode);
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
    simplify::run(&mut function);
    simplify::merge_conditions(&mut function);
    dump(&function, "simplified");
    let (nodes, _) = structure::structure(&function);
    let title = program
        .name(entry)
        .unwrap_or_else(|| format!("sub_{:08x}", entry - function.base));
    Ok(Writer::new(&function).function(&title, registers, &nodes))
}

fn stack_name(o: i64) -> String {
    if o < 0 {
        format!("var_{:x}", -o)
    } else if o >= 0x28 && o % 8 == 0 {
        format!("a{}", 5 + (o - 0x28) / 8)
    } else {
        format!("home_{o:x}")
    }
}

struct Writer<'f> {
    f: &'f Function,
    text: String,
    depth: usize,
}

impl<'f> Writer<'f> {
    fn new(f: &'f Function) -> Self {
        Self {
            f,
            text: String::new(),
            depth: 0,
        }
    }

    fn expr(&self, e: &Expr) -> String {
        Printer {
            stack_name: &stack_name,
        }
        .expr(e)
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

    fn function(mut self, title: &str, registers: u8, nodes: &[Node]) -> String {
        let mut stack_params = 0;
        for block in &self.f.blocks {
            let mut note = |e: &Expr| {
                e.visit(&mut |x| {
                    if let Expr::Var(Loc::Stack(o), _) = x
                        && *o >= 0x28
                        && o % 8 == 0
                    {
                        stack_params = stack_params.max(5 + (o - 0x28) / 8);
                    }
                })
            };
            block.stmts.iter().flat_map(Stmt::exprs).for_each(&mut note);
            if let Some(e) = block.exit.expr() {
                note(e);
            }
        }
        let count = i64::from(registers).max(stack_params);
        let params: Vec<String> = (1..=count).map(|n| format!("u64 a{n}")).collect();
        let returns = match self.f.returns {
            None => "void",
            Some(1) => "u8",
            Some(2) => "u16",
            Some(4) => "u32",
            Some(_) => "u64",
        };
        self.line(&format!("{returns} {title}({}) {{", params.join(", ")));
        self.depth = 1;
        self.nodes(nodes);
        self.depth = 0;
        self.line("}");
        self.text
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

    fn nodes(&mut self, nodes: &[Node]) {
        for node in nodes {
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
