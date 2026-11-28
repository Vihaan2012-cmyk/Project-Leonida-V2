//! The console audio libraries: `AcpHal.dll`, `xapu.dll` and `chad.dll` (XDsp).
//!
//! The shapes come from how the executable calls them (`audio_hal_exe_calls`, refined by reading its code);
//! everything the sites do not show is noted with [`crate::trace::note`] the first time it matters:
//!
//! - `AcpHalCreate(&object, 0)` writes a pointer to an object whose first word is a vtable (see [`hal`]).
//!   `AcpHalAllocateShapeContexts(&info)` reads the context count from `info + 0x10` and writes the address of
//!   that many zeroed 64-byte XMA contexts to `info + 0x38`; `AcpHalReleaseShapeContexts()` frees them.
//! - `ApuCreateHeap(kind, size)`, `ApuAlloc(&virtual, &apu_or_null, size, alignment)` and `ApuFree(address)`
//!   hand out zeroed host memory with a 32-bit coprocessor address (alignment at least 0x800).
//!   `ApuIsVirtualAddressValid(address, size)` is true for any non-null address; `ApuMapVirtualAddress(address,
//!   size)` gives the coprocessor address of any CPU buffer and `ApuMapApuAddress(apu)` the way back (null when
//!   unmapped).
//! - `XApuConnect(&info, &region, &handle)` starts the Opus decoder worker of [`xapu`] and writes the shared
//!   region's address and a positive handle; `XApuEnqueueCommand(handle, command, 0)` hands it a 0x40-byte
//!   command; `XApuGetResultCount()` is the number of results waiting and `XApuDequeueResult(handle, &result)`
//!   takes one (0x40 bytes, negative `E_PENDING` when there is none); `XApuDisconnect(handle, 0x20)` ends it.
//! - The `XDsp*` functions are the title's convolution reverb and nothing else (see [`dsp`]): the title mixes on
//!   the CPU in 256-sample blocks, so no final mix passes through them.
//!   `XDspConnectWithMaximumStreamLimit(memory, size, version, limit, 1, &handle)` opens a connection,
//!   `XDspActivate(handle, &info, &status, &stream)` a stream, `XDspSubmitCommand(stream, &command, &fence)` runs one
//!   batch to completion before it returns, `XDspDeactivate(stream)` and `XDspDisconnect(handle)` close them.
//!
//! - The title's own output goes through `CoCreateInstance(MMDeviceEnumerator)` and the spatial audio client (see
//!   [`spatial`] and `spatial_com`). That import is provided here: the host's objects when they can open the
//!   stream the title asks for, this crate's own otherwise (`LEONIDA_SPATIAL_AUDIO=host|emulate|auto`, and always
//!   with `LEONIDA_AUDIO=off`). The own objects mix the title's speaker buffers to stereo on the shared mixer.
//!
//! Voices reach the mixer through [`Engine`]; the device opens when the first sink asks for the mixer.

use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::Shim;

mod apu;
mod convolve;
pub mod dsp;
mod engine;
pub mod hal;
mod mixer;
pub mod opus;
mod output;
pub mod spatial;
#[cfg(windows)]
pub mod spatial_com;
pub mod xapu;
mod xma;
pub mod xma_hw;

pub use apu::{ApuError, ApuSpace};
pub use convolve::{Convolver, Fft};
pub use dsp::{Command as DspCommand, Dsp, DspError, StreamInfo as DspStreamInfo};
pub use engine::{DEFAULT_RATE, Engine, OutputMode, ReopenGate};
pub use mixer::{Buffer, BufferId, Completion, Looping, Mixer, VoiceId, VoiceParams};
pub use output::{Output, OutputError, SharedMixer};
pub use xapu::{OpusFactory, OpusStream, XApu, XApuResult, set_opus_factory};
pub use xma::{
    DecodeError, Decoded, Decoder, OwnedXmaRequest, XmaRequest, decode as decode_xma, set_decoder,
};

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
static DSP: OnceLock<Mutex<Dsp>> = OnceLock::new();

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

