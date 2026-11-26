use std::collections::{BTreeMap, BTreeSet, HashMap};

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};

use crate::{Error, Program};

const MAX_INSTRUCTIONS: usize = 200_000;

pub struct RawBlock {
    pub start: u64,
    pub instructions: Vec<Instruction>,
    /// A conditional jump into another function: the block stands for a tail call.
    pub external: bool,
    pub switch: Option<Switch>,
}

#[derive(Clone)]
pub struct Switch {
    /// The register compared against the case count before the table lookup.
    pub index: Register,
    /// The target of each case value, from 0.
    pub cases: Vec<u64>,
    /// The instructions that only look up the table.
    pub lookup: Vec<u64>,
}

fn is_immediate(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate64
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
            | OpKind::Immediate8to64
            | OpKind::Immediate32to64
    )
}

fn same(a: Register, b: Register) -> bool {
    a != Register::None && a.full_register() == b.full_register()
}

/// Recognises `cmp i, n; ja default; lea b, [image base]; (movzx i, byte [b+i+t1];) mov r, [b+i*4+t2]; add r, b; jmp r`.
fn jump_table(
    program: &dyn Program,
    entry: u64,
    instructions: &BTreeMap<u64, Instruction>,
    jump: &Instruction,
) -> Option<Switch> {
    let recent: Vec<&Instruction> = instructions
        .range(..jump.ip())
        .rev()
        .take(16)
        .map(|(_, i)| i)
        .collect();
    let load = recent.iter().find(|i| {
        matches!(i.mnemonic(), Mnemonic::Mov | Mnemonic::Movsxd)
            && i.op_count() == 2
            && i.op1_kind() == OpKind::Memory
            && i.memory_index_scale() == 4
            && i.memory_index() != Register::None
            && i.memory_base() != Register::None
    })?;
    let lea = recent.iter().find(|i| {
        i.mnemonic() == Mnemonic::Lea
            && i.is_ip_rel_memory_operand()
            && same(i.op0_register(), load.memory_base())
    })?;
    let base = lea.ip_rel_memory_address();
    let signed = load.mnemonic() == Mnemonic::Movsxd;
    let table = base.wrapping_add(load.memory_displacement64());
    let bytes = recent.iter().find(|i| {
        i.mnemonic() == Mnemonic::Movzx
            && i.ip() < load.ip()
            && i.op0_kind() == OpKind::Register
            && same(i.op0_register(), load.memory_index())
            && i.op1_kind() == OpKind::Memory
            && i.memory_size().size() == 1
            && same(i.memory_base(), load.memory_base())
    });
    let bound = bytes.map_or(load.memory_index(), |b| b.memory_index());
    let compare = recent.iter().find(|i| {
        i.mnemonic() == Mnemonic::Cmp
            && i.op0_kind() == OpKind::Register
            && same(i.op0_register(), bound)
            && is_immediate(i.op1_kind())
    })?;
    let count = compare.immediate(1).checked_add(1).filter(|c| *c <= 4096)?;
    let read = |va: u64, size: usize| -> Option<u64> {
        let bytes = program.bytes(va)?.get(..size)?;
        Some(
            bytes
                .iter()
                .rev()
                .fold(0u64, |v, b| (v << 8) | u64::from(*b)),
        )
    };
    let slots: Vec<u64> = match bytes {
        Some(b) => {
            let first = base.wrapping_add(b.memory_displacement64());
            (0..count)
                .map(|k| read(first + k, 1))
                .collect::<Option<_>>()?
        }
        None => (0..count).collect(),
    };
    let cases: Vec<u64> = slots
        .iter()
        .map(|slot| {
            read(table + 4 * slot, 4).map(|entry| {
                let offset = if signed {
                    i64::from(entry as u32 as i32) as u64
                } else {
                    entry
                };
                base.wrapping_add(offset)
            })
        })
        .collect::<Option<_>>()?;
    if !cases
        .iter()
        .all(|t| program.inside(entry, *t) && program.code(*t).is_some())
    {
        return None;
    }
    let mut lookup = vec![load.ip(), lea.ip()];
    lookup.extend(bytes.map(|b| b.ip()));
    for i in &recent {
        let extends = matches!(
            i.mnemonic(),
            Mnemonic::Cdqe | Mnemonic::Movsxd | Mnemonic::Mov
        ) && i.op_count() <= 2
            && (i.op_count() == 0
                || (i.op0_kind() == OpKind::Register
                    && i.op1_kind() == OpKind::Register
                    && same(i.op0_register(), i.op1_register())))
            && i.ip() > compare.ip();
        let adds = i.mnemonic() == Mnemonic::Add
            && i.op0_kind() == OpKind::Register
            && same(i.op0_register(), jump.op0_register())
            && i.op1_kind() == OpKind::Register
            && same(i.op1_register(), load.memory_base());
        if extends || adds {
            lookup.push(i.ip());
        }
    }
    Some(Switch {
        index: compare.op0_register(),
        cases,
        lookup,
    })
}

fn branch_target(i: &Instruction) -> Option<u64> {
    let target = i.near_branch_target();
    (target != 0).then_some(target)
}

