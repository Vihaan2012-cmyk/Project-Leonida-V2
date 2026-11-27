use std::collections::{BTreeMap, BTreeSet, HashMap};

use pe_image::Image;
use rayon::prelude::*;

use crate::model::{FunctionInfo, GlobalInfo};
use crate::naming::{self, Evidence};
use crate::rtti;
use crate::scan::{self, Layout, Scan};
use crate::{BuildOptions, ProgramDb};

const MAX_ROUNDS: usize = 32;

struct Func {
    rva: u32,
    chunks: Vec<(u32, u32)>,
    discovered: bool,
}

/// Every code range of every function, sorted by start.
struct Index {
    entries: Vec<(u32, u32, usize)>,
}

impl Index {
    fn new(funcs: &[Func]) -> Self {
        let mut entries: Vec<_> = funcs
            .iter()
            .enumerate()
            .flat_map(|(index, f)| f.chunks.iter().map(move |&(b, e)| (b, e, index)))
            .collect();
        entries.sort_unstable();
        Index { entries }
    }

    fn find(&self, rva: u32) -> Option<usize> {
        let at = self.entries.partition_point(|e| e.0 <= rva);
        let &(_, end, index) = self.entries[..at].last()?;
        (rva < end).then_some(index)
    }

    fn next_start(&self, rva: u32) -> u32 {
        let at = self.entries.partition_point(|e| e.0 <= rva);
        self.entries.get(at).map_or(u32::MAX, |e| e.0)
    }
}

fn known_functions(image: &Image<'_>) -> Vec<Func> {
    let mut groups: BTreeMap<u32, BTreeSet<(u32, u32)>> = BTreeMap::new();
    for function in image.functions() {
        let primary = image.primary(function);
        let set = groups.entry(primary.begin).or_default();
        set.insert((primary.begin, primary.end));
        set.insert((function.begin, function.end));
    }
    groups
        .into_iter()
        .map(|(rva, set)| {
            let mut chunks: Vec<_> = set.into_iter().collect();
            chunks.sort_by_key(|c| (c.0 != rva, c.0));
            Func {
                rva,
                chunks,
                discovered: false,
            }
        })
        .collect()
}

/// Adds functions for call targets and seeds that no known function covers.
fn discover(
    layout: &Layout<'_, '_>,
    funcs: &mut Vec<Func>,
    index: &Index,
    mut targets: Vec<u32>,
) -> usize {
    targets.sort_unstable();
    targets.dedup();
    let before = funcs.len();
    let mut last_end = 0;
    for target in targets {
        if target < last_end || index.find(target).is_some() {
            continue;
        }
        let Some(code) = layout.code_at(target) else {
            continue;
        };
        if code.first() == Some(&0xCC) {
            continue;
        }
        let limit = index.next_start(target);
        let Some(size) = scan::guess_extent(layout, target, limit) else {
            continue;
        };
        last_end = target + size;
        funcs.push(Func {
            rva: target,
            chunks: vec![(target, target + size)],
            discovered: true,
        });
    }
    funcs.len() - before
}

pub(crate) fn run(image: &Image<'_>, options: &BuildOptions) -> ProgramDb {
    let build = || analyse(image);
    match rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads.max(1))
        .build()
    {
        Ok(pool) => pool.install(build),
        Err(_) => build(),
    }
}

