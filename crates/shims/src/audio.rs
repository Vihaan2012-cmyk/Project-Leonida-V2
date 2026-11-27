//! The console audio libraries: `AcpHal.dll`, `xapu.dll` and `chad.dll` (XDsp).
//!
//! The shapes come from how the executable calls them (`audio_hal_exe_calls`); everything the sites do not show
//! is noted with [`crate::trace::note`] the first time it matters and answered with the result that lets the title
//! carry on:
//!
//! - `AcpHalCreate(&handle, 0)` and `AcpHalAllocateShapeContexts(&contexts, ...)` write a pointer to zeroed
//!   host memory and return `S_OK`; `AcpHalReleaseShapeContexts(handle)` frees the contexts.
//! - `ApuCreateHeap(kind, size)`, `ApuAlloc(&virtual, &apu_or_null, size, alignment)` and `ApuFree(address)`
//!   hand out zeroed host memory with a 32-bit coprocessor address (alignment at least 0x800).
//!   `ApuIsVirtualAddressValid(address, size)` is true for any non-null address; `ApuMapVirtualAddress(address,
//!   size)` gives the coprocessor address of any CPU buffer and `ApuMapApuAddress(apu)` the way back (null when
//!   unmapped).
//! - `XApuConnect()` returns a positive handle; `XApuEnqueueCommand(handle, command, 0)` takes a command that
//!   completes at once; `XApuGetResultCount()` is the number of results waiting and `XApuDequeueResult(handle,
//!   &result)` takes one (negative `E_PENDING` when there is none); `XApuDisconnect(handle, 0x20)` ends it.
//! - `XDspConnectWithMaximumStreamLimit(..., 0x80)`, `XDspActivate`, `XDspSubmitCommand(a, b)`,
//!   `XDspDeactivate` and `XDspDisconnect` return 0. Activating opens the output device.
//!
//! Voices reach the mixer through [`Engine`]; the layouts of the commands that would start them are unmeasured.

use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::Shim;

mod apu;
mod engine;
mod mixer;
mod output;
mod queue;
mod xma;

pub use apu::{ApuError, ApuSpace};
pub use engine::{DEFAULT_RATE, Engine, OutputMode};
pub use mixer::{Buffer, BufferId, Completion, Looping, Mixer, VoiceId, VoiceParams};
pub use output::{Output, OutputError, SharedMixer};
pub use queue::{Command, CommandQueue, CommandResult};
pub use xma::{DecodeError, Decoded, Decoder, XmaRequest, decode as decode_xma, set_decoder};

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();

fn engine() -> MutexGuard<'static, Engine> {
    let lock = ENGINE.get_or_init(|| {
        let off = std::env::var("LEONIDA_AUDIO").is_ok_and(|v| v.eq_ignore_ascii_case("off"));
        Mutex::new(Engine::new(if off {
            OutputMode::Off
        } else {
            OutputMode::Auto
        }))
    });
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `f` on the engine the shims share.
pub fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    f(&mut engine())
}

/// Chooses whether the DSP opens the default device when activated.
pub fn set_output_mode(mode: OutputMode) {
    engine().set_mode(mode);
}

#[cfg(windows)]
mod ffi {
    use super::engine;
    use super::queue::COMMAND_BYTES;

    const S_OK: i32 = 0;
    const E_PENDING: i32 = 0x8000_000Au32 as i32;
    const E_POINTER: i32 = 0x8000_4003u32 as i32;
    const E_OUTOFMEMORY: i32 = 0x8007_000Eu32 as i32;
    const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;

    pub extern "win64" fn acp_hal_create(out: *mut usize, flags: u64) -> i32 {
        let mut e = engine();
        if flags != 0 {
            e.note_once("acp-create-flags", || {
                format!("AcpHalCreate called with second argument {flags:#x}")
            });
        }
        let handle = e.acp_create();
        if !out.is_null() {
            // SAFETY: the title passes the address of its handle variable.
            unsafe { out.write_unaligned(handle) };
        }
        S_OK
    }

