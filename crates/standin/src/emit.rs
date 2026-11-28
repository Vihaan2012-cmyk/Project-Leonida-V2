//! The emitter: x86-64 code for the Windows x64 calling convention over `iced-x86`'s `CodeAssembler`, and the
//! declarations (imports and data) the code refers to.
//!
//! Code is assembled twice. The first pass only collects what the code names, so the image's layout can be
//! fixed; the second assembles with the real addresses. Every reference to imports and data is RIP-relative with
//! a 32-bit displacement, so both passes produce the same bytes in the same sizes.

use std::collections::HashMap;

use iced_x86::code_asm::{CodeAssembler, CodeLabel, eax, qword_ptr, rax, rcx, rdx, rsp};
use iced_x86::{BlockEncoderOptions, Code, IcedError, Instruction, MemoryOperand, Register};

/// The image base the stand-in asks for.
pub const BASE: u64 = 0x1_8000_0000;

/// The stack a boot function reserves: 0x20 bytes of shadow space, room for six stack arguments, locals the steps
/// never need, and 8 more so the stack is 16-byte aligned at every call (the return address makes it 8 off on
/// entry).
pub const ENTRY_FRAME: u32 = 0x78;

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("assembling: {0}")]
    Asm(#[from] IcedError),
    #[error("{0}")]
    Layout(String),
}

pub type Res<T = ()> = Result<T, BuildError>;

/// Something the code can address: a declared data item, an import's address-table slot, or the image's first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    Data(usize),
    Import(usize),
    Image,
}

/// A symbol plus a byte offset into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub sym: Sym,
    pub offset: u32,
}

impl Sym {
    pub fn at(self, offset: u32) -> Place {
        Place { sym: self, offset }
    }
}

impl From<Sym> for Place {
    fn from(sym: Sym) -> Place {
        Place { sym, offset: 0 }
    }
}

/// What a pointer stored in data refers to once the image is placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Place(Place),
    Function(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportName {
    Name(String),
    Ordinal(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDecl {
    pub dll: String,
    pub name: ImportName,
}

#[derive(Debug, Clone)]
pub struct DataDecl {
    pub name: String,
    pub align: usize,
    pub init: Vec<u8>,
    /// Eight-byte fields that hold an absolute address, with what they point at. Each gets a base relocation.
    pub pointers: Vec<(usize, Target)>,
}

/// Everything the code names, in first-use order.
#[derive(Debug, Default, Clone)]
pub struct Decls {
    pub imports: Vec<ImportDecl>,
    pub data: Vec<DataDecl>,
    names: HashMap<String, usize>,
    import_names: HashMap<(String, String), usize>,
}

impl Decls {
    pub fn data_named(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }
}

/// Where each declaration lies, as offsets from the image base.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub data_rva: Vec<u32>,
    pub iat_rva: Vec<u32>,
}

/// One boot step, for reporting which one failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepInfo {
    pub id: u32,
    pub stage: crate::Stage,
    pub name: &'static str,
}

/// A function whose bounds the exception table records.
#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub name: String,
    pub begin: CodeLabel,
    pub end: CodeLabel,
    pub frame: u32,
}

/// An argument of a call.
#[derive(Debug, Clone, Copy)]
pub enum Arg {
    /// A constant, zero-extended to 64 bits.
    Imm(u64),
    /// The address of a place.
    Addr(Place),
    /// The eight bytes at a place.
    Load64(Place),
    /// The four bytes at a place, zero-extended.
    Load32(Place),
}

impl Arg {
    pub fn addr(place: impl Into<Place>) -> Arg {
        Arg::Addr(place.into())
    }

    pub fn load64(place: impl Into<Place>) -> Arg {
        Arg::Load64(place.into())
    }

    pub fn load32(place: impl Into<Place>) -> Arg {
        Arg::Load32(place.into())
    }
}

fn arg_register(index: usize) -> (Register, Register) {
    match index {
        0 => (Register::RCX, Register::ECX),
        1 => (Register::RDX, Register::EDX),
        2 => (Register::R8, Register::R8D),
        _ => (Register::R9, Register::R9D),
    }
}

