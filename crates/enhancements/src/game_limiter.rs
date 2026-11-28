//! The game's own frame waits, which would hold the rate once the frame events fire at once.
//!
//! What the executable (build 1.0.2.1) does, found by reading its code, never by running it. Addresses are RVAs.
//! The title paces itself in three ways, all keyed on a per-frame record (array at `0x75de4b0`, stride `0x90`, the
//! frame index is the slot, the interval in microseconds at `+0x18`, the pacing mode byte at `+0x20`, the frame's
//! start counter at `+0x88`). `sub_01df4dc0` fills a record from the globals `0x98f84ac` (the interval) and
//! `0x98f84b4` (the mode):
//!
//! - mode 2: the main thread blocks in `WaitFrameEventX` (`sub_024845b0`). The frame events are ours, so with a
//!   frame limit that fires them at once this wait does not hold anything back.
//! - mode 1 and 4: `sub_02484540` and `sub_02484660` wait until the previous record's start counter (`+0x88`) plus the
//!   interval has passed (mode 4 scales the interval by the float at `0x615c360`).
//! - mode 3: the render thread (`sub_02481e90` through `sub_024827c0`) waits until the previous frame's stamp at
//!   `+0x58`, less a running average of the frame time, plus the interval has passed.
//!
//! Modes 1, 3 and 4 end in one routine, `sub_02482bc0`: it converts the target counter value into milliseconds,
//! calls `Sleep` through `sub_02482c50` while more than a threshold (`0x6159bd4`) remains, then spins on
//! `QueryPerformanceCounter` until the target. The interval is `1e6 / min(limit or 1000, 1000)` microseconds, from
//! the user's frame limit in the settings object (`+0x210`), written to `0x98f84ac` by `sub_01e0fb30`
//! (`0x1e0fc94`) and `sub_022434a0` (`0x22435ce`); its initial value is 1000 (`0x246c930`). With these modes the game
//! therefore runs at no more than its own limit, up to 1000 frames a second, whatever the frame events do.
//!
//! The mode byte is written once, to 0, by the static initialiser `sub_024709c0` (`0x2470efd`), and read by
//! `sub_01df4dc0`, `sub_00cd7320` and `sub_02478440` (`0x2478d86`); no other instruction
//! in the image stores to `0x98f84b4`: every code reference was listed and the file holds no absolute pointer to it. With mode 0 `sub_009ef0f0` waits
//! for nothing, so as far as the code goes none of the three waits runs. Whether something outside the image's
//! code (a script, a console variable table reached through a computed pointer) sets another mode could not be
//! determined statically. The patch makes that question moot for the cap: with a custom frame limit `sub_02482bc0`
//! returns at once, so no mode can hold the rate below ours.
//!
//! `sub_00120530` is a second limiter of the same kind: a counter-based one that, when its enable byte (`0x7453ec9`,
//! read only here, never written anywhere in the image) is set, waits until `1e6 / interval` ticks have passed since
//! its last call (`0x7453f20`, `0x7453f28`) and returns 1 when more than the threshold remains so its callers
//! (`sub_02257e80`, `sub_04fd1b20`) skip the frame. With the byte at 0 it returns 0 straight away. The patch makes it
//! return 0 always. Its state is read nowhere else.
//!
//! Both patches are `xor eax, eax; ret` over the three `push` instructions that open the function, three bytes each,
//! so no code cave is needed. The sites are named by the function's start and the hash of the whole function; the
//! bytes are not stored.

use crate::patch::{Finder, Patch, PatchError};
use crate::widescreen::fnv1a64;

/// `xor eax, eax; ret`.
pub const RETURN_ZERO: [u8; 3] = [0x31, 0xC0, 0xC3];
/// `push rsi; push rdi; push rbx`: what both functions start with, the bytes the patch replaces.
const PROLOGUE: usize = 3;

/// One wait routine of the game.
#[derive(Debug, Clone, Copy)]
pub struct Wait {
    pub name: &'static str,
    pub rva: u32,
    /// FNV-1a 64 of the whole function in build 1.0.2.1.
    pub hash: u64,
    pub what: &'static str,
}

pub const WAITS: [Wait; 2] = [
    Wait {
        name: "game frame wait",
        rva: 0x0248_2bc0,
        hash: 0x46e0_0986_d8d6_7abb,
        what: "sub_02482bc0: sleep, then spin until a counter value; the end of modes 1, 3 and 4",
    },
    Wait {
        name: "game counter limiter",
        rva: 0x0012_0530,
        hash: 0x63dc_349f_df87_b3ab,
        what: "sub_00120530: counter-based limiter behind an enable byte nothing sets",
    },
];

/// The hash of the function at `wait.rva`, when one starts there.
pub fn function_hash(finder: &Finder, wait: &Wait) -> Option<u64> {
    let image = finder.image();
    let function = image.function_containing(wait.rva)?;
    if function.begin != wait.rva {
        return None;
    }
    image
        .read(function.begin, (function.end - function.begin) as usize)
        .map(fnv1a64)
}

/// The patches that make `waits` return at once.
pub fn find_in(finder: &Finder, waits: &[Wait]) -> Result<Vec<Patch>, PatchError> {
    waits
        .iter()
        .map(|wait| {
            if function_hash(finder, wait) != Some(wait.hash) {
                return Err(PatchError::NotFound {
                    name: wait.name.to_string(),
                });
            }
            finder.patch_instructions(
                format!("{} at {:#x}", wait.name, wait.rva),
                wait.rva,
                PROLOGUE,
                &RETURN_ZERO,
            )
        })
        .collect()
}

pub fn find(finder: &Finder) -> Result<Vec<Patch>, PatchError> {
    find_in(finder, &WAITS)
}
