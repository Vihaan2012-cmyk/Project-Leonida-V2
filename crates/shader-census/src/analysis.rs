//! Which SGPRs a program reads before writing them, and what it does with them.
//!
//! A forward dataflow over the program's control-flow graph. Every SGPR starts holding itself ("pristine");
//! each value also remembers which entry SGPRs it was copied or computed from, whether it came from
//! `s_getpc_b64`, and a constant when it is one. Reads of pristine registers are the program's inputs; the
//! consumers (`s_load`, descriptor operands, `s_setpc`) classify what the origin bits of their operands say.

use std::collections::VecDeque;

use rdna::{Inst, Operand};

const REGS: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Val {
    src: u128,
    pc: bool,
    pristine: bool,
    konst: Option<u32>,
}

impl Val {
    const UNKNOWN: Val = Val {
        src: 0,
        pc: false,
        pristine: false,
        konst: None,
    };

    fn konst(value: u32) -> Val {
        Val {
            konst: Some(value),
            ..Val::UNKNOWN
        }
    }

    fn join(self, other: Val) -> Val {
        Val {
            src: self.src | other.src,
            pc: self.pc | other.pc,
            pristine: self.pristine | other.pristine,
            konst: if self.konst == other.konst {
                self.konst
            } else {
                None
            },
        }
    }
}

type State = [Val; REGS];

fn entry_state() -> State {
    let mut state = [Val::UNKNOWN; REGS];
    for (reg, val) in state.iter_mut().enumerate() {
        val.src = 1 << reg;
        val.pristine = true;
    }
    state
}

/// What the program does with its entry SGPRs. Masks are over `s0..s127`; bit N is `sN`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    pub instructions: usize,
    pub invalid_words: usize,
    /// Read while still holding the value the hardware or driver put there.
    pub read_first: u128,
    /// Low half of a 64-bit base given to `s_load` or `global_*` and the high half of it.
    pub load_ptr64: u128,
    pub load_ptr64_hi: u128,
    /// A base whose high half is a constant or not from the user data.
    pub load_ptr32: u128,
    /// The first dword of a buffer, image or sampler resource descriptor built from entry SGPRs.
    pub vsharp: u128,
    pub tsharp: u128,
    pub ssharp: u128,
    /// The second dword of such a descriptor when it continues the 64-bit address.
    pub desc_hi: u128,
    /// Any other descriptor dword that comes from an entry SGPR.
    pub desc_word: u128,
    /// First dwords of descriptors whose every dword is an unmodified copy of consecutive entry SGPRs.
    pub desc_direct: u128,
    /// Used as the offset operand of a load or buffer access.
    pub offset: u128,
    /// Combined with a value that came from `s_getpc_b64`.
    pub pc_mix: u128,
    /// The target of `s_setpc_b64` or `s_swappc_b64`.
    pub set_pc: u128,
    /// A pointer base that is the sum of more than one entry SGPR.
    pub mixed: u128,
    pub uses_getpc: bool,
    pub uses_setpc: bool,
    pub loads: usize,
    pub loads_from_entry: usize,
    pub loads_from_pc: usize,
    pub loads_chained: usize,
}

const NO_DESTINATION: &[&str] = &[
    "s_cmp",
    "s_bitcmp",
    "s_waitcnt",
    "s_branch",
    "s_cbranch",
    "s_nop",
    "s_endpgm",
    "s_sendmsg",
    "s_setreg",
    "s_barrier",
    "s_setpc",
    "s_store",
    "s_buffer_store",
    "s_dcache",
    "s_icache",
    "s_inst_prefetch",
    "s_trap",
    "s_sleep",
    "s_setprio",
    "s_ttrace",
    "s_code_end",
    "s_clause",
    "s_wakeup",
    "s_set_gpr_idx",
    "s_delay_alu",
    "s_version",
    "s_denorm_mode",
    "s_round_mode",
    "s_gl1_inv",
    "s_atc_probe",
    "s_rfe",
];