/// The assembled code and what it defined.
pub struct Assembled {
    pub code: Vec<u8>,
    pub steps: Vec<StepInfo>,
    /// Function name to offset from the image base, and its end.
    pub functions: Vec<(String, u32, u32, u32)>,
}

pub struct Emitter {
    /// The assembler, for code the helpers do not cover.
    pub asm: CodeAssembler,
    decls: Decls,
    layout: Option<Layout>,
    frozen: bool,
    steps: Vec<StepInfo>,
    functions: Vec<FunctionDecl>,
    labels: HashMap<String, CodeLabel>,
    fail: Option<CodeLabel>,
    exit: CodeLabel,
    stage: crate::Stage,
    step_number: u32,
    text_rva: u32,
}

impl Emitter {
    /// An emitter for the layout pass (no `layout`) or for the final pass.
    pub(crate) fn new(decls: Option<Decls>, layout: Option<Layout>, text_rva: u32) -> Res<Emitter> {
        let mut asm = CodeAssembler::new(64)?;
        let exit = asm.create_label();
        let frozen = decls.is_some();
        let mut emitter = Emitter {
            asm,
            decls: decls.unwrap_or_default(),
            layout,
            frozen,
            steps: Vec::new(),
            functions: Vec::new(),
            labels: HashMap::new(),
            fail: None,
            exit,
            stage: crate::Stage::Loader,
            step_number: 0,
            text_rva,
        };
        for (name, size) in [
            ("failed", 4),
            ("completed", 4),
            ("detail", 8),
            ("tls_index", 4),
            ("tls_reason", 4),
            ("tls_mask", 4),
            ("tls_calls", 4),
        ] {
            emitter.buffer(name, size);
        }
        Ok(emitter)
    }

    pub(crate) fn decls(&self) -> &Decls {
        &self.decls
    }

    // declarations

    fn declare(&mut self, decl: DataDecl) -> Sym {
        if let Some(&index) = self.decls.names.get(&decl.name) {
            return Sym::Data(index);
        }
        assert!(
            !self.frozen,
            "{} was declared in the final pass only",
            decl.name
        );
        let index = self.decls.data.len();
        self.decls.names.insert(decl.name.clone(), index);
        self.decls.data.push(decl);
        Sym::Data(index)
    }

    /// Zeroed writable bytes.
    pub fn buffer(&mut self, name: &str, size: usize) -> Sym {
        self.declare(DataDecl {
            name: name.to_string(),
            align: 16,
            init: vec![0; size],
            pointers: Vec::new(),
        })
    }

    /// Writable bytes with contents.
    pub fn bytes(&mut self, name: &str, bytes: &[u8]) -> Sym {
        self.declare(DataDecl {
            name: name.to_string(),
            align: 16,
            init: bytes.to_vec(),
            pointers: Vec::new(),
        })
    }

    /// Data with absolute-address fields: `pointers` are `(offset, target)` pairs inside `bytes`.
    pub fn bytes_with_pointers(
        &mut self,
        name: &str,
        bytes: &[u8],
        pointers: Vec<(usize, Target)>,
    ) -> Sym {
        self.declare(DataDecl {
            name: name.to_string(),
            align: 16,
            init: bytes.to_vec(),
            pointers,
        })
    }

    /// A NUL-terminated UTF-16 string.
    pub fn wide(&mut self, name: &str, text: &str) -> Sym {
        let bytes: Vec<u8> = text
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        self.bytes(name, &bytes)
    }

    /// A NUL-terminated 8-bit string.
    pub fn ascii(&mut self, name: &str, text: &str) -> Sym {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        self.bytes(name, &bytes)
    }

    /// An import by name; the program calls through its address-table slot.
    pub fn import(&mut self, dll: &str, name: &str) -> Sym {
        self.import_as(dll, ImportName::Name(name.to_string()))
    }

    /// An import by ordinal.
    pub fn import_ordinal(&mut self, dll: &str, ordinal: u16) -> Sym {
        self.import_as(dll, ImportName::Ordinal(ordinal))
    }