fn analyse(image: &Image<'_>) -> ProgramDb {
    let layout = Layout::new(image);
    let rtti = rtti::scan(&layout);
    let exports: Vec<_> = image
        .exports()
        .into_iter()
        .filter(|e| e.forwarder.is_none() && layout.is_code(e.rva))
        .collect();

    let mut funcs = known_functions(image);
    let mut index = Index::new(&funcs);
    let mut seeds: Vec<u32> = rtti
        .vtables
        .iter()
        .flat_map(|v| v.slots.iter().copied())
        .chain(exports.iter().map(|e| e.rva))
        .chain([image.entry])
        .collect();
    let mut scans: Vec<Scan> = Vec::new();
    for _ in 0..MAX_ROUNDS {
        let from = scans.len();
        let fresh: Vec<Scan> = funcs[from..]
            .par_iter()
            .map(|f| scan::scan_function(&layout, &f.chunks))
            .collect();
        let mut targets = std::mem::take(&mut seeds);
        targets.extend(fresh.iter().flat_map(|s| s.calls.iter().copied()));
        scans.extend(fresh);
        if discover(&layout, &mut funcs, &index, targets) == 0 {
            break;
        }
        index = Index::new(&funcs);
    }

    funcs.truncate(scans.len());
    let mut paired: Vec<(Func, Scan)> = funcs.into_iter().zip(scans).collect();
    paired.sort_by_key(|(f, _)| f.rva);
    let (funcs, scans): (Vec<Func>, Vec<Scan>) = paired.into_iter().unzip();
    let index = Index::new(&funcs);
    let at = |rva: u32| funcs.binary_search_by_key(&rva, |f| f.rva).ok();

    let reached: Vec<Vec<usize>> = scans
        .par_iter()
        .enumerate()
        .map(|(own, scan)| {
            let mut out: Vec<usize> = scan
                .calls
                .iter()
                .chain(&scan.jumps)
                .filter_map(|t| index.find(*t))
                .filter(|j| *j != own)
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect();
    let mut callers: Vec<Vec<u32>> = vec![Vec::new(); funcs.len()];
    for (own, callees) in reached.iter().enumerate() {
        for callee in callees {
            callers[*callee].push(funcs[own].rva);
        }
    }

    let text_rvas: Vec<u32> = scans
        .iter()
        .flat_map(|s| s.strings.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let strings: Vec<(u32, String)> = text_rvas
        .par_iter()
        .filter_map(|rva| scan::text_at(&layout, *rva).map(|t| (*rva, t)))
        .collect();
    let texts: HashMap<u32, String> = strings.iter().cloned().collect();

    let mut vtable_slots: Vec<Vec<(u32, u32)>> = vec![Vec::new(); funcs.len()];
    for vtable in &rtti.vtables {
        for (slot, target) in vtable.slots.iter().enumerate() {
            if let Some(owner) = at(*target) {
                vtable_slots[owner].push((vtable.rva, slot as u32));
            }
        }
    }

    let mut infos: Vec<FunctionInfo> = funcs
        .iter()
        .zip(&scans)
        .zip(reached.iter().zip(callers))
        .zip(vtable_slots)
        .map(|(((func, scan), (reached, callers)), vtable_slots)| {
            let mut imports: Vec<String> = scan
                .imports
                .iter()
                .filter_map(|slot| layout.imports.get(slot).cloned())
                .collect();
            for callee in scan.calls.iter().chain(&scan.jumps) {
                if let Some(j) = at(*callee)
                    && let Some(slot) = scans[j].thunk
                    && let Some(name) = layout.imports.get(&slot)
                {
                    imports.push(name.clone());
                }
            }
            imports.sort();
            imports.dedup();
            FunctionInfo {
                rva: func.rva,
                size: func.chunks.iter().map(|(b, e)| e - b).sum(),
                chunks: func.chunks.clone(),
                discovered: func.discovered,
                name: None,
                name_evidence: None,
                class: None,
                source_file: None,
                source_evidence: None,
                callers,
                callees: reached.iter().map(|j| funcs[*j].rva).collect(),
                strings: scan.strings.clone(),
                imports,
                vtable_slots,
            }
        })
        .collect();

    let thunks = funcs
        .iter()
        .zip(&scans)
        .filter_map(|(f, s)| Some((f.rva, layout.imports.get(&s.thunk?)?.clone())))
        .collect();
    let evidence = Evidence {
        texts: &texts,
        exports: exports
            .iter()
            .filter_map(|e| Some((e.rva, e.name.clone()?, e.ordinal)))
            .collect(),
        thunks,
        vtables: &rtti.vtables,
        classes: &rtti.classes,
    };
    naming::name_functions(&mut infos, &evidence);

    let mut globals = collect_globals(&funcs, &scans);
    naming::name_globals(&mut globals, &infos);

    ProgramDb::assemble(
        image.image_base,
        infos,
        globals,
        rtti.classes,
        rtti.vtables,
        strings,
    )
}

fn collect_globals(funcs: &[Func], scans: &[Scan]) -> Vec<GlobalInfo> {
    const READ: u8 = 0;
    const WRITE: u8 = 1;
    const TAKEN: u8 = 2;
    let mut uses: Vec<(u32, u8, u32)> = Vec::new();
    for (func, scan) in funcs.iter().zip(scans) {
        uses.extend(scan.reads.iter().map(|g| (*g, READ, func.rva)));
        uses.extend(scan.writes.iter().map(|g| (*g, WRITE, func.rva)));
        uses.extend(scan.taken.iter().map(|g| (*g, TAKEN, func.rva)));
    }
    uses.par_sort_unstable();
    let mut globals: Vec<GlobalInfo> = Vec::new();
    for (global, kind, user) in uses {
        if globals.last().is_none_or(|g| g.rva != global) {
            globals.push(GlobalInfo {
                rva: global,
                name: None,
                name_evidence: None,
                readers: Vec::new(),
                writers: Vec::new(),
                address_taken: Vec::new(),
            });
        }
        let last = globals.last_mut().expect("just pushed");
        match kind {
            READ => last.readers.push(user),
            WRITE => last.writers.push(user),
            _ => last.address_taken.push(user),
        }
    }
    globals
}
