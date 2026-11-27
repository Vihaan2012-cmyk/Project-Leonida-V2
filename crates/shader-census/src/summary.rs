use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::pairs::Techniques;
use crate::program::Program;

/// How many programs use one user SGPR in each way.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SgprRow {
    pub sgpr: u32,
    pub read: usize,
    pub plain: usize,
    pub load_ptr32: usize,
    pub load_ptr64: usize,
    pub load_ptr64_hi: usize,
    pub vsharp: usize,
    pub tsharp: usize,
    pub ssharp: usize,
    pub desc_hi: usize,
    pub desc_word: usize,
    pub desc_direct: usize,
    pub offset: usize,
    pub pc_mix: usize,
    pub set_pc: usize,
    pub mixed: usize,
}

/// The census of all programs of one stage.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct StageSummary {
    pub stage: String,
    pub programs: usize,
    pub wave64: usize,
    pub instructions: usize,
    pub invalid_programs: usize,
    pub with_rts0: usize,
    pub heap_resource: usize,
    pub heap_sampler: usize,
    pub uses_getpc: usize,
    pub uses_setpc: usize,
    pub reads_nothing: usize,
    pub reads_system: usize,
    pub rows: Vec<SgprRow>,
    /// Lowest user SGPR read before written, and lowest one used as a pointer or descriptor base.
    pub lowest_read: BTreeMap<u32, usize>,
    pub lowest_pointer: BTreeMap<u32, usize>,
    pub highest_read: BTreeMap<u32, usize>,
    pub highest_pointer: BTreeMap<u32, usize>,
    pub header_user_sgprs: BTreeMap<u32, usize>,
    /// Header user SGPR count minus (highest user read + 1), over programs that read any.
    pub header_minus_highest_read: BTreeMap<i32, usize>,
    /// Highest pointer or descriptor base minus the header user SGPR count, by which heap-indexing `SFI0` bits
    /// the program sets: `none`, `resource`, `sampler` or `both`.
    pub pointer_end: BTreeMap<String, BTreeMap<i32, usize>>,
    /// Programs that read a user-range SGPR at or above the header's user count.
    pub reads_at_or_above_header: usize,
    /// Compute only: programs whose header count equals `COMPUTE_PGM_RSRC2.USER_SGPR`.
    pub header_equals_rsrc2: usize,
    pub with_rsrc2: usize,
    /// Programs with a pair read as a 64-bit pointer or descriptor base at `s0`, the spill pointer's place.
    pub pointer_at_s0: usize,
    pub loads: usize,
    pub loads_from_entry: usize,
    pub loads_from_pc: usize,
    pub loads_chained: usize,
}

fn count(map: &mut BTreeMap<u32, usize>, key: Option<u32>) {
    if let Some(key) = key {
        *map.entry(key).or_default() += 1;
    }
}

impl StageSummary {
    pub fn of(stage: &str, programs: &[&Program]) -> StageSummary {
        let mut s = StageSummary {
            stage: stage.to_string(),
            programs: programs.len(),
            rows: (0..32)
                .map(|sgpr| SgprRow {
                    sgpr,
                    ..SgprRow::default()
                })
                .collect(),
            ..StageSummary::default()
        };
        for p in programs {
            s.wave64 += usize::from(p.wave64);
            s.instructions += p.instructions;
            s.invalid_programs += usize::from(p.invalid_words > 0);
            s.with_rts0 += usize::from(p.has_rts0);
            s.heap_resource += usize::from(p.heap_resource());
            s.heap_sampler += usize::from(p.heap_sampler());
            s.uses_getpc += usize::from(p.uses_getpc);
            s.uses_setpc += usize::from(p.uses_setpc);
            s.reads_nothing += usize::from(p.reads.0 == 0 && p.highest_system_read.is_none());
            s.reads_system += usize::from(p.highest_system_read.is_some());
            s.loads += p.loads;
            s.loads_from_entry += p.loads_from_entry;
            s.loads_from_pc += p.loads_from_pc;
            s.loads_chained += p.loads_chained;
            let plain = p.plain();
            for row in &mut s.rows {
                let r = row.sgpr;
                row.read += usize::from(p.reads.contains(r));
                row.plain += usize::from(plain.contains(r));
                row.load_ptr32 += usize::from(p.load_ptr32.contains(r));
                row.load_ptr64 += usize::from(p.load_ptr64.contains(r));
                row.load_ptr64_hi += usize::from(p.load_ptr64_hi.contains(r));
                row.vsharp += usize::from(p.vsharp.contains(r));
                row.tsharp += usize::from(p.tsharp.contains(r));
                row.ssharp += usize::from(p.ssharp.contains(r));
                row.desc_hi += usize::from(p.desc_hi.contains(r));
                row.desc_word += usize::from(p.desc_word.contains(r));
                row.desc_direct += usize::from(p.desc_direct.contains(r));
                row.offset += usize::from(p.offset.contains(r));
                row.pc_mix += usize::from(p.pc_mix.contains(r));
                row.set_pc += usize::from(p.set_pc.contains(r));
                row.mixed += usize::from(p.mixed.contains(r));
            }
            count(&mut s.lowest_read, p.reads.lowest());
            count(&mut s.highest_read, p.reads.highest());
            count(&mut s.lowest_pointer, p.pointers().lowest());
            count(&mut s.highest_pointer, p.pointers().highest());
            *s.header_user_sgprs.entry(p.header_user_sgprs).or_default() += 1;
            if let Some(high) = p.reads.highest() {
                let delta = p.header_user_sgprs as i32 - (high as i32 + 1);
                *s.header_minus_highest_read.entry(delta).or_default() += 1;
                s.reads_at_or_above_header += usize::from(high >= p.header_user_sgprs);
            }
            if let Some(high) = p.pointers().highest() {
                let class = match (p.heap_resource(), p.heap_sampler()) {
                    (false, false) => "none",
                    (true, false) => "resource",
                    (false, true) => "sampler",
                    (true, true) => "both",
                };
                let delta = high as i32 - p.header_user_sgprs as i32;
                *s.pointer_end
                    .entry(class.to_string())
                    .or_default()
                    .entry(delta)
                    .or_default() += 1;
            }
            if let Some(rsrc2) = p.rsrc2_user_sgprs {
                s.with_rsrc2 += 1;
                s.header_equals_rsrc2 += usize::from(rsrc2 == p.header_user_sgprs);
            }
            s.pointer_at_s0 += usize::from(
                p.load_ptr64.contains(0) || p.vsharp.contains(0) || p.tsharp.contains(0),
            );
        }
        s
    }
}

