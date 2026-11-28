//! Game logic that counts frames or floors its time step, and the patches that make it follow real time above 60 fps.
//!
//! What the executable (build 1.0.2.1) does, found by reading its code, never by running it. Addresses are RVAs; add
//! `0x1_4000_0000` for the virtual address. Function names are the disassembler's `sub_<rva>`. The frame-rate
//! patches of [`crate::framerate`] free the clock (`STEP_FLOOR`, `CLOCK_FLOOR`); this module deals with the code that
//! was written for a clock that never ran faster than 60 steps per second.
//!
//! The title registers frame intervals for 60, 30, 20 and 15 frames per second (the table at `0x61ece60`; 120, 60, 40
//! and 30 on a 120 Hz display), so code that is tuned for frames was already run at 30 and at 60. The reference rate used here is
//! 60 steps per second: nothing below 60 fps changes (every patch is the identity there), above it the logic
//! advances at the 60 fps rate.
//!
//! # Findings, ranked by what a player notices
//!
//! 1. **Time step floors of 1/60 s** (patched by [`dt_floor_patches`]). Four functions clamp their time step to at least
//!    1/60 s (`step < 1/60 ? 1/60 : min(0.1, step)`, a literal at `0x6159e10`), the same mistake as the second
//!    floor of `sub_022264d0`. Above 60 fps they advance 1/60 s per frame, so everything they drive runs at up to
//!    twice the speed at 120 fps. `sub_00b74dd0` (compare `0xb74e03`, blend value `0xb74df6`) returns the time step of one
//!    entity (the record found by `sub_04e371c0`, step at `+0x54`, else the global step) to its five callers, all in one
//!    class (`sub_00b74be0`, `sub_00b74c70`, `sub_00b76020`, `sub_00b77200`, `sub_00b78390`): `sub_00b74be0` turns it
//!    into a blend factor `min(1, step * 10)` that moves a vector towards a target (`sub_05180510`), a rate that is
//!    per frame at the floor and per second below it. `sub_01241ed0` (`0x12421e0`, `0x12421e9`), `sub_031e5bc0`
//!    (`0x31e5c7f`, `0x31e5c88`) and `sub_031e67e0` (`0x31e69c7`, `0x31e69d0`) floor the step they receive and then
//!    ease a value towards a target in sub-steps of a per-object rate (`sub_031e5bc0` loops over whole sub-steps and
//!    interpolates the remainder), so the floor makes the easing run fast. All three look up per-entity extension
//!    records from an entity handle (`sub_04d9f7c0`); what exactly they animate could not be determined. The entity
//!    step is the global step times the entity's own time scale, so slow motion below 1/60 s per frame is held at the
//!    floor even at 30 fps: a patch that simply lowered the floor would change that, so the floor becomes
//!    `min(1/60, step of the main clock)` kept in a cell (see below). The patch points both loads of each function
//!    at the cell.
//! 2. **Timers counted in frames of the main clock** (patched by [`tick_scale_patches`]). The frame counter of the main
//!    clock (record `0xabbef40`, counter `+4`, whole milliseconds `+0`, step in seconds `+0xc`; `sub_02226040` adds 1 to
//!    it once per update) has 689 mentions in 534 functions: about 216 store it as a stamp, 105 test it for
//!    equality, 254 load it into a local and 47 subtract it directly; 27 hash it or take a modulo. Most uses mean "this frame" or "last
//!    frame" (`frame - stamp < 2`, `< 3`, equality) and do not depend on the rate. The ones that count frames as time
//!    are `frame - stamp` compared with 4 or more: a state that ends after 90 frames (`sub_031b23c0`), a window
//!    of 300 (`sub_05902910`) or 501 (`sub_025e1600`) frames, a 15 frame fade (`sub_008eeb30`), a cache stale after 31
//!    frames (`sub_02788e80`), and cool-downs read from data (`sub_04238850`, `sub_04238a40`, `sub_04611d80`). Above
//!    60 fps they ran in half the time or less. [`TICK_SITES`] lists the thirteen sites. Each is `sub r32, mem`
//!    producing the delta in frames; the patch jumps to a stub that scales the delta by the ratio of 60 Hz ticks to
//!    real frames and returns. The ratio comes from a deficit kept in a cell, updated after every main clock
//!    update by a wrapper around the call at `0x2226759`: the deficit moves 1/8 of the way each frame towards
//!    `max(0, 1 - step * 60)`, scaled by 65536. At or below 60 fps it stays 0 and the stubs return the delta
//!    unchanged. The averaging keeps one slow frame from letting a timer expire early. Sites that compare the delta
//!    with a constant in the stub's own span (`cmp`) are handled by copying the `cmp` into the stub.
//! 3. **A 1/30 s floor** in `sub_0088e740` (`0x88ea1b`): an accumulator in a ped animation task advances by
//!    `max(step, 1/30)`. It already runs at 60 fps, so what happens above is not "the same as at 60" in any way a patch
//!    could express; left alone.
//! 4. **An exponent of the frame delta** in `sub_013070e0` (`0x1307164`): `x / 2^(frame - stamp)`, a halving per
//!    frame. At 120 fps the decay is twice as fast per second. Not patched: the half-life would need the stamp.
//! 5. **Work spreading by frame parity or modulo** (`frame % 10` in `sub_00d66c10`; `frame & 1`, `& 7`, `& 0x3f`,
//!    `% n` in `sub_006260b0`, `sub_008861d0`, `sub_01fc47b0`, `sub_01fc47d0`, `sub_00087ae0`, `sub_01636520`,
//!    `sub_0211ce80`, `sub_00fbed90`, `sub_05255e50`, `sub_02d8a4d0`, `sub_03cb0fb0`): refresh a slice every N frames.
//!    More frames per second means more refreshes per second, which costs CPU time and changes nothing the player sees.
//! 6. **Jobs and budgets by frame** (`sub_00b082c0`: a streaming job is queued again after 10 frames,
//!    `sub_00b08660`): more frequent at high rates, harmless.
//! 7. **Fixed-step simulation.** No fixed step loop was found for gameplay or physics: the places that integrate in
//!    sub-steps (`sub_031e5bc0`) take a time step argument and work out the number of sub-steps from it. The right
//!    behaviour above 60 fps is the variable step the floors above restore, so no fixed-step patch is made. Whether
//!    the physics solver has its own cap could not be determined statically.
//! 8. The 1/30 s literal at `0x615b6b4` and 1/60 s at `0x6159e10` are otherwise time of day arithmetic, a frame
//!    rate normaliser (`sub_04ed92d0`) or a cap (`sub_009e5ce0`) and follow the step: see [`crate::framerate`].
//!
//! # The code cave and the cells
//!
//! The last page of `.text` is mapped executable and zero past the section's end (`0x5f40a00` to `0x5f41000`). The top
//! [`TICK_SLOT`] bytes of it hold the constants, the wrapper and the stubs of [`tick_scale_patches`]. The last page of
//! `.data` (`0xabce398` to `0xabcf000`) is mapped read-write and zero: its top [`CELLS`] bytes hold two words the
//! wrapper writes every frame, the averaged deficit (`u32`, scaled by 65536; 0 means 60 or fewer frames per second)
//! and the floor of the time step floors (`f32`, starting at 1/60 s, afterwards `max(1/3000, min(1/60, step))` of
//! the main clock). The floors then keep their old value up to 60 fps (the floor is the frame time, which is at least
//! 1/60 s) and fall to the real frame time above it, so slow motion that relied on them behaves as at 60 fps.
//! Code in the cave has no unwind information; none of it calls anything that can throw. Another patch that needs a
//! cave or a cell should take space from the bottom of the same pages.

