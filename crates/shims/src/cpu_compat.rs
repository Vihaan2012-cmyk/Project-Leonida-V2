//! Instructions of the console's AMD Zen 2 CPU that a PC processor may lack, emulated when they fault.
//!
//! The first boot on an Intel CPU stopped on `CLZERO` (`0F 01 FC`) at rva 0x4e8aa70, in a loop that clears memory
//! a cache line at a time (`clzero; add rax,40h; cmp rax,r10; jb`). The other AMD-only instructions of the same
//! group are handled the same way should the title use them:
//!
//! - `CLZERO`: the 64-byte line holding the address in `rax` becomes zeros.
//! - `MONITORX` (`0F 01 FA`): arms nothing; `MWAITX` (`0F 01 FB`) then returns at once, as a spurious wake-up may.
//! - `RDPRU` (`0F 01 FD`): `ecx` 0 asks for MPERF and 1 for APERF; both are answered with the time stamp counter,
//!   which grows at a fixed rate as MPERF does. Any other `ecx` reads 0, as the instruction does for an
//!   unsupported register.
//!
//! Each emulated instruction costs an exception; the first one at each address is noted with its count so far,
//! and a hot site can be patched instead.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
};

use crate::trace::{image_offset, note};

const ILLEGAL_INSTRUCTION: i32 = 0xC000_001Du32 as i32;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const LINE: u64 = 64;

/// What the faulting bytes are, if they are an instruction this module emulates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emulated {
    Clzero,
    Monitorx,
    Mwaitx,
    Rdpru,
}

impl Emulated {
    pub fn decode(bytes: [u8; 3]) -> Option<Emulated> {
        match bytes {
            [0x0F, 0x01, 0xFC] => Some(Emulated::Clzero),
            [0x0F, 0x01, 0xFA] => Some(Emulated::Monitorx),
            [0x0F, 0x01, 0xFB] => Some(Emulated::Mwaitx),
            [0x0F, 0x01, 0xFD] => Some(Emulated::Rdpru),
            _ => None,
        }
    }

    pub const LENGTH: u64 = 3;
}

/// The start of the cache line `CLZERO` clears for the address in `rax`.
pub fn line_start(rax: u64) -> u64 {
    rax & !(LINE - 1)
}

/// `RDPRU`'s answer as `(edx, eax)` for register `ecx`, given the time stamp counter.
pub fn rdpru(ecx: u32, tsc: u64) -> (u32, u32) {
    match ecx {
        0 | 1 => ((tsc >> 32) as u32, tsc as u32),
        _ => (0, 0),
    }
}

static INSTALLED: AtomicBool = AtomicBool::new(false);
static EMULATED: AtomicU64 = AtomicU64::new(0);
static SITES: Mutex<Option<HashMap<u64, Emulated>>> = Mutex::new(None);

fn first_at(rip: u64, what: Emulated) -> bool {
    let mut sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
    sites
        .get_or_insert_with(HashMap::new)
        .insert(rip, what)
        .is_none()
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows passes valid exception and context records to a vectored handler.
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
    if record.ExceptionCode != ILLEGAL_INSTRUCTION {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let rip = context.Rip;
    // SAFETY: the faulting instruction's bytes were just fetched by the CPU, so they are readable.
    let bytes = unsafe { std::ptr::read_unaligned(rip as *const [u8; 3]) };
    let Some(what) = Emulated::decode(bytes) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    match what {
        Emulated::Clzero => {
            let start = line_start(context.Rax);
            // SAFETY: the title asked the CPU to clear this line, so it is its writable memory.
            unsafe { std::ptr::write_bytes(start as *mut u8, 0, LINE as usize) };
        }
        Emulated::Monitorx | Emulated::Mwaitx => std::hint::spin_loop(),
        Emulated::Rdpru => {
            // SAFETY: `rdtsc` is available on every x86-64 processor.
            let tsc = unsafe { core::arch::x86_64::_rdtsc() };
            let (edx, eax) = rdpru(context.Rcx as u32, tsc);
            context.Rdx = u64::from(edx);
            context.Rax = u64::from(eax);
        }
    }
    context.Rip = rip + Emulated::LENGTH;
    let count = EMULATED.fetch_add(1, Ordering::Relaxed) + 1;
    if first_at(rip, what) {
        note(format!(
            "{what:?} at {} is emulated (this CPU lacks it); {count} emulated so far",
            image_offset(rip as usize)
        ));
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// Installs the emulation ahead of every other handler. Safe to call more than once.
pub fn install() {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: registers a handler that only touches the faulting thread's context and the line it names.
    unsafe {
        AddVectoredExceptionHandler(1, Some(handler));
    }
}

/// How many instructions have been emulated.
pub fn emulated() -> u64 {
    EMULATED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_amd_only_instructions_are_recognised_by_their_bytes() {
        assert_eq!(Emulated::decode([0x0F, 0x01, 0xFC]), Some(Emulated::Clzero));
        assert_eq!(
            Emulated::decode([0x0F, 0x01, 0xFA]),
            Some(Emulated::Monitorx)
        );
        assert_eq!(Emulated::decode([0x0F, 0x01, 0xFB]), Some(Emulated::Mwaitx));
        assert_eq!(Emulated::decode([0x0F, 0x01, 0xFD]), Some(Emulated::Rdpru));
        assert_eq!(
            Emulated::decode([0x0F, 0x01, 0xF9]),
            None,
            "rdtscp is not ours"
        );
    }

    #[test]
    fn clzero_clears_the_whole_line_holding_the_address() {
        assert_eq!(line_start(0x1000), 0x1000);
        assert_eq!(line_start(0x103f), 0x1000);
        assert_eq!(line_start(0x1040), 0x1040);
    }

    #[test]
    fn rdpru_answers_mperf_and_aperf_from_the_counter_and_zero_otherwise() {
        assert_eq!(rdpru(0, 0x1_2345_6789), (0x1, 0x2345_6789));
        assert_eq!(rdpru(1, 0x1_2345_6789), (0x1, 0x2345_6789));
        assert_eq!(rdpru(2, 0x1_2345_6789), (0, 0));
    }

    /// A real `CLZERO` run through the handler on a CPU that lacks it (on AMD it simply runs).
    #[test]
    fn a_clzero_in_running_code_clears_its_line() {
        install();
        #[repr(align(64))]
        struct Line([u8; 128]);
        let mut buffer = Line([0xAB; 128]);
        let target = buffer.0.as_mut_ptr().wrapping_add(70);
        // SAFETY: `clzero` clears the 64-byte line holding `rax`, which lies inside `buffer`.
        unsafe { core::arch::asm!(".byte 0x0f, 0x01, 0xfc", in("rax") target) };
        assert!(buffer.0[..64].iter().all(|&b| b == 0xAB));
        assert!(buffer.0[64..].iter().all(|&b| b == 0));
    }
}