/// The whole census: one summary per stage and the programs behind them.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Census {
    pub stages: Vec<StageSummary>,
    pub techniques: Techniques,
    pub programs: Vec<Program>,
}

/// The name a program's summary group has: the stage, with `-first-half` for a merged vertex shader's extra
/// program.
pub fn group_name(program: &Program) -> String {
    if program.first_half {
        format!("{}-first-half", program.stage)
    } else {
        program.stage.clone()
    }
}

impl Census {
    pub fn of(programs: Vec<Program>) -> Census {
        let mut groups: BTreeMap<String, Vec<&Program>> = BTreeMap::new();
        for program in &programs {
            groups.entry(group_name(program)).or_default().push(program);
        }
        let stages = groups
            .iter()
            .map(|(name, list)| StageSummary::of(name, list))
            .collect();
        let techniques = Techniques::of(&programs);
        Census {
            stages,
            techniques,
            programs,
        }
    }

    pub fn stage(&self, name: &str) -> Option<&StageSummary> {
        self.stages.iter().find(|s| s.stage == name)
    }

    pub fn report(&self) -> String {
        let mut out = String::new();
        for stage in &self.stages {
            stage.write(&mut out);
        }
        self.techniques.write(&mut out);
        out
    }
}

fn histogram<K: std::fmt::Display>(map: &BTreeMap<K, usize>) -> String {
    if map.is_empty() {
        return "none".to_string();
    }
    map.iter()
        .map(|(k, v)| format!("{k}:{v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl StageSummary {
    fn write(&self, out: &mut String) {
        let n = self.programs;
        let _ = writeln!(
            out,
            "== {} == {n} programs, {} wave64, {} instructions, {} with undecodable words",
            self.stage, self.wave64, self.instructions, self.invalid_programs
        );
        let _ = writeln!(
            out,
            "container RTS0: {}   resource-heap SFI0 bit: {}   sampler-heap SFI0 bit: {}   s_getpc: {}   s_setpc/swappc: {}",
            self.with_rts0, self.heap_resource, self.heap_sampler, self.uses_getpc, self.uses_setpc
        );
        let _ = writeln!(
            out,
            "reading no entry SGPR: {}   reading s32 or above before writing: {}",
            self.reads_nothing, self.reads_system
        );
        let _ = writeln!(
            out,
            "lowest user SGPR read:    {}",
            histogram(&self.lowest_read)
        );
        let _ = writeln!(
            out,
            "lowest pointer/descr base: {}",
            histogram(&self.lowest_pointer)
        );
        let _ = writeln!(
            out,
            "highest user SGPR read:   {}",
            histogram(&self.highest_read)
        );
        let _ = writeln!(
            out,
            "highest pointer/descr base: {}",
            histogram(&self.highest_pointer)
        );
        let _ = writeln!(
            out,
            "header user SGPR count:   {}",
            histogram(&self.header_user_sgprs)
        );
        let _ = writeln!(
            out,
            "header count - (highest read + 1): {}   reads at or above the header count: {}",
            histogram(&self.header_minus_highest_read),
            self.reads_at_or_above_header
        );
        for (class, map) in &self.pointer_end {
            let _ = writeln!(
                out,
                "highest pointer - header count, heap bits {class}: {}",
                histogram(map)
            );
        }
        if self.with_rsrc2 > 0 {
            let _ = writeln!(
                out,
                "header count equals COMPUTE_PGM_RSRC2.USER_SGPR: {} of {}",
                self.header_equals_rsrc2, self.with_rsrc2
            );
        }
        let _ = writeln!(
            out,
            "64-bit pointer or descriptor base at s0: {}   s_load/global bases: {} ({} from entry SGPRs, {} from s_getpc, {} loaded earlier)",
            self.pointer_at_s0,
            self.loads,
            self.loads_from_entry,
            self.loads_from_pc,
            self.loads_chained
        );
        let _ = writeln!(
            out,
            "sgpr  read plain  ptr32  ptr64 ptr64hi vsharp tsharp ssharp deschi descwd direct offset pcmix setpc mixed"
        );
        for r in self.rows.iter().filter(|r| r.read > 0) {
            let _ = writeln!(
                out,
                "s{:<4}{:>5}{:>6}{:>7}{:>7}{:>8}{:>7}{:>7}{:>7}{:>7}{:>7}{:>7}{:>7}{:>6}{:>6}{:>6}",
                r.sgpr,
                r.read,
                r.plain,
                r.load_ptr32,
                r.load_ptr64,
                r.load_ptr64_hi,
                r.vsharp,
                r.tsharp,
                r.ssharp,
                r.desc_hi,
                r.desc_word,
                r.desc_direct,
                r.offset,
                r.pc_mix,
                r.set_pc,
                r.mixed
            );
        }
        out.push('\n');
    }
}