use crate::patch::{Finder, Patch, PatchError, Pattern, Probe};
use crate::widescreen::fnv1a64;
use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};

/// `sub_022264d0`, the per-frame timer update.
pub const TIMER_UPDATE: u32 = 0x0222_64d0;
/// Record of the main game clock (`0xabbef40`): `+4` is the frame counter, `+0xc` the step in seconds.
pub const MAIN_CLOCK: u32 = 0x0abb_ef40;
const STEP_OFFSET: u32 = 0x0c;
const FRAME_OFFSET: u32 = 0x04;
/// 1/60 s as `f32` bits: the literal the floors read.
pub const ONE_SIXTIETH_BITS: u32 = 0x3c88_8889;
/// 1/3000 s as `f32` bits: the lowest floor the game itself allows its clocks.
pub const FLOOR_MIN_BITS: u32 = 0x39ae_c33e;
const REFERENCE_HZ: f32 = 60.0;
const GAIN: f32 = 0.125;
const ONE: f32 = 65536.0;

/// Bytes of the last `.data` page that hold the two cells: the deficit (`u32`) and the floor (`f32`).
pub const CELLS: u32 = 16;
/// Bytes of the last `.text` page for the constants, the wrapper and the stubs.
pub const TICK_SLOT: u32 = 0x400;
/// Floors found: functions that clamp their time step to 1/60 s, each with a compare and a blend value.
pub const DT_FLOOR_FUNCTIONS: [u32; 4] = [0x00b7_4dd0, 0x0124_1ed0, 0x031e_5bc0, 0x031e_67e0];

/// A `frame - stamp` that stands for time.
#[derive(Debug, Clone, Copy)]
pub struct TickSite {
    /// The `sub` that produces the delta in frames.
    pub rva: u32,
    /// Bytes of that instruction and those after it that the stub copies (at least 5).
    pub len: u8,
    /// FNV-1a 64 of those bytes in build 1.0.2.1; the bytes themselves are not stored.
    pub hash: u64,
    /// The constant the delta is compared with right after the subtraction, when there is one.
    pub frames: Option<u32>,
    pub function: u32,
    pub what: &'static str,
}

