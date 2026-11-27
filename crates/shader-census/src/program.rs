use std::ops::ControlFlow;

use serde::{Serialize, Serializer};
use shader_container::{CodeRange, Fssm, Stage, WaveSize, carve, find_part, parts, unpack_all};

use crate::analysis::{Analysis, analyze};

/// A set of user SGPRs, `s0..s31`, written as a list of register numbers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegSet(pub u32);

impl RegSet {
    fn of(mask: u128) -> RegSet {
        RegSet(mask as u32)
    }

    pub fn contains(self, reg: u32) -> bool {
        reg < 32 && self.0 >> reg & 1 != 0
    }

    pub fn lowest(self) -> Option<u32> {
        (self.0 != 0).then(|| self.0.trailing_zeros())
    }

    pub fn highest(self) -> Option<u32> {
        (self.0 != 0).then(|| 31 - self.0.leading_zeros())
    }

    pub fn iter(self) -> impl Iterator<Item = u32> {
        (0..32).filter(move |&r| self.contains(r))
    }
}

impl Serialize for RegSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

/// How a program uses one entry SGPR, in the order a table or view parameter would show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Use {
    /// The low dword of a 64-bit address: a root view or a 64-bit base.
    Pointer64,
    /// The high dword of one.
    PointerHigh,
    /// A 32-bit base: a descriptor table or heap.
    Pointer32,
    /// The first dword of a descriptor held in entry SGPRs.
    Descriptor,
    /// An offset or a descriptor word that is not an address.
    Offset,
    Plain,
}

/// What one GPU program does with the SGPRs it starts with, and what its container says about it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    pub stage: String,
    /// The first of the two programs a vertex shader carries when it is merged with a geometry shader.
    pub first_half: bool,
    pub wave64: bool,
    /// The user SGPR count in the program header (bits 16-21 of the word at 0xD4).
    pub header_user_sgprs: u32,
    /// `COMPUTE_PGM_RSRC2.USER_SGPR`, compute only.
    pub rsrc2_user_sgprs: Option<u32>,
    pub has_rts0: bool,
    pub sfi0: Option<u64>,
    pub instructions: usize,
    pub invalid_words: usize,
    pub reads: RegSet,
    /// The highest register at or above `s32` read before it was written.
    pub highest_system_read: Option<u32>,
    pub load_ptr32: RegSet,
    pub load_ptr64: RegSet,
    pub load_ptr64_hi: RegSet,
    pub vsharp: RegSet,
    pub tsharp: RegSet,
    pub ssharp: RegSet,
    pub desc_hi: RegSet,
    pub desc_word: RegSet,
    pub desc_direct: RegSet,
    pub offset: RegSet,
    pub pc_mix: RegSet,
    pub set_pc: RegSet,
    pub mixed: RegSet,
    pub uses_getpc: bool,
    pub uses_setpc: bool,
    pub loads: usize,
    pub loads_from_entry: usize,
    pub loads_from_pc: usize,
    pub loads_chained: usize,
}

/// `SFI0` shader feature bits: the program indexes the resource or sampler descriptor heap directly.
pub const SFI0_RESOURCE_HEAP: u64 = 1 << 25;
pub const SFI0_SAMPLER_HEAP: u64 = 1 << 26;

impl Program {
    /// Every register used as something other than a plain 32-bit value.
    pub fn classified(&self) -> RegSet {
        RegSet(
            self.load_ptr32.0
                | self.load_ptr64.0
                | self.load_ptr64_hi.0
                | self.vsharp.0
                | self.tsharp.0
                | self.ssharp.0
                | self.desc_hi.0
                | self.desc_word.0
                | self.offset.0
                | self.pc_mix.0
                | self.set_pc.0
                | self.mixed.0,
        )
    }

    /// How the program uses `reg`, which it must read before writing.
    pub fn use_of(&self, reg: u32) -> Use {
        let pair = self.vsharp.contains(reg) && self.desc_hi.contains(reg + 1);
        if self.load_ptr64.contains(reg) || pair {
            Use::Pointer64
        } else if self.load_ptr64_hi.contains(reg) || self.desc_hi.contains(reg) {
            Use::PointerHigh
        } else if self.load_ptr32.contains(reg) {
            Use::Pointer32
        } else if self.vsharp.contains(reg)
            || self.tsharp.contains(reg)
            || self.ssharp.contains(reg)
        {
            Use::Descriptor
        } else if self.offset.contains(reg) || self.desc_word.contains(reg) {
            Use::Offset
        } else {
            Use::Plain
        }
    }

    /// Registers read only as 32-bit values.
    pub fn plain(&self) -> RegSet {
        RegSet(self.reads.0 & !self.classified().0)
    }

    /// Registers whose value is a pointer or the start of a descriptor: the ones a table or view lands in.
    pub fn pointers(&self) -> RegSet {
        RegSet(
            self.load_ptr32.0 | self.load_ptr64.0 | self.vsharp.0 | self.tsharp.0 | self.ssharp.0,
        )
    }