    fn import_as(&mut self, dll: &str, name: ImportName) -> Sym {
        let key = (
            dll.to_ascii_lowercase(),
            match &name {
                ImportName::Name(n) => n.clone(),
                ImportName::Ordinal(o) => format!("#{o}"),
            },
        );
        if let Some(&index) = self.decls.import_names.get(&key) {
            return Sym::Import(index);
        }
        assert!(
            !self.frozen,
            "import {key:?} was declared in the final pass only"
        );
        let index = self.decls.imports.len();
        self.decls.import_names.insert(key, index);
        self.decls.imports.push(ImportDecl {
            dll: dll.to_string(),
            name,
        });
        Sym::Import(index)
    }

    // addresses

    /// The address of a place: the real one in the final pass, and any reachable one in the layout pass.
    fn address(&self, place: Place) -> u64 {
        let Some(layout) = &self.layout else {
            return BASE + u64::from(self.text_rva);
        };
        let rva = match place.sym {
            Sym::Data(i) => layout.data_rva[i],
            Sym::Import(i) => layout.iat_rva[i],
            Sym::Image => 0,
        };
        BASE + u64::from(rva) + u64::from(place.offset)
    }

    fn memory(&self, place: impl Into<Place>) -> MemoryOperand {
        MemoryOperand::with_base_displ(Register::RIP, self.address(place.into()) as i64)
    }

    fn add(&mut self, instruction: Result<Instruction, IcedError>) -> Res {
        self.asm.add_instruction(instruction?)?;
        Ok(())
    }

    // single instructions on places

