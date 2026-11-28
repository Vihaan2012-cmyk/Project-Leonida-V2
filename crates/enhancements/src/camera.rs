//! Looking with the mouse: what the executable (build 1.0.2.1) does between a stick and the camera, found by reading
//! its code and never by running it. Addresses are RVAs; add `0x1_4000_0000` for the virtual address. Function names
//! are the disassembler's `sub_<rva>`.
//!
//! There is no code patch here. The place where the on-foot follow camera, the aim cameras, the vehicle chase
//! cameras and the first person camera integrate the look input into their yaw and pitch was not found, and a patch
//! that adds mouse counts to the camera angles needs it. What was found is the input side, the one shared function
//! that turns the look controls into a rate for the cinematic and mounted cameras, and the places that were ruled
//! out. The input shim (`shims::gameinput::curve`) keeps making the mouse look like a stick that the game turns by a
//! fixed angle per count. [`ENTRIES`] records each finding with its address and how sure it is.
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
//! # From the pad object to the control the camera reads
//!
//! `sub_051a4590` (pad vtable slot 9) answers for source ids `0x3010` (left X), `0x3011` (left Y), `0x3012` (right
//! X), `0x3013` (right Y), `0x3009` and `0x300c` (triggers) with the stored floats. `sub_00d53b60` reads them into the
//! mapper's value table every frame; `sub_00323930` publishes changes as events.
//!
//! Controls are named by the joaat hash of the lower-cased `INPUT_*` name. The table of 245 hashes is at `0x6191240`
//! and a control's index is its position in it (`INPUT_LOOK_LR` `0xa987235f` is 6, `INPUT_LOOK_UD` `0xd2047988` is 7,
//! `INPUT_AIM` 52, `INPUT_LOOK_BEHIND` 55, `INPUT_MOVE_LR` 65, `INPUT_MOVE_UD` 66; every name was checked by
//! hashing it). The player's control object (`sub_00512ae0` returns the main player's; `sub_050718b0(ped, 1)` a
//! ped's) keeps a handle for index `i` at [`control_handle_offset`]`(i)`: a `u16` slot and a `u8` kind at `+2`, next
//! to the value manager at `+0x308` and a byte at `+0x2a45`. So the look pair is at `+0x2a68`/`+0x2a6c`, move at
//! `+0x2b54`/`+0x2b58`, and the alternate look pair the camera helper reads is X at `+0x2c94` (index 145) and Y at
//! `+0x2c88` (index 142), whose names are not known. The read is `sub_0501bda0` (handle to value slot), `sub_0501bc50`
//! (the value object at `+0xd0`, or a static zero object) and `sub_0501bc80`, which calls `sub_04db8640(value, 0,
//! option)` (option global `0xa9b3f50`) and clamps to -1..1; `sub_053d54a0` is the same read without the clamp.
//! `sub_036d5ce0`, called from the gameplay director update `sub_036d5530`, reads both look controls by hash through
//! the registry `sub_00b1b050` and only records when the player last moved the camera, for the auto-centre logic.
//!
//! # The look helper
//!
//! `sub_04a20480(spring, alternate, stiffness, damping, speed, out_x, out_y, raw_x, raw_y)` is the one shared reader
//! of the look controls that was found. It reads the main player's control, takes the look pair (`alternate` 0) or
//! the alternate pair (1), stores the clamped values in `raw_x` and `-raw_y`, and drives a spring envelope
//! `sub_00b6a2b0` towards 1.0 while either value is at least 1e-6 in magnitude and 0.0 otherwise. The spring is a
//! damped oscillator whose step is the camera clock's step in seconds (`0xa958364`, the second clock record at
//! `0xa958358`), with the callers' stiffness 5.0 (`0x615a2c4`) and damping 0.9 (`0x5fc6f98`), so the look speed
//! ramps up over about a second after the stick leaves the centre. The outputs are
//! `out_x = speed * envelope * raw_x` and `out_y = speed * envelope * raw_y`, the products at `0x4a20787..0x4a207b2`.
//! There is no frame time in them: the unit is whatever `speed` is per update.
//!
//! When bit 0 of the platform flags at `0xa02cf1c` is set (`sub_00132350`; the flags are filled by `sub_0073c770`) the
//! helper reads the controls again without clamping and then calls `sub_0381fd20`, which sets both raw values to
//! zero. That is the console build's stub where a build with a mouse reads it; it confirms the code once had a mouse
//! path and shows that this build has none.
//!
//! Three wrappers use it, with these consumers (each a camera class `update`, vtable slot 3 of the class):
//!
//! - `sub_04a207e0` (`speed` from the camera's metadata at `+0x15c`) feeds `sub_01cb49b0`, the orientation update of
//!   a camera class with attach points and limits (update `sub_059cd0c0`, vtable `0x63d9080`). It adds `out_x` to the
//!   heading at `+0x1b0` of the object at `+0x1b8`, wraps by 2 pi, clamps to the limits in degrees at `+0x178`/`+0x17c`
//!   (times pi/180, `0x6159b38`), and adds `out_y` to the pitch at `+0x1b4` with limits at `+0x180`/`+0x184`. This is
//!   the one place that integrates a look rate into a yaw and pitch. Which camera it is (a mounted camera or a first
//!   person one) is not known.
//! - `sub_04a20400` returns `out` as a position offset along the camera's right and up axes. Callers: the same
//!   `sub_01cb49b0` (second branch, accumulates at `+0xb0` with limits `+0x164..+0x170`), `sub_04176700` (vtable
//!   `0x65aed30`) and `sub_04182720` (vtable `0x65af0f0`). The look moves the camera, it does not turn it.
//! - `sub_04a20840` multiplies the envelope-scaled stick by metadata angles, wraps to -pi..pi and smooths the result
//!   with a second spring: a heading and pitch offset that follows the stick deflection, not an accumulated angle.
//!   Callers: `sub_04a20980`, which rotates a basis by those two angles (callers `sub_041632b0`, `sub_04171d80` at
//!   vtable `0x65ae990`, `sub_04174c70` at `0x65aeac0`, all with `alternate` 1), and the state machine `sub_04260440`
//!   (metadata picks `alternate`).
//!
//! # What was ruled out
//!
//! Other readers of the look pair do not turn a camera: `sub_04228f80` (target selection by stick direction),
//! `sub_04263870` (whether the look stick is active, for the class of `sub_04262480`), `sub_036d5ce0` (idle timer),
//! `sub_0322cb40` (a free camera with its own control handles at `+0x3580..+0x358e`), `sub_04194510`, `sub_04233570`
//! and `sub_042135c0` (move axes). `sub_04232f00` reads three controls whose indices come from metadata (`+0x30`,
//! `+0x34`, `+0x38` of the class metadata, unclamped and negated) for the modifier `sub_04232ca0`; the follow
//! cameras may take their look from controls chosen that way, which a static read cannot resolve. The class names
//! of the on-foot cameras are not in the image: only `camBaseCamera` `0xfe12ce88`, `camGameplayDirector`
//! `0x3e9ed27f`, `camControlHelper` `0xd026e9a8` and `camCinematicMountedCamera` `0x2f96ebcd` (users `sub_04166a40`,
//! `sub_0419f420`, `sub_00a784b0`) occur as hash literals, and the other class names of the camera family were
//! tried by hash and are absent. Camera vtables start `0x983f40`, `0x73c940`, `0x4380` and the update is slot 3; the
//! cinematic group is `0x65ae860..0x65af980` and the base group `0x63d9080..0x63da460`.
//!
//! # A patch at the helper, if the cameras that use it are the ones wanted
//!
//! The rewrite point is the tail of `sub_04a20480`, `0x4a20787..0x4a207c5` (62 bytes, between the call of the spring
//! and the `setbe`). Adding the mouse after the products needs two extra loads and adds (about 16 bytes); the bytes
//! can come from `vmovss xmm1,[0x5fc6f00]; vucomiss xmm1,xmm8` (12 bytes) becoming `vucomiss xmm8,[0x5fc6f00]` with
//! `setae` (8 bytes), and from using `vfmadd231ss xmm,xmm0,[rdi]` on a register that holds the mouse term. It only
//! reaches the cameras listed above, and for `sub_04a20840` and `sub_04a20400` it would add to an offset, not to an
//! angle, so it was not written.

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

