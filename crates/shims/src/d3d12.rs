//! The console graphics entry point: `D3D12XboxCreateDevice` creates the device of the `xbox-d3d12` crate.
//! Methods and interfaces that crate does not know are noted in the trace, by interface, slot and method
//! name from `d3d12_slots.txt`, with the caller's address, and fail with `E_NOTIMPL`.
//!
//! The entry point's shape, `(adapter, params, riid, out)` with a 24-word parameter block, was measured in the
//! executable, not documented.

use std::ffi::c_void;

use crate::Shim;
use crate::trace::{image_offset, note};

fn trace_note(text: String) {
    note(text);
}

/// # Safety
/// The measured console shape: `params` points at 24 readable words or is null, `riid` at an IID, `out` is
/// writable or null.
unsafe extern "win64" fn create_device(
    _adapter: *mut c_void,
    params: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    xbox_d3d12::com::set_trace(trace_note);
    xbox_d3d12::com::set_address_format(image_offset);
    // SAFETY: as the caller promises.
    unsafe { xbox_d3d12::create_default_device(params, riid, out) }
}

pub fn shims() -> Vec<Shim> {
    vec![Shim {
        dll: "d3d12_xs.dll",
        name: "D3D12XboxCreateDevice",
        address: create_device as *const () as usize,
    }]
}
