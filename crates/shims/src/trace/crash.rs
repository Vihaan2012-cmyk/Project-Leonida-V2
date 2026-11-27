//! The crash handler behind `--trace`: what the program died of, where, with registers, a stack walk and the
//! program's own last log lines, written to `crash.txt` and summarised in `summary.txt`.
//!
//! Seen: access violations and the like, `int3` inside the image (the program's failed checks), C++ exceptions
//! (kept as a short history, they are often caught), invalid-parameter calls and `abort` of the C runtime, and
//! `exit`. Not seen: `__fastfail` (`int 29h`), which Windows ends without running any handler; patch those
//! sites to `int3` first (see `enhancements::diagnostics`).

use std::ffi::c_void;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    RtlCaptureContext, RtlLookupFunctionEntry, RtlVirtualUnwind,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

use super::{CrashFacts, Explainer};

pub(super) static WATCH: Mutex<Option<(PathBuf, usize, usize)>> = Mutex::new(None);
pub(super) static EXPLAINER: Mutex<Option<Explainer>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

const BREAKPOINT: u32 = 0x8000_0003;
const CXX_EXCEPTION: u32 = 0xE06D_7363;
const FATAL: [u32; 6] = [
    0xC000_0005,
    0xC000_001D,
    0xC000_0096,
    0xC000_00FD,
    0xC000_0094,
    0xC000_0409,
];

/// Whether `length` bytes at `address` can be read.
pub fn readable(address: usize, length: usize) -> bool {
    if address == 0 || length == 0 {
        return false;
    }
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: VirtualQuery only inspects the address space.
    let got = unsafe { VirtualQuery(address as *const c_void, &mut info, size_of_val(&info)) };
    if got == 0 || info.State != MEM_COMMIT || info.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0 {
        return false;
    }
    let end = info.BaseAddress as usize + info.RegionSize;
    address.checked_add(length).is_some_and(|last| last <= end)
}

pub fn read_memory(address: usize, length: usize) -> Option<Vec<u8>> {
    // SAFETY: checked readable just above, and only copied.
    readable(address, length)
        .then(|| unsafe { std::slice::from_raw_parts(address as *const u8, length).to_vec() })
}

