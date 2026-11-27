//! The trapping graphics device: every method call is noted with its slot and name and fails.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;

type Create = unsafe extern "win64" fn(*mut c_void, *const u32, *const u8, *mut *mut c_void) -> u32;
type Method = unsafe extern "win64" fn(*mut c_void, usize, usize, usize) -> u32;
type Query = unsafe extern "win64" fn(*mut c_void, *const u8, *mut *mut c_void) -> u32;

fn slot(object: *mut c_void, index: usize) -> usize {
    // SAFETY: `object` is a COM object whose first word is its vtable.
    unsafe { *(*(object as *const *const usize)).add(index) }
}

#[test]
fn the_console_device_notes_each_method_it_does_not_have() {
    let shim = shims::builtin()
        .into_iter()
        .find(|s| s.name == "D3D12XboxCreateDevice")
        .expect("registered");
    // SAFETY: the shim has the measured shape `(adapter, params, riid, out)`.
    let create: Create = unsafe { std::mem::transmute(shim.address) };
    let params = [0x1234u32; 24];
    let iid = [7u8; 16];
    let mut device = std::ptr::null_mut();
    // SAFETY: valid pointers for each argument.
    assert_eq!(
        unsafe {
            create(
                std::ptr::null_mut(),
                params.as_ptr(),
                iid.as_ptr(),
                &mut device,
            )
        },
        0
    );
    assert!(!device.is_null());
    // SAFETY: slot 45 of the device takes `this` and any three arguments.
    let method: Method = unsafe { std::mem::transmute(slot(device, 45)) };
    // SAFETY: as above.
    assert_eq!(unsafe { method(device, 1, 2, 3) }, 0x8000_4001);
    // SAFETY: slot 0 is QueryInterface.
    let query: Query = unsafe { std::mem::transmute(slot(device, 0)) };
    let mut other = std::ptr::null_mut();
    // SAFETY: valid IID and out pointer.
    assert_eq!(unsafe { query(device, iid.as_ptr(), &mut other) }, 0);
    let unknown: Method = unsafe { std::mem::transmute(slot(other, 200)) };
    // SAFETY: as above.
    assert_eq!(unsafe { unknown(other, 0, 0, 0) }, 0x8000_4001);
    let notes = shims::trace::notes();
    assert!(
        notes.iter().any(|n| n.contains("parameters 00001234")),
        "{notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.starts_with(
            "ID3D12Device slot 45 (SetEventOnMultipleFenceCompletion, assumed from the PC order)"
        )),
        "{notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("slot 200 (unknown)")),
        "{notes:?}"
    );
}