pub const ENTRIES: [Entry; 8] = [
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
        id: "control-handles",
        summary: "INPUT_* hash table; the control object keeps a handle for index i at +0x2a50 + 4 * i",
        proof: Proof::Static,
        evidence: &[
            Evidence {
                rva: 0x0619_1240,
                what: "245 joaat hashes of INPUT_* names; LOOK_LR 6, LOOK_UD 7, AIM 52, MOVE_LR 65",
            },
            Evidence {
                rva: 0x0051_2ae0,
                what: "returns the main player's control object",
            },
            Evidence {
                rva: 0x0501_bda0,
                what: "handle (u16 slot, u8 kind) to the value slot, manager at +0x308",
            },
            Evidence {
                rva: 0x0501_bc80,
                what: "control value clamped to -1..1; sub_053d54a0 is the unclamped read",
            },
            Evidence {
                rva: 0x036d_5ce0,
                what: "reads LOOK_LR and LOOK_UD by hash for the camera idle timer only",
            },
        ],
    },
    Entry {
        id: "look-helper",
        summary: "Shared look reader: speed * spring envelope * stick, no frame time; envelope ramps over about a second",
        proof: Proof::Static,
        evidence: &[
            Evidence {
                rva: 0x04a2_0480,
                what: "reads the look pair or the alternate pair, envelope by sub_00b6a2b0, two products at 0x4a20787",
            },
            Evidence {
                rva: 0x00b6_a2b0,
                what: "damped spring, step from the camera clock record at 0xa958358 (+0xc)",
            },
            Evidence {
                rva: 0x0615_a2c4,
                what: "stiffness 5.0 the callers pass",
            },
            Evidence {
                rva: 0x05fc_6f98,
                what: "damping 0.9 the callers pass",
            },
            Evidence {
                rva: 0x05fc_6f00,
                what: "1e-6, the smallest stick value that counts as input",
            },
        ],
    },
    Entry {
        id: "look-consumers",
        summary: "Wrappers of the helper and the cameras that call them: a heading and pitch rate, and two kinds of offset",
        proof: Proof::Static,
        evidence: &[
            Evidence {
                rva: 0x04a2_07e0,
                what: "heading and pitch rate wrapper, speed from metadata +0x15c",
            },
            Evidence {
                rva: 0x01cb_49b0,
                what: "adds the rate to heading +0x1b0 and pitch +0x1b4 of the object at +0x1b8, wraps, clamps to degree limits",
            },
            Evidence {
                rva: 0x04a2_0400,
                what: "position offset wrapper; callers 0x1cb49b0, 0x4176700, 0x4182720",
            },
            Evidence {
                rva: 0x04a2_0840,
                what: "stick deflection to a spring smoothed heading and pitch offset",
            },
            Evidence {
                rva: 0x04a2_0980,
                what: "rotates a basis by those two angles; callers 0x41632b0, 0x4171d80, 0x4174c70",
            },
            Evidence {
                rva: 0x0426_0440,
                what: "state machine around sub_04a20840 for a camera modifier",
            },
        ],
    },
    Entry {
        id: "mouse-stub",
        summary: "A platform flag turns the helper's raw look values into zero through an empty stub: no mouse path in this build",
        proof: Proof::Inferred,
        evidence: &[
            Evidence {
                rva: 0x0013_2350,
                what: "returns bit 0 of the flags at 0xa02cf1c",
            },
            Evidence {
                rva: 0x0381_fd20,
                what: "sets both raw values to zero and returns",
            },
            Evidence {
                rva: 0x0073_c770,
                what: "fills the platform flags",
            },
        ],
    },
    Entry {
        id: "camera-classes",
        summary: "Camera vtables: update in slot 3; the class hash literals that exist",
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
            Evidence {
                rva: 0x065a_e860,
                what: "first of the cinematic camera vtables, each 0x130 bytes",
            },
            Evidence {
                rva: 0x063d_9080,
                what: "vtable whose update is sub_059cd0c0, the class that integrates the look rate",
            },
        ],
    },
    Entry {
        id: "look-integration",
        summary: "Where the on-foot follow, aim, vehicle chase and first person cameras turn the look input into yaw and pitch",
        proof: Proof::Missing,
        evidence: &[],
    },
];