fn read_u64(address: usize) -> Option<u64> {
    read_memory(address, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
}

/// Return addresses from `context` outward, at most `limit`.
pub fn walk(context: &CONTEXT, limit: usize) -> Vec<usize> {
    let mut context = *context;
    let mut frames = Vec::new();
    while frames.len() < limit {
        let pc = context.Rip;
        if pc == 0 {
            break;
        }
        frames.push(pc as usize);
        let mut image_base = 0u64;
        // SAFETY: looks a function up in the loaded modules' tables.
        let entry = unsafe { RtlLookupFunctionEntry(pc, &mut image_base, std::ptr::null_mut()) };
        if entry.is_null() {
            let Some(next) = read_u64(context.Rsp as usize) else {
                break;
            };
            context.Rip = next;
            context.Rsp += 8;
        } else {
            let mut data: *mut c_void = std::ptr::null_mut();
            let mut frame = 0u64;
            // SAFETY: `entry` came from the lookup above and `context` is our own copy.
            unsafe {
                RtlVirtualUnwind(
                    0,
                    image_base,
                    pc,
                    entry,
                    &mut context,
                    &mut data,
                    &mut frame,
                    std::ptr::null_mut(),
                );
            }
        }
    }
    frames
}

fn registers(context: &CONTEXT) -> Vec<(&'static str, u64)> {
    vec![
        ("rax", context.Rax),
        ("rcx", context.Rcx),
        ("rdx", context.Rdx),
        ("rbx", context.Rbx),
        ("rsp", context.Rsp),
        ("rbp", context.Rbp),
        ("rsi", context.Rsi),
        ("rdi", context.Rdi),
        ("r8", context.R8),
        ("r9", context.R9),
        ("r10", context.R10),
        ("r11", context.R11),
        ("r12", context.R12),
        ("r13", context.R13),
        ("r14", context.R14),
        ("r15", context.R15),
        ("rip", context.Rip),
        ("eflags", u64::from(context.EFlags)),
    ]
}

fn image() -> (usize, usize) {
    WATCH
        .try_lock()
        .ok()
        .and_then(|w| w.as_ref().map(|(_, base, size)| (*base, *size)))
        .unwrap_or((0, 0))
}

fn facts(code: u32, address: usize, context: &CONTEXT, fault: Option<(bool, usize)>) -> CrashFacts {
    let (base, size) = image();
    let inside = |a: usize| size != 0 && (base..base + size).contains(&a);
    let rva = inside(address).then(|| (address - base) as u32);
    let function_rva = rva.and_then(|_| {
        let mut image_base = 0u64;
        // SAFETY: a table lookup.
        let entry = unsafe {
            RtlLookupFunctionEntry(address as u64, &mut image_base, std::ptr::null_mut())
        };
        // SAFETY: a non-null entry points into the image's exception table.
        (!entry.is_null()).then(|| unsafe { (*entry).BeginAddress })
    });
    CrashFacts {
        code,
        address,
        rva,
        function_rva,
        registers: registers(context),
        frames: walk(context, 24),
        image_base: base,
        image_size: size,
        code_before: [48usize, 32, 16, 8]
            .into_iter()
            .find_map(|n| read_memory(address.wrapping_sub(n), n))
            .unwrap_or_default(),
        code_at: read_memory(address, 16).unwrap_or_default(),
        fault,
    }
}

/// The text of a report for `facts` with the explainer's lines.
pub fn format_report(facts: &CrashFacts, headline: &str, explanation: &[String]) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "== {headline}");
    let _ = writeln!(
        text,
        "exception {:#010x} at {}",
        facts.code,
        facts.describe(facts.address)
    );
    if let Some(function) = facts.function_rva {
        let _ = writeln!(text, "inside the function at image offset {function:#x}");
    }
    if let Some((write, target)) = facts.fault {
        let _ = writeln!(
            text,
            "it {} address {target:#x}",
            if write { "wrote to" } else { "read or ran" }
        );
    }
    for line in explanation {
        let _ = writeln!(text, "{line}");
    }
    let regs: Vec<String> = facts
        .registers
        .iter()
        .map(|(name, value)| format!("{name}={value:#x}"))
        .collect();
    for row in regs.chunks(4) {
        let _ = writeln!(text, "  {}", row.join(" "));
    }
    let _ = writeln!(text, "stack:");
    for (depth, frame) in facts.frames.iter().enumerate() {
        let _ = writeln!(text, "  #{depth} {}", facts.describe(*frame));
    }
    if !facts.code_before.is_empty() || !facts.code_at.is_empty() {
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let _ = writeln!(
            text,
            "code: {} | {}",
            hex(&facts.code_before),
            hex(&facts.code_at)
        );
    }
    text
}

