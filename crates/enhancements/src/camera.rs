//! Looking with the mouse: what the executable (build 1.0.2.1) does between a stick and the camera, found by reading
//! its code and never by running it. Addresses are RVAs; add `0x1_4000_0000` for the virtual address. Function names
//! are the disassembler's `sub_<rva>`.
//!
//! There is no code patch here. The place where the camera integrates the look input into its yaw and pitch was not
//! found, and a patch that adds mouse counts to the camera angles needs it. What was found is the input side, which
//! the input shim (`shims::gameinput::curve`) uses to make the mouse look like a stick that the game turns by a fixed
//! angle per count. [`ENTRIES`] records each finding with its address and how sure it is.
//!
//! # From GameInput to the pad object
//!
//! `GameInputCreate` (import `GameInput.dll` ordinal 1, IAT `0x6793f10`, called through the thunk at `0x5f3fed0`) is
//! wrapped by `sub_059258a0`, which keeps the interface in `0x9ab15e8`. The title reads three things from it: the
//! keyboard (kind `0x10`, `sub_043a1c30`), the device callbacks (`sub_059257e0`, `sub_043a0f00`) and gamepads (kind
//! `0x40000`). No read of the mouse (kind `0x20`) was found, so the Xbox build has no mouse input to build on.
//!
//! The pad object is created by `sub_05b15fe0` (vtable `0x65f1cc0`, four of them from `sub_0439e080`, base class
//! constructor `sub_01d4c540`). Its update, vtable slot 2 `sub_007a4150`, reads every connected device with
//! `GetCurrentReading` (`sub_02211b40`) and `GetGamepadState` (reading vtable `+0xb0`), keeps the largest
//! absolute value of each axis across devices, and stores the left stick at `+0x38`/`+0x3c`, the right stick at
//! `+0x40`/`+0x44` and the triggers at `+0x48`/`+0x4c`, through `sub_001fec90` (left) and `sub_020cd2b0` (right).
//!
//! # The dead zone
//!
//! Both go through `sub_001fed50` and `sub_001fede0`, a radial dead zone that rescales: with `r` the length of the
//! raw stick, the result has length `clamp((r - inner) / (1 - inner - outer), 0, 1)` and the same direction, and is
//! zero when `r` is below `inner`. `inner` and `outer` are, for each stick, the override fields `+0x470`, `+0x474`
//! (inner, left and right) and `+0x478`, `+0x47c` (outer) of the pad object when they are not negative, and
//! otherwise the configured fields `+0x120`/`+0x1b4` and `+0x1d0`/`+0x1e4` (`sub_001fed20` picks). The constructor
//! `sub_05b15fe0` copies the 32 bytes at `0x617c040` into `+0x470`, which gives 0.25 inner and 0.0 outer for both
//! sticks. So a raw stick below 0.25 does nothing and the mouse has to start at 0.25 to move the camera at all.
//!
//! # Where the look value goes
//!
//! `sub_051a4590` (pad vtable slot 9) answers for source ids `0x3010` (left X), `0x3011` (left Y), `0x3012` (right
//! X), `0x3013` (right Y), `0x3009` and `0x300c` (triggers) with the stored floats. `sub_00d53b60` reads them into the
//! mapper's value table every frame; `sub_00323930` publishes changes as events. Controls are named by the joaat hash
//! of lower-cased strings: `INPUT_LOOK_LR` is `0xa987235f` and `INPUT_LOOK_UD` is `0xd2047988` (table at
//! `0x6191240`, indexes 6 and 7). The only code that names them is `sub_036d5ce0`, called from the gameplay director
//! update `sub_036d5530`: it reads both through `sub_0501bc80` (the control value) and records when the player last
//! moved the camera, for the auto-centre logic. The camera classes are RAGE's `cam*` ones (`camBaseCamera`
//! `0xfe12ce88`, `camGameplayDirector` `0x3e9ed27f`, `camControlHelper` `0xd026e9a8`); no stub or table for the third
//! person camera was reached. How a look value becomes an angle (speed, curve, acceleration over time) is metadata
//! driven and was not found.

/// How sure a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proof {
    /// Read from the code, every use looked at.
    Static,
    /// The name or role is inferred from hashes and shapes.
    Inferred,
    /// Not found.
    Missing,
}

#[derive(Debug, Clone, Copy)]
pub struct Evidence {
    pub rva: u32,
    pub what: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub id: &'static str,
    pub summary: &'static str,
    pub proof: Proof,
    pub evidence: &'static [Evidence],
}

