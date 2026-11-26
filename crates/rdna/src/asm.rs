use std::collections::HashMap;
use std::sync::OnceLock;

use crate::opcodes as op;
use crate::table::{Field, OpInfo};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct AsmError {
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Sop1,
    Sop2,
    Sopc,
    Sopk,
    Sopp,
    Vop1,
    Vop2,
    Vopc,
    Vop3,
    Vop3b,
    Ds,
    Flat(u32),
    Mubuf,
    Smem,
    Vintrp,
    Vop3p,
    Mimg,
    Mtbuf,
}

struct Entry {
    family: Family,
    opcode: u32,
    info: &'static OpInfo,
}

fn index() -> &'static HashMap<&'static str, Vec<Entry>> {
    static INDEX: OnceLock<HashMap<&'static str, Vec<Entry>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map: HashMap<&'static str, Vec<Entry>> = HashMap::new();
        let tables: [(Family, &'static [(u16, OpInfo)]); 19] = [
            (Family::Vop3p, op::VOP3P),
            (Family::Mimg, op::MIMG),
            (Family::Mtbuf, op::MTBUF),
            (Family::Sop1, op::SOP1),
            (Family::Sop2, op::SOP2),
            (Family::Sopc, op::SOPC),
            (Family::Sopk, op::SOPK),
            (Family::Sopp, op::SOPP),
            (Family::Vop1, op::VOP1),
            (Family::Vop2, op::VOP2),
            (Family::Vopc, op::VOPC),
            (Family::Vop3b, op::VOP3B),
            (Family::Vop3, op::VOP3),
            (Family::Ds, op::DS),
            (Family::Flat(0), op::FLAT),
            (Family::Flat(1), op::SCRATCH),
            (Family::Flat(2), op::GLOBAL),
            (Family::Mubuf, op::MUBUF),
            (Family::Smem, op::SMEM),
        ];
        for (family, table) in tables {
            for (opcode, info) in table {
                map.entry(info.name).or_default().push(Entry {
                    family,
                    opcode: u32::from(*opcode),
                    info,
                });
            }
        }
        for (opcode, info) in op::VINTRP {
            map.entry(info.name).or_default().push(Entry {
                family: Family::Vintrp,
                opcode: u32::from(*opcode),
                info,
            });
        }
        map
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Sgpr(u32, u32),
    Vgpr(u32, u32),
    Code(u32),
    Int(i64),
    Float(u32),
    Off,
    Attr(u32, u32),
    VList(Vec<u32>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Opnd {
    value: Value,
    neg: bool,
    abs: bool,
    sext: bool,
}

pub fn assemble(source: &str, wave32: bool) -> Result<Vec<u8>, AsmError> {
    let mut out = Vec::new();
    for (number, raw) in source.lines().enumerate() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words = assemble_line(line, wave32).map_err(|message| AsmError {
            line: number + 1,
            message: format!("{message}: {line}"),
        })?;
        for word in words {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    Ok(out)
}

fn parse_reg(text: &str, prefix: char) -> Option<(u32, u32)> {
    let rest = text.strip_prefix(prefix)?;
    if let Some(range) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let (lo, hi) = range.split_once(':')?;
        let (lo, hi): (u32, u32) = (lo.trim().parse().ok()?, hi.trim().parse().ok()?);
        return (hi >= lo).then_some((lo, hi - lo + 1));
    }
    rest.parse().ok().map(|reg| (reg, 1))
}

fn parse_int(text: &str) -> Option<i64> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let value = match digits.strip_prefix("0x") {
        Some(hex) => i64::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    Some(if negative { -value } else { value })
}

fn parse_operand(text: &str) -> Result<Opnd, String> {
    let mut text = text.trim();
    let mut neg = false;
    let mut abs = false;
    let mut sext = false;
    if let Some(inner) = text.strip_prefix("neg(").and_then(|t| t.strip_suffix(')')) {
        neg = true;
        text = inner;
    }
    if let Some(rest) = text
        .strip_prefix('-')
        .filter(|rest| !rest.starts_with(|c: char| c.is_ascii_digit()))
    {
        neg = true;
        text = rest;
    }
    if let Some(inner) = text.strip_prefix('|').and_then(|t| t.strip_suffix('|')) {
        abs = true;
        text = inner;
    }
    if let Some(inner) = text.strip_prefix("sext(").and_then(|t| t.strip_suffix(')')) {
        sext = true;
        text = inner;
    }
    let value = match text {
        "vcc_lo" | "vcc" => Value::Code(106),
        "vcc_hi" => Value::Code(107),
        "m0" => Value::Code(124),
        "null" => Value::Code(125),
        "exec_lo" | "exec" => Value::Code(126),
        "exec_hi" => Value::Code(127),
        "src_vccz" => Value::Code(251),
        "src_execz" => Value::Code(252),
        "src_scc" => Value::Code(253),
        "src_lds_direct" => Value::Code(254),
        "off" => Value::Off,
        "0.5" => Value::Float(240),
        "-0.5" => Value::Float(241),
        "1.0" => Value::Float(242),
        "-1.0" => Value::Float(243),
        "2.0" => Value::Float(244),
        "-2.0" => Value::Float(245),
        "4.0" => Value::Float(246),
        "-4.0" => Value::Float(247),
        "0.15915494" => Value::Float(248),
        _ => {
            if let Some(items) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
                let items: Vec<&str> = items.split(',').map(str::trim).collect();
                if items.iter().all(|item| item.starts_with('v')) {
                    let regs = items
                        .iter()
                        .map(|item| item[1..].parse::<u32>())
                        .collect::<Result<_, _>>();
                    Value::VList(regs.map_err(|_| "bad register list")?)
                } else {
                    let first = items
                        .first()
                        .and_then(|item| item.strip_prefix('s'))
                        .ok_or("bad register list")?;
                    Value::Sgpr(
                        first.parse().map_err(|_| "bad register list")?,
                        items.len() as u32,
                    )
                }
            } else if let Some(rest) = text.strip_prefix("ttmp") {
                let first = rest
                    .trim_start_matches('[')
                    .split([':', ']'])
                    .next()
                    .unwrap_or(rest);
                Value::Code(108 + first.parse::<u32>().map_err(|_| "bad ttmp")?)
            } else if let Some(code) = text
                .strip_prefix("src_")
                .or_else(|| text.strip_prefix("sreg_"))
            {
                Value::Code(code.parse().map_err(|_| "bad raw source")?)
            } else if let Some((reg, count)) = parse_reg(text, 'v') {
                Value::Vgpr(reg, count)
            } else if let Some((reg, count)) = parse_reg(text, 's') {
                Value::Sgpr(reg, count)
            } else if let Some(attr) = text.strip_prefix("attr") {
                let (number, chan) = attr.split_once('.').ok_or("bad attribute")?;
                let chan = "xyzw".find(chan).ok_or("bad attribute channel")? as u32;
                Value::Attr(number.parse().map_err(|_| "bad attribute")?, chan)
            } else if let Some(value) = parse_int(text) {
                Value::Int(value)
            } else {
                Value::Text(text.to_string())
            }
        }
    };
    Ok(Opnd {
        value,
        neg,
        abs,
        sext,
    })
}

fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for ch in text.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

const FLAGS: [&str; 20] = [
    "glc",
    "slc",
    "dlc",
    "lds",
    "tfe",
    "offen",
    "idxen",
    "gds",
    "clamp",
    "done",
    "vm",
    "compr",
    "unorm",
    "r128",
    "lwe",
    "a16",
    "d16",
    "high",
    "row_mirror",
    "row_half_mirror",
];

fn is_modifier(token: &str) -> bool {
    FLAGS.contains(&token)
        || token.split_once(':').is_some_and(|(key, _)| {
            !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

fn split_mods(last: &str) -> (String, Vec<String>) {
    let tokens = top_level_tokens(last);
    match tokens.iter().position(|token| is_modifier(token)) {
        Some(first) => (tokens[..first].join(" "), tokens[first..].to_vec()),
        None => (last.to_string(), Vec::new()),
    }
}
fn top_level_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for ch in text.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ' ' if depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

struct Parsed {
    operands: Vec<Opnd>,
    mods: Vec<String>,
}

impl Parsed {
    fn has(&self, flag: &str) -> bool {
        self.mods.iter().any(|m| m == flag)
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.mods
            .iter()
            .find_map(|m| m.strip_prefix(key).and_then(|rest| rest.strip_prefix(':')))
    }

    fn number(&self, key: &str) -> Result<i64, String> {
        self.value(key).map_or(Ok(0), |text| {
            parse_int(text).ok_or_else(|| format!("bad {key}"))
        })
    }
}

fn parse(rest: &str) -> Result<Parsed, String> {
    let mut parts = split_top_level(rest);
    let mut mods = Vec::new();
    if let Some(last) = parts.pop() {
        let (operand, trailing) = split_mods(&last);
        mods = trailing;
        if !operand.is_empty() {
            parts.push(operand);
        }
    }
    let operands = parts
        .iter()
        .map(|part| parse_operand(part))
        .collect::<Result<_, _>>()?;
    Ok(Parsed { operands, mods })
}

fn src_code(opnd: &Opnd, literal: &mut Option<u32>) -> Result<u32, String> {
    let code = match &opnd.value {
        Value::Sgpr(reg, _) => *reg,
        Value::Vgpr(reg, _) => 256 + reg,
        Value::Code(code) | Value::Float(code) => *code,
        Value::Int(value) if (0..=64).contains(value) => 128 + *value as u32,
        Value::Int(value) if (-16..=-1).contains(value) => (192 - value) as u32,
        Value::Int(value) => {
            let value = *value as u32;
            if literal.is_some_and(|existing| existing != value) {
                return Err("two different literals".into());
            }
            *literal = Some(value);
            255
        }
        other => return Err(format!("{other:?} is not a source")),
    };
    Ok(code)
}

fn reg_of(opnd: &Opnd) -> Result<u32, String> {
    match &opnd.value {
        Value::Sgpr(reg, _) | Value::Vgpr(reg, _) => Ok(*reg),
        Value::Code(code) => Ok(*code),
        other => Err(format!("{other:?} is not a register")),
    }
}

fn pick(entries: &'static [Entry], suffix: &str) -> Option<&'static Entry> {
    let vector32 = |family| matches!(family, Family::Vop1 | Family::Vop2 | Family::Vopc);
    match suffix {
        "_e64" => entries
            .iter()
            .find(|e| matches!(e.family, Family::Vop3 | Family::Vop3b)),
        "_e32" => entries.iter().find(|e| vector32(e.family)),
        _ => entries
            .iter()
            .find(|e| !matches!(e.family, Family::Vop3 | Family::Vop3b))
            .or_else(|| entries.first()),
    }
}

fn assemble_line(line: &str, wave32: bool) -> Result<Vec<u32>, String> {
    let (name, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    if name == "exp" {
        return export(rest.trim());
    }
    let (base, suffix) = ["_e32", "_e64", "_dpp", "_sdwa"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix).map(|base| (base, *suffix)))
        .unwrap_or((name, ""));
    let base = if base == "v_mad_legacy_f32" {
        "v_fma_legacy_f32"
    } else {
        base
    };
    let entries = index()
        .get(base)
        .ok_or_else(|| format!("unknown instruction {name}"))?;
    let lookup_suffix = if matches!(suffix, "_dpp" | "_sdwa") {
        "_e32"
    } else {
        suffix
    };
    let entry = pick(entries.as_slice(), lookup_suffix)
        .ok_or_else(|| format!("no {suffix} form of {base}"))?;
    let parsed = parse(rest.trim())?;
    match entry.family {
        Family::Sop1 | Family::Sop2 | Family::Sopc | Family::Sopk => scalar(entry, &parsed),
        Family::Sopp => sopp(entry, &parsed),
        Family::Vop1 | Family::Vop2 | Family::Vopc => vector32(entry, &parsed, suffix),
        Family::Vop3 | Family::Vop3b => vop3(entry, &parsed, wave32),
        Family::Vop3p => vop3p(entry, &parsed),
        Family::Ds => ds(entry, &parsed),
        Family::Flat(segment) => flat(entry, segment, &parsed),
        Family::Mubuf => mubuf(entry, &parsed),
        Family::Mtbuf => mtbuf(entry, &parsed),
        Family::Mimg => mimg(entry, &parsed),
        Family::Smem => smem(entry, &parsed),
        Family::Vintrp => vintrp(entry, &parsed),
    }
}

fn bit_list(parsed: &Parsed, key: &str) -> u32 {
    parsed.value(key).map_or(0, |bits| {
        bits.trim_matches(['[', ']'])
            .split(',')
            .enumerate()
            .map(|(index, bit)| u32::from(bit.trim() == "1") << index)
            .sum()
    })
}

fn special_scalar(entry: &Entry, parsed: &Parsed) -> Option<Result<Vec<u32>, String>> {
    let hw = |index: usize| -> Result<u32, String> {
        match parsed.operands.get(index).map(|opnd| &opnd.value) {
            Some(Value::Text(text)) => {
                crate::names::parse_hwreg(text).ok_or_else(|| format!("bad {text}"))
            }
            _ => Err("expected hwreg(...)".into()),
        }
    };
    let reg = |index: usize| {
        parsed
            .operands
            .get(index)
            .ok_or("missing operand".to_string())
            .and_then(reg_of)
    };
    let sopk = |sdst: u32, simm16: u32| {
        0xB000_0000 | (entry.opcode << 23) | (sdst << 16) | (simm16 & 0xFFFF)
    };
    let result = match entry.info.name {
        "s_getreg_b32" => (|| Ok(vec![sopk(reg(0)?, hw(1)?)]))(),
        "s_setreg_b32" => (|| Ok(vec![sopk(reg(1)?, hw(0)?)]))(),
        "s_setreg_imm32_b32" => (|| match parsed.operands.get(1).map(|opnd| &opnd.value) {
            Some(Value::Int(value)) => Ok(vec![sopk(0, hw(0)?), *value as u32]),
            _ => Err("expected a literal".into()),
        })(),
        "s_call_b64" => (|| match parsed.operands.get(1).map(|opnd| &opnd.value) {
            Some(Value::Int(value)) => Ok(vec![sopk(reg(0)?, *value as u32)]),
            _ => Err("expected an offset".into()),
        })(),
        _ => return None,
    };
    Some(result)
}

fn with_literal(words: u32, literal: Option<u32>) -> Vec<u32> {
    let mut out = vec![words];
    out.extend(literal);
    out
}

fn scalar(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    if let Some(result) = special_scalar(entry, parsed) {
        return result;
    }
    let mut literal = None;
    let mut fields: HashMap<&str, u32> = HashMap::new();
    let mut operands = parsed.operands.iter();
    for spec in entry.info.spec {
        let opnd = operands.next().ok_or("missing operand")?;
        match spec.field {
            Field::Sdst => {
                fields.insert("sdst", reg_of(opnd)?);
            }
            Field::Ssrc0 => {
                fields.insert("ssrc0", src_code(opnd, &mut literal)?);
            }
            Field::Ssrc1 => {
                fields.insert("ssrc1", src_code(opnd, &mut literal)?);
            }
            Field::Simm16 => match opnd.value {
                Value::Int(value) => {
                    fields.insert("simm16", (value as u32) & 0xFFFF);
                }
                _ => return Err("simm16 must be a number".into()),
            },
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        }
    }
    let get = |key| fields.get(key).copied().unwrap_or(0);
    let word = match entry.family {
        Family::Sop1 => 0xBE80_0000 | (get("sdst") << 16) | (entry.opcode << 8) | get("ssrc0"),
        Family::Sop2 => {
            0x8000_0000
                | (entry.opcode << 23)
                | (get("sdst") << 16)
                | (get("ssrc1") << 8)
                | get("ssrc0")
        }
        Family::Sopc => 0xBF00_0000 | (entry.opcode << 16) | (get("ssrc1") << 8) | get("ssrc0"),
        _ => 0xB000_0000 | (entry.opcode << 23) | (get("sdst") << 16) | get("simm16"),
    };
    Ok(with_literal(word, literal))
}

fn sopp(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let simm16 = if entry.info.name == "s_waitcnt" {
        let mut counts = [63u32, 7, 63];
        let mut all: Vec<&Opnd> = parsed.operands.iter().collect();
        let extra: Vec<Opnd> = parsed
            .mods
            .iter()
            .map(|m| parse_operand(m))
            .collect::<Result<_, _>>()?;
        all.extend(extra.iter());
        for opnd in all {
            match &opnd.value {
                Value::Int(value) => {
                    return Ok(vec![
                        0xBF80_0000 | (entry.opcode << 16) | (*value as u32 & 0xFFFF),
                    ]);
                }
                Value::Text(text) => {
                    for part in text.split_whitespace() {
                        let (key, number) = part.split_once('(').ok_or("bad waitcnt")?;
                        let number: u32 = number
                            .trim_end_matches(')')
                            .parse()
                            .map_err(|_| "bad waitcnt")?;
                        let slot = ["vmcnt", "expcnt", "lgkmcnt"]
                            .iter()
                            .position(|k| *k == key)
                            .ok_or("bad waitcnt")?;
                        counts[slot] = number;
                    }
                }
                _ => return Err("bad waitcnt".into()),
            }
        }
        (counts[0] & 0xF) | (counts[1] << 4) | (counts[2] << 8) | ((counts[0] >> 4) << 14)
    } else {
        match parsed.operands.first().map(|opnd| &opnd.value) {
            None => 0,
            Some(Value::Int(value)) => (*value as u32) & 0xFFFF,
            Some(Value::Text(text)) => {
                crate::names::parse_sendmsg(text).ok_or_else(|| format!("bad {text}"))?
            }
            Some(_) => return Err("unsupported SOPP operand".into()),
        }
    };
    Ok(vec![0xBF80_0000 | (entry.opcode << 16) | simm16])
}

fn vector32(entry: &Entry, parsed: &Parsed, suffix: &str) -> Result<Vec<u32>, String> {
    let mut literal = None;
    let (mut vdst, mut src0, mut vsrc1) = (0, 0, 0);
    let mut sources: Vec<&Opnd> = Vec::new();
    let mut sdst = None;
    let mut operands = parsed.operands.iter();
    let cmpx = entry.info.name.starts_with("v_cmpx_");
    for spec in entry.info.spec {
        if spec.field == Field::OprExec
            || (spec.field == Field::Vcc && entry.family == Family::Vopc && cmpx)
        {
            continue;
        }
        let opnd = operands.next().ok_or("missing operand")?;
        match spec.field {
            Field::Vdst => vdst = reg_of(opnd)?,
            Field::Vcc if entry.family == Family::Vopc && suffix == "_sdwa" => {
                sdst = (opnd.value != Value::Code(106))
                    .then(|| reg_of(opnd))
                    .transpose()?;
            }
            Field::Vcc => {}
            Field::Src0 => {
                sources.push(opnd);
                if suffix.is_empty() || suffix == "_e32" {
                    src0 = src_code(opnd, &mut literal)?;
                }
            }
            Field::Vsrc1 => {
                sources.push(opnd);
                if !matches!(opnd.value, Value::Vgpr(..)) && suffix == "_sdwa" {
                    vsrc1 = src_code(opnd, &mut literal)?;
                } else {
                    vsrc1 = reg_of(opnd)?;
                }
            }
            Field::Simm32 | Field::Literal => match opnd.value {
                Value::Int(value) => literal = Some(value as u32),
                _ => return Err("literal must be a number".into()),
            },
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        }
    }
    let extension = match suffix {
        "_dpp" => Some(dpp(parsed, &sources, &mut src0)?),
        "_sdwa" => Some(sdwa(entry, parsed, &sources, sdst, &mut src0)?),
        _ => None,
    };
    let word = match entry.family {
        Family::Vop1 => 0x7E00_0000 | (vdst << 17) | (entry.opcode << 9) | src0,
        Family::Vopc => 0x7C00_0000 | (entry.opcode << 17) | (vsrc1 << 9) | src0,
        _ => (entry.opcode << 25) | (vdst << 17) | (vsrc1 << 9) | src0,
    };
    match extension {
        Some(w1) => Ok(vec![word, w1]),
        None => Ok(with_literal(word, literal)),
    }
}

fn dpp(parsed: &Parsed, sources: &[&Opnd], src0: &mut u32) -> Result<u32, String> {
    let first = sources.first().ok_or("missing source")?;
    let reg = reg_of(first)?;
    if let Some(lanes) = parsed.value("dpp8") {
        *src0 = if parsed.has("fi:1") || parsed.value("fi") == Some("1") {
            234
        } else {
            233
        };
        let lanes = lanes
            .trim_matches(['[', ']'])
            .split(',')
            .enumerate()
            .map(|(i, lane)| lane.trim().parse::<u32>().map(|lane| (lane & 7) << (3 * i)))
            .sum::<Result<u32, _>>()
            .map_err(|_| "bad dpp8")?;
        return Ok((lanes << 8) | reg);
    }
    *src0 = 250;
    let ctrl = if let Some(perm) = parsed.value("quad_perm") {
        perm.trim_matches(['[', ']'])
            .split(',')
            .enumerate()
            .map(|(i, lane)| lane.trim().parse::<u32>().map(|lane| (lane & 3) << (2 * i)))
            .sum::<Result<u32, _>>()
            .map_err(|_| "bad quad_perm")?
    } else if parsed.has("row_mirror") {
        0x140
    } else if parsed.has("row_half_mirror") {
        0x141
    } else {
        let forms = [
            ("row_shl", 0x100),
            ("row_shr", 0x110),
            ("row_ror", 0x120),
            ("row_share", 0x150),
            ("row_xmask", 0x160),
        ];
        let found = forms
            .iter()
            .find_map(|(key, base)| parsed.value(key).map(|n| (n, *base)));
        match found {
            Some((amount, base)) => base + amount.parse::<u32>().map_err(|_| "bad dpp control")?,
            None => parse_int(parsed.value("dpp_ctrl").ok_or("missing dpp control")?)
                .ok_or("bad dpp_ctrl")? as u32,
        }
    };
    let mask = |key: &str| parsed.value(key).and_then(parse_int).unwrap_or(0xF) as u32;
    let flag = |key: &str| u32::from(parsed.value(key) == Some("1"));
    let mut modifiers = 0;
    for (index, opnd) in sources.iter().enumerate().take(2) {
        modifiers |=
            (u32::from(opnd.neg) << (2 * index)) | (u32::from(opnd.abs) << (2 * index + 1));
    }
    Ok((mask("row_mask") << 28)
        | (mask("bank_mask") << 24)
        | (modifiers << 20)
        | (flag("bound_ctrl") << 19)
        | (flag("fi") << 18)
        | (ctrl << 8)
        | reg)
}

fn sel(parsed: &Parsed, key: &str) -> Result<u32, String> {
    match parsed.value(key) {
        None => Ok(6),
        Some(name) => crate::names::SDWA_SELS
            .iter()
            .position(|sel| *sel == name)
            .map(|p| p as u32)
            .ok_or_else(|| format!("bad {key}")),
    }
}

fn sdwa_source(opnd: &Opnd) -> Result<u32, String> {
    let (reg, is_sgpr) = match &opnd.value {
        Value::Vgpr(reg, _) => (*reg, 0),
        _ => {
            let mut literal = None;
            let code = src_code(opnd, &mut literal)?;
            if literal.is_some() {
                return Err("SDWA sources cannot be literals".into());
            }
            (code, 1)
        }
    };
    Ok((is_sgpr << 7)
        | (u32::from(opnd.abs) << 5)
        | (u32::from(opnd.neg) << 4)
        | (u32::from(opnd.sext) << 3)
        | (reg << 8))
}

fn sdwa(
    entry: &Entry,
    parsed: &Parsed,
    sources: &[&Opnd],
    sdst: Option<u32>,
    src0: &mut u32,
) -> Result<u32, String> {
    *src0 = 249;
    let first = sdwa_source(sources.first().ok_or("missing source")?)?;
    let mut w1 = ((first >> 8) & 0xFF) | ((first & 0xFF) << 16) | (sel(parsed, "src0_sel")? << 16);
    if let Some(second) = sources.get(1) {
        let second = sdwa_source(second)?;
        w1 |= ((second & 0xF8) << 24) | (sel(parsed, "src1_sel")? << 24);
    }
    if entry.family == Family::Vopc {
        if let Some(sdst) = sdst {
            w1 |= (1 << 15) | (sdst << 8);
        }
        return Ok(w1);
    }
    let unused = match parsed.value("dst_unused") {
        None => 0,
        Some(name) => crate::names::SDWA_UNUSED
            .iter()
            .position(|u| *u == name)
            .ok_or("bad dst_unused")? as u32,
    };
    let omod = match (parsed.value("mul"), parsed.value("div")) {
        (Some("2"), _) => 1,
        (Some("4"), _) => 2,
        (_, Some("2")) => 3,
        _ => 0,
    };
    Ok(w1
        | (omod << 14)
        | (u32::from(parsed.has("clamp")) << 13)
        | (unused << 11)
        | (sel(parsed, "dst_sel")? << 8))
}

fn vop3p(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let mut literal = None;
    let mut vdst = 0;
    let mut srcs = [0u32; 3];
    let (mut neg_lo, mut neg_hi) = (bit_list(parsed, "neg_lo"), bit_list(parsed, "neg_hi"));
    let mut count = 0;
    let mut operands = parsed.operands.iter();
    for spec in entry.info.spec {
        let opnd = operands.next().ok_or("missing operand")?;
        let slot = match spec.field {
            Field::Vdst => {
                vdst = reg_of(opnd)?;
                continue;
            }
            Field::Src0 => 0,
            Field::Src1 => 1,
            Field::Src2 => 2,
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        };
        count += 1;
        srcs[slot] = src_code(opnd, &mut literal)?;
        neg_lo |= u32::from(opnd.neg) << slot;
        neg_hi |= u32::from(opnd.abs) << slot;
    }
    let is_mix = entry.info.name.starts_with("v_fma_mix");
    let op_sel = bit_list(parsed, "op_sel");
    let default_hi = if is_mix { 0 } else { (1 << count) - 1 };
    let op_sel_hi = if parsed.value("op_sel_hi").is_some() {
        bit_list(parsed, "op_sel_hi")
    } else {
        default_hi
    };
    let w0 = 0xCC00_0000
        | (entry.opcode << 16)
        | (u32::from(parsed.has("clamp")) << 15)
        | (((op_sel_hi >> 2) & 1) << 14)
        | (op_sel << 11)
        | (neg_hi << 8)
        | vdst;
    let w1 = (neg_lo << 29) | ((op_sel_hi & 3) << 27) | (srcs[2] << 18) | (srcs[1] << 9) | srcs[0];
    let mut out = vec![w0, w1];
    out.extend(literal);
    Ok(out)
}

fn buffer_fields(entry: &Entry, parsed: &Parsed) -> Result<(u32, u32, u32, u32), String> {
    let (mut vdata, mut vaddr, mut srsrc, mut soffset) = (0, 0, 0, 0);
    let mut operands = parsed.operands.iter();
    let mut literal = None;
    for spec in entry.info.spec {
        let opnd = operands.next().ok_or("missing operand")?;
        match spec.field {
            Field::Vdata => vdata = reg_of(opnd)?,
            Field::Vaddr => {
                vaddr = if opnd.value == Value::Off {
                    0
                } else {
                    reg_of(opnd)?
                }
            }
            Field::Srsrc => srsrc = reg_of(opnd)? / 4,
            Field::Soffset => soffset = src_code(opnd, &mut literal)?,
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        }
    }
    if literal.is_some() {
        return Err("buffer soffset cannot be a literal".into());
    }
    Ok((vdata, vaddr, srsrc, soffset))
}

fn mtbuf(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let (vdata, vaddr, srsrc, soffset) = buffer_fields(entry, parsed)?;
    let format = match parsed.value("format") {
        None => 1,
        Some(text) => match text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            Some(name) => {
                let full = name.replace("BUF_FMT_", "GFX10_FORMAT_");
                crate::formats::FORMATS
                    .iter()
                    .find(|(_, n)| *n == full)
                    .map(|(code, _)| u32::from(*code))
                    .ok_or_else(|| format!("unknown format {name}"))?
            }
            None => text.parse().map_err(|_| "bad format")?,
        },
    };
    let w0 = 0xE800_0000
        | ((format & 0x7F) << 19)
        | ((entry.opcode & 7) << 16)
        | (u32::from(parsed.has("dlc")) << 15)
        | (u32::from(parsed.has("glc")) << 14)
        | (u32::from(parsed.has("idxen")) << 13)
        | (u32::from(parsed.has("offen")) << 12)
        | ((parsed.number("offset")? as u32) & 0xFFF);
    let w1 = (soffset << 24)
        | (u32::from(parsed.has("tfe")) << 23)
        | (u32::from(parsed.has("slc")) << 22)
        | ((entry.opcode >> 3) << 21)
        | (srsrc << 16)
        | (vdata << 8)
        | vaddr;
    Ok(vec![w0, w1])
}

fn mimg(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let (mut vdata, mut srsrc, mut ssamp) = (0, 0, 0);
    let mut addresses = Vec::new();
    let mut operands = parsed.operands.iter();
    for spec in entry.info.spec {
        let opnd = operands.next().ok_or("missing operand")?;
        match spec.field {
            Field::Vdata => vdata = reg_of(opnd)?,
            Field::Vaddr => match &opnd.value {
                Value::VList(regs) => addresses.clone_from(regs),
                _ => addresses = vec![reg_of(opnd)?],
            },
            Field::Srsrc => srsrc = reg_of(opnd)? / 4,
            Field::Ssamp => ssamp = reg_of(opnd)? / 4,
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        }
    }
    let nsa = if addresses.len() > 1 {
        (addresses.len() - 1).div_ceil(4) as u32
    } else {
        0
    };
    let dim = match parsed.value("dim") {
        None => 0,
        Some(name) => crate::names::MIMG_DIMS
            .iter()
            .position(|d| *d == name)
            .ok_or("bad dim")? as u32,
    };
    let dmask = parsed.value("dmask").and_then(parse_int).unwrap_or(0) as u32;
    let flag = |key| u32::from(parsed.has(key));
    let w0 = 0xF000_0000
        | (flag("slc") << 25)
        | ((entry.opcode & 0x7F) << 18)
        | (flag("lwe") << 17)
        | (flag("tfe") << 16)
        | (flag("r128") << 15)
        | (flag("glc") << 13)
        | (flag("unorm") << 12)
        | ((dmask & 0xF) << 8)
        | (flag("dlc") << 7)
        | (dim << 3)
        | (nsa << 1)
        | (entry.opcode >> 7);
    let w1 = (flag("d16") << 31)
        | (flag("a16") << 30)
        | (ssamp << 21)
        | (srsrc << 16)
        | (vdata << 8)
        | addresses.first().copied().unwrap_or(0);
    let mut out = vec![w0, w1];
    let extra: Vec<u8> = addresses.iter().skip(1).map(|reg| *reg as u8).collect();
    for chunk in extra.chunks(4) {
        let mut bytes = [0u8; 4];
        bytes[..chunk.len()].copy_from_slice(chunk);
        out.push(u32::from_le_bytes(bytes));
    }
    Ok(out)
}

fn vop3(entry: &Entry, parsed: &Parsed, wave32: bool) -> Result<Vec<u32>, String> {
    let _ = wave32;
    let mut literal = None;
    let (mut vdst, mut sdst) = (0, 0);
    let mut srcs = [0u32; 3];
    let (mut neg, mut abs) = (0u32, 0u32);
    let mut operands = parsed.operands.iter();
    let promoted_cmp = entry.opcode < 0x100;
    let cmpx = entry.info.name.starts_with("v_cmpx_");
    for spec in entry.info.spec {
        if spec.field == Field::OprExec || (spec.field == Field::Vdst && promoted_cmp && cmpx) {
            continue;
        }
        let opnd = operands.next().ok_or("missing operand")?;
        let slot = match spec.field {
            Field::Vdst => {
                vdst = reg_of(opnd)?;
                continue;
            }
            Field::Sdst => {
                sdst = reg_of(opnd)?;
                continue;
            }
            Field::Src0 => 0,
            Field::Src1 => 1,
            Field::Src2 => 2,
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        };
        srcs[slot] = match &opnd.value {
            Value::Attr(number, chan) => {
                number | (chan << 6) | (u32::from(parsed.has("high")) << 8)
            }
            Value::Text(param) if slot == 1 => interp_param(param)?,
            _ => src_code(opnd, &mut literal)?,
        };
        if opnd.neg {
            neg |= 1 << slot;
        }
        if opnd.abs {
            abs |= 1 << slot;
        }
    }
    let omod = match parsed.value("mul").or(parsed.value("div")) {
        Some("2") if parsed.value("mul").is_some() => 1,
        Some("4") => 2,
        Some("2") => 3,
        _ => 0,
    };
    let mut op_sel = 0;
    if let Some(bits) = parsed.value("op_sel") {
        let bits: Vec<u32> = bits
            .trim_matches(['[', ']'])
            .split(',')
            .map(|b| b.trim().parse().unwrap_or(0))
            .collect();
        for (index, bit) in bits.iter().enumerate() {
            let position = if index == bits.len() - 1 && !promoted_cmp {
                3
            } else {
                index
            };
            op_sel |= bit << position;
        }
    }
    let clamp = u32::from(parsed.has("clamp"));
    let low = if entry.family == Family::Vop3b {
        sdst << 8
    } else {
        (op_sel << 11) | (abs << 8)
    };
    let w0 = 0xD400_0000 | (entry.opcode << 16) | (clamp << 15) | low | vdst;
    let w1 = (neg << 29) | (omod << 27) | (srcs[2] << 18) | (srcs[1] << 9) | srcs[0];
    let mut out = vec![w0, w1];
    out.extend(literal);
    Ok(out)
}

fn ds(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let (mut vdst, mut addr, mut data0, mut data1) = (0, 0, 0, 0);
    let mut operands = parsed.operands.iter();
    for spec in entry.info.spec {
        let reg = reg_of(operands.next().ok_or("missing operand")?)?;
        match spec.field {
            Field::Vdst => vdst = reg,
            Field::Addr => addr = reg,
            Field::Data0 => data0 = reg,
            Field::Data1 => data1 = reg,
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        }
    }
    let (offset0, offset1) =
        if parsed.value("offset0").is_some() || parsed.value("offset1").is_some() {
            (
                parsed.number("offset0")? as u32,
                parsed.number("offset1")? as u32,
            )
        } else {
            let offset = match parsed.value("offset") {
                Some(text) if text.starts_with("swizzle(") => {
                    crate::names::parse_swizzle(text).ok_or("bad swizzle")?
                }
                _ => parsed.number("offset")? as u32,
            };
            (offset & 0xFF, (offset >> 8) & 0xFF)
        };
    let w0 = 0xD800_0000
        | (entry.opcode << 18)
        | (u32::from(parsed.has("gds")) << 17)
        | (offset1 << 8)
        | offset0;
    Ok(vec![w0, (vdst << 24) | (data1 << 16) | (data0 << 8) | addr])
}

fn flat(entry: &Entry, segment: u32, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let (mut vdst, mut addr, mut data, mut saddr) = (0, 0, 0, 0x7D);
    let mut addr_off = false;
    let mut operands = parsed.operands.iter();
    let atomic = entry.info.name.contains("_atomic_");
    let no_vdst = (atomic && !parsed.has("glc")) || (parsed.has("lds") && segment != 0);
    for spec in entry.info.spec {
        if (spec.field == Field::Saddr && segment == 0) || (spec.field == Field::Vdst && no_vdst) {
            continue;
        }
        let opnd = operands.next().ok_or("missing operand")?;
        match spec.field {
            Field::Vdst => vdst = reg_of(opnd)?,
            Field::Addr if opnd.value == Value::Off => addr_off = true,
            Field::Addr => addr = reg_of(opnd)?,
            Field::Data => data = reg_of(opnd)?,
            Field::Saddr if opnd.value == Value::Off => {
                saddr = if segment == 1 && addr_off { 0x7F } else { 0x7D }
            }
            Field::Saddr => saddr = reg_of(opnd)?,
            _ => return Err(format!("unsupported field {:?}", spec.field)),
        }
    }
    let offset = (parsed.number("offset")? as u32) & 0xFFF;
    let flags = (u32::from(parsed.has("slc")) << 17)
        | (u32::from(parsed.has("glc")) << 16)
        | (u32::from(parsed.has("lds")) << 13)
        | (u32::from(parsed.has("dlc")) << 12);
    let w0 = 0xDC00_0000 | (entry.opcode << 18) | (segment << 14) | flags | offset;
    Ok(vec![w0, (vdst << 24) | (saddr << 16) | (data << 8) | addr])
}

fn mubuf(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let (vdata, vaddr, srsrc, soffset) = buffer_fields(entry, parsed)?;
    let w0 = 0xE000_0000
        | (entry.opcode << 18)
        | (u32::from(parsed.has("lds")) << 16)
        | (u32::from(parsed.has("dlc")) << 15)
        | (u32::from(parsed.has("glc")) << 14)
        | (u32::from(parsed.has("idxen")) << 13)
        | (u32::from(parsed.has("offen")) << 12)
        | ((parsed.number("offset")? as u32) & 0xFFF);
    let w1 = (soffset << 24)
        | (u32::from(parsed.has("tfe")) << 23)
        | (u32::from(parsed.has("slc")) << 22)
        | (srsrc << 16)
        | (vdata << 8)
        | vaddr;
    Ok(vec![w0, w1])
}

fn smem(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let (mut sdata, mut sbase) = (0, 0);
    let mut operands = parsed.operands.iter();
    for spec in entry.info.spec {
        match spec.field {
            Field::Sdata => sdata = reg_of(operands.next().ok_or("missing operand")?)?,
            Field::Sbase => sbase = reg_of(operands.next().ok_or("missing operand")?)? / 2,
            _ => {}
        }
    }
    let (soffset, offset) = match operands.next().map(|opnd| &opnd.value) {
        None | Some(Value::Code(125)) => (125, 0),
        Some(Value::Int(value)) => (125, *value),
        Some(Value::Sgpr(reg, _)) => (*reg, parsed.number("offset")?),
        Some(Value::Code(code)) => (*code, parsed.number("offset")?),
        Some(other) => return Err(format!("bad SMEM offset {other:?}")),
    };
    let w0 = 0xF400_0000
        | (entry.opcode << 18)
        | (u32::from(parsed.has("glc")) << 16)
        | (u32::from(parsed.has("dlc")) << 14)
        | (sdata << 6)
        | sbase;
    Ok(vec![w0, (soffset << 25) | ((offset as u32) & 0x1F_FFFF)])
}

fn interp_param(param: &str) -> Result<u32, String> {
    match param {
        "p10" => Ok(0),
        "p20" => Ok(1),
        "p0" => Ok(2),
        _ => param
            .strip_prefix('p')
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| format!("bad parameter {param}")),
    }
}

fn vintrp(entry: &Entry, parsed: &Parsed) -> Result<Vec<u32>, String> {
    let [vdst, middle, attr] = parsed.operands.as_slice() else {
        return Err("expected three operands".into());
    };
    let vsrc = match &middle.value {
        Value::Text(param) => interp_param(param)?,
        _ => reg_of(middle)?,
    };
    let Value::Attr(number, chan) = attr.value else {
        return Err("expected an attribute".into());
    };
    Ok(vec![
        0xC800_0000
            | (reg_of(vdst)? << 18)
            | (entry.opcode << 16)
            | (number << 10)
            | (chan << 8)
            | vsrc,
    ])
}

fn export(rest: &str) -> Result<Vec<u32>, String> {
    let (target, sources) = rest
        .split_once(char::is_whitespace)
        .ok_or("missing export target")?;
    let target = match target {
        t if t.starts_with("mrtz") => 8,
        t if t.starts_with("mrt") => t[3..].parse::<u32>().map_err(|_| "bad target")?,
        "null" => 9,
        "prim" => 20,
        t if t.starts_with("pos") => 12 + t[3..].parse::<u32>().map_err(|_| "bad target")?,
        t if t.starts_with("param") => 32 + t[5..].parse::<u32>().map_err(|_| "bad target")?,
        t if t.starts_with("tgt") => t[3..].parse::<u32>().map_err(|_| "bad target")?,
        _ => return Err("bad export target".into()),
    };
    let parsed = parse(sources)?;
    let compressed = parsed.has("compr");
    let mut enable = 0;
    let mut regs = [0u32; 4];
    let step = if compressed { 2 } else { 1 };
    for (index, opnd) in parsed.operands.iter().enumerate().step_by(step) {
        if opnd.value != Value::Off {
            let slot = index / step;
            regs[slot] = reg_of(opnd)?;
            enable |= if compressed {
                3 << (2 * slot)
            } else {
                1 << slot
            };
        }
    }
    let w0 = 0xF800_0000
        | (u32::from(parsed.has("vm")) << 12)
        | (u32::from(parsed.has("done")) << 11)
        | (u32::from(compressed) << 10)
        | (target << 4)
        | enable;
    Ok(vec![
        w0,
        regs[0] | (regs[1] << 8) | (regs[2] << 16) | (regs[3] << 24),
    ])
}