/// Operand indexes of the SGPRs an instruction writes.
fn destinations(name: &str) -> [Option<usize>; 2] {
    if name.starts_with("s_") {
        if NO_DESTINATION.iter().any(|prefix| name.starts_with(prefix)) {
            return [None, None];
        }
        return [Some(0), None];
    }
    if name == "v_readfirstlane_b32" || name == "v_readlane_b32" {
        return [Some(0), None];
    }
    if name.starts_with("v_cmp") {
        return [name.ends_with("_e64").then_some(0), None];
    }
    if name.contains("_co_")
        || name.starts_with("v_mad_u64_u32")
        || name.starts_with("v_mad_i64_i32")
        || name.starts_with("v_div_scale")
    {
        return [None, Some(1)];
    }
    [None, None]
}

fn dword(state: &State, operand: &Operand, d: usize) -> Val {
    match operand {
        Operand::Sgpr { reg, count } => {
            let count = *count as usize;
            let index = if count <= 1 { 0 } else { d.min(count - 1) };
            state
                .get(*reg as usize + index)
                .copied()
                .unwrap_or(Val::UNKNOWN)
        }
        Operand::Int(value) => Val::konst(*value as u32),
        Operand::Literal(value) => Val::konst(*value),
        Operand::Float(_) => Val {
            konst: operand.float_bits(),
            ..Val::UNKNOWN
        },
        _ => Val::UNKNOWN,
    }
}

#[derive(Clone, Copy)]
enum Descriptor {
    Buffer,
    Image,
    Sampler,
}

impl Analysis {
    fn pointer(&mut self, lo: Val, hi: Val) {
        self.loads += 1;
        if lo.pc {
            self.loads_from_pc += 1;
            self.pc_mix |= lo.src;
        } else if lo.src == 0 {
            self.loads_chained += 1;
        } else {
            self.loads_from_entry += 1;
        }
        if lo.pc || lo.src == 0 {
            return;
        }
        if lo.src.count_ones() > 1 {
            self.mixed |= lo.src;
            return;
        }
        let n = lo.src.trailing_zeros();
        if hi.src >> (n + 1) & 1 != 0 {
            self.load_ptr64 |= 1 << n;
            self.load_ptr64_hi |= 1 << (n + 1);
        } else {
            self.load_ptr32 |= 1 << n;
        }
    }

    fn descriptor(&mut self, state: &State, operand: &Operand, kind: Descriptor) {
        let Operand::Sgpr { reg, count } = operand else {
            return;
        };
        let vals: Vec<Val> = (0..*count as usize)
            .map(|d| {
                state
                    .get(*reg as usize + d)
                    .copied()
                    .unwrap_or(Val::UNKNOWN)
            })
            .collect();
        let base = vals[0];
        let mut hi_bit = 0u128;
        if base.src.count_ones() == 1 {
            let n = base.src.trailing_zeros();
            let class = match kind {
                Descriptor::Buffer => &mut self.vsharp,
                Descriptor::Image => &mut self.tsharp,
                Descriptor::Sampler => &mut self.ssharp,
            };
            *class |= 1 << n;
            if vals.get(1).is_some_and(|v| v.src >> (n + 1) & 1 != 0) {
                hi_bit = 1 << (n + 1);
                self.desc_hi |= hi_bit;
            }
            if vals
                .iter()
                .enumerate()
                .all(|(j, v)| v.src == 1u128 << (n as usize + j) && v.konst.is_none())
            {
                self.desc_direct |= 1 << n;
            }
        } else if base.src.count_ones() > 1 {
            self.mixed |= base.src;
        }
        for v in &vals[1..] {
            self.desc_word |= v.src & !hi_bit;
        }
    }
}

fn is_branch(name: &str) -> bool {
    name == "s_branch" || name.starts_with("s_cbranch")
}

/// The instruction index a branch jumps to.
fn branch_target(insts: &[Inst], at: usize) -> Option<usize> {
    let Some(Operand::Int(words)) = insts[at].args.first().map(|a| &a.operand) else {
        return None;
    };
    let pc = insts[at].pc as i64 + 4 + 4 * *words;
    usize::try_from(pc)
        .ok()
        .and_then(|pc| insts.binary_search_by_key(&pc, |i| i.pc).ok())
}