    pub fn heap_resource(&self) -> bool {
        self.sfi0.is_some_and(|f| f & SFI0_RESOURCE_HEAP != 0)
    }

    pub fn heap_sampler(&self) -> bool {
        self.sfi0.is_some_and(|f| f & SFI0_SAMPLER_HEAP != 0)
    }

    fn new(name: &str, stage: &str, first_half: bool, fssm: &Fssm<'_>, found: Analysis) -> Program {
        let system = found.read_first >> 32;
        Program {
            name: name.to_string(),
            stage: stage.to_string(),
            first_half,
            wave64: fssm.wave_size() == WaveSize::Wave64,
            header_user_sgprs: fssm.user_sgprs(),
            rsrc2_user_sgprs: None,
            has_rts0: false,
            sfi0: None,
            instructions: found.instructions,
            invalid_words: found.invalid_words,
            reads: RegSet::of(found.read_first),
            highest_system_read: (system != 0).then(|| 32 + 127 - system.leading_zeros()),
            load_ptr32: RegSet::of(found.load_ptr32),
            load_ptr64: RegSet::of(found.load_ptr64),
            load_ptr64_hi: RegSet::of(found.load_ptr64_hi),
            vsharp: RegSet::of(found.vsharp),
            tsharp: RegSet::of(found.tsharp),
            ssharp: RegSet::of(found.ssharp),
            desc_hi: RegSet::of(found.desc_hi),
            desc_word: RegSet::of(found.desc_word),
            desc_direct: RegSet::of(found.desc_direct),
            offset: RegSet::of(found.offset),
            pc_mix: RegSet::of(found.pc_mix),
            set_pc: RegSet::of(found.set_pc),
            mixed: RegSet::of(found.mixed),
            uses_getpc: found.uses_getpc,
            uses_setpc: found.uses_setpc,
            loads: found.loads,
            loads_from_entry: found.loads_from_entry,
            loads_from_pc: found.loads_from_pc,
            loads_chained: found.loads_chained,
        }
    }
}

/// Runs the census on one program blob (the FSSM bytes).
pub fn census_blob(name: &str, stage: &str, first_half: bool, blob: &[u8]) -> Option<Program> {
    let fssm = Fssm::parse(blob).ok()?;
    let CodeRange::Header { start, end } = fssm.code_range() else {
        return None;
    };
    let code = blob.get(start..end)?;
    let found = analyze(code, fssm.wave_size() == WaveSize::Wave32);
    let mut program = Program::new(name, stage, first_half, &fssm, found);
    if stage == "cs"
        && let Some((rsrc2, _)) = fssm.compute_setup()
    {
        program.rsrc2_user_sgprs = Some(rsrc2 >> 1 & 0x1F);
    }
    Some(program)
}

/// The programs of one shader container: its last program, and a vertex shader's extra first half.
pub fn census_container(name: &str, stage: Option<Stage>, dxbc: &[u8]) -> Vec<Program> {
    let label = stage.map_or_else(|| "none".to_string(), Stage::short_name);
    let Ok(blobs) = unpack_all(dxbc) else {
        return Vec::new();
    };
    let list = parts(dxbc).unwrap_or_default();
    let has_rts0 = find_part(&list, b"RTS0").is_some();
    let sfi0 = find_part(&list, b"SFI0")
        .and_then(|part| part.get(..8))
        .map(|bytes| u64::from_le_bytes(bytes.try_into().expect("8 bytes")));
    let mut out = Vec::new();
    for (index, blob) in blobs.iter().enumerate() {
        let first_half = stage == Some(Stage::Vertex) && blobs.len() == 2 && index == 0;
        if index + 1 != blobs.len() && !first_half {
            continue;
        }
        if let Some(mut program) = census_blob(name, &label, first_half, blob) {
            program.has_rts0 = has_rts0;
            program.sfi0 = sfi0;
            out.push(program);
        }
    }
    out
}

/// Which containers of `shaders.rpf` to read.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Stage names as `Stage::short_name` gives them: `ps`, `vs`, `gs`, `ms`, `cs`.
    pub stages: Vec<String>,
    /// Stop after this many containers have been read.
    pub limit: Option<usize>,
    /// Read every Nth container.
    pub every: usize,
}

/// The census of the containers of an archive that `selection` picks.
pub fn census_archive(archive: &[u8], selection: &Selection) -> Vec<Program> {
    let mut programs = Vec::new();
    let mut seen = 0usize;
    let mut taken = 0usize;
    let step = selection.every.max(1);
    carve(archive, |shader| {
        let label = shader
            .stage
            .map_or_else(|| "none".to_string(), Stage::short_name);
        if !selection.stages.is_empty() && !selection.stages.contains(&label) {
            return ControlFlow::Continue(());
        }
        seen += 1;
        if !(seen - 1).is_multiple_of(step) {
            return ControlFlow::Continue(());
        }
        taken += 1;
        programs.extend(census_container(&shader.name, shader.stage, &shader.dxbc));
        if selection.limit.is_some_and(|limit| taken >= limit) {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    programs
}
