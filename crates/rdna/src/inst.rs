use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Sgpr { reg: u8, count: u8 },
    Vgpr { reg: u8, count: u8 },
    VgprList(Vec<u8>),
    ScalarList(Vec<String>),
    Named(String),
    Int(i64),
    Literal(u32),
    Float(&'static str),
    Attr { attr: u8, chan: u8 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    pub operand: Operand,
    pub neg: bool,
    pub abs: bool,
    pub sext: bool,
}

impl Arg {
    pub fn new(operand: Operand) -> Arg {
        Arg {
            operand,
            neg: false,
            abs: false,
            sext: false,
        }
    }

    pub fn named(text: impl Into<String>) -> Arg {
        Arg::new(Operand::Named(text.into()))
    }

    fn render(&self, hex_imm: bool) -> String {
        let mut text = match &self.operand {
            Operand::Sgpr { reg, count } => register('s', *reg, *count),
            Operand::Vgpr { reg, count } => register('v', *reg, *count),
            Operand::VgprList(regs) => {
                let parts: Vec<String> = regs.iter().map(|reg| format!("v{reg}")).collect();
                format!("[{}]", parts.join(","))
            }
            Operand::ScalarList(items) => format!("[{}]", items.join(",")),
            Operand::Named(text) => text.clone(),
            Operand::Int(value) if hex_imm || *value >= 100 => format!("0x{value:x}"),
            Operand::Int(value) => value.to_string(),
            Operand::Literal(value) => format!("0x{value:x}"),
            Operand::Float(text) => (*text).to_string(),
            Operand::Attr { attr, chan } => {
                format!("attr{attr}.{}", ['x', 'y', 'z', 'w'][*chan as usize & 3])
            }
        };
        if self.sext {
            text = format!("sext({text})");
        }
        if self.abs {
            text = format!("|{text}|");
        }
        if self.neg {
            text = match self.operand {
                Operand::Int(_) | Operand::Literal(_) | Operand::Float(_) => format!("neg({text})"),
                _ => format!("-{text}"),
            };
        }
        text
    }
}

fn register(prefix: char, reg: u8, count: u8) -> String {
    if count <= 1 {
        format!("{prefix}{reg}")
    } else {
        format!("{prefix}[{reg}:{}]", reg as u32 + count as u32 - 1)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inst {
    pub name: String,
    pub args: Vec<Arg>,
    pub mods: Vec<String>,
    pub pc: usize,
    pub bytes: Vec<u8>,
}

impl Inst {
    pub fn text(&self) -> String {
        let hex_imm = self.name == "s_ttracedata_imm";
        let mut head = self.name.clone();
        if self.name == "exp" && !self.args.is_empty() {
            let rest: Vec<String> = self.args[1..]
                .iter()
                .map(|arg| arg.render(hex_imm))
                .collect();
            let _ = write!(
                head,
                " {} {}",
                self.args[0].render(hex_imm),
                rest.join(", ")
            );
        } else if !self.args.is_empty() {
            let args: Vec<String> = self.args.iter().map(|arg| arg.render(hex_imm)).collect();
            let separator = if self.name == "s_waitcnt" { " " } else { ", " };
            let _ = write!(head, " {}", args.join(separator));
        }
        if !self.mods.is_empty() {
            let _ = write!(head, " {}", self.mods.join(" "));
        }
        head
    }

    pub fn listing_line(&self) -> String {
        let bytes: Vec<String> = self
            .bytes
            .iter()
            .map(|byte| format!("0x{byte:02x}"))
            .collect();
        format!("\t{:<40}; encoding: [{}]", self.text(), bytes.join(","))
    }
}
