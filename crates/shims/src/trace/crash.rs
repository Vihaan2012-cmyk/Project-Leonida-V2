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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
    let mut copy = Aligned(*context);
    let context = &mut copy.0;
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
                    context,
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
        pointed: dump_registers(&registers(context), read_prefix),
    }
}

/// The first `length` bytes at `address` that can be read, or fewer (down to 8) when the region ends sooner.
fn read_prefix(address: usize, length: usize) -> Option<Vec<u8>> {
    [length, length / 2, length / 4, 8]
        .into_iter()
        .filter(|n| *n >= 8 && *n <= length)
        .find_map(|n| read_memory(address, n))
}

/// The registers that hold addresses of readable memory: 64 bytes at each, and 32 at each pointer found in the
/// first 64 bytes (at most eight a register), so an object's data pointer shows its contents without a debugger.
/// Equal addresses are shown once. `read` gives up to the length asked from an address, or nothing.
pub fn dump_registers(
    registers: &[(&'static str, u64)],
    read: impl Fn(usize, usize) -> Option<Vec<u8>>,
) -> Vec<(String, u64, Vec<u8>)> {
    const LOWEST: u64 = 0x1_0000;
    let mut out: Vec<(String, u64, Vec<u8>)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &(name, value) in registers {
        if name == "eflags" || value < LOWEST || !seen.insert(value) {
            continue;
        }
        let Some(bytes) = read(value as usize, 64) else {
            continue;
        };
        let words: Vec<u64> = bytes
            .chunks_exact(8)
            .map(|w| u64::from_le_bytes(w.try_into().unwrap_or([0; 8])))
            .collect();
        out.push((name.to_string(), value, bytes));
        let mut followed = 0;
        for (k, word) in words.into_iter().enumerate() {
            if followed == 8 {
                break;
            }
            if word < LOWEST || !seen.insert(word) {
                continue;
            }
            if let Some(inner) = read(word as usize, 32) {
                out.push((format!("{name}+{:#x}", k * 8), word, inner));
                followed += 1;
            }
        }
    }
    out
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
    for (name, address, bytes) in &facts.pointed {
        let _ = writeln!(text, "memory at {name} = {address:#x}:");
        for (row, chunk) in bytes.chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            let _ = writeln!(text, "  {:04x}: {}", row * 16, hex.join(" "));
        }
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

/// `RtlCaptureContext` stores with aligned moves, and the bindings' `CONTEXT` is not declared 16-aligned.
#[repr(C, align(16))]
struct Aligned(CONTEXT);

fn capture(headline: &str) {
    if BUSY.swap(true, Ordering::Acquire) {
        return;
    }
    // SAFETY: a zeroed CONTEXT is filled in by the call.
    let mut context: Aligned = unsafe { std::mem::zeroed() };
    // SAFETY: captures this thread's registers.
    unsafe { RtlCaptureContext(&mut context.0) };
    let found = facts(0, context.0.Rip as usize, &context.0, None);
    record(headline.to_string(), &found);
    BUSY.store(false, Ordering::Release);
}

type InvalidParameterHandler = unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize);

/// The handler the program installed over ours, called after the stop is recorded.
static PREVIOUS: AtomicUsize = AtomicUsize::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn invalid_parameter(
    expression: *const u16,
    function: *const u16,
    file: *const u16,
    line: u32,
    reserved: usize,
) {
    capture("invalid parameter passed to the C runtime (the program's _invalid_parameter_noinfo)");
    let previous = PREVIOUS.load(Ordering::Relaxed);
    if previous != 0 {
        // SAFETY: a handler the program registered with the C runtime, which has this signature.
        let previous: InvalidParameterHandler = unsafe { std::mem::transmute(previous) };
        // SAFETY: called with the arguments we were given.
        unsafe { previous(expression, function, file, line, reserved) };
    }
    // SAFETY: ends the process as the default handler would.
    unsafe { TerminateProcess(GetCurrentProcess(), 0xC000_000D) };
}

/// Puts the invalid-parameter handler back in front when the program has replaced it (its crash-dump library
/// does at startup); the replacement is kept and called after ours.
pub fn keep_hooks() {
    if !INSTALLED.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: sets the C runtime's handler to ours and gets the one it replaces.
    let current = unsafe { _set_invalid_parameter_handler(Some(invalid_parameter)) };
    if current != 0 && current != invalid_parameter as *const () as usize {
        PREVIOUS.store(current, Ordering::Relaxed);
        let (base, size) = image();
        let place = if (base..base + size).contains(&current) {
            format!("image offset {:#x}", current - base)
        } else {
            format!("{current:#x}")
        };
        super::note(format!(
            "the program replaced the invalid-parameter handler with {place}; it is kept behind the trace's"
        ));
    }
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
    fn _set_invalid_parameter_handler(handler: Option<InvalidParameterHandler>) -> usize;
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
    INSTALLED.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(memory: &'static [(usize, [u8; 64])]) -> impl Fn(usize, usize) -> Option<Vec<u8>> {
        move |address, length| {
            memory
                .iter()
                .find(|(base, _)| *base == address)
                .map(|(_, bytes)| bytes[..length.min(64)].to_vec())
        }
    }

    #[test]
    fn registers_that_point_at_memory_are_dumped_with_the_pointers_inside() {
        let mut object = [0u8; 64];
        object[8..16].copy_from_slice(&0x50000u64.to_le_bytes());
        object[16..24].copy_from_slice(&0x70u64.to_le_bytes());
        let mut data = [0u8; 64];
        data[..4].copy_from_slice(&[0xE7, 0xDC, 0x0D, 0x00]);
        let memory: &'static [(usize, [u8; 64])] =
            Box::leak(vec![(0x40000usize, object), (0x50000, data)].into_boxed_slice());
        let regs = [
            ("rax", 0u64),
            ("rcx", 0x40000),
            ("rdx", 0x40000),
            ("rbx", 0x9999_9999),
            ("eflags", 0x40000),
        ];
        let dumps = dump_registers(&regs, fake(memory));
        assert_eq!(
            dumps.len(),
            2,
            "rax is null, rdx repeats rcx, rbx is unreadable, eflags is no address"
        );
        assert_eq!(dumps[0].0, "rcx");
        assert_eq!(dumps[0].2.len(), 64);
        assert_eq!(dumps[1].0, "rcx+0x8");
        assert_eq!(dumps[1].1, 0x50000);
        assert_eq!(dumps[1].2.len(), 32);
        assert_eq!(&dumps[1].2[..4], &[0xE7, 0xDC, 0x0D, 0x00]);
    }

    #[test]
    fn real_memory_is_read_only_where_it_is_readable() {
        let value = Box::new([0xABu8; 64]);
        let at = value.as_ptr() as usize;
        assert_eq!(read_prefix(at, 16), Some(value[..16].to_vec()));
        assert!(read_prefix(0x10, 64).is_none());
        let regs = [("rcx", at as u64), ("rdx", 0x20)];
        let dumps = dump_registers(&regs, read_prefix);
        assert_eq!(dumps.len(), 1);
        assert_eq!(dumps[0].0, "rcx");
    }

    #[test]
    fn the_report_prints_the_dumped_bytes() {
        let facts = CrashFacts {
            code: 0x8000_0003,
            address: 0x1000,
            rva: None,
            function_rva: None,
            registers: vec![("rcx", 0x4000)],
            frames: vec![],
            image_base: 0,
            image_size: 0,
            code_before: vec![],
            code_at: vec![],
            fault: None,
            pointed: vec![("rcx".into(), 0x4000, (0..20).collect())],
        };
        let text = format_report(&facts, "test", &[]);
        assert!(text.contains("memory at rcx = 0x4000:"), "{text}");
        assert!(
            text.contains("  0000: 00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f"),
            "{text}"
        );
        assert!(text.contains("  0010: 10 11 12 13"), "{text}");
    }
}
