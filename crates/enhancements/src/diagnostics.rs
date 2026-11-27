//! Explains why the program stopped, for `leonida run --trace`.
//!
//! How the game stops (measured from the executable, image offsets):
//!
//! - **Failed checks.** `__debugbreak()` (`int3`) in the middle of a function, reached when a guarding compare
//!   fails: about 490 such sites carry no message. 54 more follow `call 0x57d1ec0`, which queues an assert
//!   record (an id struct in `rcx`, `edx` severity, `r8d`, `r9d` a hash) and then breaks. Another 9 follow the
//!   `CrashNow: ...` helpers at 0x3ce9900..0x3ce9a30, which format into the in-memory crash-context log at
//!   0x7450d20 (buffer pointer at +0, length at +0x1c).
//! - **Throw helpers** (about 480 sites of `call` then `int3`): `std::_Xlength_error` and friends, which raise
//!   a C++ exception (0xE06D7363) carrying a message such as "vector too long".
//! - **Fast fail.** The C runtime's `__scrt_fastfail` (0x56004c0, 0x55e9bcc) and the stack cookie check
//!   (0x5600575) end in `int 29h`, which Windows handles without running any exception handler. They are
//!   patched to `int3` so the crash handler sees them, with the code in `ecx`.
//! - **Invalid parameters** (108 sites calling `_invalid_parameter_noinfo_noreturn`), `abort` (2 sites) and
//!   `exit` (4 sites): handled by the trace's own runtime hooks.
//! - The program imports neither `ExitProcess` nor `RaiseException`; its only `TerminateProcess` is in the
//!   fast-fail fallback at 0x5600728. `XErrorReport` is reached through `xgameruntime.dll`'s export and noted by
//!   the loader shim.

use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Formatter, Instruction, IntelFormatter, Mnemonic,
};
use std::sync::Mutex;

use shims::trace::{CrashFacts, Explanation};

use crate::patch::{Applied, CodeMemory, Patch, PatchError, apply_all};

/// Image offsets of `int 29h` instructions: the fast-fail helpers of the C runtime.
pub const FASTFAIL_SITES: [u32; 3] = [0x0560_04dd, 0x055e_9bf4, 0x0560_058d];

const INT_29: [u8; 2] = [0xCD, 0x29];
const INT3_NOP: [u8; 2] = [0xCC, 0x90];

/// Image offset of the program's in-memory crash-context log (a string builder).
pub const CRASH_LOG: u32 = 0x0745_0d20;

/// A check that is known by what it guards.
pub struct Check {
    pub rva: u32,
    pub what: &'static str,
    pub subsystem: &'static str,
}

/// Failed checks that have been read and understood.
pub const CHECKS: &[Check] = &[Check {
    rva: 0x01cf_336c,
    what: "descriptor heap start address is not the address the title requested (the title passes a fixed \
           address to heap creation and compares it with what the device's heap reports)",
    subsystem: "graphics: descriptor heap creation",
}];

/// What a call target does when its call is followed by `int3`.
const CALL_TARGETS: &[(u32, &str)] = &[
    (
        0x057d_1ec0,
        "assert record queued, then break (id struct in rcx, severity edx, r8d, hash r9d)",
    ),
    (0x0222_ce40, "crash-context log line written, then break"),
    (0x055e_9bcc, "C runtime fast fail"),
    (0x055e_db20, "throws std::length_error \"vector too long\""),
    (0x0220_b8a0, "throws std::length_error \"string too long\""),
    (0x0221_58b0, "throws std::length_error \"map/set too long\""),
    (0x0217_bc60, "throws std::length_error \"vector too long\""),
    (0x055e_3630, "throws std::length_error \"string too long\""),
    (
        0x055f_2eb0,
        "throws std::out_of_range \"invalid string position\"",
    ),
    (
        0x055e_9900,
        "throws std::out_of_range \"invalid vector<T> subscript\"",
    ),
    (0x05f4_0450, "throws a C++ exception (_CxxThrowException)"),
    (0x05f3_fdf0, "throws std::bad_function_call"),
    (0x05f3_fbf0, "throws std::bad_alloc"),
    (0x05f4_00c0, "throws a std system error"),
];