fn dsp() -> MutexGuard<'static, Dsp> {
    DSP.get_or_init(|| Mutex::new(Dsp::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs `f` on the DSP the shims share.
pub fn with_dsp<R>(f: impl FnOnce(&mut Dsp) -> R) -> R {
    f(&mut dsp())
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
    use super::xapu::{COMMAND_BYTES, RESULT_BYTES};
    use super::{DspError, dsp, engine};

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

    pub extern "win64" fn acp_hal_allocate_shape_contexts(info: *mut u8) -> i32 {
        if info.is_null() {
            return E_POINTER;
        }
        // SAFETY: the title passes the address of its context description, at least 0x40 bytes: the count is read
        // from +0x10 and the contexts' address written to +0x38.
        let count = unsafe { info.add(0x10).cast::<u64>().read_unaligned() };
        if count == 0 || count > 4096 {
            return E_INVALIDARG;
        }
        let contexts = engine().allocate_shape_contexts(count as usize);
        // SAFETY: as above.
        unsafe { info.add(0x38).cast::<usize>().write_unaligned(contexts) };
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

    pub extern "win64" fn xapu_connect(
        _info: *mut u8,
        region: *mut usize,
        handle: *mut u64,
    ) -> i32 {
        let (h, base) = engine().xapu.connect();
        if !region.is_null() {
            // SAFETY: the title passes the address of its region pointer variable.
            unsafe { region.write_unaligned(base) };
        }
        if !handle.is_null() {
            // SAFETY: the title passes the address of its handle variable.
            unsafe { handle.write_unaligned(h) };
        }
        let mut e = engine();
        e.note_once("xapu-info", || {
            "XApuConnect first argument is left untouched: what the coprocessor writes there is unmeasured".into()
        });
        S_OK
    }

    pub extern "win64" fn xapu_disconnect(_handle: u64, _size: u64) -> i32 {
        engine().xapu.disconnect();
        S_OK
    }

    pub extern "win64" fn xapu_enqueue_command(
        _handle: u64,
        command: *const u8,
        _flags: u64,
    ) -> i32 {
        if command.is_null() {
            return E_POINTER;
        }
        // SAFETY: the title passes the address of a command it built on its stack; commands are 0x40 bytes
        // or shorter with the rest of the frame behind them.
        let bytes = unsafe { command.cast::<[u8; COMMAND_BYTES]>().read_unaligned() };
        let mut e = engine();
        match e.xapu.enqueue(&bytes) {
            Some(op) => {
                if !(1..=3).contains(&op) && op != 5 {
                    e.note_once(&format!("xapu-op-{op}"), || {
                        format!("XApuEnqueueCommand operation {op}: not one the title has been seen to use")
                    });
                }
                S_OK
            }
            None => E_PENDING,
        }
    }

    pub extern "win64" fn xapu_dequeue_result(_handle: u64, out: *mut u8) -> i32 {
        let Some(result) = engine().xapu.dequeue() else {
            return E_PENDING;
        };
        if !out.is_null() {
            let bytes: [u8; RESULT_BYTES] = result.to_bytes();
            // SAFETY: a non-null second argument is the title's 0x40-byte result variable.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, RESULT_BYTES) };
        }
        S_OK
    }

    pub extern "win64" fn xapu_get_result_count() -> u32 {
        engine().xapu.result_count() as u32
    }

    pub extern "win64" fn xdsp_connect(
        _memory: usize,
        _size: u32,
        version: u32,
        limit: u32,
        _flags: u32,
        out: *mut u64,
    ) -> u32 {
        let handle = dsp().connect(version, limit);
        if !out.is_null() {
            // SAFETY: the title passes the address of the handle field of its device record.
            unsafe { out.write_unaligned(handle) };
        }
        0
    }

    pub extern "win64" fn xdsp_activate(
        connection: u64,
        info: *const u8,
        status_out: *mut usize,
        stream_out: *mut u64,
    ) -> u32 {
        if info.is_null() {
            return DspError::NullBuffer.code();
        }
        // SAFETY: the title passes the address of the 0x1c-byte description it built on its stack.
        let bytes = unsafe { info.cast::<[u8; dsp::INFO_BYTES]>().read_unaligned() };
        let info = dsp::StreamInfo::parse(&bytes);
        // SAFETY: a convolution stream's address and length describe the impulse response the title keeps loaded
        // while the stream opens.
        let response = if info.kind == dsp::KIND_CONVOLUTION {
            unsafe { dsp::read_response(&info) }
        } else {
            Vec::new()
        };
        let opened = dsp().activate(connection, &info, &response);
        match opened {
            Ok(opened) => {
                if !status_out.is_null() {
                    // SAFETY: the title passes the address of its status pointer field.
                    unsafe { status_out.write_unaligned(opened.status) };
                }
                if !stream_out.is_null() {
                    // SAFETY: the title passes the address of its stream handle field.
                    unsafe { stream_out.write_unaligned(opened.stream) };
                }
                0
            }
            Err(error) => {
                engine().note_once(&format!("xdsp-activate-{error}"), || {
                    format!("XDspActivate refused: {error}")
                });
                error.code()
            }
        }
    }

    pub extern "win64" fn xdsp_submit_command(
        stream: u64,
        command: *const u8,
        fence_out: *mut u32,
    ) -> u32 {
        if command.is_null() {
            return DspError::NullBuffer.code();
        }
        // SAFETY: the title passes the address of the 0x20-byte command it built on its stack.
        let bytes = unsafe { command.cast::<[u8; dsp::COMMAND_BYTES]>().read_unaligned() };
        let command = dsp::Command::parse(&bytes);
        let result = dsp().stream(stream).and_then(|stream| {
            let mut stream = dsp::lock_stream(&stream);
            // SAFETY: the title's command names the two buffers of a pair it took from its pool, each 0x1000
            // bytes, which holds a whole batch (at most 1024 floats).
            unsafe { stream.run(&command) }
        });
        let code = match result {
            Ok(fence) => {
                if !fence_out.is_null() {
                    // SAFETY: the title passes the address of its 32-bit fence variable.
                    unsafe { fence_out.write_unaligned(fence) };
                }
                0
            }
            Err(error) => {
                engine().note_once(&format!("xdsp-submit-{error}"), || {
                    format!("XDspSubmitCommand refused: {error}")
                });
                error.code()
            }
        };
        engine().pump();
        code
    }

    pub extern "win64" fn xdsp_deactivate(stream: u64) -> u32 {
        dsp().deactivate(stream).map_or_else(|e| e.code(), |()| 0)
    }

    pub extern "win64" fn xdsp_disconnect(connection: u64) -> u32 {
        dsp()
            .disconnect(connection)
            .map_or_else(|e| e.code(), |()| 0)
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
        let com: [(&str, usize); 1] = [(
            "CoCreateInstance",
            spatial_com::co_create_instance as *const () as usize,
        )];
        let table = |dll: &'static str, list: &[(&'static str, usize)]| {
            list.iter()
                .map(|&(name, address)| Shim { dll, name, address })
                .collect::<Vec<_>>()
        };
        let mut shims = table("AcpHal.dll", &acp);
        shims.extend(table("xapu.dll", &apu));
        shims.extend(table("chad.dll", &dsp));
        shims.extend(table("api-ms-win-core-com-l1-1-0.dll", &com));
        shims
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
