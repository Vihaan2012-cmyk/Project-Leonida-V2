//! Turning the camera with the mouse: a code patch at the place where the look controller of the player adds the
//! per frame look deltas to the view direction. Found by reading the executable (build 1.0.2.1), never by running it.
//! Addresses are RVAs.
//!
//! # The look controller
//!
//! `sub_0420a0e0` (vtable slot at `0x65b8c18`, object size `0x1cc4`) is the per frame update of a look controller.
//! It evaluates two pads of input nodes (`sub_00e4e3d0` at `+0xa70`, `sub_007c5c40` at `+0x1140`), the four channels
//! at `+0x40`, `+0x1e0`, `+0x380` and `+0x518` (each: two summed sources and a scale) and stores two floats,
//! the heading delta at `+0x1ca0` and the pitch delta at `+0x1ca4`, each `(source A + source B) * scale` of the
//! channel at `+0x380` and `+0x518` (`0x420a1c0..0x420a207`). Nothing else in the image writes those two fields of this
//! class. Then `sub_008f36c0` (called at `0x420a32c`) adds them to a heading and pitch that `sub_00677760` (called
//! first in the update) rebuilt from the live direction (`atan2(-x, y)` and `asin(z)` of the vector at `+0x1c00`): the
//! heading is wrapped to -pi..pi (`sub_008f3830`), the pitch is reflected into -pi/2..pi/2 (`sub_052a1570`), limits
//! are applied (`sub_008f3a90`, `sub_008f3a30`) and the result goes to `+0x10`, `+0x1c30` and `+0x1c40`. The camera
//! side reads the same component by its type hash `0x6bb7d1b6` (`sub_05162240`): `sub_04242dd0` (a camera modifier,
//! using the camera clock record `0xa958358`) forms `wrap(+0x1ca0 + +0x1c30)` and `reflect(+0x1ca4 + +0x1c34)`.
//! So these two floats are radians added to the current view direction in one update, in the engine's convention:
//! heading grows counter clockwise (turning left), pitch grows upwards.
//!
//! The stick reaches the channels through the data driven node graph, so no code names the look controls here; that
//! is why the earlier search by control index could not find it. What is inferred and not proven: that the channel
//! at `+0x380` and `+0x518` carries the right stick of the on foot follow camera (the two other channels, `+0x40` and
//! `+0x1e0`, are shared with sibling classes through `sub_007fdbf0` and go through a different smoothing path), and
//! the sign convention above. Both were read from code and never observed.
//!
//! # The patch
//!
//! The 40 bytes `0x420a1e7..0x420a20f` (the heading store, the pitch loads and multiply, the pitch store; five whole
//! instructions) become a jump to a routine in the zeroed tail of the last `.text` page. The routine takes the
//! counts the input shim has collected (an exchange with zero, so a count is used once), multiplies them by the
//! per axis scale of the cell, adds them to the stick's delta and stores both fields as the original did. A
//! stick that does not move gives exactly the old behaviour; a mouse that does not move adds exactly 0.
//!
//! The cell is 16 bytes at the bottom of the zeroed tail of the last `.data` page: `dx: i32`, `dy: i32`,
//! `scale_x: f32`, `scale_y: f32` (see [`shims::gameinput::direct`], which fills it from the window's raw motion and
//! stops the shim from also pushing the stick). The frame timer patches of [`crate::frame_timers`] use the top of the
//! same pages; [`patches`] refuses to overlap them.

use crate::patch::{Finder, Patch, PatchError, Pattern};
use crate::widescreen::fnv1a64;
use iced_x86::{Instruction, Mnemonic, OpKind, Register};

/// Mouse movement as the code patch wants it, from the settings of `input.toml`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseLookSettings {
    /// `mouse_sensitivity * mouse_sensitivity_x` and `mouse_sensitivity * mouse_sensitivity_y`.
    pub sensitivity: [f32; 2],
    /// `mouse_invert_x` and `mouse_invert_y`.
    pub invert: [bool; 2],
}