/// Regions of the image by what they do, for naming the part of the program a stop came from.
pub const BOOT_STAGES: &[(u32, u32, &str)] = &[
    (
        0x056a_1ec8,
        0x056a_2070,
        "process entry and C runtime startup",
    ),
    (
        0x055e_29dc,
        0x055e_2e00,
        "XGameRuntime loading and initialisation",
    ),
    (0x055d_ca58, 0x055e_0000, "XGameRuntime API wrappers"),
    (
        0x0221_3d00,
        0x0221_5200,
        "crash reporting (Windows Error Reporting files, crashcontext.log)",
    ),
    (0x0222_ce40, 0x0222_cf80, "crash-context log"),
    (
        0x0247_4500,
        0x0247_4640,
        "device and memory-pressure handler setup",
    ),
    (0x057d_1ec0, 0x057d_2100, "assert reporting"),
    (
        0x055e_8000,
        0x055e_a000,
        "C runtime crash and termination paths",
    ),
    (
        0x0560_0000,
        0x0560_0800,
        "C runtime fast fail and stack cookie checks",
    ),
];

/// Names of the Windows fast-fail codes.
pub fn fastfail_name(code: u32) -> &'static str {
    match code {
        0 => "LEGACY_GS_VIOLATION",
        1 => "VTGUARD_CHECK_FAILURE",
        2 => "STACK_COOKIE_CHECK_FAILURE",
        3 => "CORRUPT_LIST_ENTRY",
        4 => "INCORRECT_STACK",
        5 => "INVALID_ARG",
        6 => "GS_COOKIE_INIT",
        7 => "FATAL_APP_EXIT",
        8 => "RANGE_CHECK_FAILURE",
        9 => "UNSAFE_REGISTRY_ACCESS",
        10 => "GUARD_ICALL_CHECK_FAILURE",
        11 => "GUARD_WRITE_CHECK_FAILURE",
        12 => "INVALID_FIBER_SWITCH",
        13 => "INVALID_SET_OF_CONTEXT",
        14 => "INVALID_REFERENCE_COUNT",
        18 => "INVALID_JUMP_BUFFER",
        19 => "MRDATA_MODIFIED",
        20 => "CERTIFICATION_FAILURE",
        21 => "INVALID_EXCEPTION_CHAIN",
        22 => "CRYPTO_LIBRARY",
        23 => "INVALID_CALL_IN_DLL_CALLOUT",
        24 => "INVALID_IMAGE_BASE",
        25 => "DLOAD_PROTECTION_FAILURE",
        26 => "UNSAFE_EXTENSION_CALL",
        27 => "DEPRECATED_SERVICE_INVOKED",
        28 => "INVALID_BUFFER_ACCESS",
        29 => "INVALID_BALANCED_TREE",
        30 => "INVALID_NEXT_THREAD",
        31 => "GUARD_ICALL_CHECK_SUPPRESSED",
        _ => "unknown fast-fail code",
    }
}

/// The patches that turn the program's `int 29h` sites into `int3`, so the crash handler gets to see them.
pub fn fastfail_patches(memory: &dyn CodeMemory, sites: &[u32]) -> Vec<Patch> {
    sites
        .iter()
        .filter(|rva| memory.read(**rva, 2).is_ok_and(|b| b == INT_29))
        .map(|rva| Patch {
            name: format!("fast fail trap at {rva:#x}"),
            rva: *rva,
            original: INT_29.to_vec(),
            replacement: INT3_NOP.to_vec(),
        })
        .collect()
}