fn transfer(inst: &Inst, state: &mut State, ev: Option<&mut Analysis>) {
    let name = inst.name.as_str();
    let dests = destinations(name);
    let is_dest = |i: usize| dests.contains(&Some(i));

    if let Some(ev) = ev {
        for (i, arg) in inst.args.iter().enumerate() {
            let Operand::Sgpr { reg, count } = arg.operand else {
                continue;
            };
            if is_dest(i) {
                continue;
            }
            for d in 0..count as usize {
                let r = reg as usize + d;
                if r < REGS && state[r].pristine {
                    ev.read_first |= 1 << r;
                }
            }
        }
        observe(inst, state, ev);
    }

    let mut result: Vec<(usize, Operand, Vec<Val>)> = Vec::new();
    for (i, arg) in inst.args.iter().enumerate() {
        if !is_dest(i) {
            continue;
        }
        let Operand::Sgpr { count, .. } = arg.operand else {
            continue;
        };
        let mut vals = Vec::with_capacity(count as usize);
        for d in 0..count as usize {
            vals.push(written(inst, state, i, d));
        }
        result.push((i, arg.operand.clone(), vals));
    }
    for (_, operand, vals) in result {
        let Operand::Sgpr { reg, .. } = operand else {
            continue;
        };
        for (d, val) in vals.into_iter().enumerate() {
            if let Some(slot) = state.get_mut(reg as usize + d) {
                *slot = val;
            }
        }
    }
}

/// The value dword `d` of destination operand `at` takes.
fn written(inst: &Inst, state: &State, at: usize, d: usize) -> Val {
    let name = inst.name.as_str();
    if name == "s_getpc_b64" || name == "s_swappc_b64" {
        return Val {
            pc: true,
            ..Val::UNKNOWN
        };
    }
    let is_scalar_alu = name.starts_with("s_")
        && !name.starts_with("s_load")
        && !name.starts_with("s_buffer_load")
        && !name.starts_with("s_scratch_load")
        && !name.contains("saveexec")
        && !name.starts_with("s_getreg")
        && !name.starts_with("s_mem");
    if !is_scalar_alu || at != 0 {
        return Val::UNKNOWN;
    }
    if matches!(name, "s_mov_b32" | "s_mov_b64" | "s_movk_i32") {
        return inst.args.get(1).map_or(Val::UNKNOWN, |arg| {
            let mut val = dword(state, &arg.operand, d);
            val.pristine = false;
            val
        });
    }
    let mut merged = Val::UNKNOWN;
    for arg in inst.args.iter().skip(1) {
        let val = dword(state, &arg.operand, d);
        merged.src |= val.src;
        merged.pc |= val.pc;
    }
    if name == "s_cmov_b32" || name == "s_cmov_b64" {
        let old = dword(state, &inst.args[0].operand, d);
        merged.src |= old.src;
        merged.pc |= old.pc;
    }
    merged
}

fn observe(inst: &Inst, state: &State, ev: &mut Analysis) {
    let name = inst.name.as_str();
    let args = &inst.args;
    let val = |i: usize, d: usize| {
        args.get(i)
            .map_or(Val::UNKNOWN, |a| dword(state, &a.operand, d))
    };
    let offset = |ev: &mut Analysis, i: usize| {
        if matches!(args.get(i).map(|a| &a.operand), Some(Operand::Sgpr { .. })) {
            ev.offset |= val(i, 0).src;
        }
    };
    ev.uses_getpc |= name == "s_getpc_b64";
    if name == "s_setpc_b64" || name == "s_swappc_b64" {
        ev.uses_setpc = true;
        let target = if name == "s_setpc_b64" { 0 } else { 1 };
        ev.set_pc |= val(target, 0).src | val(target, 1).src;
    } else if name.starts_with("s_load") {
        if matches!(
            args.get(1).map(|a| &a.operand),
            Some(Operand::Sgpr { count: 2, .. })
        ) {
            ev.pointer(val(1, 0), val(1, 1));
            offset(ev, 2);
            if val(1, 0).pc {
                ev.pc_mix |= val(2, 0).src;
            }
        }
    } else if name.starts_with("s_buffer_load") {
        if let Some(arg) = args.get(1) {
            ev.descriptor(state, &arg.operand, Descriptor::Buffer);
        }
        offset(ev, 2);
    } else if name.starts_with("buffer_") || name.starts_with("tbuffer_") {
        let mut found = false;
        for (i, arg) in args.iter().enumerate() {
            match arg.operand {
                Operand::Sgpr { count: 4, .. } if !found => {
                    found = true;
                    ev.descriptor(state, &arg.operand, Descriptor::Buffer);
                }
                Operand::Sgpr { count: 1, .. } if found => offset(ev, i),
                _ => {}
            }
        }
    } else if name.starts_with("image_") {
        let mut seen = 0;
        for arg in args {
            if matches!(arg.operand, Operand::Sgpr { count, .. } if count >= 4) {
                let kind = if seen == 0 {
                    Descriptor::Image
                } else {
                    Descriptor::Sampler
                };
                ev.descriptor(state, &arg.operand, kind);
                seen += 1;
            }
        }
    } else if name.starts_with("global_") || name.starts_with("flat_") {
        for (i, arg) in args.iter().enumerate() {
            if matches!(arg.operand, Operand::Sgpr { count: 2, .. }) {
                ev.pointer(val(i, 0), val(i, 1));
            }
        }
    } else if name.starts_with("s_") {
        let mut merged = Val::UNKNOWN;
        for (i, arg) in args.iter().enumerate().skip(1) {
            let Operand::Sgpr { count, .. } = arg.operand else {
                continue;
            };
            for d in 0..count as usize {
                let v = val(i, d);
                merged.src |= v.src;
                merged.pc |= v.pc;
            }
        }
        if merged.pc && merged.src != 0 {
            ev.pc_mix |= merged.src;
        }
    }
}

