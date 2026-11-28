//! `LEONIDA_WATCH=<rva>[,<rva>...]`: notes each instruction that writes the 4 bytes at an image offset (up to four,
//! 4-byte aligned), the first time that instruction does, with the thread, the registers and the stack. A debug
//! register of every thread holds each address; a thread that starts later gets them within 50 ms. For finding
//! who sets a flag the program waits on, when the code that does it cannot be found by reading.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    GetThreadContext, SetThreadContext,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
    THREAD_GET_CONTEXT, THREAD_SET_CONTEXT, THREAD_SUSPEND_RESUME,
};

use super::{image_offset, note};

const SINGLE_STEP: u32 = 0x8000_0004;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
/// `CONTEXT_DEBUG_REGISTERS` for x64.
const CONTEXT_DEBUG_REGISTERS: u32 = 0x0010_0010;
/// How many distinct writing instructions are noted.
const LIMIT: usize = 64;

static ADDRESSES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static SEEN: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static NOTED: AtomicUsize = AtomicUsize::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

#[repr(C, align(16))]
struct Aligned(CONTEXT);

/// The image offsets in `LEONIDA_WATCH`, at most four.
pub fn sites_from_env() -> Vec<u32> {
    std::env::var("LEONIDA_WATCH")
        .map(|text| {
            text.split(',')
                .map(str::trim)
                .filter_map(|site| match site.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => site.parse().ok(),
                })
                .filter(|rva| rva % 4 == 0)
                .take(4)
                .collect()
        })
        .unwrap_or_default()
}

/// `DR7` for write watches of 4 bytes on the first `count` debug registers.
pub fn dr7(count: usize) -> u64 {
    (0..count.min(4)).fold(0u64, |dr7, i| {
        let local_enable = 1u64 << (i * 2);
        let write_four_bytes = 0b1101u64 << (16 + i * 4);
        dr7 | local_enable | write_four_bytes
    })
}

fn arm_thread(id: u32, addresses: &[usize]) -> bool {
    // SAFETY: a thread of this process, opened for what is done to it here and closed after; it is suspended while
    // its debug registers change.
    unsafe {
        let handle = OpenThread(
            THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_SET_CONTEXT,
            0,
            id,
        );
        if handle.is_null() {
            return false;
        }
        let mut armed = false;
        if SuspendThread(handle) != u32::MAX {
            let mut context: Aligned = std::mem::zeroed();
            context.0.ContextFlags = CONTEXT_DEBUG_REGISTERS;
            if GetThreadContext(handle, &mut context.0) != 0 {
                let c = &mut context.0;
                let words = [&mut c.Dr0, &mut c.Dr1, &mut c.Dr2, &mut c.Dr3];
                for (word, address) in words.into_iter().zip(addresses) {
                    *word = *address as u64;
                }
                c.Dr7 = dr7(addresses.len());
                armed = SetThreadContext(handle, &context.0) != 0;
            }
            ResumeThread(handle);
        }
        windows_sys::Win32::Foundation::CloseHandle(handle);
        armed
    }
}

fn other_threads() -> Vec<u32> {
    let mut ids = Vec::new();
    // SAFETY: a snapshot of this system's threads, read with a correctly sized entry and closed after.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot.is_null() || snapshot as isize == -1 {
            return ids;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let (me, this) = (GetCurrentProcessId(), GetCurrentThreadId());
        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == me && entry.th32ThreadID != this {
                    ids.push(entry.th32ThreadID);
                }
                if Thread32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        windows_sys::Win32::Foundation::CloseHandle(snapshot);
    }
    ids
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows passes valid exception and context records to a vectored handler.
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
    if record.ExceptionCode as u32 != SINGLE_STEP || context.Dr6 & 0xF == 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let which = context.Dr6.trailing_zeros() as usize;
    context.Dr6 = 0;
    let rip = context.Rip as usize;
    let first = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(rip);
    if first && NOTED.fetch_add(1, Ordering::Relaxed) < LIMIT {
        let address = ADDRESSES
            .lock()
            .map(|a| a.get(which).copied().unwrap_or(0))
            .unwrap_or(0);
        // SAFETY: the watched address was just written, so it is readable.
        let value = unsafe { (address as *const u32).read_volatile() };
        let stack = super::crash::walk(context, 10)
            .into_iter()
            .map(image_offset)
            .collect::<Vec<_>>()
            .join(" <- ");
        // SAFETY: reads this thread's id.
        let thread = unsafe { GetCurrentThreadId() };
        note(format!(
            "watch: {} now {value:#010x}, written before {} on thread {thread}: rax={:#x} rcx={:#x} rdx={:#x} \
             r8={:#x} r9={:#x}; stack {stack}",
            image_offset(address),
            image_offset(rip),
            context.Rax,
            context.Rcx,
            context.Rdx,
            context.R8,
            context.R9,
        ));
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// Watches the image offsets of the image at `base` in every thread, from now on.
pub fn start(base: usize, sites: &[u32]) {
    if sites.is_empty() || INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    let addresses: Vec<usize> = sites.iter().map(|rva| base + *rva as usize).collect();
    if let Ok(mut held) = ADDRESSES.lock() {
        *held = addresses.clone();
    }
    // SAFETY: registers a handler that only answers debug-register stops on the watched addresses.
    unsafe {
        AddVectoredExceptionHandler(1, Some(handler));
    }
    let mut armed = HashSet::new();
    for id in other_threads() {
        if arm_thread(id, &addresses) {
            armed.insert(id);
        }
    }
    std::thread::spawn(move || {
        loop {
            for id in other_threads() {
                if !armed.contains(&id) && arm_thread(id, &addresses) {
                    armed.insert(id);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dr7_enables_four_byte_write_watches() {
        assert_eq!(dr7(1), 0x000D_0001);
        assert_eq!(dr7(2), 0x00DD_0005);
        assert_eq!(dr7(9), dr7(4));
    }
}