    pub extern "win64" fn acp_hal_allocate_shape_contexts(
        out: *mut usize,
        a: u64,
        b: u64,
        c: u64,
    ) -> i32 {
        let mut e = engine();
        e.note_once("shape-contexts", || {
            format!(
                "AcpHalAllocateShapeContexts further arguments {a:#x} {b:#x} {c:#x}: the count and size of contexts are unmeasured"
            )
        });
        let contexts = e.allocate_shape_contexts();
        if !out.is_null() {
            // SAFETY: the title passes the address of its context variable.
            unsafe { out.write_unaligned(contexts) };
        }
        S_OK
    }

    pub extern "win64" fn acp_hal_release_shape_contexts(_handle: usize) -> i32 {
        engine().release_shape_contexts();
        S_OK
    }

    pub extern "win64" fn apu_create_heap(kind: u64, size: u64) -> i32 {
        let mut e = engine();
        let heap = e.apu.create_heap(kind as u32, size as usize);
        e.note_once("apu-heap", || {
            format!("ApuCreateHeap(kind {kind:#x}, size {size:#x}) made heap {heap}: its meaning is inferred")
        });
        S_OK
    }

    pub extern "win64" fn apu_alloc(
        out: *mut usize,
        out_apu: *mut u32,
        size: u32,
        alignment: u32,
        _flags: u64,
    ) -> i32 {
        let mut e = engine();
        let result = e.apu.alloc(size as usize, alignment as usize);
        e.note_once("apu-alloc", || {
            format!("ApuAlloc(size {size:#x}, alignment {alignment:#x}) -> {result:?}: the first argument is read as an out pointer")
        });
        let (virt, apu) = result.unwrap_or((0, 0));
        if !out.is_null() {
            // SAFETY: the first argument is the address of the title's pointer variable.
            unsafe { out.write_unaligned(virt) };
        }
        if !out_apu.is_null() {
            // SAFETY: a non-null second argument is the address of a 32-bit variable.
            unsafe { out_apu.write_unaligned(apu) };
        }
        match result {
            Ok(_) => S_OK,
            Err(super::ApuError::Invalid) => E_INVALIDARG,
            Err(_) => E_OUTOFMEMORY,
        }
    }

    pub extern "win64" fn apu_free(address: usize) -> i32 {
        let mut e = engine();
        if !e.apu.free(address) {
            e.note_once("apu-free-unknown", || {
                format!("ApuFree of {address:#x}, which is not a block ApuAlloc made")
            });
        }
        S_OK
    }

    pub extern "win64" fn apu_is_virtual_address_valid(address: usize, _size: usize) -> u32 {
        u32::from(engine().apu.is_valid(address))
    }

    pub extern "win64" fn apu_map_virtual_address(address: usize, size: usize) -> u32 {
        engine().apu.map_virtual(address, size).unwrap_or(0)
    }

    pub extern "win64" fn apu_map_apu_address(apu: u32) -> usize {
        engine().apu.map_apu(apu).unwrap_or(0)
    }

    pub extern "win64" fn xapu_connect() -> u64 {
        engine().queue.connect()
    }

    pub extern "win64" fn xapu_disconnect(_handle: u64, _size: u64) -> i32 {
        engine().queue.disconnect();
        S_OK
    }

    pub extern "win64" fn xapu_enqueue_command(
        _handle: u64,
        command: *const u8,
        flags: u64,
    ) -> i32 {
        if command.is_null() {
            return E_POINTER;
        }
        // SAFETY: the title passes the address of a command it built; every command is longer than its header.
        let head = unsafe { command.cast::<[u8; COMMAND_BYTES]>().read_unaligned() };
        let mut e = engine();
        let sequence = e.queue.enqueue(head, flags);
        let word = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        e.note_once(&format!("xapu-command-{word:#x}"), || {
            format!(
                "XApuEnqueueCommand command {sequence} starts {word:#010x} (flags {flags:#x}): the layout is unmeasured, so it completes without effect"
            )
        });
        e.pump();
        S_OK
    }

