//! DirectStorage as the console exposes it (XCRDAPI.dll: DStorageGetFactory).
//!
//! `DStorageGetFactory` hands out a COM factory whose queues, files and status arrays follow the console layouts
//! the executable uses (see `com`): the factory's slots (OpenFile 4, CreateStatusArray 5, SetStagingBufferSize 8,
//! CreateQueue 10), the queue description and the 0x48-byte request record. A queue has a worker thread: `Submit`
//! hands it the requests enqueued so far; requests between two status entries, fences or events run side by side,
//! and those markers are signalled once everything ahead of them has finished.
//!
//! The request's compression byte is `options & 0xF` (bit 0 deflate, bits 1-3 BCPack mode). Deflate (zlib or raw)
//! is built in; other formats fail their requests with `E_NOTIMPL` and a trace note until a decompressor is
//! registered (see [`set_decompressor`]). Not measured: the factory's slots 3, 6, 7 and 9, the queue's `Query`
//! record, and the meaning of option bits 4-7. Methods nothing calls land in slots that note the call and fail.
//!
//! // integration
//! - GPU destinations: [`set_gpu_copy`] takes a callback that receives the decompressed bytes and where they go.
//! - Fences: [`set_fence_signal`] replaces the default, a call to slot 10 (`Signal`) of the fence's vtable.
//! - Files: names go through `files::read_location`, so console paths and the overlay apply.

use crate::Shim;

#[cfg(windows)]
mod bcpack;
#[cfg(windows)]
mod com;
#[cfg(windows)]
mod engine;

#[cfg(windows)]
pub use bcpack::set_bcpack_format;
#[cfg(windows)]
pub use engine::{Decompress, FenceSignal, GpuCopy, GpuDestination, GpuUpload, PageWriter};

/// Sets or clears the callback that copies request data into GPU resources. It returns an `HRESULT`.
#[cfg(windows)]
pub fn set_gpu_copy(copy: Option<GpuCopy>) {
    engine::set_gpu_copy(copy);
}

/// Sets or clears the writer for page-table destinations, the console form: a run of 64 KiB physical pages named
/// by 8-byte entries. Without one such requests go to the GPU copy callback as `GpuDestination::Pages`.
#[cfg(windows)]
pub fn set_page_writer(writer: Option<PageWriter>) {
    engine::set_page_writer(writer);
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