pub const TICK_SITES: [TickSite; 13] = [
    TickSite {
        rva: 0x008e_ee38,
        len: 6,
        hash: 0xe50e_b459_f25f_ce9c,
        frames: None,
        function: 0x008e_eb30,
        what: "fade factor min(1, delta / 15) of a ped control blend, stamp at +0x1610",
    },
    TickSite {
        rva: 0x031b_248f,
        len: 6,
        hash: 0x5961_33e1_3023_1986,
        frames: Some(90),
        function: 0x031b_23c0,
        what: "a state ends 90 frames after it began (stamp +0x214, state +0x210)",
    },
    TickSite {
        rva: 0x0590_2952,
        len: 9,
        hash: 0x8892_3343_e91b_91ff,
        frames: Some(300),
        function: 0x0590_2910,
        what: "a condition holds for 300 frames after its stamp (+0x48 of a ped extension)",
    },
    TickSite {
        rva: 0x025e_196c,
        len: 8,
        hash: 0xddbc_cd9e_632a_0045,
        frames: Some(501),
        function: 0x025e_1600,
        what: "a 501 frame window after a stamp (+0x3c)",
    },
    TickSite {
        rva: 0x0278_932b,
        len: 7,
        hash: 0xc1b6_5402_94b0_b944,
        frames: Some(31),
        function: 0x0278_8e80,
        what: "bit 16 of an entry's flags is set once 31 frames passed since the stamp at +0xe4",
    },
    TickSite {
        rva: 0x03cc_b9d5,
        len: 5,
        hash: 0x8784_f919_5a07_c448,
        frames: Some(59),
        function: 0x03cc_b9a0,
        what: "more than 59 frames since the stamp at +0x158",
    },
    TickSite {
        rva: 0x0558_bfb3,
        len: 7,
        hash: 0x96fb_6c20_410e_8f24,
        frames: Some(59),
        function: 0x0558_bf80,
        what: "more than 59 frames since the stamp at +0x150",
    },
    TickSite {
        rva: 0x017a_eade,
        len: 6,
        hash: 0x340a_dda1_16ac_8ee2,
        frames: Some(6),
        function: 0x017a_ead0,
        what: "fewer than 6 frames since the stamp at +0x14c",
    },
    TickSite {
        rva: 0x03a1_8777,
        len: 6,
        hash: 0xb3fb_fc2d_9038_599e,
        frames: Some(6),
        function: 0x03a1_8620,
        what: "fewer than 6 frames (signed) since the stamp at +0x690 holds a ped state",
    },
    TickSite {
        rva: 0x0251_5c0f,
        len: 6,
        hash: 0xb99f_cd72_f2a5_9161,
        frames: Some(5),
        function: 0x0251_5ba0,
        what: "a flag bit is set once 5 frames passed since the stamp at +0xa0",
    },
    TickSite {
        rva: 0x0423_889e,
        len: 6,
        hash: 0x0653_d25e_cd7b_53a1,
        frames: None,
        function: 0x0423_8850,
        what: "delta since the stamp at +0x70 against a count read from data (+0x24 of the owner's type)",
    },
    TickSite {
        rva: 0x0423_8a50,
        len: 7,
        hash: 0x6452_aae0_a8e6_60cf,
        frames: None,
        function: 0x0423_8a40,
        what: "the same stamp and count as 0x423889e, read by the sibling that reports the remaining time",
    },
    TickSite {
        rva: 0x0461_1d93,
        len: 6,
        hash: 0x5278_084e_d840_5321,
        frames: None,
        function: 0x0461_1d80,
        what: "delta since the stamp at +0x130 against a count at +0x10 of the caller",
    },
];

/// The call in `sub_022264d0` that updates the main clock, hashed with its displacement.
const HOOK_HASH: u64 = 0x27a3_e7ca_94f4_2cf3;

fn rel32(next: u32, target: u32) -> [u8; 4] {
    (i64::from(target) - i64::from(next))
        .try_into()
        .map(i32::to_le_bytes)
        .unwrap_or([0; 4])
}

fn not_found(what: impl Into<String>) -> PatchError {
    PatchError::NotFound { name: what.into() }
}

