//! The second step floor `1 / limit`, stored in `0x6849844`, for every game clock instead of only one.
//!
//! The load patched by [`crate::framerate::STEP_FLOOR`] (`0x2226854`, in the timer update `sub_022264d0`) serves
//! one clock, the one at `0xa958358` whose step is `0xa958364`. The other clocks are updated by `sub_02226040`,
//! which is a leaf with no exception-table entry, so neither `leonida exe xrefs` nor [`crate::patch::Finder`]
//! sees it. It takes a clock record (`rcx`), the measured frame time (`xmm1`), a scale (`xmm2`) and a flag (`r9b`);
//! with the flag set it clamps the time to `[floor, 1/15 s]` exactly as `sub_022264d0` does, loading the same
//! `0x6849844` at `0x222605a` when the byte at `0x7453cc0` is not 0, and storing the result at record `+0xc`
//! (float seconds), `+0x10` (whole milliseconds) and `+0x14` (its inverse). `sub_022264d0` calls it for the records at
//! `0xab5d010`, `0xabbef40`, `0x74fa558` (no clamp), `0xa943768`, `0x856c6b4`, `0xa943788` (no clamp), `0xa958358`,
//! `0xa958380` and `0xab73138`. The step of `0xabbef40` (`0xabbef4c`) is read by 522 exported source files:
//! it is the step most game systems use, so above the game's own limit it stayed held at `1 / limit`.
//!
//! Every reader of `0x6849844` reads it from memory (four references: the two stores below, `0x2226854`
//! and `0x222605a`), so the floor is fixed where it is stored. The value that takes its place is the first floor,
//! `1/3000 s` (`0x39aec33e`), which `sub_02226130` stores in `0x6847660` just before.
//!
//! - `sub_02226130` (`0x2226190`): `mov dword ptr [0x6847660], 0x39aec33e` then `vmovss [0x6849844], xmm0`, then
//!   `sete [0x7453cc0]`. Both stores become `mov eax, imm32; mov [0x6847660], eax; mov [0x6849844], eax`, 17 of
//!   the 18 bytes. `eax` is dead there: it is loaded from `0x7446398` and compared before the `jg`, and next
//!   written by `xor eax, eax` at `0x22261a9`. No flag is changed, and `sete` still reads `test rsi, rsi`
//!   from `0x2226178`.
//! - `sub_022434a0` (`0x22435e6`): when the settings change it computes `1 / sub_02241fd0()` with
//!   `vdivss xmm0, xmm1, xmm0` and stores it with `vmovss [0x6849844], xmm0`. Both become
//!   `mov dword ptr [0x6849844], 0x39aec33e`, 10 of the 12 bytes. `xmm0` and `xmm1` are overwritten before
//!   they are read again (`vmovss xmm0, [rsi+0d4h]` at `0x224360a`, after a call).
//!
//! The store addresses are taken from the code that is found, not from constants in this module; the second site
//! is matched on the address the first one stores to. Original bytes are checked before writing like every patch.

use crate::patch::{Finder, Patch, PatchError, Pattern, Site};
use iced_x86::{Instruction, Mnemonic, OpKind, Register};

/// `1/3000 s` as `f32` bits: the game's own first floor.
pub const FLOOR_BITS: u32 = 0x39ae_c33e;

fn rip_memory_operand(i: &Instruction) -> bool {
    i.op0_kind() == OpKind::Memory && i.is_ip_rel_memory_operand()
}

/// `mov dword ptr [rip+d], 1/3000; vmovss [rip+d], xmm0; sete [rip+d]`: the floor setup of `sub_02226130`.
pub fn init_pattern() -> Pattern {
    Pattern::new("clock floor setup")
        .then(|i, _| {
            i.mnemonic() == Mnemonic::Mov
                && rip_memory_operand(i)
                && i.op1_kind() == OpKind::Immediate32
                && i.immediate32() == FLOOR_BITS
        })
        .then(|i, _| {
            i.mnemonic() == Mnemonic::Vmovss
                && rip_memory_operand(i)
                && i.op1_register() == Register::XMM0
        })
        .then(|i, _| i.mnemonic() == Mnemonic::Sete && rip_memory_operand(i))
}

/// `vdivss xmm0, xmm1, xmm0; vmovss [limit_floor], xmm0`: the recomputation after a settings change.
pub fn settings_pattern(limit_floor: u64) -> Pattern {
    Pattern::new("clock floor from settings")
        .then(|i, _| {
            i.mnemonic() == Mnemonic::Vdivss
                && i.op0_register() == Register::XMM0
                && i.op1_register() == Register::XMM1
                && i.op2_register() == Register::XMM0
        })
        .then(move |i, _| {
            i.mnemonic() == Mnemonic::Vmovss
                && rip_memory_operand(i)
                && i.op1_register() == Register::XMM0
                && i.ip_rel_memory_address() == limit_floor
        })
}

fn rel32(from_next: u32, target: u64, base: u64) -> [u8; 4] {
    let target = target.wrapping_sub(base);
    (target.wrapping_sub(u64::from(from_next)) as i32).to_le_bytes()
}

fn init_patch(finder: &Finder, site: &Site) -> Result<Patch, PatchError> {
    let [first, second, _] = [
        site.instructions[0],
        site.instructions[1],
        site.instructions[2],
    ];
    let base = finder.image().image_base;
    let mut code = vec![0xB8];
    code.extend_from_slice(&FLOOR_BITS.to_le_bytes());
    let at = first.rva;
    code.extend_from_slice(&[0x89, 0x05]);
    code.extend_from_slice(&rel32(
        at + 11,
        first.instruction.ip_rel_memory_address(),
        base,
    ));
    code.extend_from_slice(&[0x89, 0x05]);
    code.extend_from_slice(&rel32(
        at + 17,
        second.instruction.ip_rel_memory_address(),
        base,
    ));
    let replaced = usize::from(first.len) + usize::from(second.len);
    finder.patch_instructions("clock-floor-init", at, replaced, &code)
}

fn settings_patch(finder: &Finder, site: &Site) -> Result<Patch, PatchError> {
    let divide = site.instructions[0];
    let store = site.instructions[1];
    let base = finder.image().image_base;
    let mut code = vec![0xC7, 0x05];
    code.extend_from_slice(&rel32(
        divide.rva + 10,
        store.instruction.ip_rel_memory_address(),
        base,
    ));
    code.extend_from_slice(&FLOOR_BITS.to_le_bytes());
    let replaced = usize::from(divide.len) + usize::from(store.len);
    finder.patch_instructions("clock-floor-settings", divide.rva, replaced, &code)
}

/// The two stores of `1 / limit`, both turned into stores of `1/3000 s`.
pub fn find(finder: &Finder) -> Result<Vec<Patch>, PatchError> {
    let init = finder.find_one(&init_pattern())?;
    let limit_floor = init.instructions[1].instruction.ip_rel_memory_address();
    let settings = finder.find_one(&settings_pattern(limit_floor))?;
    Ok(vec![
        init_patch(finder, &init)?,
        settings_patch(finder, &settings)?,
    ])
}
