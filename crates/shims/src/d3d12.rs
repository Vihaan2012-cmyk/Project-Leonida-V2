//! The console graphics entry point, as a trap until the renderer exists: `D3D12XboxCreateDevice` hands back
//! a device whose every method is noted in the trace, by interface, slot and method name from
//! `d3d12_slots.txt`, with the caller's address, and then fails with `E_NOTIMPL`. `QueryInterface` gives
//! another such object, so the program's next steps show up too.
//!
//! The entry point's shape, `(adapter, params, riid, out)` with a 24-word parameter block, was measured in the
//! executable, not documented.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::Shim;
use crate::trace::{image_offset, note};

const SLOTS: usize = 256;
const S_OK: u32 = 0;
const E_POINTER: u32 = 0x8000_4003;
const E_NOTIMPL: u32 = 0x8000_4001;

struct Slot {
    interface: &'static str,
    slot: usize,
    method: &'static str,
    measured: bool,
}

fn table() -> Vec<Slot> {
    include_str!("d3d12_slots.txt")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some(Slot {
                interface: words.next()?,
                slot: words.next()?.parse().ok()?,
                method: words.next()?,
                measured: words.next()? == "measured",
            })
        })
        .collect()
}

/// A method's name and whether its slot was measured, if the table knows it.
pub fn method(interface: &str, slot: usize) -> Option<(&'static str, bool)> {
    table()
        .into_iter()
        .find(|s| s.interface == interface && s.slot == slot)
        .map(|s| (s.method, s.measured))
}

/// An object the program sees as a COM interface. The vtable pointer must come first.
#[repr(C)]
struct Trapped {
    vtable: *const usize,
    refs: AtomicU32,
    interface: &'static str,
}

static NOTED: Mutex<Option<HashSet<(&'static str, usize)>>> = Mutex::new(None);

fn first_time(interface: &'static str, slot: usize) -> bool {
    NOTED
        .lock()
        .map(|mut seen| {
            seen.get_or_insert_with(HashSet::new)
                .insert((interface, slot))
        })
        .unwrap_or(false)
}

/// # Safety
/// `this` is a `Trapped` from `trapped_object`.
unsafe extern "win64" fn trapped_method(this: *const Trapped, slot: usize, caller: usize) -> u32 {
    // SAFETY: as the caller promises.
    let interface = unsafe { (*this).interface };
    if first_time(interface, slot) {
        let name = match method(interface, slot) {
            Some((name, true)) => format!("{name}, measured"),
            Some((name, false)) => format!("{name}, assumed from the PC order"),
            None => "unknown".to_string(),
        };
        note(format!(
            "{interface} slot {slot} ({name}) called from {}; not implemented",
            image_offset(caller)
        ));
    }
    E_NOTIMPL
}

fn guid(riid: *const u8) -> String {
    if riid.is_null() {
        return "null".into();
    }
    // SAFETY: COM passes a 16-byte IID.
    let bytes = unsafe { std::slice::from_raw_parts(riid, 16) };
    let d1 = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes"));
    let d2 = u16::from_le_bytes([bytes[4], bytes[5]]);
    let d3 = u16::from_le_bytes([bytes[6], bytes[7]]);
    let tail: String = bytes[8..].iter().map(|b| format!("{b:02x}")).collect();
    format!("{d1:08x}-{d2:04x}-{d3:04x}-{}-{}", &tail[..4], &tail[4..])
}

/// # Safety
/// COM's `QueryInterface`: `riid` points at an IID and `out` at a writable pointer, either may be null.
unsafe extern "win64" fn query_interface(
    this: *const Trapped,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: as the caller promises.
    let interface = unsafe { (*this).interface };
    let wanted = guid(riid);
    note(format!("{interface} asked for interface {wanted}"));
    let label: &'static str = Box::leak(format!("{interface} as {wanted}").into_boxed_str());
    let object = trapped_object(label);
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = object };
    if object.is_null() { E_POINTER } else { S_OK }
}

/// # Safety
/// `this` is a `Trapped`.
unsafe extern "win64" fn add_ref(this: *const Trapped) -> u32 {
    // SAFETY: as the caller promises.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

/// # Safety
/// `this` is a `Trapped`. Trapped objects are never freed: the program may still hold pointers to them.
unsafe extern "win64" fn release(this: *const Trapped) -> u32 {
    // SAFETY: as the caller promises.
    unsafe {
        (*this)
            .refs
            .fetch_sub(1, Ordering::Relaxed)
            .saturating_sub(1)
    }
}

static VTABLE: Mutex<usize> = Mutex::new(0);

#[cfg(windows)]
fn vtable() -> *const usize {
    let mut table = VTABLE.lock().expect("no panic while held");
    if *table == 0 {
        let mut code = crate::code::Code::default();
        let mut entries = vec![
            query_interface as *const () as usize,
            add_ref as *const () as usize,
            release as *const () as usize,
        ];
        for slot in 3..SLOTS {
            let thunk = code
                .method_trap(slot, trapped_method as *const () as usize)
                .unwrap_or(0);
            entries.push(thunk);
        }
        if code.seal().is_err() {
            return std::ptr::null();
        }
        std::mem::forget(code);
        *table = Box::leak(entries.into_boxed_slice()).as_ptr() as usize;
    }
    *table as *const usize
}

#[cfg(not(windows))]
fn vtable() -> *const usize {
    std::ptr::null()
}

fn trapped_object(interface: &'static str) -> *mut c_void {
    let vtable = vtable();
    if vtable.is_null() {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(Trapped {
        vtable,
        refs: AtomicU32::new(1),
        interface,
    })) as *mut c_void
}

/// # Safety
/// The measured console shape: `params` points at 24 readable words or is null, `out` is writable or null.
unsafe extern "win64" fn create_device(
    _adapter: *mut c_void,
    params: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let words = if params.is_null() {
        "none".to_string()
    } else {
        // SAFETY: as the caller promises.
        let words = unsafe { std::slice::from_raw_parts(params, 24) };
        words
            .iter()
            .map(|w| format!("{w:08x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    note(format!(
        "D3D12XboxCreateDevice asked for {} with parameters {words}",
        guid(riid)
    ));
    if out.is_null() {
        return E_POINTER;
    }
    let device = trapped_object("ID3D12Device");
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = device };
    if device.is_null() { E_POINTER } else { S_OK }
}

pub fn shims() -> Vec<Shim> {
    vec![Shim {
        dll: "d3d12_xs.dll",
        name: "D3D12XboxCreateDevice",
        address: create_device as *const () as usize,
    }]
}