    pub extern "win64" fn xapu_dequeue_result(_handle: u64, out: *mut u32) -> i32 {
        let mut e = engine();
        e.pump();
        let Some(result) = e.queue.dequeue() else {
            return E_PENDING;
        };
        e.note_once("xapu-result", || {
            "XApuDequeueResult writes only a zero 32-bit status: the result layout is unmeasured"
                .into()
        });
        if !out.is_null() {
            // SAFETY: a non-null second argument is the title's result variable, at least 32 bits wide.
            unsafe { out.write_unaligned(result.status) };
        }
        S_OK
    }

    pub extern "win64" fn xapu_get_result_count() -> u32 {
        let mut e = engine();
        e.pump();
        e.queue.result_count() as u32
    }

    pub extern "win64" fn xdsp_connect(a: u64, b: u64, c: u64, limit: u64) -> u32 {
        engine().note_once("xdsp-connect", || {
            format!("XDspConnectWithMaximumStreamLimit({a:#x}, {b:#x}, {c:#x}, {limit:#x}): nothing is written")
        });
        0
    }

    pub extern "win64" fn xdsp_activate() -> u32 {
        engine().activate();
        0
    }

    pub extern "win64" fn xdsp_submit_command(a: u64, b: u64) -> u32 {
        let mut e = engine();
        e.note_once("xdsp-submit", || {
            format!("XDspSubmitCommand({a:#x}, {b:#x}): ignored, the command layout is unmeasured")
        });
        e.pump();
        0
    }

    pub extern "win64" fn xdsp_deactivate() -> u32 {
        engine().deactivate();
        0
    }

    pub extern "win64" fn xdsp_disconnect() -> u32 {
        0
    }
}

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let acp: [(&str, usize); 9] = [
            ("AcpHalCreate", ffi::acp_hal_create as *const () as usize),
            (
                "AcpHalAllocateShapeContexts",
                ffi::acp_hal_allocate_shape_contexts as *const () as usize,
            ),
            (
                "AcpHalReleaseShapeContexts",
                ffi::acp_hal_release_shape_contexts as *const () as usize,
            ),
            ("ApuCreateHeap", ffi::apu_create_heap as *const () as usize),
            ("ApuAlloc", ffi::apu_alloc as *const () as usize),
            ("ApuFree", ffi::apu_free as *const () as usize),
            (
                "ApuIsVirtualAddressValid",
                ffi::apu_is_virtual_address_valid as *const () as usize,
            ),
            (
                "ApuMapVirtualAddress",
                ffi::apu_map_virtual_address as *const () as usize,
            ),
            (
                "ApuMapApuAddress",
                ffi::apu_map_apu_address as *const () as usize,
            ),
        ];
        let apu: [(&str, usize); 5] = [
            ("XApuConnect", ffi::xapu_connect as *const () as usize),
            ("XApuDisconnect", ffi::xapu_disconnect as *const () as usize),
            (
                "XApuEnqueueCommand",
                ffi::xapu_enqueue_command as *const () as usize,
            ),
            (
                "XApuDequeueResult",
                ffi::xapu_dequeue_result as *const () as usize,
            ),
            (
                "XApuGetResultCount",
                ffi::xapu_get_result_count as *const () as usize,
            ),
        ];
        let dsp: [(&str, usize); 5] = [
            (
                "XDspConnectWithMaximumStreamLimit",
                ffi::xdsp_connect as *const () as usize,
            ),
            ("XDspActivate", ffi::xdsp_activate as *const () as usize),
            (
                "XDspSubmitCommand",
                ffi::xdsp_submit_command as *const () as usize,
            ),
            ("XDspDeactivate", ffi::xdsp_deactivate as *const () as usize),
            ("XDspDisconnect", ffi::xdsp_disconnect as *const () as usize),
        ];
        let table = |dll: &'static str, list: &[(&'static str, usize)]| {
            list.iter()
                .map(|&(name, address)| Shim { dll, name, address })
                .collect::<Vec<_>>()
        };
        let mut shims = table("AcpHal.dll", &acp);
        shims.extend(table("xapu.dll", &apu));
        shims.extend(table("chad.dll", &dsp));
        shims
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