/// The end of the page after the last executable (or the largest writable) section: the loader maps it with the
/// section's protection and zeroed. Fails when something else starts inside it or fewer than `needed` bytes remain.
fn slack_end(finder: &Finder, code: bool, needed: u32) -> Result<u32, PatchError> {
    let image = finder.image();
    let section = if code {
        image
            .sections
            .iter()
            .filter(|s| s.executable())
            .max_by_key(|s| s.virtual_address)
    } else {
        image
            .sections
            .iter()
            .filter(|s| s.writable() && !s.executable())
            .max_by_key(|s| s.virtual_size.max(s.raw_size))
    }
    .ok_or_else(|| not_found("section for a cave"))?;
    let extent = section.virtual_address + section.virtual_size.max(section.raw_size);
    let end = (extent + 0xfff) & !0xfff;
    let crowded = image
        .sections
        .iter()
        .any(|s| s.virtual_address > section.virtual_address && s.virtual_address < end);
    if crowded || end - extent < needed {
        return Err(PatchError::TooLong {
            old: (end - extent) as usize,
            new: needed as usize,
        });
    }
    Ok(end)
}

/// The two cells in the zeroed tail of `.data`: the averaged deficit and the floor of the time step floors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cells {
    pub deficit: u32,
    pub floor: u32,
}

pub fn cells(finder: &Finder) -> Result<Cells, PatchError> {
    let end = slack_end(finder, false, CELLS)?;
    Ok(Cells {
        deficit: end - CELLS,
        floor: end - CELLS + 4,
    })
}

/// A patch that writes `bytes` into slack: it checks that the memory is zero first.
fn cave_patch(finder: &Finder, name: &str, rva: u32, bytes: Vec<u8>) -> Result<Patch, PatchError> {
    let original = vec![0u8; bytes.len()];
    if let Some(found) = finder.image().read(rva, bytes.len())
        && found != original.as_slice()
    {
        return Err(PatchError::Mismatch {
            rva,
            expected: original,
            found: found.to_vec(),
        });
    }
    Ok(Patch {
        name: name.into(),
        rva,
        original,
        replacement: bytes,
    })
}

fn is_literal(bits: u32) -> impl Fn(&Instruction, &Probe) -> bool + Send + Sync {
    move |i, probe| {
        matches!(i.mnemonic(), Mnemonic::Vcmpss | Mnemonic::Vbroadcastss)
            && probe
                .rip_target(i)
                .and_then(|rva| probe.u32_at(rva))
                .is_some_and(|value| value == bits)
    }
}

/// Every compare or broadcast of 1/60 s in a function that clamps its step to it.
pub fn dt_floor_pattern() -> Pattern {
    Pattern::new("time step floor of 1/60 s").then(is_literal(ONE_SIXTIETH_BITS))
}

/// Where things are in one build of the executable; tests describe a synthetic image with the same shape.
#[derive(Debug, Clone, Copy)]
pub struct Layout<'a> {
    /// The main clock record.
    pub record: u32,
    /// The function that updates the clocks and holds the call to wrap.
    pub update: u32,
    /// FNV-1a 64 of that call's five bytes.
    pub hook_hash: u64,
    pub sites: &'a [TickSite],
    /// Functions that floor their step at 1/60 s, by first byte.
    pub floors: &'a [u32],
}

/// Build 1.0.2.1.
pub const GAME: Layout<'static> = Layout {
    record: MAIN_CLOCK,
    update: TIMER_UPDATE,
    hook_hash: HOOK_HASH,
    sites: &TICK_SITES,
    floors: &DT_FLOOR_FUNCTIONS,
};

/// The patches of the time step floors: the floor cell starts at 1/60 s, and the two loads of 1/60 s in each flooring
/// function read the cell instead (the wrapper of the main clock update keeps it at `min(1/60, step)`).
pub fn dt_floor_patches(
    finder: &Finder,
    layout: &Layout,
    cells: &Cells,
) -> Result<Vec<Patch>, PatchError> {
    let sites = finder.find(&dt_floor_pattern());
    let mut functions: Vec<u32> = sites
        .iter()
        .filter(|s| s.instructions[0].instruction.mnemonic() == Mnemonic::Vcmpss)
        .map(|s| s.function.begin)
        .collect();
    functions.sort_unstable();
    functions.dedup();
    if functions != layout.floors {
        return Err(PatchError::Ambiguous {
            name: "time step floor of 1/60 s".into(),
            count: functions.len(),
        });
    }
    let mut patches = vec![cave_patch(
        finder,
        "dt-floor cell",
        cells.floor,
        ONE_SIXTIETH_BITS.to_le_bytes().to_vec(),
    )?];
    for site in sites
        .iter()
        .filter(|s| functions.contains(&s.function.begin))
    {
        let first = site.instructions[0];
        let displacement = i32::from_le_bytes(rel32(first.rva + u32::from(first.len), cells.floor));
        patches.push(finder.set_displacement("dt-floor", site, 0, displacement)?);
    }
    if patches.len() != 1 + 2 * layout.floors.len() {
        return Err(PatchError::Ambiguous {
            name: "time step floor loads".into(),
            count: patches.len() - 1,
        });
    }
    Ok(patches)
}

/// Every patch of this module: the wrapper, the stubs and the floors.
pub fn find_frame_timers(finder: &Finder) -> Result<Vec<Patch>, PatchError> {
    frame_timer_patches(finder, &GAME)
}

