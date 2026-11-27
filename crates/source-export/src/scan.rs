use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic};
use pe_image::{Function, Image};
use rayon::prelude::*;

use crate::image_data::{ImportTable, text_at, thunk_target};
use crate::symbols::Symbols;

/// Most bytes read for a function the exception table does not list.
const LEAF_EXTENT: u32 = 0x400;
/// Most strings and globals kept per function.
const KEEP: usize = 64;

/// A function the export writes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub rva: u32,
    /// Zero for functions found only through calls to them: they have no unwind data to give a size.
    pub size: u32,
}

/// What every function calls and uses; all indices into `entries` unless noted.
#[derive(Default)]
pub struct Facts {
    pub entries: Vec<Entry>,
    pub callees: Vec<Vec<u32>>,
    pub callers: Vec<Vec<u32>>,
    /// Indices into the `ImportTable`.
    pub imports: Vec<Vec<u32>>,
    /// RVAs of the text referenced.
    pub strings: Vec<Vec<u32>>,
    /// RVAs of the named globals referenced.
    pub globals: Vec<Vec<u32>>,
}

impl Facts {
    pub fn index_of(&self, rva: u32) -> Option<usize> {
        self.entries.binary_search_by_key(&rva, |e| e.rva).ok()
    }
}

#[derive(Default)]
struct Raw {
    imports: Vec<u32>,
    owners: Vec<u32>,
    leaves: Vec<u32>,
    strings: Vec<u32>,
    globals: Vec<u32>,
}

impl Raw {
    fn absorb(&mut self, mut other: Raw) {
        self.imports.append(&mut other.imports);
        self.owners.append(&mut other.owners);
        self.leaves.append(&mut other.leaves);
        self.strings.append(&mut other.strings);
        self.globals.append(&mut other.globals);
    }

    fn tidy(&mut self) {
        for list in [
            &mut self.imports,
            &mut self.owners,
            &mut self.leaves,
            &mut self.strings,
            &mut self.globals,
        ] {
            list.sort_unstable();
            list.dedup();
        }
        self.strings.truncate(KEEP);
        self.globals.truncate(KEEP);
    }
}

struct Ctx<'a> {
    image: &'a Image<'a>,
    imports: &'a ImportTable,
    symbols: &'a dyn Symbols,
    functions: &'a [Function],
    primary: &'a [u32],
}

impl Ctx<'_> {
    fn pdata_index(&self, rva: u32) -> Option<usize> {
        let index = self.functions.partition_point(|f| f.begin <= rva);
        let index = index.checked_sub(1)?;
        (rva < self.functions[index].end).then_some(index)
    }

    fn executable(&self, rva: u32) -> bool {
        self.image.section_of(rva).is_some_and(|s| s.executable())
    }

    fn thunk_import(&self, rva: u32) -> Option<u32> {
        let slot = thunk_target(self.image, rva)?;
        self.imports.by_slot(slot).map(|i| i as u32)
    }

    fn branch(&self, raw: &mut Raw, own: u32, target: u32, is_call: bool) {
        match self.pdata_index(target) {
            Some(index) => {
                let owner = self.primary[index];
                if owner == own {
                    return;
                }
                let function = self.functions[index];
                if function.begin == target
                    && function.end - function.begin <= 16
                    && let Some(import) = self.thunk_import(target)
                {
                    raw.imports.push(import);
                    return;
                }
                raw.owners.push(owner);
            }
            None => {
                if let Some(import) = self.thunk_import(target) {
                    raw.imports.push(import);
                } else if is_call && self.executable(target) {
                    raw.leaves.push(target);
                }
            }
        }
    }

    fn memory(&self, raw: &mut Raw, target: u32) {
        if let Some(import) = self.imports.by_slot(target) {
            raw.imports.push(import as u32);
            return;
        }
        let Some(section) = self.image.section_of(target) else {
            return;
        };
        if section.executable() {
            if let Ok(index) = self.functions.binary_search_by_key(&target, |f| f.begin) {
                raw.owners.push(self.primary[index]);
            }
        } else if text_at(self.image, target).is_some() {
            raw.strings.push(target);
        } else if self.symbols.global_name(target).is_some() {
            raw.globals.push(target);
        }
    }

    /// Decodes up to `limit` bytes from `begin`; `leaf` stops at padding and invalid code.
    fn scan(&self, own: u32, begin: u32, limit: usize, leaf: bool) -> Raw {
        let mut raw = Raw::default();
        let Some(bytes) = self.image.bytes_at(begin) else {
            return raw;
        };
        let bytes = &bytes[..bytes.len().min(limit)];
        let base = self.image.image_base;
        let mut decoder =
            Decoder::with_ip(64, bytes, base + u64::from(begin), DecoderOptions::NONE);
        let mut instruction = Instruction::default();
        while decoder.can_decode() {
            decoder.decode_out(&mut instruction);
            if leaf && (instruction.is_invalid() || instruction.mnemonic() == Mnemonic::Int3) {
                break;
            }
            let flow = instruction.flow_control();
            let direct = matches!(
                flow,
                FlowControl::Call
                    | FlowControl::UnconditionalBranch
                    | FlowControl::ConditionalBranch
            ) && instruction.near_branch_target() != 0;
            if direct {
                let target = instruction.near_branch_target().wrapping_sub(base);
                if let Ok(target) = u32::try_from(target) {
                    self.branch(&mut raw, own, target, flow == FlowControl::Call);
                }
            } else if instruction.is_ip_rel_memory_operand() {
                let target = instruction.ip_rel_memory_address().wrapping_sub(base);
                if let Ok(target) = u32::try_from(target) {
                    self.memory(&mut raw, target);
                }
            }
        }
        raw.tidy();
        raw
    }
}