    pub fn lea(&mut self, to: Register, place: impl Into<Place>) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Lea_r64_m, to, memory))
    }

    pub fn load64(&mut self, to: Register, place: impl Into<Place>) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Mov_r64_rm64, to, memory))
    }

    pub fn load32(&mut self, to: Register, place: impl Into<Place>) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Mov_r32_rm32, to, memory))
    }

    pub fn store64(&mut self, place: impl Into<Place>, from: Register) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Mov_rm64_r64, memory, from))
    }

    pub fn store32(&mut self, place: impl Into<Place>, from: Register) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Mov_rm32_r32, memory, from))
    }

    pub fn store32_imm(&mut self, place: impl Into<Place>, value: u32) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Mov_rm32_imm32, memory, value))
    }

    pub fn add32_imm(&mut self, place: impl Into<Place>, value: i32) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Add_rm32_imm32, memory, value))
    }

    pub fn or32_imm(&mut self, place: impl Into<Place>, value: u32) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with2(Code::Or_rm32_imm32, memory, value))
    }

    /// `lock inc dword [place]`.
    pub fn lock_inc32(&mut self, place: impl Into<Place>) -> Res {
        let memory = self.memory(place);
        let mut instruction = Instruction::with1(Code::Inc_rm32, memory)?;
        instruction.set_has_lock_prefix(true);
        self.asm.add_instruction(instruction)?;
        Ok(())
    }

    /// `call [place]`, through an import's slot or a pointer in data.
    pub fn call_place(&mut self, place: impl Into<Place>) -> Res {
        let memory = self.memory(place);
        self.add(Instruction::with1(Code::Call_rm64, memory))
    }

    // calls

    fn put_argument(&mut self, index: usize, arg: Arg) -> Res {
        if index >= 4 {
            self.put_register(Register::RAX, Register::EAX, arg)?;
            self.asm
                .mov(qword_ptr(rsp + (0x20 + 8 * (index - 4)) as i32), rax)?;
            return Ok(());
        }
        let (wide, narrow) = arg_register(index);
        self.put_register(wide, narrow, arg)
    }

    fn put_register(&mut self, wide: Register, narrow: Register, arg: Arg) -> Res {
        match arg {
            Arg::Imm(0) => self.add(Instruction::with2(Code::Xor_r32_rm32, narrow, narrow)),
            Arg::Imm(v) if v <= u64::from(u32::MAX) => {
                self.add(Instruction::with2(Code::Mov_r32_imm32, narrow, v as u32))
            }
            Arg::Imm(v) => self.add(Instruction::with2(Code::Mov_r64_imm64, wide, v)),
            Arg::Addr(place) => self.lea(wide, place),
            Arg::Load64(place) => self.load64(wide, place),
            Arg::Load32(place) => self.load32(narrow, place),
        }
    }

    fn put_arguments(&mut self, args: &[Arg]) -> Res {
        if args.len() > 10 {
            return Err(BuildError::Layout("more than ten arguments".into()));
        }
        for (index, arg) in args.iter().enumerate().skip(4) {
            self.put_argument(index, *arg)?;
        }
        for (index, arg) in args.iter().enumerate().take(4) {
            self.put_argument(index, *arg)?;
        }
        Ok(())
    }

    /// Calls an import (or any pointer at `place`) with `args`; the result is in `rax`.
    pub fn call(&mut self, target: impl Into<Place>, args: &[Arg]) -> Res {
        self.put_arguments(args)?;
        self.call_place(target)
    }

    /// Calls slot `slot` of the object whose pointer is stored at `object`. `this` is the first argument.
    pub fn vcall(&mut self, object: impl Into<Place>, slot: u32, args: &[Arg]) -> Res {
        let mut all = vec![Arg::Load64(object.into())];
        all.extend_from_slice(args);
        self.put_arguments(&all)?;
        self.asm.mov(rax, qword_ptr(rcx))?;
        self.asm.call(qword_ptr(rax + (8 * slot) as i32))?;
        Ok(())
    }

    // steps and checks

    pub(crate) fn begin_step(&mut self, stage: crate::Stage, name: &'static str) -> u32 {
        if stage != self.stage {
            self.stage = stage;
            self.step_number = 0;
        }
        self.step_number += 1;
        let id = (stage as u32) << 8 | self.step_number;
        self.steps.push(StepInfo { id, stage, name });
        self.fail = Some(self.asm.create_label());
        id
    }

    /// Ends a step: on success it counts the step done and falls through; every check that failed lands in code
    /// that records the step id and the value that failed, and leaves the entry point.
    pub(crate) fn end_step(&mut self, id: u32) -> Res {
        let mut fail = self.fail.take().expect("a step is open");
        let mut next = self.asm.create_label();
        self.asm.jmp(next)?;
        self.asm.set_label(&mut fail)?;
        let detail = self.decls_place("detail");
        let failed = self.decls_place("failed");
        self.store64(detail, Register::RAX)?;
        self.store32_imm(failed, id)?;
        self.asm.jmp(self.exit)?;
        self.asm.set_label(&mut next)?;
        let completed = self.decls_place("completed");
        self.add32_imm(completed, 1)
    }

    fn decls_place(&self, name: &str) -> Place {
        Sym::Data(self.decls.names[name]).into()
    }

    /// A fixed cell the entry writes, by name.
    pub fn cell(&self, name: &str) -> Sym {
        Sym::Data(
            *self
                .decls
                .names
                .get(name)
                .unwrap_or_else(|| panic!("no data named {name}")),
        )
    }

    pub(crate) fn exit_label(&mut self) -> &mut CodeLabel {
        &mut self.exit
    }

    fn fail_label(&self) -> CodeLabel {
        self.fail.expect("checks belong to a step")
    }

    /// Fails the step when `eax` is a negative `HRESULT`.
    pub fn fail_if_hresult_failed(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.test(eax, eax)?;
        self.asm.js(fail)?;
        Ok(())
    }

    /// Fails the step when `eax` is not zero.
    pub fn fail_if_nonzero32(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.test(eax, eax)?;
        self.asm.jnz(fail)?;
        Ok(())
    }

    /// Fails the step when `eax` is zero (a `BOOL` that is false).
    pub fn fail_if_zero32(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.test(eax, eax)?;
        self.asm.jz(fail)?;
        Ok(())
    }

    /// Fails the step when `rax` is zero (a null pointer or handle).
    pub fn fail_if_zero64(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.test(rax, rax)?;
        self.asm.jz(fail)?;
        Ok(())
    }

    /// Fails the step when `rax` is not zero.
    pub fn fail_if_nonzero64(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.test(rax, rax)?;
        self.asm.jnz(fail)?;
        Ok(())
    }

    /// Fails the step when `rax` is -1 (an invalid handle).
    pub fn fail_if_invalid_handle(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.cmp(rax, -1)?;
        self.asm.je(fail)?;
        Ok(())
    }

    /// Fails the step when `eax` differs from `value`.
    pub fn fail_if_ne32(&mut self, value: u32) -> Res {
        let fail = self.fail_label();
        self.asm.cmp(eax, value as i32)?;
        self.asm.jne(fail)?;
        Ok(())
    }

    /// Fails the step when `rax` differs from `value`.
    pub fn fail_if_ne64(&mut self, value: u64) -> Res {
        let fail = self.fail_label();
        if let Ok(small) = i32::try_from(value as i64) {
            self.asm.cmp(rax, small)?;
        } else {
            self.asm.mov(rdx, value)?;
            self.asm.cmp(rax, rdx)?;
        }
        self.asm.jne(fail)?;
        Ok(())
    }

    /// Fails the step when the four bytes at `place` differ from `value`; `rax` holds what was read.
    pub fn fail_if_mem32_ne(&mut self, place: impl Into<Place>, value: u32) -> Res {
        self.load32(Register::EAX, place)?;
        self.fail_if_ne32(value)
    }

    /// Fails the step when `eax` is above `value` (unsigned).
    pub fn fail_if_above32(&mut self, value: u32) -> Res {
        let fail = self.fail_label();
        self.asm.cmp(eax, value as i32)?;
        self.asm.ja(fail)?;
        Ok(())
    }

    /// Fails the step when `eax` is below `value` (unsigned).
    pub fn fail_if_below32(&mut self, value: u32) -> Res {
        let fail = self.fail_label();
        self.asm.cmp(eax, value as i32)?;
        self.asm.jb(fail)?;
        Ok(())
    }

    /// Fails the step when `rax` is below `value` (unsigned, a small value).
    pub fn fail_if_below64(&mut self, value: u32) -> Res {
        let fail = self.fail_label();
        self.asm.cmp(rax, value as i32)?;
        self.asm.jb(fail)?;
        Ok(())
    }

    /// Fails the step unless the bit is set in `eax`.
    pub fn fail_unless_bit(&mut self, bit: u8) -> Res {
        let fail = self.fail_label();
        self.asm.bt(eax, u32::from(bit))?;
        self.asm.jnc(fail)?;
        Ok(())
    }

    /// Jumps to the step's failure code unconditionally.
    pub fn fail(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.jmp(fail)?;
        Ok(())
    }

    /// A fresh code label.
    pub fn label(&mut self) -> CodeLabel {
        self.asm.create_label()
    }

    pub fn place(&mut self, label: &mut CodeLabel) -> Res {
        self.asm.set_label(label)?;
        Ok(())
    }

    // functions

    /// Starts a function: its label, and a stack frame of `frame` bytes (zero for a leaf).
    pub fn begin_function(&mut self, name: &str, frame: u32) -> Res {
        let mut begin = self.asm.create_label();
        self.asm.set_label(&mut begin)?;
        if frame != 0 {
            self.asm.sub(rsp, frame as i32)?;
        }
        let end = self.asm.create_label();
        self.labels.insert(name.to_string(), begin);
        self.functions.push(FunctionDecl {
            name: name.to_string(),
            begin,
            end,
            frame,
        });
        Ok(())
    }

    /// Ends the function started last: the epilogue and its end marker.
    pub fn end_function(&mut self) -> Res {
        let function = self.functions.last().cloned().expect("a function is open");
        if function.frame != 0 {
            self.asm.add(rsp, function.frame as i32)?;
        }
        self.asm.ret()?;
        let mut end = function.end;
        self.asm.set_label(&mut end)?;
        self.asm.int3()?;
        if let Some(last) = self.functions.last_mut() {
            last.end = end;
        }
        Ok(())
    }

    /// Assembles at the section's address.
    pub(crate) fn finish(mut self, text_rva: u32) -> Res<Assembled> {
        let result = self.asm.assemble_options(
            BASE + u64::from(text_rva),
            BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS,
        )?;
        let at = |label: &CodeLabel| -> Res<u32> { Ok((result.label_ip(label)? - BASE) as u32) };
        let mut functions = Vec::new();
        for function in &self.functions {
            functions.push((
                function.name.clone(),
                at(&function.begin)?,
                at(&function.end)?,
                function.frame,
            ));
        }
        Ok(Assembled {
            code: result.inner.code_buffer,
            steps: self.steps,
            functions,
        })
    }
}

