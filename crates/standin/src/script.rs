//! A script of boot steps and the glue that turns it into an image: the TLS callback, the worker thread's
//! procedure and the entry point that runs the steps in order.

use iced_x86::Register;
use iced_x86::code_asm::{cl, dword_ptr, eax, ecx, edx, rcx};

use crate::emit::{ENTRY_FRAME, Emitter, Res};
use crate::image::{self, Built};

/// The part of the runtime a step exercises. Its number is the high half of the step ids it owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum Stage {
    Loader = 1,
    Memory = 2,
    Files = 3,
    Threads = 4,
    Input = 5,
    Audio = 6,
    Device = 7,
    Present = 8,
}

impl Stage {
    pub const ALL: [Stage; 8] = [
        Stage::Loader,
        Stage::Memory,
        Stage::Files,
        Stage::Threads,
        Stage::Input,
        Stage::Audio,
        Stage::Device,
        Stage::Present,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Stage::Loader => "loader",
            Stage::Memory => "memory",
            Stage::Files => "files",
            Stage::Threads => "threads",
            Stage::Input => "input",
            Stage::Audio => "audio",
            Stage::Device => "device",
            Stage::Present => "present",
        }
    }

    pub fn from_id(id: u32) -> Option<Stage> {
        Stage::ALL.into_iter().find(|s| *s as u32 == id >> 8)
    }
}

/// What a step does: emit the code of its call sequence and checks.
pub type StepFn = fn(&mut Emitter) -> Res;

struct Entry {
    stage: Stage,
    name: &'static str,
    emit: StepFn,
}

#[derive(Default)]
pub struct Script {
    steps: Vec<Entry>,
}

/// The value the entry returns when every step passed is `PASSED | steps`; when one failed it is `FAILED | id`.
pub const PASSED: u32 = 0x600D_0000;
pub const FAILED: u32 = 0xBAD0_0000;

impl Script {
    pub fn new() -> Script {
        Script::default()
    }

    /// Appends a step.
    pub fn step(mut self, stage: Stage, name: &'static str, emit: StepFn) -> Script {
        self.steps.push(Entry { stage, name, emit });
        self
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Writes the image.
    pub fn build(&self) -> Res<Built> {
        image::build(&|e| self.emit(e))
    }

    fn emit(&self, e: &mut Emitter) -> Res {
        tls_callback(e)?;
        worker(e)?;
        e.begin_function("entry", ENTRY_FRAME)?;
        for step in &self.steps {
            let id = e.begin_step(step.stage, step.name);
            (step.emit)(e)?;
            e.end_step(id)?;
        }
        let failed = e.cell("failed");
        let completed = e.cell("completed");
        let mut exit = *e.exit_label();
        let mut bad = e.label();
        let mut done = e.label();
        e.place(&mut exit)?;
        e.load32(Register::EAX, failed)?;
        e.asm.test(eax, eax)?;
        e.asm.jnz(bad)?;
        e.load32(Register::EAX, completed)?;
        e.asm.or(eax, PASSED as i32)?;
        e.asm.jmp(done)?;
        e.place(&mut bad)?;
        e.asm.or(eax, FAILED as i32)?;
        e.place(&mut done)?;
        e.end_function()
    }
}

/// `BOOL tls_callback(HANDLE module, DWORD reason, void*)`: remembers the last reason, which reasons came, and how
/// often it was called.
fn tls_callback(e: &mut Emitter) -> Res {
    let reason = e.cell("tls_reason");
    let mask = e.cell("tls_mask");
    let calls = e.cell("tls_calls");
    e.begin_function("tls_callback", 0)?;
    e.store32(reason, Register::EDX)?;
    e.asm.mov(ecx, edx)?;
    e.asm.mov(eax, 1)?;
    e.asm.shl(eax, cl)?;
    e.lock_or32(mask, Register::EAX)?;
    e.lock_inc32(calls)?;
    e.end_function()
}

/// The worker thread: stores its parameter, reads and bumps its own thread-local variables, and sets a flag.
fn worker(e: &mut Emitter) -> Res {
    let param = e.buffer("worker_param", 8);
    let first = e.buffer("worker_tls_first", 4);
    let second = e.buffer("worker_tls_second", 4);
    let done = e.buffer("worker_done", 4);
    e.begin_function("worker", 0x28)?;
    e.store64(param, Register::RCX)?;
    e.tls_block()?;
    e.asm.mov(eax, dword_ptr(rcx))?;
    e.store32(first, Register::EAX)?;
    e.asm.add(dword_ptr(rcx + 4), 1)?;
    e.asm.mov(eax, dword_ptr(rcx + 4))?;
    e.store32(second, Register::EAX)?;
    e.store32_imm(done, 1)?;
    e.asm.mov(eax, 7)?;
    e.end_function()?;
    Ok(())
}