impl Default for MouseLookSettings {
    fn default() -> Self {
        Self {
            sensitivity: [1.0, 1.0],
            invert: [false, false],
        }
    }
}

/// Degrees one count turns the camera at sensitivity 1.0, the value the input shim's stick emulation uses.
pub const DEGREES_PER_COUNT: f32 = 0.0225;

impl MouseLookSettings {
    /// Radians added to the heading and to the pitch per count. The engine's heading grows turning left and its
    /// pitch grows looking up, a mouse moving right (+x) turns right and moving down (+y) looks down, so both
    /// are negative unless inverted.
    pub fn scales(&self) -> [f32; 2] {
        let radians = DEGREES_PER_COUNT.to_radians();
        let axis = |i: usize| {
            let sign = if self.invert[i] { 1.0 } else { -1.0 };
            sign * self.sensitivity[i] * radians
        };
        [axis(0), axis(1)]
    }
}

/// Where the patch goes in one build.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// The look controller update.
    pub update: u32,
    /// FNV-1a 64 of the 40 bytes replaced.
    pub hash: u64,
}

/// Build 1.0.2.1.
pub const GAME: Layout = Layout {
    update: 0x0420_a0e0,
    hash: 0x0fa1_dc9b_0031_73b6,
};

/// Bytes the hook replaces: the heading store, the pitch loads and multiply, the pitch store.
pub const HOOK_LEN: usize = 40;
/// Bytes of the last `.text` page the routine may take, from the bottom of the zeroed tail.
pub const CAVE_SLOT: u32 = 0x100;
/// Bytes of the cell.
pub const CELL_LEN: u32 = 16;

/// Offsets of the object the look controller update works on.
const HEADING_DELTA: i64 = 0x1ca0;
const PITCH_DELTA: i64 = 0x1ca4;
const PITCH_SOURCES: [i64; 3] = [0x528, 0x520, 0x524];

fn on_object(i: &Instruction, displacement: i64) -> bool {
    i.op0_kind() == OpKind::Memory
        && i.memory_base() == Register::RSI
        && i.memory_index() == Register::None
        && i.memory_displacement64() as i64 == displacement
}

fn loads_from_object(i: &Instruction, displacement: i64) -> bool {
    i.op0_register() == Register::XMM0
        && i.memory_base() == Register::RSI
        && i.memory_index() == Register::None
        && i.memory_displacement64() as i64 == displacement
}

/// The heading store followed by the pitch computation and store.
pub fn hook_pattern() -> Pattern {
    Pattern::new("look controller deltas")
        .then(|i, _| {
            i.mnemonic() == Mnemonic::Vmovss
                && on_object(i, HEADING_DELTA)
                && i.op1_register() == Register::XMM0
        })
        .then(|i, _| i.mnemonic() == Mnemonic::Vmovss && loads_from_object(i, PITCH_SOURCES[0]))
        .then(|i, _| {
            i.mnemonic() == Mnemonic::Vaddss
                && i.op1_register() == Register::XMM0
                && loads_from_object(i, PITCH_SOURCES[1])
        })
        .then(|i, _| {
            i.mnemonic() == Mnemonic::Vmulss
                && i.op1_register() == Register::XMM0
                && loads_from_object(i, PITCH_SOURCES[2])
        })
        .then(|i, _| {
            i.mnemonic() == Mnemonic::Vmovss
                && on_object(i, PITCH_DELTA)
                && i.op1_register() == Register::XMM0
        })
}

fn rel32(next: u32, target: u32) -> Result<[u8; 4], PatchError> {
    i32::try_from(i64::from(target) - i64::from(next))
        .map(i32::to_le_bytes)
        .map_err(|_| PatchError::Outside { rva: target })
}

fn not_found(what: &str) -> PatchError {
    PatchError::NotFound { name: what.into() }
}