/// Scans every function's code for what it calls, uses and imports. Runs on the current rayon pool.
pub fn analyze(
    image: &Image<'_>,
    imports: &ImportTable,
    symbols: &dyn Symbols,
    functions: &[Function],
) -> Facts {
    let primary: Vec<u32> = functions.iter().map(|f| image.primary(*f).begin).collect();
    let ctx = Ctx {
        image,
        imports,
        symbols,
        functions,
        primary: &primary,
    };
    let raws: Vec<Raw> = functions
        .par_iter()
        .enumerate()
        .map(|(i, f)| ctx.scan(primary[i], f.begin, (f.end - f.begin) as usize, false))
        .collect();

    let mut entries: Vec<Entry> = functions
        .iter()
        .zip(&primary)
        .filter(|(f, p)| f.begin == **p)
        .map(|(f, _)| Entry {
            rva: f.begin,
            size: f.end - f.begin,
        })
        .collect();
    entries.dedup_by_key(|e| e.rva);

    let mut leaves: Vec<u32> = raws.iter().flat_map(|r| r.leaves.iter().copied()).collect();
    let extras: std::collections::HashMap<u32, u32> =
        symbols.extra_functions().into_iter().collect();
    leaves.extend(
        extras
            .keys()
            .copied()
            .filter(|&rva| ctx.executable(rva) && ctx.pdata_index(rva).is_none()),
    );
    leaves.sort_unstable();
    leaves.dedup();
    leaves.retain(|rva| entries.binary_search_by_key(rva, |e| e.rva).is_err());
    let leaf_raws: Vec<Raw> = leaves
        .par_iter()
        .enumerate()
        .map(|(i, &rva)| {
            let next = leaves.get(i + 1).copied().unwrap_or(u32::MAX);
            let pdata_next = functions
                .get(functions.partition_point(|f| f.begin <= rva))
                .map_or(u32::MAX, |f| f.begin);
            match extras.get(&rva).copied().filter(|size| *size > 0) {
                Some(size) => ctx.scan(rva, rva, size as usize, false),
                None => {
                    let extent = LEAF_EXTENT.min(next.min(pdata_next) - rva);
                    ctx.scan(rva, rva, extent as usize, true)
                }
            }
        })
        .collect();

    entries.extend(leaves.iter().map(|&rva| Entry {
        rva,
        size: extras.get(&rva).copied().unwrap_or(0),
    }));
    entries.sort_by_key(|e| e.rva);
    let index_of = |rva: u32| entries.binary_search_by_key(&rva, |e| e.rva).ok();

    let mut merged: Vec<Raw> = std::iter::repeat_with(Raw::default)
        .take(entries.len())
        .collect();
    for (i, raw) in raws.into_iter().enumerate() {
        if let Some(e) = index_of(primary[i]) {
            merged[e].absorb(raw);
        }
    }
    for (rva, raw) in leaves.iter().zip(leaf_raws) {
        if let Some(e) = index_of(*rva) {
            merged[e].absorb(raw);
        }
    }

    let mut facts = Facts {
        callees: vec![Vec::new(); entries.len()],
        callers: vec![Vec::new(); entries.len()],
        imports: Vec::with_capacity(entries.len()),
        strings: Vec::with_capacity(entries.len()),
        globals: Vec::with_capacity(entries.len()),
        entries: Vec::new(),
    };
    for (e, mut raw) in merged.into_iter().enumerate() {
        raw.tidy();
        let mut callees: Vec<u32> = raw
            .owners
            .iter()
            .chain(&raw.leaves)
            .filter_map(|rva| index_of(*rva))
            .map(|i| i as u32)
            .filter(|i| *i as usize != e)
            .collect();
        callees.sort_unstable();
        callees.dedup();
        for &callee in &callees {
            facts.callers[callee as usize].push(e as u32);
        }
        facts.callees[e] = callees;
        facts.imports.push(raw.imports);
        facts.strings.push(raw.strings);
        facts.globals.push(raw.globals);
    }
    facts.entries = entries;
    facts
}
