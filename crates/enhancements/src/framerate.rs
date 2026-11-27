//! Uncapping the frame rate: the game behaviour tied to frame time, and the patches that free it.
//!
//! What the executable (build 1.0.2.1) does with frame time, found by reading its code, never by running it.
//! Addresses are RVAs; add `0x1_4000_0000` for the virtual address. Function names are the disassembler's
//! `sub_<rva>`.
//!
//! # The frame event wait
//!
//! The title keeps the device in a global (`0x8759d00`, filled in `sub_02478440` from `D3D12XboxCreateDevice`).
//! `sub_0247ab10` registers the cadence: `SetFrameIntervalX(NULL, interval_us, latency, 0)` through device slot 78
//! (`call [rax+270h]` at `0x247abd8`), then `ScheduleFrameEventX(0, offset_us, NULL, 0)` through slot 79 (`0x247abfc`).
//! The interval comes from a table at `0x61ece60`, five dwords per row, indexed by a divisor of 1 to 4: row 0
//! (display without a 120 Hz mode) holds 16667, 16667, 33333, 50000, 66667 microseconds; row 1 (a 120 Hz mode was
//! found by `sub_02481ca0`) holds 8333, 8333, 16667, 25000, 33333. The divisor is the display rate divided by the
//! title's own frame limit, at most 4. The offset is the remainder that aligns a limit that is not a divisor of
//! the display rate; the latency is 3, or 2 or more when the limit is not a multiple. After a suspend and resume
//! `sub_02490080` registers again (`sub_0247ab10`, then slot 79 with event type 2 at `0x24900e3`), as the public samples do.
//!
//! `sub_009ef0f0` is the per-frame synchronisation, bracketed by two `QueryPerformanceCounter` calls for the
//! profiler. By the mode byte of the frame record (`0x75de4b0`, stride `0x90`, mode at `+0x20`): 2 calls
//! `sub_024845b0`, which waits `WaitFrameEventX(0, INFINITE, NULL, 0, &token)` through slot 80 (`0x2484637`) and keeps
//! the token in the record; 1 and 4 call `sub_02484540` and `sub_02484660`, which wait on the performance
//! counter until the frame record's microsecond interval (`+0x18`) has passed (`sub_02482bc0`, sleep then spin).
//! `sub_02478440` treats mode 2 as "frame events" when it builds the device. Nothing in the image writes the mode
//! except an initial zero (`0x2470efd`), so how it becomes 2 is not known statically.
//!
//! The render thread (`sub_02481e90`) waits twice more: event type 2 with the token the main thread stored,
//! timeout infinite (`0x2482405`), and event type 0 with timeout 0 and flag 2 (`0x2482448`), which polls the
//! current token. Both go through slot 80. Type 2 means something other than the frame origin and its exact
//! meaning is not known; a presenter should not feed it to the origin grid.
//!
//! # The game's own limit
//!
//! The user's frame limit lives in the settings object at `+0x210` (`+0x214` is the fallback when that is 0,
//! `sub_02241fd0`). `sub_01e0fb30` (`0x1e0fc94`) and `sub_022434a0` (`0x22435ce`) turn it into a microsecond
//! interval, `1e6 / min(limit or 1000, 1000)`, in the global at `0x98f84ac`; a limit of 0 therefore means 1000.
//! `sub_00120530`, a counter-based limiter that reads that interval, is gated by a byte at `0x7453ec9` that
//! nothing in the image sets, so it is off.
//!
//! # The simulation step
//!
//! `sub_022264d0` is the per-frame timer update, called from the main loop (`sub_00812620`, `sub_021b7970`,
//! `sub_021b79f0`, `sub_02246930`) according to a flag at `0x7453cc0`. It takes the counter delta times
//! the seconds-per-tick value that `sub_04daa2e0` derives from `QueryPerformanceFrequency` (`0x9e4f5c0`), and
//! clamps it before storing the step as seconds (`0xa958364`), whole milliseconds (`0xa958368`) and its inverse
//! (`0xa95836c`):
//!
//! - ceiling `0x6159ec0` = `0x3d888889` = 1/15 s, a constant read in 49 functions;
//! - floor `0x6847660`, set to `0x39aec33e` = 1/3000 s by `sub_02226130` (`0x2226190`);
//! - and, when the flag at `0x7453cc0` is 1 (the usual case), a second floor `0x6849844` = 1 / limit, stored by
//!   `sub_02226130` (`0x222619a`, argument computed at `0x2241198` from `sub_02241fd0`) and again by `sub_022434a0`
//!   (`0x22435ea`) whenever the settings change. It is loaded at `0x2226854`.
//!
//! The second floor is the one that matters. On the console frames never come faster than the limit, so the step
//! is the real frame time. Faster than that, the step stays at 1/limit while less time has passed, and the whole
//! game runs fast. With the frame events uncapped or capped above the title's own limit that is exactly what
//! happens, so [`STEP_FLOOR`] points that load at the 1/3000 s floor instead.
//!
//! # Other frame-time constants
//!
//! 1/30 s (`0x3d088889`) is read by 105 instructions in 80 functions and 1/60 s by 80 more in 65, mostly through shared
//! literal pools (`0x615b6b4`, `0x6159e10`); 0.1 s and 0.05 s by thousands. They are animation and gameplay
//! constants of many systems and cannot be told apart statically from the ones that follow the step, so none is
//! patched. [`Entry`] records them as data.