/// The inner dead zone the game applies to a stick by default.
pub const DEFAULT_INNER_DEAD_ZONE: f32 = 0.25;
/// The outer dead zone the game applies to a stick by default.
pub const DEFAULT_OUTER_DEAD_ZONE: f32 = 0.0;

/// The table of control name hashes, one `u32` per control.
pub const CONTROL_HASH_TABLE: u32 = 0x0619_1240;
/// Position of `INPUT_LOOK_LR` in the table.
pub const LOOK_LR: u32 = 6;
/// Position of `INPUT_LOOK_UD` in the table.
pub const LOOK_UD: u32 = 7;
/// Joaat of `input_look_lr`.
pub const LOOK_LR_HASH: u32 = 0xa987_235f;
/// Joaat of `input_look_ud`.
pub const LOOK_UD_HASH: u32 = 0xd204_7988;

/// Where the control object keeps the handle of the control at `index` in the hash table.
pub const fn control_handle_offset(index: u32) -> u32 {
    0x2a50 + 4 * index
}

/// What the look helper (`sub_04a20480`) drives its envelope towards for a raw stick.
pub fn envelope_target(raw_x: f32, raw_y: f32) -> f32 {
    const EPSILON: f32 = 1e-6;
    if raw_x.abs() >= EPSILON || raw_y.abs() >= EPSILON {
        1.0
    } else {
        0.0
    }
}