/// Patches the fast-fail sites that hold `int 29h` and gives the explainer to the crash handler.
pub fn enable(memory: &mut dyn CodeMemory, sites: &[u32]) -> Result<Vec<Applied>, PatchError> {
    shims::trace::set_explainer(explain);
    let patches = fastfail_patches(memory, sites);
    let applied = apply_all(memory, &patches)?;
    if let Ok(mut patched) = PATCHED.lock() {
        patched.extend(patches.iter().map(|p| p.rva));
    }
    Ok(applied)
}

static PATCHED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn is_fastfail_site(rva: u32) -> bool {
    PATCHED.lock().is_ok_and(|p| p.contains(&rva))
}

/// The instructions of the function containing `facts.rva`, up to the stop.
fn instructions_before(facts: &CrashFacts) -> Vec<Instruction> {
    let (Some(rva), Some(function)) = (facts.rva, facts.function_rva) else {
        return Vec::new();
    };
    let length = rva.saturating_sub(function) as usize;
    if length == 0 || length > 1 << 16 {
        return Vec::new();
    }
    let Some(bytes) = shims::trace::read_memory(facts.image_base + function as usize, length)
    else {
        return Vec::new();
    };
    let ip = facts.image_base as u64 + u64::from(function);
    let mut decoder = Decoder::with_ip(64, &bytes, ip, DecoderOptions::NONE);
    let mut all = Vec::new();
    while decoder.can_decode() {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            break;
        }
        all.push(instruction);
    }
    let keep = all.len().saturating_sub(10);
    all.split_off(keep)
}

fn text(instruction: &Instruction) -> String {
    let mut out = String::new();
    IntelFormatter::new().format(instruction, &mut out);
    out
}

fn immediate(instructions: &[Instruction], register: iced_x86::Register) -> Option<u64> {
    instructions.iter().rev().find_map(|i| {
        (i.mnemonic() == Mnemonic::Mov
            && i.op0_register() == register
            && i.op0_kind() == iced_x86::OpKind::Register
            && matches!(
                i.op1_kind(),
                iced_x86::OpKind::Immediate32
                    | iced_x86::OpKind::Immediate32to64
                    | iced_x86::OpKind::Immediate64
            ))
        .then(|| i.immediate(1))
    })
}

fn stage_of(facts: &CrashFacts) -> Option<&'static str> {
    facts.frames.iter().find_map(|frame| {
        let rva = facts.offset(*frame)?;
        BOOT_STAGES
            .iter()
            .find(|(from, to, _)| (*from..*to).contains(&rva))
            .map(|s| s.2)
    })
}