use crate::patch::{Finder, Patch, PatchError, Pattern, Site};
use iced_x86::{Mnemonic, OpKind, Register};

/// What backs a patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proof {
    /// Read from the code and consistent in every function that touches the value; never observed running.
    Static,
    /// A guess from the shape of the code, kept as data.
    Unproven,
}

/// When a patch is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// With the console frame rate nothing changes; with any other limit the patch is applied.
    WithCustomLimit,
    /// Only when asked for by name.
    Optional,
    /// Not a patch: a finding recorded for later.
    DataOnly,
}

/// One address with what is known about it.
#[derive(Debug, Clone, Copy)]
pub struct Evidence {
    pub rva: u32,
    pub what: &'static str,
}

/// A frame-rate change or finding.
pub struct Entry {
    pub id: &'static str,
    pub summary: &'static str,
    pub proof: Proof,
    pub activation: Activation,
    pub evidence: &'static [Evidence],
    find: Option<fn(&Finder) -> Result<Vec<Patch>, PatchError>>,
}

impl Entry {
    /// The patches for this image; empty for data-only entries.
    pub fn patches(&self, finder: &Finder) -> Result<Vec<Patch>, PatchError> {
        self.find.map_or(Ok(Vec::new()), |find| find(finder))
    }
}

pub const STEP_FLOOR: Entry = Entry {
    id: "sim-step-floor",
    summary: "Stop the simulation step from being held at 1 / the game's frame limit",
    proof: Proof::Static,
    activation: Activation::WithCustomLimit,
    evidence: &[
        Evidence {
            rva: 0x0222_64d0,
            what: "per-frame timer update: clamps counter delta to [floor, 1/15 s]",
        },
        Evidence {
            rva: 0x0222_6854,
            what: "vmovss xmm1,[0x6849844]: second floor, 1 / limit; the patched load",
        },
        Evidence {
            rva: 0x0222_619a,
            what: "sub_02226130 stores 1 / limit into 0x6849844",
        },
        Evidence {
            rva: 0x0222_6190,
            what: "sub_02226130 stores 1/3000 s into the first floor 0x6847660",
        },
        Evidence {
            rva: 0x0224_1fd0,
            what: "the limit: settings +0x210, else +0x214",
        },
    ],
    find: Some(find_step_floor),
};

pub const FRAME_EVENT_INTERVAL: Entry = Entry {
    id: "frame-event-interval",
    summary: "The interval the title registers for the frame events, 8333 to 66667 microseconds",
    proof: Proof::Static,
    activation: Activation::DataOnly,
    evidence: &[
        Evidence {
            rva: 0x0247_abd8,
            what: "SetFrameIntervalX(NULL, table[display][divisor], latency, 0) through slot 78",
        },
        Evidence {
            rva: 0x0247_abfc,
            what: "ScheduleFrameEventX(0, offset, NULL, 0) through slot 79",
        },
        Evidence {
            rva: 0x061e_ce60,
            what: "interval table, five dwords per row, row 1 for a 120 Hz display",
        },
        Evidence {
            rva: 0x0248_4637,
            what: "WaitFrameEventX(0, INFINITE, NULL, 0, &token) through slot 80",
        },
    ],
    find: None,
};

pub const CEILING: Entry = Entry {
    id: "sim-step-ceiling",
    summary: "The step ceiling of 1/15 s is a literal read in 49 functions",
    proof: Proof::Unproven,
    activation: Activation::DataOnly,
    evidence: &[Evidence {
        rva: 0x0615_9ec0,
        what: "0x3d888889, read by sub_022264d0 and 48 other functions; not patched",
    }],
    find: None,
};