fn join_into(slot: &mut Option<Box<State>>, incoming: &State) -> bool {
    match slot {
        None => {
            *slot = Some(Box::new(*incoming));
            true
        }
        Some(existing) => {
            let mut changed = false;
            for (old, new) in existing.iter_mut().zip(incoming) {
                let joined = old.join(*new);
                if joined != *old {
                    *old = joined;
                    changed = true;
                }
            }
            changed
        }
    }
}

/// Analyses `code`, the instruction bytes of one program.
pub fn analyze(code: &[u8], wave32: bool) -> Analysis {
    let decoded = rdna::decode_all(code, wave32, 0);
    let insts = decoded.insts;
    let mut result = Analysis {
        instructions: insts.len(),
        invalid_words: decoded.invalid,
        ..Analysis::default()
    };
    if insts.is_empty() {
        return result;
    }

    let mut leader = vec![false; insts.len()];
    leader[0] = true;
    for (i, inst) in insts.iter().enumerate() {
        let ends = is_branch(&inst.name)
            || matches!(
                inst.name.as_str(),
                "s_endpgm" | "s_setpc_b64" | "s_endpgm_saved"
            );
        if ends && i + 1 < insts.len() {
            leader[i + 1] = true;
        }
        if is_branch(&inst.name)
            && let Some(target) = branch_target(&insts, i)
        {
            leader[target] = true;
        }
    }
    let starts: Vec<usize> = (0..insts.len()).filter(|&i| leader[i]).collect();
    let block_of = |index: usize| starts.partition_point(|&s| s <= index) - 1;
    let end_of = |b: usize| starts.get(b + 1).copied().unwrap_or(insts.len());

    let successors = |b: usize| -> Vec<usize> {
        let last = end_of(b) - 1;
        let inst = &insts[last];
        let mut out = Vec::new();
        let falls = !matches!(
            inst.name.as_str(),
            "s_endpgm" | "s_endpgm_saved" | "s_setpc_b64" | "s_branch"
        );
        if is_branch(&inst.name)
            && let Some(target) = branch_target(&insts, last)
        {
            out.push(block_of(target));
        }
        if falls && last + 1 < insts.len() {
            out.push(block_of(last + 1));
        }
        out
    };

    let mut inputs: Vec<Option<Box<State>>> = vec![None; starts.len()];
    inputs[0] = Some(Box::new(entry_state()));
    let mut queue = VecDeque::from([0usize]);
    let mut queued = vec![false; starts.len()];
    queued[0] = true;
    while let Some(b) = queue.pop_front() {
        queued[b] = false;
        let mut state: State = **inputs[b].as_ref().expect("queued blocks have an input");
        for inst in &insts[starts[b]..end_of(b)] {
            transfer(inst, &mut state, None);
        }
        for next in successors(b) {
            if join_into(&mut inputs[next], &state) && !queued[next] {
                queued[next] = true;
                queue.push_back(next);
            }
        }
    }
    for (b, input) in inputs.iter().enumerate() {
        let Some(input) = input else {
            continue;
        };
        let mut state = **input;
        for inst in &insts[starts[b]..end_of(b)] {
            transfer(inst, &mut state, Some(&mut result));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> Analysis {
        let code = rdna::assemble(source, false).expect("assembles");
        analyze(&code, false)
    }

    fn bits(mut mask: u128) -> impl Iterator<Item = u32> {
        std::iter::from_fn(move || {
            if mask == 0 {
                return None;
            }
            let bit = mask.trailing_zeros();
            mask &= mask - 1;
            Some(bit)
        })
    }

    fn registers(mask: u128, limit: u32) -> impl Iterator<Item = u32> {
        bits(mask).filter(move |&r| r < limit)
    }

    fn set(mask: u128) -> Vec<u32> {
        bits(mask).collect()
    }

    #[test]
    fn plain_read_before_write() {
        let a = run("s_add_u32 s20, s9, s10\ns_mov_b32 s9, 0\ns_add_u32 s21, s9, s20\ns_endpgm");
        assert_eq!(set(a.read_first), vec![9, 10]);
    }

    #[test]
    fn copy_then_read_counts_the_source_once() {
        let a = run("s_mov_b32 s20, s8\ns_add_u32 s21, s20, s20\ns_endpgm");
        assert_eq!(set(a.read_first), vec![8]);
    }

    #[test]
    fn pointer_with_constant_high_half_is_32_bit() {
        let a = run(
            "s_mov_b32 s20, s8\ns_mov_b32 s21, 0\ns_load_dwordx4 s[4:7], s[20:21], 0x10\ns_endpgm",
        );
        assert_eq!(set(a.load_ptr32), vec![8]);
        assert_eq!(a.load_ptr64, 0);
        assert_eq!(a.loads_from_entry, 1);
    }

    #[test]
    fn pointer_pair_is_64_bit() {
        let a = run("s_load_dwordx4 s[40:43], s[2:3], 0x0\ns_endpgm");
        assert_eq!(set(a.load_ptr64), vec![2]);
        assert_eq!(set(a.load_ptr64_hi), vec![3]);
        assert_eq!(a.load_ptr32, 0);
    }

    #[test]
    fn pointer_adjusted_before_use_keeps_its_origin() {
        let a = run(
            "s_add_u32 s20, s6, 0x40\ns_addc_u32 s21, s7, 0\ns_load_dwordx2 s[4:5], s[20:21], 0x0\ns_endpgm",
        );
        assert_eq!(set(a.load_ptr64), vec![6]);
        assert_eq!(set(a.load_ptr64_hi), vec![7]);
    }

    #[test]
    fn loaded_pointer_is_chained_not_entry() {
        let a = run(
            "s_load_dwordx2 s[20:21], s[8:9], 0x0\ns_waitcnt lgkmcnt(0)\ns_load_dwordx4 s[4:7], s[20:21], 0x0\ns_endpgm",
        );
        assert_eq!(a.loads, 2);
        assert_eq!(a.loads_chained, 1);
        assert_eq!(a.loads_from_entry, 1);
        assert_eq!(set(a.load_ptr64), vec![8]);
    }

    #[test]
    fn buffer_descriptor_from_a_pair_and_constants() {
        let a = run(
            "s_mov_b32 s20, s6\ns_and_b32 s21, s7, 0xffff\ns_mov_b32 s22, 0x100\ns_mov_b32 s23, 0x1000\n\
             s_buffer_load_dword s4, s[20:23], 0x0\ns_endpgm",
        );
        assert_eq!(set(a.vsharp), vec![6]);
        assert_eq!(set(a.desc_hi), vec![7]);
        assert_eq!(a.desc_word, 0);
        assert_eq!(a.desc_direct, 0);
    }

    #[test]
    fn descriptor_held_whole_in_entry_sgprs() {
        let a = run("s_buffer_load_dword s40, s[8:11], 0x4\ns_endpgm");
        assert_eq!(set(a.vsharp), vec![8]);
        assert_eq!(set(a.desc_direct), vec![8]);
        assert_eq!(set(a.read_first), vec![8, 9, 10, 11]);
    }

    #[test]
    fn constant_feeding_a_descriptor_word() {
        let a = run(
            "s_mov_b32 s20, s6\ns_mov_b32 s21, s7\ns_mov_b32 s22, s12\ns_mov_b32 s23, 0x1000\n\
             s_buffer_load_dword s4, s[20:23], 0x0\ns_endpgm",
        );
        assert_eq!(set(a.desc_word), vec![12]);
        assert_eq!(a.desc_direct, 0);
    }

    #[test]
    fn image_resource_and_sampler_operands() {
        let a = run(
            "image_sample v[0:3], v[4:5], s[8:15], s[16:19] dmask:0xf dim:SQ_RSRC_IMG_2D\ns_endpgm",
        );
        assert_eq!(set(a.tsharp), vec![8]);
        assert_eq!(set(a.ssharp), vec![16]);
    }

    #[test]
    fn buffer_operand_and_soffset() {
        let a = run("buffer_load_dword v1, v3, s[4:7], s9 idxen\ns_endpgm");
        assert_eq!(set(a.vsharp), vec![4]);
        assert_eq!(set(a.offset), vec![9]);
    }

    #[test]
    fn load_offset_register_is_an_offset() {
        let a = run("s_load_dwordx4 s[40:43], s[10:11], s5\ns_endpgm");
        assert_eq!(set(a.offset), vec![5]);
        assert_eq!(set(a.load_ptr64), vec![10]);
    }

    #[test]
    fn getpc_arithmetic_marks_the_entry_sgpr_that_joins_it() {
        let a = run(
            "s_getpc_b64 s[20:21]\ns_add_u32 s20, s20, s12\ns_addc_u32 s21, s21, 0\n\
             s_load_dwordx4 s[4:7], s[20:21], 0x0\ns_endpgm",
        );
        assert!(a.uses_getpc);
        assert_eq!(set(a.pc_mix), vec![12]);
        assert_eq!(a.loads_from_pc, 1);
        assert_eq!(a.load_ptr32 | a.load_ptr64, 0);
    }

    #[test]
    fn setpc_target_is_recorded() {
        let a = run("s_getpc_b64 s[20:21]\ns_add_u32 s20, s20, s5\ns_setpc_b64 s[20:21]");
        assert!(a.uses_setpc);
        assert_eq!(set(a.set_pc), vec![5]);
    }

    #[test]
    fn write_on_one_branch_still_leaves_the_entry_value_reachable() {
        let a = run(
            "s_cmp_eq_u32 s3, 0\ns_cbranch_scc1 1\ns_mov_b32 s9, 1\ns_add_u32 s20, s9, 1\ns_endpgm",
        );
        assert_eq!(set(a.read_first), vec![3, 9]);
    }

    #[test]
    fn write_on_every_path_hides_the_entry_value() {
        let a = run(
            "s_cmp_eq_u32 s3, 0\ns_cbranch_scc1 2\ns_mov_b32 s9, 1\ns_branch 1\ns_mov_b32 s9, 2\n\
             s_add_u32 s20, s9, 1\ns_endpgm",
        );
        assert_eq!(set(a.read_first), vec![3]);
    }

    #[test]
    fn loop_back_edge_terminates() {
        let a = run(
            "s_mov_b32 s20, 0\ns_add_u32 s20, s20, s8\ns_cmp_lt_u32 s20, s9\ns_cbranch_scc1 -3\ns_endpgm",
        );
        assert_eq!(set(a.read_first), vec![8, 9]);
    }

    #[test]
    fn destination_of_vector_compare_is_not_a_read() {
        let a = run("v_cmp_eq_u32_e64 s[2:3], 1, v0\ns_and_b64 s[4:5], s[2:3], s[2:3]\ns_endpgm");
        assert_eq!(a.read_first, 0);
    }

    #[test]
    fn readfirstlane_destination_is_not_a_read() {
        let a = run("v_readfirstlane_b32 s5, v0\ns_add_u32 s6, s5, s5\ns_endpgm");
        assert_eq!(a.read_first, 0);
    }

    #[test]
    fn read_beyond_the_user_data_is_reported_with_its_register_number() {
        let a = run("s_add_u32 s20, s33, 1\ns_endpgm");
        assert_eq!(set(a.read_first), vec![33]);
        assert_eq!(registers(a.read_first, 32).count(), 0);
    }
}
