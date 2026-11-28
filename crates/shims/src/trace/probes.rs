//! `LEONIDA_PASS_AT=<rva,...>`: notes the first `HITS` times (or `LEONIDA_PASS_HITS`) execution reaches each listed image offset, with
//! the thread, the argument registers (and the text any of them points at) and the caller, and lets it carry on.
//! An `int3` sits on each site; when it is reached the original byte goes back, the instruction runs one step,
//! and the `int3` is put back. Threads passing in that one step go unnoted. For telling which of several paths a
//! stalled program never takes, which a breakpoint that stops the run cannot.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    FlushInstructionCache,
};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId};

use super::{image_offset, note};

const BREAKPOINT: u32 = 0x8000_0003;
const SINGLE_STEP: u32 = 0x8000_0004;
const TRAP_FLAG: u32 = 0x100;
/// How many times each site is noted before its `int3` stays off, unless `LEONIDA_PASS_HITS` says otherwise.
const HITS: u32 = 16;

fn hits() -> u32 {
    static HITS_SET: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *HITS_SET.get_or_init(|| {
        std::env::var("LEONIDA_PASS_HITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(HITS)
    })
}
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;

/// Each armed site's address, its original byte, and how often it has been reached.
static SITES: Mutex<Option<HashMap<usize, (u8, u32)>>> = Mutex::new(None);

thread_local! {
    /// The site this thread is stepping over, to re-arm once the step is done.
    static STEPPING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// The image offsets in `LEONIDA_PASS_AT`.
pub fn sites_from_env() -> Vec<u32> {
    std::env::var("LEONIDA_PASS_AT")
        .map(|text| {
            text.split(',')
                .map(str::trim)
                .filter_map(|site| match site.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => site.parse().ok(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Writes one byte of code.
///
/// # Safety
/// `address` must be a byte of mapped code that no thread is executing mid-instruction in a way the write breaks.
unsafe fn poke(address: usize, byte: u8) -> bool {
    // Two threads changing the same page's protection at once could leave it read-only under the other's write.
    static POKING: Mutex<()> = Mutex::new(());
    let _held = POKING.lock().unwrap_or_else(|e| e.into_inner());
    let mut old = 0u32;
    // SAFETY: as the caller promises; the protection is put back after.
    unsafe {
        if VirtualProtect(address as *const _, 1, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        (address as *mut u8).write_volatile(byte);
        VirtualProtect(address as *const _, 1, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), address as *const _, 1);
    }
    true
}

/// Arms a probe on each image offset of the image at `base`.
///
/// # Safety
/// `base` must be the start of the loaded image, each site the first byte of one of its instructions, and no
/// thread of the image may be running yet.
pub unsafe fn arm(base: usize, sites: &[u32]) -> usize {
    install();
    let mut armed = 0;
    let mut table = SITES.lock().unwrap_or_else(|e| e.into_inner());
    let table = table.get_or_insert_with(HashMap::new);
    for rva in sites {
        let address = base + *rva as usize;
        // SAFETY: a byte of the image, as the caller promises.
        let original = unsafe { (address as *const u8).read_volatile() };
        // SAFETY: as above; nothing runs there yet.
        if unsafe { poke(address, 0xCC) } {
            table.insert(address, (original, 0));
            armed += 1;
        }
    }
    armed
}

/// The text a register points at, when it points at readable memory that starts with a short string.
fn text_at(value: u64) -> Option<String> {
    let bytes = super::crash::read_memory(value as usize, 64)?;
    let end = bytes.iter().position(|&b| b == 0)?;
    let text = &bytes[..end];
    (end >= 3 && text.iter().all(|b| b.is_ascii_graphic() || *b == b' '))
        .then(|| String::from_utf8_lossy(text).into_owned())
}

/// One step of `LEONIDA_PASS_DUMP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Starts from this register (an index into `registers`) instead of `rcx`.
    From(usize),
    Add(i64),
    Deref,
}

/// The registers a dump can start from, in `Step::From` order.
pub const REGISTERS: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];

#[cfg(windows)]
fn registers(c: &windows_sys::Win32::System::Diagnostics::Debug::CONTEXT) -> [u64; 16] {
    [
        c.Rax, c.Rcx, c.Rdx, c.Rbx, c.Rsp, c.Rbp, c.Rsi, c.Rdi, c.R8, c.R9, c.R10, c.R11, c.R12,
        c.R13, c.R14, c.R15,
    ]
}

/// `LEONIDA_PASS_DUMP`: steps from `rcx` (or from the register named first), separated by spaces: `+n` or `-n`
/// moves (hex with `0x`), `*` reads the pointer there. Each noted hit shows the 256 bytes where the steps end, and the first 32 bytes behind every
/// readable pointer among them.
pub fn parse_steps(text: &str) -> Option<Vec<Step>> {
    text.split_whitespace()
        .map(|token| {
            if token == "*" {
                return Some(Step::Deref);
            }
            if let Some(index) = REGISTERS.iter().position(|r| *r == token) {
                return Some(Step::From(index));
            }
            let (negative, digits) = match token.as_bytes().first()? {
                b'-' => (true, &token[1..]),
                b'+' => (false, &token[1..]),
                _ => (false, token),
            };
            let value = match digits.strip_prefix("0x") {
                Some(hex) => i64::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            Some(Step::Add(if negative { -value } else { value }))
        })
        .collect()
}

fn dump_steps() -> Option<&'static [Step]> {
    static STEPS: std::sync::OnceLock<Option<Vec<Step>>> = std::sync::OnceLock::new();
    STEPS
        .get_or_init(|| parse_steps(&std::env::var("LEONIDA_PASS_DUMP").ok()?))
        .as_deref()
}

fn read_u64(address: u64) -> Option<u64> {
    let bytes = super::crash::read_memory(address as usize, 8)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

fn hex_row(text: &mut String, label: String, bytes: &[u8]) {
    text.push_str(&format!(
        "
  {label}:"
    ));
    for byte in bytes {
        text.push_str(&format!(" {byte:02x}"));
    }
}

fn dump_at(registers: &[u64; 16], steps: &[Step]) -> String {
    let mut address = registers[1];
    for step in steps {
        match step {
            Step::From(index) => address = registers[*index],
            Step::Add(n) => address = address.wrapping_add_signed(*n),
            Step::Deref => match read_u64(address) {
                Some(next) => address = next,
                None => {
                    return format!(
                        "
  {address:#x}: unreadable"
                    );
                }
            },
        }
    }
    let Some(bytes) = super::crash::read_memory(address as usize, 256) else {
        return format!(
            "
  {address:#x}: unreadable"
        );
    };
    let mut text = format!(
        "
  at {address:#x}"
    );
    for (row, chunk) in bytes.chunks(16).enumerate() {
        hex_row(&mut text, format!("{:#06x}", row * 16), chunk);
    }
    for (index, word) in bytes.chunks(8).enumerate() {
        let pointer = u64::from_le_bytes(word.try_into().unwrap_or([0; 8]));
        if pointer < 0x10000 {
            continue;
        }
        if let Some(behind) = super::crash::read_memory(pointer as usize, 32) {
            hex_row(
                &mut text,
                format!("[{:#05x}] -> {pointer:#x}", index * 8),
                &behind,
            );
        }
    }
    text
}

fn register(name: &str, value: u64) -> String {
    match text_at(value) {
        Some(text) => format!("{name}={value:#x} \"{text}\""),
        None => format!("{name}={value:#x}"),
    }
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows passes valid exception and context records to a vectored handler.
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
    let code = record.ExceptionCode as u32;
    if code == SINGLE_STEP {
        let site = STEPPING.with(|s| s.replace(0));
        if site == 0 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // SAFETY: the site this thread just stepped past, re-armed as it was.
        unsafe { poke(site, 0xCC) };
        context.EFlags &= !TRAP_FLAG;
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    if code != BREAKPOINT {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let at = record.ExceptionAddress as usize;
    let mut table = SITES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = table.as_mut().and_then(|t| t.get_mut(&at)) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    entry.1 += 1;
    let count = entry.1;
    // SAFETY: the site's own byte, put back so the instruction can run.
    unsafe { poke(at, entry.0) };
    drop(table);
    if count < hits() {
        STEPPING.with(|s| s.set(at));
        context.EFlags |= TRAP_FLAG;
    }
    // SAFETY: the stack pointer at a function's first instruction points at its return address.
    let caller = unsafe { (context.Rsp as *const usize).read_unaligned() };
    // SAFETY: reads this thread's id.
    let thread = unsafe { GetCurrentThreadId() };
    let dump = dump_steps()
        .map(|steps| dump_at(&registers(context), steps))
        .unwrap_or_default();
    note(format!(
        "reached {} (time {count}) on thread {thread}: rax={:#x} rsi={:#x} rdi={:#x} {} {} {} {}, [rsp]={}{dump}",
        image_offset(at),
        context.Rax,
        context.Rsi,
        context.Rdi,
        register("rcx", context.Rcx),
        register("rdx", context.Rdx),
        register("r8", context.R8),
        register("r9", context.R9),
        image_offset(caller)
    ));
    context.Rip = at as u64;
    EXCEPTION_CONTINUE_EXECUTION
}

fn install() {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: registers a handler that only answers breakpoints at the sites armed here.
    unsafe {
        AddVectoredExceptionHandler(1, Some(handler));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_steps_parse_moves_and_reads() {
        assert_eq!(
            parse_steps("-0x40 +0x20 * 16"),
            Some(vec![
                Step::Add(-0x40),
                Step::Add(0x20),
                Step::Deref,
                Step::Add(16)
            ])
        );
        assert_eq!(
            parse_steps("r14 +0x10 *"),
            Some(vec![Step::From(14), Step::Add(0x10), Step::Deref])
        );
        assert_eq!(parse_steps("+zz"), None);
    }
}