impl Emitter {
    /// `lock or dword [place], reg`.
    pub fn lock_or32(&mut self, place: impl Into<Place>, from: Register) -> Res {
        let memory = self.memory(place);
        let mut instruction = Instruction::with2(Code::Or_rm32_r32, memory, from)?;
        instruction.set_has_lock_prefix(true);
        self.asm.add_instruction(instruction)?;
        Ok(())
    }

    /// Leaves in `rcx` the calling thread's block of thread-local variables, found the way `/GA` code does: through
    /// the thread environment block at `gs:[0x58]` and the image's TLS index. Clobbers `rax`.
    pub fn tls_block(&mut self) -> Res {
        let index = self.cell("tls_index");
        self.load32(Register::EAX, index)?;
        let slots = MemoryOperand::new(
            Register::None,
            Register::None,
            1,
            0x58,
            4,
            false,
            Register::GS,
        );
        self.add(Instruction::with2(Code::Mov_r64_rm64, Register::RCX, slots))?;
        self.asm.mov(rcx, qword_ptr(rcx + rax * 8))?;
        Ok(())
    }
}

impl Emitter {
    /// Fails the step when `rax` differs from the address of `place`. Clobbers `rdx`.
    pub fn fail_if_rax_ne_address(&mut self, place: impl Into<Place>) -> Res {
        let fail = self.fail_label();
        self.lea(Register::RDX, place)?;
        self.asm.cmp(rax, rdx)?;
        self.asm.jne(fail)?;
        Ok(())
    }