fn record(headline: String, facts: &CrashFacts) {
    if super::stops().len() >= 64 {
        return;
    }
    let explanation = EXPLAINER
        .try_lock()
        .ok()
        .and_then(|e| *e)
        .map(|explain| explain(facts))
        .unwrap_or_default();
    let headline = explanation.title.unwrap_or(headline);
    let report = format_report(facts, &headline, &explanation.lines);
    super::push_stop(super::Stop {
        headline,
        lines: explanation.lines,
        subsystem: explanation.subsystem,
        frames: facts
            .frames
            .iter()
            .filter_map(|f| facts.offset(*f))
            .take(8)
            .collect(),
    });
    let folder = WATCH
        .try_lock()
        .ok()
        .and_then(|w| w.as_ref().map(|w| w.0.clone()));
    if let Some(folder) = folder {
        let _ = std::fs::create_dir_all(&folder);
        let path = folder.join("crash.txt");
        let mut old = std::fs::read_to_string(&path).unwrap_or_default();
        if old.len() > 200_000 {
            old.clear();
        }
        old.push_str(&report);
        old.push('\n');
        let _ = std::fs::write(path, old);
        super::write_summary(&folder);
    }
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows passes valid exception pointers to vectored handlers.
    let (record_ptr, context) = unsafe { ((*info).ExceptionRecord, (*info).ContextRecord) };
    // SAFETY: as above.
    let exception = unsafe { &*record_ptr };
    let code = exception.ExceptionCode as u32;
    let address = exception.ExceptionAddress as usize;
    let (base, size) = image();
    let in_image = size != 0 && (base..base + size).contains(&address);
    let wanted = FATAL.contains(&code) || code == CXX_EXCEPTION || (code == BREAKPOINT && in_image);
    if !wanted || BUSY.swap(true, Ordering::Acquire) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // SAFETY: as above, the context is valid for the duration of the call.
    let context = unsafe { &*context };
    let fault = (code == 0xC000_0005 && exception.NumberParameters >= 2).then(|| {
        (
            exception.ExceptionInformation[0] == 1,
            exception.ExceptionInformation[1],
        )
    });
    let found = facts(code, address, context, fault);
    if code == CXX_EXCEPTION {
        let object = (exception.NumberParameters >= 2)
            .then(|| exception.ExceptionInformation[1])
            .and_then(what_of);
        super::push_cxx(object.unwrap_or_default(), &found);
    } else {
        let headline = if code == BREAKPOINT {
            "breakpoint (int3) inside the program".to_string()
        } else {
            format!("exception {code:#010x} (first chance: the program may handle it)")
        };
        record(headline, &found);
    }
    BUSY.store(false, Ordering::Release);
    EXCEPTION_CONTINUE_SEARCH
}

/// The `what()` text of a standard exception object, if `object` looks like one.
fn what_of(object: usize) -> Option<String> {
    let pointer = read_u64(object + 8)? as usize;
    let mut bytes = Vec::new();
    for at in 0..160 {
        let byte = read_memory(pointer + at, 1)?[0];
        if byte == 0 {
            break;
        }
        bytes.push(byte);
    }
    (!bytes.is_empty() && bytes.len() < 160 && bytes.iter().all(|b| (0x20..0x7F).contains(b)))
        .then(|| String::from_utf8_lossy(&bytes).into_owned())
}

fn capture(headline: &str) {
    if BUSY.swap(true, Ordering::Acquire) {
        return;
    }
    // SAFETY: a zeroed CONTEXT is filled in by the call.
    let mut context: CONTEXT = unsafe { std::mem::zeroed() };
    // SAFETY: captures this thread's registers.
    unsafe { RtlCaptureContext(&mut context) };
    let found = facts(0, context.Rip as usize, &context, None);
    record(headline.to_string(), &found);
    BUSY.store(false, Ordering::Release);
}

unsafe extern "C" fn invalid_parameter(
    _expression: *const u16,
    _function: *const u16,
    _file: *const u16,
    _line: u32,
    _reserved: usize,
) {
    capture("invalid parameter passed to the C runtime (the program's _invalid_parameter_noinfo)");
    // SAFETY: ends the process as the default handler would.
    unsafe { TerminateProcess(GetCurrentProcess(), 0xC000_000D) };
}

extern "C" fn abort_signal(_signal: i32) {
    capture("abort() called");
}

extern "C" fn at_exit() {
    if !BUSY.load(Ordering::Acquire) {
        super::push_exit();
    }
}

unsafe extern "C" {
    fn _set_invalid_parameter_handler(
        handler: Option<unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize)>,
    ) -> usize;
    fn signal(signal: i32, handler: usize) -> usize;
    fn atexit(callback: extern "C" fn()) -> i32;
}

pub fn install() {
    // SAFETY: registers handlers that only read the exception and write files.
    unsafe {
        AddVectoredExceptionHandler(1, Some(handler));
        _set_invalid_parameter_handler(Some(invalid_parameter));
        signal(22, abort_signal as extern "C" fn(i32) as usize);
        atexit(at_exit);
    }
}