pub fn frame_timer_patches(finder: &Finder, layout: &Layout) -> Result<Vec<Patch>, PatchError> {
    let cells = cells(finder)?;
    let mut patches = tick_scale_patches(finder, layout, &cells)?;
    patches.extend(dt_floor_patches(finder, layout, &cells)?);
    Ok(patches)
}

/// Machine code for the cave, written by hand to stay free of any encoder dependency.
struct Code {
    base: u32,
    bytes: Vec<u8>,
}

impl Code {
    fn new(base: u32) -> Self {
        Self {
            base,
            bytes: Vec::new(),
        }
    }

    fn here(&self) -> u32 {
        self.base + self.bytes.len() as u32
    }

    fn put(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// A RIP-relative operand: `tail` bytes of immediate follow the displacement.
    fn rip(&mut self, target: u32, tail: u32) {
        let next = self.here() + 4 + tail;
        self.put(&rel32(next, target));
    }

    fn jump(&mut self, target: u32) {
        self.put(&[0xE9]);
        self.rip(target, 0);
    }
}

/// The REX prefix for a 32 or 64 bit operation on registers `reg` and `rm`, when it is needed.
fn rex(wide: bool, reg: u8, rm: u8) -> Option<u8> {
    let value = 0x40 | u8::from(wide) << 3 | u8::from(reg >= 8) << 2 | u8::from(rm >= 8);
    (value != 0x40).then_some(value)
}

fn modrm(reg: u8, rm: u8) -> u8 {
    0xC0 | (reg & 7) << 3 | (rm & 7)
}

fn number(register: Register) -> Option<u8> {
    register.is_gpr32().then(|| register.number() as u8)
}

/// What the stub at a site does: the copied instructions, then the scaling of the delta in `reg`.
fn stub(code: &mut Code, original: &[u8], first_len: usize, reg: u8, deficit: u32, back: u32) {
    let scratch = if reg == 2 { 11 } else { 2 };
    code.put(&original[..first_len]);
    if let Some(prefix) = rex(false, reg, reg) {
        code.put(&[prefix]);
    }
    code.put(&[0x85, modrm(reg, reg)]);
    let mut scale = Code::new(0);
    if scratch >= 8 {
        scale.put(&[0x41]);
    }
    scale.put(&[0x50 + (scratch & 7)]);
    if let Some(prefix) = rex(false, scratch, 0) {
        scale.put(&[prefix]);
    }
    scale.put(&[0x8B, 0x05 | (scratch & 7) << 3]);
    // The displacement is relative to the real position: filled in below.
    let load_at = scale.bytes.len();
    scale.put(&[0; 4]);
    scale.put(&[
        0x40 | 8 | u8::from(scratch >= 8) << 2 | u8::from(reg >= 8),
        0x0F,
        0xAF,
    ]);
    scale.put(&[modrm(scratch, reg)]);
    scale.put(&[
        0x48 | u8::from(scratch >= 8),
        0xC1,
        0xE8 | (scratch & 7),
        16,
    ]);
    if let Some(prefix) = rex(false, scratch, reg) {
        scale.put(&[prefix]);
    }
    scale.put(&[0x29, modrm(scratch, reg)]);
    if scratch >= 8 {
        scale.put(&[0x41]);
    }
    scale.put(&[0x58 + (scratch & 7)]);
    code.put(&[0x78, scale.bytes.len() as u8]);
    let start = code.here();
    let next = start + load_at as u32 + 4;
    scale.bytes[load_at..load_at + 4].copy_from_slice(&rel32(next, deficit));
    code.put(&scale.bytes);
    code.put(&original[first_len..]);
    code.jump(back);
}

/// The wrapper of the main clock update: calls it, then moves the deficit a step towards `max(0, 1 - step * 60)` and
/// sets the floor cell to `max(1/3000, min(1/60, step))`.
fn wrapper(code: &mut Code, callee: u32, constants: u32, record: u32, cells: &Cells) {
    let step = record + STEP_OFFSET;
    code.put(&[0x48, 0x83, 0xEC, 0x28, 0xE8]);
    code.rip(callee, 0);
    code.put(&[0xC5, 0xFA, 0x10, 0x1D]);
    code.rip(step, 0);
    code.put(&[0xC5, 0xE2, 0x59, 0x1D]);
    code.rip(constants, 0);
    code.put(&[0xC5, 0xFA, 0x10, 0x25]);
    code.rip(constants + 4, 0);
    code.put(&[0xC5, 0xDA, 0x5C, 0xDB]);
    code.put(&[0xC5, 0xD8, 0x57, 0xE4]);
    code.put(&[0xC5, 0xE2, 0x5F, 0xDC]);
    code.put(&[0xC5, 0xDA, 0x2A, 0x25]);
    code.rip(cells.deficit, 0);
    code.put(&[0xC5, 0xE2, 0x5C, 0xDC]);
    code.put(&[0xC5, 0xE2, 0x59, 0x1D]);
    code.rip(constants + 8, 0);
    code.put(&[0xC5, 0xDA, 0x58, 0xE3]);
    code.put(&[0xC5, 0xFA, 0x2D, 0xD4]);
    code.put(&[0x89, 0x15]);
    code.rip(cells.deficit, 0);
    code.put(&[0xC5, 0xFA, 0x10, 0x1D]);
    code.rip(step, 0);
    code.put(&[0xC5, 0xE2, 0x5D, 0x1D]);
    code.rip(constants + 12, 0);
    code.put(&[0xC5, 0xE2, 0x5F, 0x1D]);
    code.rip(constants + 16, 0);
    code.put(&[0xC5, 0xFA, 0x11, 0x1D]);
    code.rip(cells.floor, 0);
    code.put(&[0x48, 0x83, 0xC4, 0x28, 0xC3]);
}

/// The constants at the start of the stub area: `60 * 65536`, `65536`, the gain, 1/60 s, 1/3000 s.
fn constants() -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0..4].copy_from_slice(&(REFERENCE_HZ * ONE).to_le_bytes());
    out[4..8].copy_from_slice(&ONE.to_le_bytes());
    out[8..12].copy_from_slice(&GAIN.to_le_bytes());
    out[12..16].copy_from_slice(&ONE_SIXTIETH_BITS.to_le_bytes());
    out[16..20].copy_from_slice(&FLOOR_MIN_BITS.to_le_bytes());
    out
}