    /// Fails the step when `rax` differs from the eight bytes at `cell`. Clobbers `rdx`.
    pub fn fail_if_rax_ne_cell(&mut self, cell: impl Into<Place>) -> Res {
        self.load64(Register::RDX, cell)?;
        self.fail_if_rax_ne_rdx()
    }

    /// Fails the step when `rax` differs from `rdx`.
    pub fn fail_if_rax_ne_rdx(&mut self) -> Res {
        let fail = self.fail_label();
        self.asm.cmp(rax, rdx)?;
        self.asm.jne(fail)?;
        Ok(())
    }

    /// Writes `value` eight bytes at `offset` into the memory a stored pointer names, reads it back and fails the
    /// step if it is not what was written.
    pub fn touch(&mut self, pointer: impl Into<Place>, offset: i32, value: u64) -> Res {
        self.load64(Register::RCX, pointer)?;
        self.asm.mov(rdx, value)?;
        self.asm.mov(qword_ptr(rcx + offset), rdx)?;
        self.asm.mov(rax, qword_ptr(rcx + offset))?;
        self.fail_if_rax_ne_rdx()
    }

    /// Leaves the address of the next instruction in `rcx`.
    pub fn current_address(&mut self) -> Res {
        let mut here = self.asm.create_label();
        self.asm.call(here)?;
        self.asm.set_label(&mut here)?;
        self.asm.pop(rcx)?;
        Ok(())
    }

    /// Declares a GUID in memory order.
    pub fn guid(&mut self, name: &str, text: &str) -> Sym {
        self.bytes(name, &guid_bytes(text))
    }
}

/// A GUID written `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` as the 16 bytes COM passes.
pub fn guid_bytes(text: &str) -> [u8; 16] {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b'-')
        .map(|b| (b as char).to_digit(16).expect("a hexadecimal digit") as u8)
        .collect();
    assert_eq!(digits.len(), 32, "a GUID has 32 digits");
    let byte = |i: usize| digits[2 * i] << 4 | digits[2 * i + 1];
    let mut out = [0u8; 16];
    for (to, from) in [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15]
        .into_iter()
        .enumerate()
    {
        out[to] = byte(from);
    }
    out
}
