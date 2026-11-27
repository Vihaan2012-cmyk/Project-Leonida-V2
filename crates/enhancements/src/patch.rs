//! Code patches on the loaded image: found by instruction patterns with iced-x86, applied at instruction boundaries, reversible.
//!
//! Sites are found by decoding every function of the exception table from its first byte, so a pattern can only
//! match at a real instruction boundary and never inside an immediate or a jump table. A [`Patch`] is a byte
//! range that covers whole instructions (or one operand of one instruction), with the bytes it expects to find.
//! [`apply`] checks those bytes in memory first, flips page protection to read-write (never writable and
//! executable at once), writes, restores the protection and flushes the instruction cache. [`Applied::revert`]
//! undoes it. Pages are not executable while a write is in flight, so patches go in before the image's threads run.

use iced_x86::{ConstantOffsets, Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use pe_image::{Function, Image};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatchError {
    #[error("{name}: pattern not found")]
    NotFound { name: String },
    #[error("{name}: pattern found {count} times, expected once")]
    Ambiguous { name: String, count: usize },
    #[error("{what} at {rva:#x} is not in executable code")]
    NotCode { what: &'static str, rva: u32 },
    #[error("{what} at {rva:#x} does not start and end on instruction boundaries")]
    NotBoundary { what: &'static str, rva: u32 },
    #[error("{what} at {rva:#x} covers relocated bytes")]
    Relocated { what: &'static str, rva: u32 },
    #[error("replacement of {new} bytes does not fit the {old} bytes it replaces")]
    TooLong { old: usize, new: usize },
    #[error("instruction {index} has no {field}")]
    NoOperand { index: usize, field: &'static str },
    #[error("{rva:#x} is outside the image")]
    Outside { rva: u32 },
    #[error("bytes at {rva:#x} are {found:02x?}, expected {expected:02x?}")]
    Mismatch {
        rva: u32,
        expected: Vec<u8>,
        found: Vec<u8>,
    },
    #[error("{name} is already applied")]
    AlreadyApplied { name: String },
    #[error("page protection at {rva:#x} could not be changed (error {code})")]
    Protect { rva: u32, code: u32 },
}

/// Read access to the image for predicates: constants referenced by instructions.
pub struct Probe<'a> {
    image: &'a Image<'a>,
}

impl Probe<'_> {
    pub fn read(&self, rva: u32, len: usize) -> Option<&[u8]> {
        self.image.read(rva, len)
    }

    /// The RVA a RIP-relative memory operand points at.
    pub fn rip_target(&self, instruction: &Instruction) -> Option<u32> {
        instruction
            .is_ip_rel_memory_operand()
            .then(|| {
                instruction
                    .ip_rel_memory_address()
                    .wrapping_sub(self.image.image_base)
            })
            .and_then(|rva| u32::try_from(rva).ok())
    }

    pub fn f32_at(&self, rva: u32) -> Option<f32> {
        Some(f32::from_le_bytes(self.read(rva, 4)?.try_into().ok()?))
    }

    pub fn f64_at(&self, rva: u32) -> Option<f64> {
        Some(f64::from_le_bytes(self.read(rva, 8)?.try_into().ok()?))
    }

    pub fn u32_at(&self, rva: u32) -> Option<u32> {
        Some(u32::from_le_bytes(self.read(rva, 4)?.try_into().ok()?))
    }
}

type Predicate = Box<dyn Fn(&Instruction, &Probe) -> bool + Send + Sync>;

struct Step {
    /// Instructions that may come between this step and the one before it.
    gap: usize,
    test: Predicate,
}

/// A run of instructions to look for: each step is a test on one instruction, in order.
pub struct Pattern {
    name: String,
    steps: Vec<Step>,
}

impl Pattern {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            steps: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The next instruction, directly after the previous step.
    pub fn then(self, test: impl Fn(&Instruction, &Probe) -> bool + Send + Sync + 'static) -> Self {
        self.then_within(0, test)
    }

    /// The first instruction that passes `test` within `gap` instructions after the previous step.
    pub fn then_within(
        mut self,
        gap: usize,
        test: impl Fn(&Instruction, &Probe) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.steps.push(Step {
            gap,
            test: Box::new(test),
        });
        self
    }
}

/// Ready-made tests for pattern steps.
pub mod test {
    use super::*;

    /// A call or jump through `[register + displacement]`, as a call to a COM method does.
    pub fn calls_slot(displacement: u64) -> impl Fn(&Instruction, &Probe) -> bool + Send + Sync {
        move |i, _| {
            matches!(i.mnemonic(), Mnemonic::Call | Mnemonic::Jmp)
                && i.op0_kind() == OpKind::Memory
                && i.memory_base() != Register::RIP
                && i.memory_base() != Register::None
                && i.memory_index() == Register::None
                && i.memory_displacement64() == displacement
        }
    }

    /// Any instruction that reads a 4-byte float from memory (RIP-relative) for which `accept` holds.
    pub fn loads_f32(
        accept: impl Fn(f32) -> bool + Send + Sync,
    ) -> impl Fn(&Instruction, &Probe) -> bool + Send + Sync {
        move |i, probe| {
            !matches!(i.mnemonic(), Mnemonic::Lea | Mnemonic::Call | Mnemonic::Jmp)
                && probe
                    .rip_target(i)
                    .and_then(|rva| probe.f32_at(rva))
                    .is_some_and(&accept)
        }
    }

    /// `mov r32, imm32` of this value.
    pub fn mov_imm32(value: u32) -> impl Fn(&Instruction, &Probe) -> bool + Send + Sync {
        move |i, _| {
            i.mnemonic() == Mnemonic::Mov
                && i.op0_kind() == OpKind::Register
                && i.op0_register().is_gpr32()
                && i.op1_kind() == OpKind::Immediate32
                && i.immediate32() == value
        }
    }
}

/// One instruction of a matched site.
#[derive(Debug, Clone, Copy)]
pub struct SiteInstruction {
    pub rva: u32,
    pub len: u8,
    pub instruction: Instruction,
}

/// Where a pattern matched: the function and the matched instructions in order.
#[derive(Debug, Clone)]
pub struct Site {
    pub function: Function,
    pub instructions: Vec<SiteInstruction>,
}

impl Site {
    pub fn rva(&self) -> u32 {
        self.instructions[0].rva
    }
}

/// Finds sites in an image and builds patches that are safe to apply.
pub struct Finder<'a> {
    image: &'a Image<'a>,
    functions: Vec<Function>,
    relocated: Vec<u32>,
}

impl<'a> Finder<'a> {
    pub fn new(image: &'a Image<'a>) -> Self {
        let functions = image
            .functions()
            .into_iter()
            .filter(|f| image.section_of(f.begin).is_some_and(|s| s.executable()))
            .collect();
        let mut relocated: Vec<u32> = image
            .relocations()
            .into_iter()
            .filter(|(_, kind)| *kind != 0)
            .map(|(rva, _)| rva)
            .collect();
        relocated.sort_unstable();
        Self {
            image,
            functions,
            relocated,
        }
    }

    pub fn image(&self) -> &Image<'a> {
        self.image
    }

    /// Calls `visit` with the decoded instructions of each function, from its first byte.
    fn decode(&self, function: Function) -> Vec<SiteInstruction> {
        let Some(code) = self
            .image
            .read(function.begin, (function.end - function.begin) as usize)
        else {
            return Vec::new();
        };
        let base = self.image.image_base;
        let mut decoder = Decoder::with_ip(
            64,
            code,
            base + u64::from(function.begin),
            DecoderOptions::NONE,
        );
        let mut out = Vec::new();
        while decoder.can_decode() {
            let instruction = decoder.decode();
            if instruction.is_invalid() {
                break;
            }
            out.push(SiteInstruction {
                rva: (instruction.ip() - base) as u32,
                len: instruction.len() as u8,
                instruction,
            });
        }
        out
    }

    fn matches(
        &self,
        pattern: &Pattern,
        code: &[SiteInstruction],
        out: &mut Vec<Vec<SiteInstruction>>,
    ) {
        let probe = Probe { image: self.image };
        let Some(first) = pattern.steps.first() else {
            return;
        };
        for start in 0..code.len() {
            if !(first.test)(&code[start].instruction, &probe) {
                continue;
            }
            let mut found = vec![code[start]];
            let mut at = start;
            let mut complete = true;
            for step in &pattern.steps[1..] {
                let last = (at + 1 + step.gap).min(code.len().saturating_sub(1));
                match (at + 1..=last).find(|&j| (step.test)(&code[j].instruction, &probe)) {
                    Some(j) => {
                        found.push(code[j]);
                        at = j;
                    }
                    None => {
                        complete = false;
                        break;
                    }
                }
            }
            if complete {
                out.push(found);
            }
        }
    }

    /// Every site of `pattern` in executable functions.
    pub fn find(&self, pattern: &Pattern) -> Vec<Site> {
        self.functions
            .iter()
            .flat_map(|&function| self.find_in(function, pattern))
            .collect()
    }

    /// Every site of `pattern` inside one function.
    pub fn find_in(&self, function: Function, pattern: &Pattern) -> Vec<Site> {
        let code = self.decode(function);
        let mut found = Vec::new();
        self.matches(pattern, &code, &mut found);
        found
            .into_iter()
            .map(|instructions| Site {
                function,
                instructions,
            })
            .collect()
    }

    /// The only site of `pattern`; an error when there is none or several.
    pub fn find_one(&self, pattern: &Pattern) -> Result<Site, PatchError> {
        let mut sites = self.find(pattern);
        match sites.len() {
            0 => Err(PatchError::NotFound {
                name: pattern.name.clone(),
            }),
            1 => Ok(sites.remove(0)),
            count => Err(PatchError::Ambiguous {
                name: pattern.name.clone(),
                count,
            }),
        }
    }

    fn unrelocated(&self, what: &'static str, rva: u32, len: usize) -> Result<(), PatchError> {
        let end = rva as u64 + len as u64;
        let first = self
            .relocated
            .partition_point(|r| u64::from(*r) + 8 <= u64::from(rva));
        match self.relocated.get(first) {
            Some(r) if u64::from(*r) < end => Err(PatchError::Relocated { what, rva }),
            _ => Ok(()),
        }
    }

    fn original(&self, rva: u32, len: usize) -> Result<Vec<u8>, PatchError> {
        self.image
            .read(rva, len)
            .map(<[u8]>::to_vec)
            .ok_or(PatchError::Outside { rva })
    }

    /// A patch that replaces whole instructions from `rva` on with `replacement`, padded to the replaced length
    /// with NOPs. `rva` and the end of the replaced range must be instruction boundaries.
    pub fn patch_instructions(
        &self,
        name: impl Into<String>,
        rva: u32,
        replaced: usize,
        replacement: &[u8],
    ) -> Result<Patch, PatchError> {
        if replacement.len() > replaced {
            return Err(PatchError::TooLong {
                old: replaced,
                new: replacement.len(),
            });
        }
        let function = self
            .image
            .function_containing(rva)
            .filter(|f| self.functions.contains(f))
            .ok_or(PatchError::NotCode { what: "patch", rva })?;
        let end = rva + replaced as u32;
        let code = self.decode(function);
        let boundary = |at: u32| at == function.end || code.iter().any(|i| i.rva == at);
        if !boundary(rva) || !boundary(end) || end > function.end {
            return Err(PatchError::NotBoundary { what: "patch", rva });
        }
        self.unrelocated("patch", rva, replaced)?;
        let mut bytes = replacement.to_vec();
        nop_fill(&mut bytes, replaced);
        Ok(Patch {
            name: name.into(),
            rva,
            original: self.original(rva, replaced)?,
            replacement: bytes,
        })
    }

    /// Replaces the instruction `index` of `site` with `replacement` (NOP-padded).
    pub fn replace(
        &self,
        name: impl Into<String>,
        site: &Site,
        index: usize,
        replacement: &[u8],
    ) -> Result<Patch, PatchError> {
        let target = site.instructions[index];
        self.patch_instructions(name, target.rva, usize::from(target.len), replacement)
    }

    /// Replaces the instruction `index` of `site` with NOPs.
    pub fn nop(
        &self,
        name: impl Into<String>,
        site: &Site,
        index: usize,
    ) -> Result<Patch, PatchError> {
        self.replace(name, site, index, &[])
    }

    fn constants(&self, target: &SiteInstruction) -> Option<ConstantOffsets> {
        let bytes = self.image.read(target.rva, usize::from(target.len))?;
        let mut decoder = Decoder::with_ip(
            64,
            bytes,
            self.image.image_base + u64::from(target.rva),
            DecoderOptions::NONE,
        );
        let instruction = decoder.decode();
        Some(decoder.get_constant_offsets(&instruction))
    }

    /// Replaces the immediate of the instruction `index` of `site`; `value` must be as long as the immediate.
    pub fn set_immediate(
        &self,
        name: impl Into<String>,
        site: &Site,
        index: usize,
        value: &[u8],
    ) -> Result<Patch, PatchError> {
        let target = site.instructions[index];
        let offsets = self
            .constants(&target)
            .filter(|o| o.has_immediate())
            .ok_or(PatchError::NoOperand {
                index,
                field: "immediate",
            })?;
        if offsets.immediate_size() != value.len() {
            return Err(PatchError::TooLong {
                old: offsets.immediate_size(),
                new: value.len(),
            });
        }
        let rva = target.rva + offsets.immediate_offset() as u32;
        self.unrelocated("immediate", rva, value.len())?;
        Ok(Patch {
            name: name.into(),
            rva,
            original: self.original(rva, value.len())?,
            replacement: value.to_vec(),
        })
    }

    /// Replaces the 32-bit displacement of the instruction `index` of `site` (a RIP-relative operand's offset).
    pub fn set_displacement(
        &self,
        name: impl Into<String>,
        site: &Site,
        index: usize,
        value: i32,
    ) -> Result<Patch, PatchError> {
        let target = site.instructions[index];
        let offsets = self
            .constants(&target)
            .filter(|o| o.has_displacement() && o.displacement_size() == 4)
            .ok_or(PatchError::NoOperand {
                index,
                field: "32-bit displacement",
            })?;
        let rva = target.rva + offsets.displacement_offset() as u32;
        self.unrelocated("displacement", rva, 4)?;
        Ok(Patch {
            name: name.into(),
            rva,
            original: self.original(rva, 4)?,
            replacement: value.to_le_bytes().to_vec(),
        })
    }
}

/// Pads `bytes` to `len` with multi-byte NOPs.
pub fn nop_fill(bytes: &mut Vec<u8>, len: usize) {
    const NOPS: [&[u8]; 9] = [
        &[0x90],
        &[0x66, 0x90],
        &[0x0F, 0x1F, 0x00],
        &[0x0F, 0x1F, 0x40, 0x00],
        &[0x0F, 0x1F, 0x44, 0x00, 0x00],
        &[0x66, 0x0F, 0x1F, 0x44, 0x00, 0x00],
        &[0x0F, 0x1F, 0x80, 0x00, 0x00, 0x00, 0x00],
        &[0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
        &[0x66, 0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
    ];
    while bytes.len() < len {
        let step = (len - bytes.len()).min(NOPS.len());
        bytes.extend_from_slice(NOPS[step - 1]);
    }
}

/// A byte range of the image and the bytes that replace it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub name: String,
    pub rva: u32,
    /// What the image holds there before the patch.
    pub original: Vec<u8>,
    pub replacement: Vec<u8>,
}

/// Memory of a mapped image that code can be written into.
pub trait CodeMemory {
    fn read(&self, rva: u32, len: usize) -> Result<Vec<u8>, PatchError>;
    /// Writes into pages that may be read-only or read-execute, leaving their protection as it was.
    fn write(&mut self, rva: u32, bytes: &[u8]) -> Result<(), PatchError>;
}

/// A patch that has been written, and what it needs to be undone.
#[derive(Debug)]
pub struct Applied {
    patch: Patch,
}

impl Applied {
    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Puts the original bytes back, if the patched bytes are still there.
    pub fn revert(self, memory: &mut dyn CodeMemory) -> Result<(), PatchError> {
        let found = memory.read(self.patch.rva, self.patch.replacement.len())?;
        if found != self.patch.replacement {
            return Err(PatchError::Mismatch {
                rva: self.patch.rva,
                expected: self.patch.replacement,
                found,
            });
        }
        memory.write(self.patch.rva, &self.patch.original)
    }
}

/// Checks the original bytes are in memory and writes the replacement.
pub fn apply(memory: &mut dyn CodeMemory, patch: &Patch) -> Result<Applied, PatchError> {
    let found = memory.read(patch.rva, patch.original.len())?;
    if found != patch.original {
        if found == patch.replacement {
            return Err(PatchError::AlreadyApplied {
                name: patch.name.clone(),
            });
        }
        return Err(PatchError::Mismatch {
            rva: patch.rva,
            expected: patch.original.clone(),
            found,
        });
    }
    memory.write(patch.rva, &patch.replacement)?;
    Ok(Applied {
        patch: patch.clone(),
    })
}

/// Applies every patch or none: when one fails, those already written are reverted.
pub fn apply_all(
    memory: &mut dyn CodeMemory,
    patches: &[Patch],
) -> Result<Vec<Applied>, PatchError> {
    let mut done = Vec::with_capacity(patches.len());
    for patch in patches {
        match apply(memory, patch) {
            Ok(applied) => done.push(applied),
            Err(error) => {
                revert_all(memory, done);
                return Err(error);
            }
        }
    }
    Ok(done)
}

/// Reverts in the opposite order. A patch that cannot be reverted is skipped.
pub fn revert_all(memory: &mut dyn CodeMemory, applied: Vec<Applied>) {
    for patch in applied.into_iter().rev() {
        let _ = patch.revert(memory);
    }
}

/// An image held in a byte buffer, for checking patches without a mapping.
pub struct Buffer(pub Vec<u8>);

impl CodeMemory for Buffer {
    fn read(&self, rva: u32, len: usize) -> Result<Vec<u8>, PatchError> {
        let start = rva as usize;
        self.0
            .get(start..start.checked_add(len).ok_or(PatchError::Outside { rva })?)
            .map(<[u8]>::to_vec)
            .ok_or(PatchError::Outside { rva })
    }

    fn write(&mut self, rva: u32, bytes: &[u8]) -> Result<(), PatchError> {
        let start = rva as usize;
        let end = start
            .checked_add(bytes.len())
            .ok_or(PatchError::Outside { rva })?;
        self.0
            .get_mut(start..end)
            .ok_or(PatchError::Outside { rva })?
            .copy_from_slice(bytes);
        Ok(())
    }
}

#[cfg(windows)]
pub use mapped::Mapped;

#[cfg(windows)]
mod mapped {
    use super::{CodeMemory, PatchError};
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
        PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READONLY,
        PAGE_READWRITE, PAGE_WRITECOPY, VirtualProtect, VirtualQuery,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// An image mapped into this process.
    pub struct Mapped {
        base: usize,
        size: usize,
    }

    impl Mapped {
        /// # Safety
        /// `base..base + size` must be a mapping of the image that stays mapped as long as this value is used.
        pub unsafe fn new(base: usize, size: usize) -> Self {
            Self { base, size }
        }

        fn range(&self, rva: u32, len: usize) -> Result<usize, PatchError> {
            let start = rva as usize;
            match start.checked_add(len) {
                Some(end) if end <= self.size => Ok(self.base + start),
                _ => Err(PatchError::Outside { rva }),
            }
        }
    }

    /// The same page access without execute, and never writable and executable together.
    fn writable(old: PAGE_PROTECTION_FLAGS) -> Option<PAGE_PROTECTION_FLAGS> {
        if old & PAGE_GUARD != 0 {
            return None;
        }
        match old & 0xFF {
            PAGE_READWRITE | PAGE_WRITECOPY => Some(PAGE_READWRITE),
            PAGE_READONLY
            | PAGE_EXECUTE
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY => Some(PAGE_READWRITE),
            PAGE_NOACCESS => None,
            _ => None,
        }
    }

    impl CodeMemory for Mapped {
        fn read(&self, rva: u32, len: usize) -> Result<Vec<u8>, PatchError> {
            let at = self.range(rva, len)?;
            // SAFETY: inside the mapping the constructor's caller vouched for; image pages are readable.
            Ok(unsafe { std::slice::from_raw_parts(at as *const u8, len) }.to_vec())
        }

        fn write(&mut self, rva: u32, bytes: &[u8]) -> Result<(), PatchError> {
            let at = self.range(rva, bytes.len())?;
            let end = at + bytes.len();
            let mut changed: Vec<(usize, usize, PAGE_PROTECTION_FLAGS)> = Vec::new();
            let mut failure = None;
            let mut cursor = at;
            while cursor < end {
                // SAFETY: `info` is a plain out-structure; the query reads no memory of the image.
                let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
                let queried = unsafe {
                    VirtualQuery(
                        cursor as *const _,
                        &mut info,
                        std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                let region_end = info.BaseAddress as usize + info.RegionSize;
                let Some(open) = (queried != 0).then(|| writable(info.Protect)).flatten() else {
                    failure = Some(PatchError::Protect { rva, code: 0 });
                    break;
                };
                let chunk_end = end.min(region_end);
                let mut previous = 0;
                // SAFETY: a page range of the mapping; only read-write, never execute, is requested.
                let done = unsafe {
                    VirtualProtect(cursor as *const _, chunk_end - cursor, open, &mut previous)
                };
                if done == 0 {
                    failure = Some(PatchError::Protect {
                        rva,
                        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
                    });
                    break;
                }
                changed.push((cursor, chunk_end - cursor, info.Protect));
                cursor = chunk_end;
            }
            if failure.is_none() {
                // SAFETY: every page of the range was made read-write above; the range lies in the mapping.
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
                }
            }
            for (start, len, old) in changed.into_iter().rev() {
                let mut previous = 0;
                // SAFETY: puts back the protection these pages had.
                let restored =
                    unsafe { VirtualProtect(start as *const _, len, old, &mut previous) };
                if restored == 0 && failure.is_none() {
                    failure = Some(PatchError::Protect {
                        rva,
                        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
                    });
                }
            }
            // SAFETY: flushes the cache for the range just written.
            unsafe {
                FlushInstructionCache(GetCurrentProcess(), at as *const _, bytes.len());
            }
            failure.map_or(Ok(()), Err)
        }
    }
}