fn decode_at(finder: &Finder, rva: u32, len: usize) -> Result<Vec<Instruction>, PatchError> {
    let bytes = finder
        .image()
        .read(rva, len)
        .ok_or(PatchError::Outside { rva })?;
    let base = finder.image().image_base;
    let mut decoder = Decoder::with_ip(64, bytes, base + u64::from(rva), DecoderOptions::NONE);
    let mut out = Vec::new();
    while decoder.can_decode() {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            break;
        }
        out.push(instruction);
    }
    Ok(out)
}

/// True when the first instruction after `rva` that touches the flags overwrites them without reading them.
fn flags_are_rewritten(finder: &Finder, rva: u32) -> bool {
    let Ok(instructions) = decode_at(finder, rva, 192) else {
        return false;
    };
    for instruction in instructions.iter().take(40) {
        if instruction.flow_control() == FlowControl::Call {
            return true;
        }
        if instruction.rflags_read() != 0 {
            return false;
        }
        if instruction.rflags_modified() != 0 {
            return true;
        }
        if instruction.flow_control() != FlowControl::Next {
            return false;
        }
    }
    false
}

/// What a tick site is: the register that receives the delta and the length of its first instruction.
fn describe(finder: &Finder, site: &TickSite) -> Result<(u8, usize), PatchError> {
    let label = || format!("frame delta at {:#x}", site.rva);
    let bytes = finder
        .image()
        .read(site.rva, usize::from(site.len))
        .ok_or(PatchError::Outside { rva: site.rva })?;
    if fnv1a64(bytes) != site.hash {
        return Err(not_found(format!("{}: code changed", label())));
    }
    let instructions = decode_at(finder, site.rva, usize::from(site.len))?;
    let total: usize = instructions.iter().map(Instruction::len).sum();
    let first = *instructions.first().ok_or_else(|| not_found(label()))?;
    let reg = number(first.op0_register());
    let shape = first.mnemonic() == Mnemonic::Sub
        && first.op0_kind() == OpKind::Register
        && matches!(first.op1_kind(), OpKind::Register | OpKind::Memory)
        && total == usize::from(site.len);
    let movable = instructions
        .iter()
        .all(|i| !i.is_ip_rel_memory_operand() && i.flow_control() == FlowControl::Next);
    match reg {
        Some(reg) if shape && movable => Ok((reg, first.len())),
        _ => Err(not_found(format!("{}: unexpected instructions", label()))),
    }
}

fn reads_frame_counter(finder: &Finder, site: &TickSite, record: u32) -> bool {
    let Some(function) = finder.image().function_containing(site.rva) else {
        return false;
    };
    let counter = u64::from(record + FRAME_OFFSET) + finder.image().image_base;
    decode_at(
        finder,
        function.begin,
        (site.rva - function.begin) as usize + 1,
    )
    .is_ok_and(|code| {
        code.iter().any(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.is_ip_rel_memory_operand()
                && i.ip_rel_memory_address() == counter
        })
    })
}

