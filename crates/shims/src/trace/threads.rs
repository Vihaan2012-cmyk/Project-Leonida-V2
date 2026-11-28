//! `LEONIDA_DUMP_THREADS=<seconds>`: every that many seconds, every other thread of the process is paused in turn and
//! its registers and stack are written to `threads.txt` in the trace folder (the last dump stays). For finding what a hung program waits
//! on, which the crash handler cannot see when the crash is a watchdog on another thread.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
    THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

use super::crash::walk;

/// `CONTEXT_FULL` for x64.
const CONTEXT_FULL: u32 = 0x0010_000B;

#[repr(C, align(16))]
struct Aligned(CONTEXT);

fn thread_ids() -> Vec<u32> {
    let mut ids = Vec::new();
    // SAFETY: a snapshot of this system's threads, read with a correctly sized entry and closed by dropping.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot.is_null() || snapshot as isize == -1 {
            return ids;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let me = GetCurrentProcessId();
        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == me {
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

/// The registers and stack of every thread but this one, as text.
pub fn dump() -> String {
    let mut text = String::new();
    // SAFETY: reads this thread's id.
    let me = unsafe { GetCurrentThreadId() };
    for id in thread_ids().into_iter().filter(|id| *id != me) {
        // SAFETY: a thread of this process, opened for what is done to it here and closed after.
        unsafe {
            let handle = OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                id,
            );
            if handle.is_null() {
                continue;
            }
            if SuspendThread(handle) == u32::MAX {
                windows_sys::Win32::Foundation::CloseHandle(handle);
                continue;
            }
            let mut context: Aligned = std::mem::zeroed();
            context.0.ContextFlags = CONTEXT_FULL;
            let got = GetThreadContext(handle, &mut context.0) != 0;
            let frames = if got {
                walk(&context.0, 24)
            } else {
                Vec::new()
            };
            ResumeThread(handle);
            windows_sys::Win32::Foundation::CloseHandle(handle);
            let _ = writeln!(text, "== thread {id}");
            if got {
                let c = &context.0;
                let _ = writeln!(
                    text,
                    "  rip={} rsp={:#x} rcx={:#x} rdx={:#x} r8={:#x} r9={:#x}",
                    super::image_offset(c.Rip as usize),
                    c.Rsp,
                    c.Rcx,
                    c.Rdx,
                    c.R8,
                    c.R9
                );
            }
            for (depth, frame) in frames.iter().enumerate() {
                let _ = writeln!(text, "  #{depth} {}", super::image_offset(*frame));
            }
        }
    }
    text
}

/// Starts the timer when `LEONIDA_DUMP_THREADS` is set.
pub fn schedule(folder: PathBuf) {
    let Some(seconds) = std::env::var("LEONIDA_DUMP_THREADS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(seconds.max(1)));
            let _ = std::fs::write(folder.join("threads.txt"), dump());
        }
    });
}