/// The program's own crash-context log, its last `limit` bytes.
fn crash_log(facts: &CrashFacts, limit: usize) -> Option<String> {
    let base = facts.image_base + CRASH_LOG as usize;
    let header = shims::trace::read_memory(base, 0x20)?;
    let pointer = u64::from_le_bytes(header[0..8].try_into().ok()?) as usize;
    let length = u32::from_le_bytes(header[0x1c..0x20].try_into().ok()?) as usize;
    if pointer == 0 || length == 0 || length > 1 << 24 {
        return None;
    }
    let from = length.saturating_sub(limit);
    let bytes = shims::trace::read_memory(pointer + from, length - from)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Turns the facts of a stop into words.
pub fn explain(facts: &CrashFacts) -> Explanation {
    let mut out = Explanation::default();
    let Some(rva) = facts.rva else {
        if facts.code == 0xE06D_7363 {
            out.lines.push("a C++ exception".into());
        }
        out.subsystem = stage_of(facts).map(String::from);
        return out;
    };
    if facts.code == 0x8000_0003 {
        explain_break(facts, rva, &mut out);
    } else {
        out.lines.push(format!(
            "access or fault inside the image at {rva:#x}; rax={:#x} rcx={:#x} rdx={:#x}",
            facts.register("rax").unwrap_or(0),
            facts.register("rcx").unwrap_or(0),
            facts.register("rdx").unwrap_or(0)
        ));
    }
    if out.subsystem.is_none() {
        out.subsystem = stage_of(facts).map(String::from);
    }
    for note in shims::trace::notes()
        .iter()
        .filter(|n| n.starts_with("XErrorReport("))
        .rev()
        .take(3)
    {
        out.lines.push(format!("the program reported: {note}"));
    }
    if let Some(log) = crash_log(facts, 1500) {
        out.lines
            .push("the program's crash-context log ends with:".into());
        out.lines
            .extend(log.lines().rev().take(12).map(|l| format!("  | {l}")));
    }
    out
}

fn explain_break(facts: &CrashFacts, rva: u32, out: &mut Explanation) {
    if is_fastfail_site(rva) {
        let code = facts.register("rcx").unwrap_or(0) as u32;
        out.title = Some(format!("__fastfail({code}) = {}", fastfail_name(code)));
        out.lines.push(
            "the C runtime ended the process (invalid parameter, stack cookie or abort)".into(),
        );
        out.subsystem = Some("C runtime fast fail".into());
        return;
    }
    if let Some(check) = CHECKS.iter().find(|c| c.rva == rva) {
        out.lines.push(format!("known check: {}", check.what));
        out.subsystem = Some(check.subsystem.into());
        return;
    }
    let before = instructions_before(facts);
    match before.last() {
        Some(last) if last.flow_control() == FlowControl::Call => {
            let target = last
                .near_branch_target()
                .wrapping_sub(facts.image_base as u64) as u32;
            let what = CALL_TARGETS
                .iter()
                .find(|(t, _)| *t == target)
                .map_or("a call that should not return", |t| t.1);
            out.lines.push(format!(
                "break after the call to image offset {target:#x}: {what}"
            ));
            if target == 0x057d_1ec0 {
                use iced_x86::Register::{EDX, R8D, R9D};
                let id = before
                    .iter()
                    .rev()
                    .find(|i| i.is_ip_rel_memory_operand() && i.mnemonic() == Mnemonic::Lea)
                    .map(|i| {
                        i.ip_rel_memory_address()
                            .wrapping_sub(facts.image_base as u64)
                    });
                out.lines.push(format!(
                    "assert record: info struct at image offset {:#x}, edx={:#x} r8d={:#x} r9d (hash)={:#x}",
                    id.unwrap_or(0),
                    immediate(&before, EDX).unwrap_or(0),
                    immediate(&before, R8D).unwrap_or(0),
                    immediate(&before, R9D).unwrap_or(0)
                ));
            }
        }
        Some(_) => {
            let guard: Vec<String> = before.iter().rev().take(3).rev().map(text).collect();
            out.lines.push(format!(
                "a check failed (no message in the program): execution fell through `{}` into int3",
                guard.join("; ")
            ));
            out.title = Some("a failed check (int3) inside the program".into());
        }
        None => out
            .lines
            .push("break in the image; the function could not be decoded".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::Buffer;

    #[test]
    fn only_int_29h_sites_are_patched() {
        let mut bytes = vec![0x90u8; 32];
        bytes[8..10].copy_from_slice(&INT_29);
        bytes[20..22].copy_from_slice(&[0xB9, 0x02]);
        let memory = Buffer(bytes);
        let patches = fastfail_patches(&memory, &[8, 20, 100]);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].rva, 8);
        assert_eq!(patches[0].replacement, INT3_NOP);
    }

    #[test]
    fn fast_fail_codes_have_names() {
        assert_eq!(fastfail_name(2), "STACK_COOKIE_CHECK_FAILURE");
        assert_eq!(fastfail_name(5), "INVALID_ARG");
        assert_eq!(fastfail_name(999), "unknown fast-fail code");
    }

    #[test]
    fn the_heap_check_is_in_the_table() {
        assert!(CHECKS.iter().any(|c| c.rva == 0x01cf_336c));
    }
}