/// Every instruction reachable from `entry` without leaving the function, split into basic blocks.
pub fn discover(program: &dyn Program, entry: u64) -> Result<Vec<RawBlock>, Error> {
    let mut instructions: BTreeMap<u64, Instruction> = BTreeMap::new();
    let mut leaders = BTreeSet::from([entry]);
    let mut external = BTreeSet::new();
    let mut work = vec![entry];
    let mut switches: HashMap<u64, Switch> = HashMap::new();
    while let Some(start) = work.pop() {
        let mut at = start;
        loop {
            if instructions.contains_key(&at) {
                break;
            }
            let Some(bytes) = program.code(at) else {
                break;
            };
            let mut decoder =
                Decoder::with_ip(64, &bytes[..bytes.len().min(15)], at, DecoderOptions::NONE);
            let instruction = decoder.decode();
            if instruction.is_invalid() {
                break;
            }
            instructions.insert(at, instruction);
            if instructions.len() > MAX_INSTRUCTIONS {
                return Err(Error::TooLarge);
            }
            at = instruction.next_ip();
            match instruction.flow_control() {
                FlowControl::UnconditionalBranch => {
                    if let Some(target) =
                        branch_target(&instruction).filter(|t| program.inside(entry, *t))
                    {
                        leaders.insert(target);
                        work.push(target);
                    }
                    break;
                }
                FlowControl::ConditionalBranch => {
                    if let Some(target) = branch_target(&instruction) {
                        leaders.insert(target);
                        if program.inside(entry, target) {
                            work.push(target);
                        } else {
                            external.insert(target);
                        }
                    }
                    leaders.insert(at);
                    work.push(at);
                    break;
                }
                FlowControl::IndirectBranch => {
                    if instruction.op0_kind() == OpKind::Register
                        && let Some(switch) =
                            jump_table(program, entry, &instructions, &instruction)
                    {
                        for &target in &switch.cases {
                            leaders.insert(target);
                            work.push(target);
                        }
                        switches.insert(instruction.ip(), switch);
                    }
                    break;
                }
                FlowControl::Return | FlowControl::Exception | FlowControl::Interrupt => {
                    break;
                }
                _ => {}
            }
        }
    }
    let mut blocks: Vec<RawBlock> = Vec::new();
    let mut previous_end = None;
    for (address, instruction) in instructions {
        let continues = previous_end == Some(address) && !leaders.contains(&address);
        match blocks.last_mut() {
            Some(block) if continues => block.instructions.push(instruction),
            _ => blocks.push(RawBlock {
                start: address,
                instructions: vec![instruction],
                external: false,
                switch: None,
            }),
        }
        let ends = !matches!(
            instruction.flow_control(),
            FlowControl::Next
                | FlowControl::Call
                | FlowControl::IndirectCall
                | FlowControl::XbeginXabortXend
        );
        previous_end = (!ends).then(|| instruction.next_ip());
    }
    for block in &mut blocks {
        if let Some(last) = block.instructions.last() {
            block.switch = switches.remove(&last.ip());
        }
    }
    for target in external {
        if !blocks.iter().any(|b| b.start == target) {
            blocks.push(RawBlock {
                start: target,
                instructions: Vec::new(),
                external: true,
                switch: None,
            });
        }
    }
    Ok(blocks)
}

/// Immediate dominators by the Cooper–Harvey–Kennedy algorithm; `None` for nodes unreachable from `entry`.
pub fn dominators(
    n: usize,
    entry: usize,
    succ: &[Vec<usize>],
    pred: &[Vec<usize>],
) -> Vec<Option<usize>> {
    let mut postorder = Vec::with_capacity(n);
    let mut visited = vec![false; n];
    let mut stack = vec![(entry, 0usize)];
    visited[entry] = true;
    while let Some((node, next)) = stack.pop() {
        if let Some(&s) = succ[node].get(next) {
            stack.push((node, next + 1));
            if !visited[s] {
                visited[s] = true;
                stack.push((s, 0));
            }
        } else {
            postorder.push(node);
        }
    }
    let mut number = vec![usize::MAX; n];
    for (index, node) in postorder.iter().enumerate() {
        number[*node] = index;
    }
    let mut idom: Vec<Option<usize>> = vec![None; n];
    idom[entry] = Some(entry);
    let mut changed = true;
    while changed {
        changed = false;
        for &node in postorder.iter().rev() {
            if node == entry {
                continue;
            }
            let mut new: Option<usize> = None;
            for &p in &pred[node] {
                if idom[p].is_none() {
                    continue;
                }
                new = Some(match new {
                    None => p,
                    Some(mut a) => {
                        let mut b = p;
                        while a != b {
                            while number[a] < number[b] {
                                a = idom[a].expect("processed");
                            }
                            while number[b] < number[a] {
                                b = idom[b].expect("processed");
                            }
                        }
                        a
                    }
                });
            }
            if new.is_some() && idom[node] != new {
                idom[node] = new;
                changed = true;
            }
        }
    }
    idom
}

pub fn dominates(idom: &[Option<usize>], a: usize, mut b: usize) -> bool {
    loop {
        if a == b {
            return true;
        }
        match idom[b] {
            Some(parent) if parent != b => b = parent,
            _ => return false,
        }
    }
}