/// What the look helper hands its caller: the speed and the envelope scale the raw value, with no frame time.
pub fn helper_look_output(speed: f32, envelope: f32, raw: f32) -> f32 {
    speed * envelope * raw
}

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

    #[test]
    fn control_handles_sit_four_bytes_apart_from_0x2a50() {
        assert_eq!(control_handle_offset(LOOK_LR), 0x2a68);
        assert_eq!(control_handle_offset(LOOK_UD), 0x2a6c);
        assert_eq!(control_handle_offset(65), 0x2b54);
        assert_eq!(control_handle_offset(145), 0x2c94);
        assert_eq!(control_handle_offset(142), 0x2c88);
    }

    #[test]
    fn the_hashes_are_the_joaat_of_the_lower_cased_names() {
        fn joaat(name: &str) -> u32 {
            let mut hash = 0u32;
            for byte in name.to_ascii_lowercase().bytes() {
                hash = hash.wrapping_add(u32::from(byte));
                hash = hash.wrapping_add(hash << 10);
                hash ^= hash >> 6;
            }
            hash = hash.wrapping_add(hash << 3);
            hash ^= hash >> 11;
            hash.wrapping_add(hash << 15)
        }
        assert_eq!(joaat("INPUT_LOOK_LR"), LOOK_LR_HASH);
        assert_eq!(joaat("INPUT_LOOK_UD"), LOOK_UD_HASH);
        assert_eq!(joaat("camBaseCamera"), 0xfe12_ce88);
    }

    #[test]
    fn the_envelope_follows_whether_any_stick_value_is_non_zero() {
        assert_eq!(envelope_target(0.0, 0.0), 0.0);
        assert_eq!(envelope_target(5e-7, -5e-7), 0.0);
        assert_eq!(envelope_target(0.0, -0.01), 1.0);
        assert_eq!(envelope_target(-1.0, 0.0), 1.0);
    }

    #[test]
    fn the_helper_scales_by_speed_and_envelope_and_not_by_time() {
        assert_eq!(helper_look_output(0.04, 1.0, 0.5), 0.02);
        assert_eq!(helper_look_output(0.04, 0.5, 0.5), 0.01);
        assert_eq!(helper_look_output(0.04, 1.0, -1.0), -0.04);
    }
}