pub const ENTRIES: [Entry; 5] = [
    Entry {
        id: "pad-input",
        summary: "GameInput gamepad state to the pad object's axes; no mouse reading exists",
        proof: Proof::Static,
        evidence: &[
            Evidence {
                rva: 0x0592_58a0,
                what: "GameInput interface accessor, global 0x9ab15e8",
            },
            Evidence {
                rva: 0x007a_4150,
                what: "pad update: GetGamepadState, axes into +0x38..+0x4c",
            },
            Evidence {
                rva: 0x05b1_5fe0,
                what: "pad constructor, vtable 0x65f1cc0",
            },
            Evidence {
                rva: 0x043a_1c30,
                what: "the only other kind read: the keyboard (0x10)",
            },
        ],
    },
    Entry {
        id: "stick-dead-zone",
        summary: "Radial dead zone with rescale, inner 0.25 and outer 0.0 by default",
        proof: Proof::Static,
        evidence: &[
            Evidence {
                rva: 0x001f_ede0,
                what: "clamp((r - inner) / (1 - inner - outer), 0, 1) along the stick",
            },
            Evidence {
                rva: 0x001f_ec90,
                what: "left stick wrapper, writes +0x38/+0x3c",
            },
            Evidence {
                rva: 0x020c_d2b0,
                what: "right stick wrapper, writes +0x40/+0x44",
            },
            Evidence {
                rva: 0x0617_c040,
                what: "defaults copied to pad +0x470: 0.25, 0.25, 0, 0",
            },
        ],
    },
    Entry {
        id: "look-controls",
        summary: "INPUT_LOOK_LR and INPUT_LOOK_UD by hash, read for the camera idle timer",
        proof: Proof::Inferred,
        evidence: &[
            Evidence {
                rva: 0x036d_5ce0,
                what: "reads hashes 0xa987235f and 0xd2047988, normalises the pair",
            },
            Evidence {
                rva: 0x036d_5530,
                what: "gameplay director update that calls it",
            },
            Evidence {
                rva: 0x0619_1240,
                what: "control name hash table, LOOK_LR at index 6",
            },
            Evidence {
                rva: 0x0501_bc80,
                what: "control value read",
            },
        ],
    },
    Entry {
        id: "camera-classes",
        summary: "RAGE cam* class hashes in IsA stubs; control helper vtable",
        proof: Proof::Inferred,
        evidence: &[
            Evidence {
                rva: 0x04a2_0e20,
                what: "camControlHelper type hash stub, vtable 0x66c8390",
            },
            Evidence {
                rva: 0x04a2_0c90,
                what: "control helper constructor",
            },
        ],
    },
    Entry {
        id: "look-integration",
        summary: "Where look input becomes yaw and pitch: speed, curve, acceleration over time",
        proof: Proof::Missing,
        evidence: &[],
    },
];

/// The inner dead zone the game applies to a stick by default.
pub const DEFAULT_INNER_DEAD_ZONE: f32 = 0.25;
/// The outer dead zone the game applies to a stick by default.
pub const DEFAULT_OUTER_DEAD_ZONE: f32 = 0.0;

/// What the game turns a raw stick length into, as `sub_001fede0` computes it.
pub fn game_stick_length(raw: f32, inner: f32, outer: f32) -> f32 {
    if raw < inner {
        return 0.0;
    }
    ((raw - inner) / (1.0 - inner - outer)).clamp(0.0, 1.0)
}

/// Whether a code patch for the mouse camera exists for this build. It does not.
pub fn patch_available() -> bool {
    ENTRIES
        .iter()
        .all(|e| e.id != "look-integration" || e.proof != Proof::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dead_zone_rescales_from_the_inner_edge_to_full() {
        let (i, o) = (DEFAULT_INNER_DEAD_ZONE, DEFAULT_OUTER_DEAD_ZONE);
        assert_eq!(game_stick_length(0.2, i, o), 0.0);
        assert_eq!(game_stick_length(0.25, i, o), 0.0);
        assert!((game_stick_length(0.625, i, o) - 0.5).abs() < 1e-6);
        assert_eq!(game_stick_length(1.0, i, o), 1.0);
        assert!((game_stick_length(0.8, 0.2, 0.2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn every_entry_with_a_proof_names_its_addresses_and_the_gap_is_stated() {
        for e in &ENTRIES {
            assert_eq!(e.evidence.is_empty(), e.proof == Proof::Missing, "{}", e.id);
            let mut seen = std::collections::HashSet::new();
            assert!(e.evidence.iter().all(|x| seen.insert(x.rva)), "{}", e.id);
        }
        assert!(!patch_available());
    }
}