pub const GAME_LIMITER: Entry = Entry {
    id: "game-limiter",
    summary: "Counter-based limiter and mode 1/4 waits that use the frame record's interval",
    proof: Proof::Unproven,
    activation: Activation::DataOnly,
    evidence: &[
        Evidence {
            rva: 0x0012_0530,
            what: "limiter gated by a byte at 0x7453ec9 that nothing sets",
        },
        Evidence {
            rva: 0x0248_2bc0,
            what: "wait until a counter value: sleep when far, then spin",
        },
        Evidence {
            rva: 0x009e_f0f0,
            what: "per-frame sync by mode byte at record +0x20 (1, 2, 4)",
        },
    ],
    find: None,
};

pub const FIXED_STEP_CONSTANTS: Entry = Entry {
    id: "fixed-step-constants",
    summary: "1/30 s and 1/60 s literals in about 145 functions",
    proof: Proof::Unproven,
    activation: Activation::DataOnly,
    evidence: &[
        Evidence {
            rva: 0x0615_b6b4,
            what: "1/30 s literal pool",
        },
        Evidence {
            rva: 0x0615_9e10,
            what: "1/60 s literal pool",
        },
    ],
    find: None,
};

/// Every entry, patches first.
pub const ENTRIES: [&Entry; 5] = [
    &STEP_FLOOR,
    &FRAME_EVENT_INTERVAL,
    &CEILING,
    &GAME_LIMITER,
    &FIXED_STEP_CONSTANTS,
];

/// The patches to apply: those that go with a custom frame limit when `custom_limit` is set, plus any
/// `optional` ones named.
pub fn select(
    finder: &Finder,
    custom_limit: bool,
    optional: &[&str],
) -> Result<Vec<Patch>, PatchError> {
    let mut patches = Vec::new();
    for entry in ENTRIES {
        let wanted = match entry.activation {
            Activation::WithCustomLimit => custom_limit,
            Activation::Optional => optional.contains(&entry.id),
            Activation::DataOnly => false,
        };
        if wanted {
            patches.extend(entry.patches(finder)?);
        }
    }
    Ok(patches)
}

/// `vmovss xmm1,[floor_a]; vcmpltss; vblendvps; vmovss xmm1,[floor_b]; vminss; vcmpltss; vblendvps`: the two
/// clamps of the step, one after the other.
pub fn step_floor_pattern() -> Pattern {
    let load = |i: &iced_x86::Instruction, _: &crate::patch::Probe| {
        i.mnemonic() == Mnemonic::Vmovss
            && i.op0_kind() == OpKind::Register
            && i.op0_register() == Register::XMM1
            && i.is_ip_rel_memory_operand()
    };
    let compare = |i: &iced_x86::Instruction, _: &crate::patch::Probe| {
        i.mnemonic() == Mnemonic::Vcmpss && i.immediate8() == 1
    };
    let blend =
        |i: &iced_x86::Instruction, _: &crate::patch::Probe| i.mnemonic() == Mnemonic::Vblendvps;
    let minimum =
        |i: &iced_x86::Instruction, _: &crate::patch::Probe| i.mnemonic() == Mnemonic::Vminss;
    Pattern::new("step floor")
        .then(load)
        .then(compare)
        .then(blend)
        .then(load)
        .then(minimum)
        .then(compare)
        .then(blend)
}

fn find_step_floor(finder: &Finder) -> Result<Vec<Patch>, PatchError> {
    let site = finder.find_one(&step_floor_pattern())?;
    floor_patch(finder, &site).map(|patch| vec![patch])
}

/// Makes the first load read the address of the second: both clamps then use the same floor.
pub fn floor_patch(finder: &Finder, site: &Site) -> Result<Patch, PatchError> {
    let first = site.instructions[0];
    let second = site.instructions[3];
    let base = finder.image().image_base;
    let target = second
        .instruction
        .ip_rel_memory_address()
        .wrapping_sub(base);
    let next = u64::from(first.rva) + u64::from(first.len);
    let displacement = target.wrapping_sub(next) as i64;
    let displacement =
        i32::try_from(displacement).map_err(|_| PatchError::Outside { rva: first.rva })?;
    finder.set_displacement("sim-step-floor", site, 0, displacement)
}
