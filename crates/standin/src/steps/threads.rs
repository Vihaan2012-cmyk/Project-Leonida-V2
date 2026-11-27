//! A thread made and waited for, with its own thread-local variables, and the console's affinity calls.

use iced_x86::Register;
use iced_x86::code_asm::{dword_ptr, rcx};

use super::{PROCESS_THREADS, PROCESS_TOPOLOGY, SYNCH};
use crate::emit::{Arg, Emitter, Res, Target};
use crate::image::{TLS_FIRST, TLS_SECOND};
use crate::script::{Script, Stage};

const CURRENT_THREAD: u64 = u64::MAX - 1;
const PARAMETER: u64 = 0x77;
/// Console logical processors 0 and 1.
const CONSOLE_MASK: u64 = 0x3;

pub fn add(script: Script) -> Script {
    script
        .step(Stage::Threads, "CreateThread starts the worker", create)
        .step(
            Stage::Threads,
            "the worker ran with its own thread-local variables",
            worker_ran,
        )
        .step(
            Stage::Threads,
            "this thread's thread-local variables are untouched",
            own_storage,
        )
        .step(
            Stage::Threads,
            "SetThreadGroupAffinity takes a console mask",
            affinity,
        )
        .step(
            Stage::Threads,
            "thread priority can be set and read back",
            priority,
        )
}

fn create(e: &mut Emitter) -> Res {
    let create = e.import(PROCESS_THREADS, "CreateThread");
    let wait = e.import(SYNCH, "WaitForSingleObject");
    let close = e.import("api-ms-win-core-handle-l1-1-0.dll", "CloseHandle");
    let table = e.bytes_with_pointers(
        "worker_table",
        &[0; 8],
        vec![(0, Target::Function("worker".into()))],
    );
    let handle = e.buffer("worker_handle", 8);
    let id = e.buffer("worker_id", 4);
    e.call(
        create,
        &[
            Arg::Imm(0),
            Arg::Imm(0),
            Arg::load64(table),
            Arg::Imm(PARAMETER),
            Arg::Imm(0),
            Arg::addr(id),
        ],
    )?;
    e.fail_if_zero64()?;
    e.store64(handle, Register::RAX)?;
    e.call(wait, &[Arg::load64(handle), Arg::Imm(10_000)])?;
    e.fail_if_nonzero32()?;
    e.call(close, &[Arg::load64(handle)])?;
    e.fail_if_zero32()
}

fn worker_ran(e: &mut Emitter) -> Res {
    let param = e.cell("worker_param");
    let done = e.cell("worker_done");
    let first = e.cell("worker_tls_first");
    let second = e.cell("worker_tls_second");
    let mask = e.cell("tls_mask");
    e.fail_if_mem32_ne(param, PARAMETER as u32)?;
    e.fail_if_mem32_ne(done, 1)?;
    e.fail_if_mem32_ne(first, TLS_FIRST)?;
    e.fail_if_mem32_ne(second, TLS_SECOND + 1)?;
    e.load32(Register::EAX, mask)?;
    e.fail_unless_bit(2)
}

fn own_storage(e: &mut Emitter) -> Res {
    e.tls_block()?;
    e.asm.mov(iced_x86::code_asm::eax, dword_ptr(rcx + 4))?;
    e.fail_if_ne32(TLS_SECOND)
}

fn affinity(e: &mut Emitter) -> Res {
    let set = e.import(PROCESS_TOPOLOGY, "SetThreadGroupAffinity");
    let get = e.import(PROCESS_TOPOLOGY, "GetThreadGroupAffinity");
    let mut wanted = [0u8; 16];
    wanted[..8].copy_from_slice(&CONSOLE_MASK.to_le_bytes());
    let wanted = e.bytes("affinity_wanted", &wanted);
    let got = e.buffer("affinity_got", 16);
    e.call(
        set,
        &[Arg::Imm(CURRENT_THREAD), Arg::addr(wanted), Arg::Imm(0)],
    )?;
    e.fail_if_zero32()?;
    e.call(get, &[Arg::Imm(CURRENT_THREAD), Arg::addr(got)])?;
    e.fail_if_zero32()?;
    e.load64(Register::RAX, got)?;
    e.fail_if_zero64()
}

fn priority(e: &mut Emitter) -> Res {
    let set = e.import(PROCESS_THREADS, "SetThreadPriority");
    let get = e.import(PROCESS_THREADS, "GetThreadPriority");
    e.call(set, &[Arg::Imm(CURRENT_THREAD), Arg::Imm(1)])?;
    e.fail_if_zero32()?;
    e.call(get, &[Arg::Imm(CURRENT_THREAD)])?;
    e.fail_if_ne32(1)?;
    e.call(set, &[Arg::Imm(CURRENT_THREAD), Arg::Imm(0)])?;
    e.fail_if_zero32()
}