/// `lea rcx, [main clock]` and the call that follows: the update of the main clock in `sub_022264d0`.
pub fn hook_pattern(record: u32) -> Pattern {
    Pattern::new("main clock update call")
        .then(move |i, probe| {
            i.mnemonic() == Mnemonic::Lea
                && i.op0_register() == Register::RCX
                && probe.rip_target(i) == Some(record)
        })
        .then_within(6, |i, _| {
            i.mnemonic() == Mnemonic::Call && i.op0_kind() == OpKind::NearBranch64
        })
}

/// The wrapper around the main clock update and a stub at each frame delta site.
pub fn tick_scale_patches(
    finder: &Finder,
    layout: &Layout,
    cells: &Cells,
) -> Result<Vec<Patch>, PatchError> {
    let end = slack_end(finder, true, TICK_SLOT)?;
    let area = end - TICK_SLOT;
    let image_base = finder.image().image_base;

    let function = finder
        .image()
        .function_containing(layout.update)
        .ok_or_else(|| not_found("timer update"))?;
    let hooks = finder.find_in(function, &hook_pattern(layout.record));
    let [hook] = hooks.as_slice() else {
        return Err(PatchError::Ambiguous {
            name: "main clock update call".into(),
            count: hooks.len(),
        });
    };
    let call = hook.instructions[1];
    let call_bytes = finder
        .image()
        .read(call.rva, 5)
        .ok_or(PatchError::Outside { rva: call.rva })?;
    if call.len != 5 || fnv1a64(call_bytes) != layout.hook_hash {
        return Err(not_found("main clock update call: code changed"));
    }
    let callee = (call.instruction.near_branch_target() - image_base) as u32;

    let mut code = Code::new(area);
    code.put(&constants());
    let wrapper_at = code.here();
    wrapper(&mut code, callee, area, layout.record, cells);
    let mut patches = Vec::new();
    for site in layout.sites {
        let (reg, first_len) = describe(finder, site)?;
        if !reads_frame_counter(finder, site, layout.record) {
            return Err(not_found(format!(
                "frame counter read before {:#x}",
                site.rva
            )));
        }
        if !flags_are_rewritten(finder, site.rva + first_len as u32) {
            return Err(not_found(format!("flags after {:#x}", site.rva)));
        }
        let bytes = finder
            .image()
            .read(site.rva, usize::from(site.len))
            .ok_or(PatchError::Outside { rva: site.rva })?
            .to_vec();
        while !code.here().is_multiple_of(16) {
            code.put(&[0xCC]);
        }
        let entry = code.here();
        stub(
            &mut code,
            &bytes,
            first_len,
            reg,
            cells.deficit,
            site.rva + u32::from(site.len),
        );
        let mut jump = vec![0xE9];
        jump.extend_from_slice(&rel32(site.rva + 5, entry));
        patches.push(finder.patch_instructions(
            format!("frame-timers {:#x}", site.rva),
            site.rva,
            usize::from(site.len),
            &jump,
        )?);
    }
    if code.bytes.len() > TICK_SLOT as usize {
        return Err(PatchError::TooLong {
            old: TICK_SLOT as usize,
            new: code.bytes.len(),
        });
    }
    let mut call_to_wrapper = vec![0xE8];
    call_to_wrapper.extend_from_slice(&rel32(call.rva + 5, wrapper_at));
    patches.insert(
        0,
        finder.patch_instructions("frame-timers hook", call.rva, 5, &call_to_wrapper)?,
    );
    patches.insert(
        0,
        cave_patch(finder, "frame-timers cave", area, code.bytes)?,
    );
    Ok(patches)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x1_4000_0000;

    fn decode(bytes: &[u8], at: u32) -> Vec<Instruction> {
        let mut decoder = Decoder::with_ip(64, bytes, BASE + u64::from(at), DecoderOptions::NONE);
        let mut out = Vec::new();
        while decoder.can_decode() {
            out.push(decoder.decode());
        }
        out
    }

    fn register32(number: u8) -> Register {
        const ALL: [Register; 16] = [
            Register::EAX,
            Register::ECX,
            Register::EDX,
            Register::EBX,
            Register::ESP,
            Register::EBP,
            Register::ESI,
            Register::EDI,
            Register::R8D,
            Register::R9D,
            Register::R10D,
            Register::R11D,
            Register::R12D,
            Register::R13D,
            Register::R14D,
            Register::R15D,
        ];
        ALL[usize::from(number)]
    }

    fn to64(register: Register) -> Register {
        register.full_register()
    }

    /// `sub reg, other` then `cmp reg, 3Bh`, as the stub's source bytes.
    fn source(reg: u8) -> (Vec<u8>, usize) {
        let other = if reg == 1 { 2 } else { 1 };
        let mut sub = Vec::new();
        if let Some(prefix) = rex(false, other, reg) {
            sub.push(prefix);
        }
        sub.extend_from_slice(&[0x29, modrm(other, reg)]);
        let first = sub.len();
        if let Some(prefix) = rex(false, 0, reg) {
            sub.push(prefix);
        }
        sub.extend_from_slice(&[0x83, 0xF8 | (reg & 7), 0x3B]);
        (sub, first)
    }

    #[test]
    fn the_stub_scales_whichever_register_holds_the_delta() {
        let deficit = 0x0abc_ff00;
        let back = 0x0100_0500;
        for reg in (0..16u8).filter(|r| *r != 4) {
            let (original, first) = source(reg);
            let mut code = Code::new(0x05f4_0c00);
            stub(&mut code, &original, first, reg, deficit, back);
            let instructions = decode(&code.bytes, code.base);
            let mnemonics: Vec<Mnemonic> = instructions.iter().map(Instruction::mnemonic).collect();
            assert_eq!(
                mnemonics,
                [
                    Mnemonic::Sub,
                    Mnemonic::Test,
                    Mnemonic::Js,
                    Mnemonic::Push,
                    Mnemonic::Mov,
                    Mnemonic::Imul,
                    Mnemonic::Shr,
                    Mnemonic::Sub,
                    Mnemonic::Pop,
                    Mnemonic::Cmp,
                    Mnemonic::Jmp,
                ],
                "register {reg}"
            );
            let delta = register32(reg);
            let scratch = if reg == 2 {
                Register::R11
            } else {
                Register::RDX
            };
            let scratch32 = scratch.full_register32();
            assert_eq!(instructions[1].op0_register(), delta);
            assert_eq!(instructions[1].op1_register(), delta);
            assert_eq!(instructions[3].op0_register(), scratch);
            assert_eq!(instructions[4].op0_register(), scratch32);
            assert_eq!(
                instructions[4].ip_rel_memory_address(),
                BASE + u64::from(deficit)
            );
            assert_eq!(instructions[5].op0_register(), scratch);
            assert_eq!(instructions[5].op1_register(), to64(delta));
            assert_eq!(instructions[6].op0_register(), scratch);
            assert_eq!(instructions[6].immediate8(), 16);
            assert_eq!(instructions[7].op0_register(), delta);
            assert_eq!(instructions[7].op1_register(), scratch32);
            assert_eq!(instructions[8].op0_register(), scratch);
            assert_eq!(
                instructions[2].near_branch_target(),
                instructions[9].ip(),
                "the skip lands after the pop"
            );
            assert_eq!(instructions[9].op0_register(), delta);
            assert_eq!(
                instructions[10].near_branch_target(),
                BASE + u64::from(back)
            );
            assert_eq!(
                instructions.iter().map(Instruction::len).sum::<usize>(),
                code.bytes.len()
            );
        }
    }

    #[test]
    fn the_wrapper_calls_the_clock_update_and_stores_the_deficit_and_the_floor() {
        let record = 0x0abb_ef40;
        let cells = Cells {
            deficit: 0x0abc_ff00,
            floor: 0x0abc_ff04,
        };
        let mut code = Code::new(0x05f4_0c20);
        wrapper(&mut code, 0x0222_6040, 0x05f4_0c00, record, &cells);
        let instructions = decode(&code.bytes, code.base);
        assert!(instructions.iter().all(|i| !i.is_invalid()));
        assert_eq!(instructions[0].mnemonic(), Mnemonic::Sub);
        assert_eq!(instructions[1].mnemonic(), Mnemonic::Call);
        assert_eq!(instructions[1].near_branch_target(), BASE + 0x0222_6040);
        let reads: Vec<u64> = instructions
            .iter()
            .filter(|i| i.is_ip_rel_memory_operand())
            .map(Instruction::ip_rel_memory_address)
            .collect();
        let want = |rva: u32| BASE + u64::from(rva);
        assert_eq!(
            reads,
            [
                want(record + STEP_OFFSET),
                want(0x05f4_0c00),
                want(0x05f4_0c04),
                want(cells.deficit),
                want(0x05f4_0c08),
                want(cells.deficit),
                want(record + STEP_OFFSET),
                want(0x05f4_0c0c),
                want(0x05f4_0c10),
                want(cells.floor),
            ]
        );
        assert_eq!(
            instructions[instructions.len() - 1].mnemonic(),
            Mnemonic::Ret
        );
        assert_eq!(constants()[0..4], (60.0f32 * 65536.0).to_le_bytes());
        assert_eq!(constants()[12..16], ONE_SIXTIETH_BITS.to_le_bytes());
        assert_eq!(constants()[16..20], FLOOR_MIN_BITS.to_le_bytes());
    }

    #[test]
    fn each_site_is_long_enough_for_a_jump_and_the_functions_differ() {
        assert!(
            TICK_SITES
                .windows(2)
                .all(|w| w[0].function != w[1].function)
        );
        for site in &TICK_SITES {
            assert!(site.len >= 5 && site.len <= 9, "{:#x}", site.rva);
            assert!(site.rva > site.function);
        }
    }
}
