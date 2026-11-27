//! DirectStorage as the console exposes it (XCRDAPI.dll: DStorageGetFactory).
//!
//! `DStorageGetFactory` hands out a COM factory (`IDStorageFactory`) whose queues, files and status arrays follow
//! the public `dstorage.h`. A queue has a worker thread: `Submit` hands it the requests enqueued so far, it runs
//! them in order, and status entries, fences and events enqueued behind them are signalled once everything ahead
//! has finished.
//!
//! Not measured on the console build: the vtable layouts and structure layouts are the public ones, the
//! compression formats are only `NONE` until a decompressor is registered (see [`set_decompressor`]), and the
//! console's extra methods land in slots that note the call and fail.
//!
//! // integration
//! - GPU destinations: [`set_gpu_copy`] takes a callback that receives the decompressed bytes and where they go.
//! - Fences: [`set_fence_signal`] replaces the default, a call to slot 10 (`Signal`) of the fence's vtable.
//! - Files: names go through `files::read_location`, so console paths and the overlay apply.

use crate::Shim;

#[cfg(windows)]
mod com;
#[cfg(windows)]
mod engine;

#[cfg(windows)]
pub use engine::{Decompress, FenceSignal, GpuCopy, GpuDestination, GpuUpload};

/// Sets or clears the callback that copies request data into GPU resources. It returns an `HRESULT`.
#[cfg(windows)]
pub fn set_gpu_copy(copy: Option<GpuCopy>) {
    engine::set_gpu_copy(copy);
}

/// Sets or clears the function that signals a fence given its pointer and value.
#[cfg(windows)]
pub fn set_fence_signal(signal: Option<FenceSignal>) {
    engine::set_fence_signal(signal);
}

/// Registers a decompressor for one `DSTORAGE_COMPRESSION_FORMAT` value. Formats without one fail their requests
/// with `E_NOTIMPL` and a trace note.
#[cfg(windows)]
pub fn set_decompressor(format: u8, decompress: Decompress) {
    engine::set_decompressor(format, decompress);
}

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        vec![Shim {
            dll: "XCRDAPI.dll",
            name: "DStorageGetFactory",
            address: com::get_factory as *const () as usize,
        }]
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