/// The zeroed tail of the page after the last executable (or the largest writable) section: start, end.
fn slack(finder: &Finder, code: bool) -> Result<(u32, u32), PatchError> {
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
    if crowded {
        return Err(PatchError::TooLong { old: 0, new: 1 });
    }
    Ok(((extent + 15) & !15, end))
}

/// A write into slack: checks that the memory is zero first.
fn slack_patch(finder: &Finder, name: &str, rva: u32, bytes: Vec<u8>) -> Result<Patch, PatchError> {
    let original = vec![0u8; bytes.len()];
    match finder.image().read(rva, bytes.len()) {
        Some(found) if found != original.as_slice() => Err(PatchError::Mismatch {
            rva,
            expected: original,
            found: found.to_vec(),
        }),
        _ => Ok(Patch {
            name: name.into(),
            rva,
            original,
            replacement: bytes,
        }),
    }
}

/// What the patch needs from the caller after it is applied.
#[derive(Debug, Clone, PartialEq)]
pub struct MouseLookPatches {
    pub patches: Vec<Patch>,
    /// RVA of the cell the input shim fills.
    pub cell: u32,
    /// RVA of the routine.
    pub cave: u32,
}

struct Code {
    at: u32,
    bytes: Vec<u8>,
}

impl Code {
    fn put(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// An instruction with a RIP-relative operand: `opcode`, then the displacement to `target`.
    fn rip(&mut self, opcode: &[u8], target: u32) -> Result<(), PatchError> {
        let next = self.at + (self.bytes.len() + opcode.len() + 4) as u32;
        self.put(opcode);
        self.put(&rel32(next, target)?);
        Ok(())
    }
}

/// The routine: both deltas as the original computed them, with the mouse's counts added.
fn routine(at: u32, cell: u32, back: u32) -> Result<Vec<u8>, PatchError> {
    let mut c = Code {
        at,
        bytes: Vec::new(),
    };
    // push rax
    c.put(&[0x50]);
    // heading: xmm0 holds the stick's delta
    c.put(&[0x31, 0xC0]); // xor eax, eax
    c.rip(&[0x87, 0x05], cell)?; // xchg [dx], eax
    c.put(&[0xC5, 0xF2, 0x2A, 0xC8]); // vcvtsi2ss xmm1, xmm1, eax
    c.rip(&[0xC5, 0xF2, 0x59, 0x0D], cell + 8)?; // vmulss xmm1, xmm1, [scale_x]
    c.put(&[0xC5, 0xFA, 0x58, 0xC1]); // vaddss xmm0, xmm0, xmm1
    c.put(&[0xC5, 0xFA, 0x11, 0x86]); // vmovss [rsi+1CA0h], xmm0
    c.put(&(HEADING_DELTA as u32).to_le_bytes());
    // pitch: as the original, then the counts
    c.put(&[0x31, 0xC0]);
    c.rip(&[0x87, 0x05], cell + 4)?; // xchg [dy], eax
    c.put(&[0xC5, 0xFA, 0x10, 0x86]); // vmovss xmm0, [rsi+528h]
    c.put(&(PITCH_SOURCES[0] as u32).to_le_bytes());
    c.put(&[0xC5, 0xFA, 0x58, 0x86]); // vaddss xmm0, xmm0, [rsi+520h]
    c.put(&(PITCH_SOURCES[1] as u32).to_le_bytes());
    c.put(&[0xC5, 0xFA, 0x59, 0x86]); // vmulss xmm0, xmm0, [rsi+524h]
    c.put(&(PITCH_SOURCES[2] as u32).to_le_bytes());
    c.put(&[0xC5, 0xF2, 0x2A, 0xC8]);
    c.rip(&[0xC5, 0xF2, 0x59, 0x0D], cell + 12)?; // vmulss xmm1, xmm1, [scale_y]
    c.put(&[0xC5, 0xFA, 0x58, 0xC1]);
    c.put(&[0xC5, 0xFA, 0x11, 0x86]); // vmovss [rsi+1CA4h], xmm0
    c.put(&(PITCH_DELTA as u32).to_le_bytes());
    // pop rax; jmp back
    c.put(&[0x58]);
    c.rip(&[0xE9], back)?;
    Ok(c.bytes)
}

/// The patches for this image: the routine, the cell with its first scales, and the jump. Both pieces sit at the
/// bottom of the zeroed page tails, below the share of [`crate::frame_timers`].
pub fn patches(
    finder: &Finder,
    layout: &Layout,
    settings: &MouseLookSettings,
) -> Result<MouseLookPatches, PatchError> {
    let function = finder
        .image()
        .function_containing(layout.update)
        .ok_or_else(|| not_found("look controller update"))?;
    let sites = finder.find_in(function, &hook_pattern());
    if sites.is_empty() {
        return Err(not_found("look controller deltas"));
    }
    let [site] = sites.as_slice() else {
        return Err(PatchError::Ambiguous {
            name: "look controller deltas".into(),
            count: sites.len(),
        });
    };
    let first = site.instructions[0];
    let last = site.instructions[4];
    let span = (last.rva + u32::from(last.len) - first.rva) as usize;
    if span != HOOK_LEN {
        return Err(PatchError::TooLong {
            old: HOOK_LEN,
            new: span,
        });
    }
    let bytes = finder
        .image()
        .read(first.rva, HOOK_LEN)
        .ok_or(PatchError::Outside { rva: first.rva })?;
    if fnv1a64(bytes) != layout.hash {
        return Err(PatchError::Mismatch {
            rva: first.rva,
            expected: layout.hash.to_le_bytes().to_vec(),
            found: fnv1a64(bytes).to_le_bytes().to_vec(),
        });
    }

    let (code_start, code_end) = slack(finder, true)?;
    let (data_start, data_end) = slack(finder, false)?;
    let theirs_code = code_end - crate::frame_timers::TICK_SLOT;
    let theirs_data = data_end - crate::frame_timers::CELLS;
    if code_start + CAVE_SLOT > theirs_code || data_start + CELL_LEN > theirs_data {
        return Err(PatchError::TooLong {
            old: (theirs_code - code_start) as usize,
            new: CAVE_SLOT as usize,
        });
    }
    let (cave, cell) = (code_start, data_start);
    let code = routine(cave, cell, first.rva + HOOK_LEN as u32)?;
    if code.len() > CAVE_SLOT as usize {
        return Err(PatchError::TooLong {
            old: CAVE_SLOT as usize,
            new: code.len(),
        });
    }
    let scales = settings.scales();
    let mut cell_bytes = vec![0u8; 8];
    cell_bytes.extend_from_slice(&scales[0].to_le_bytes());
    cell_bytes.extend_from_slice(&scales[1].to_le_bytes());
    let mut jump = vec![0xE9];
    jump.extend_from_slice(&rel32(first.rva + 5, cave)?);
    Ok(MouseLookPatches {
        patches: vec![
            slack_patch(finder, "mouse look routine", cave, code)?,
            slack_patch(finder, "mouse look cell", cell, cell_bytes)?,
            finder.patch_instructions("mouse look hook", first.rva, HOOK_LEN, &jump)?,
        ],
        cell,
        cave,
    })
}

/// Hands the cell to the input shim, which from now on collects the raw mouse counts there and no longer moves the
/// stick with them.
///
/// # Safety
/// `image_base + cell` must be the cell of an image whose patches are applied, and stay mapped (writable, 4 byte
/// aligned, [`CELL_LEN`] bytes) until [`shims::gameinput::direct::detach`] is called.
pub unsafe fn attach(image_base: usize, cell: u32, settings: &MouseLookSettings) {
    let scales = settings.scales();
    // SAFETY: forwarded from the caller.
    unsafe { shims::gameinput::direct::attach((image_base + cell as usize) as *mut i32) };
    shims::gameinput::direct::set_scales(scales[0], scales[1]);
}
