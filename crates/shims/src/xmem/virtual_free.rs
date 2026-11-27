//! `VirtualFree`, as the title calls it.
//!
//! The executable imports it from `api-ms-win-core-memory-l1-1-0.dll` (IAT 0x6793a70) and uses it on memory
//! `XMemVirtualAlloc` gave it: at 0x1e86440 it splits the 185 MiB placeholder into five ranges with
//! `VirtualFree(range, size, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER)` (0x8002) before re-reserving each. It
//! imports no `VirtualProtect`. The call goes to Windows unchanged; around it this module keeps `xmem`'s records
//! true, which the GPU hooks depend on:
//!
//! - ranges recorded as mapped for the GPU that the call decommits, releases or turns back into a placeholder
//!   are reported unmapped first (the hooks want to hear before the host memory changes), and reported mapped
//!   again with `SyncPolicy::Always` if Windows refuses, whatever policy they were mapped with;
//! - a split of one of our placeholders is recorded, so a later `XMemVirtualAlloc` into the piece finds it
//!   already carved;
//! - a release forgets the GPU reservations and placeholders it covered.
//!
//! Views that `XMemMapPhysicalPages` made are not touched: Windows refuses to free a range that holds them.

use std::collections::BTreeMap;
use std::ffi::c_void;

use windows_sys::Win32::System::Memory::{
    MEM_DECOMMIT, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEMORY_BASIC_INFORMATION, VirtualFree,
    VirtualQuery,
};

use xbox_d3d12::memory::SyncPolicy;
use xbox_d3d12::memory::hooks::{self, GpuMemoryHooks};

use super::imp::{self, STATE};
use crate::Shim;
use crate::trace::note;

const MEM_COALESCE_PLACEHOLDERS: u32 = 0x1;

/// The libraries `VirtualFree` is reached through: kernel32 and the API sets that forward to it.
const DLLS: [&str; 11] = [
    "kernel32.dll",
    "kernelbase.dll",
    "api-ms-win-core-memory-l1-1-0.dll",
    "api-ms-win-core-memory-l1-1-1.dll",
    "api-ms-win-core-memory-l1-1-2.dll",
    "api-ms-win-core-memory-l1-1-3.dll",
    "api-ms-win-core-memory-l1-1-4.dll",
    "api-ms-win-core-memory-l1-1-5.dll",
    "api-ms-win-core-memory-l1-1-6.dll",
    "api-ms-win-core-memory-l1-2-0.dll",
    "api-ms-win-core-memory-l1-2-1.dll",
];

pub(super) fn shims() -> Vec<Shim> {
    DLLS.iter()
        .map(|dll| Shim {
            dll,
            name: "VirtualFree",
            address: virtual_free as *const () as usize,
        })
        .collect()
}

extern "win64" fn virtual_free(address: *mut c_void, size: usize, free_type: u32) -> i32 {
    let gives_up = free_type & (MEM_RELEASE | MEM_DECOMMIT) != 0;
    let (start, end) = if gives_up {
        extent(address as usize, size)
    } else {
        (0, 0)
    };
    let hooks = hooks::current();
    let unmapped = if hooks.is_some() && end > start {
        take_ranges(start, end, Records::Mapped)
    } else {
        Vec::new()
    };
    if let Some(hooks) = hooks.as_deref() {
        for taken in &unmapped {
            imp::report_unmapped(hooks, taken.covered.0, taken.covered.1);
        }
    }
    // SAFETY: the program's own call, forwarded as it made it.
    let done = unsafe { VirtualFree(address, size, free_type) } != 0;
    if !done {
        restore(hooks.as_deref(), &unmapped);
        return 0;
    }
    if end > start {
        record(start, end, free_type);
    }
    1
}

/// `start..end` that a call frees: the range it names, or, with no size, the whole allocation at `address`.
fn extent(address: usize, size: usize) -> (usize, usize) {
    if size != 0 {
        return (address, address.saturating_add(size));
    }
    let mut end = address;
    for _ in 0..1 << 16 {
        // SAFETY: a plain query of the process's own address space.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let got = unsafe {
            VirtualQuery(
                end as *const c_void,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if got == 0 || info.AllocationBase as usize != address || info.RegionSize == 0 {
            break;
        }
        end += info.RegionSize;
    }
    (address, end)
}

#[derive(Clone, Copy)]
enum Records {
    Mapped,
    Reserved,
    Placeholders,
}

/// A record cut by a call: what it was, and the part the call covered.
struct Taken {
    record: (usize, usize),
    covered: (usize, usize),
}

/// Removes `start..end` from the records `which` picks and returns what it covered.
fn take_ranges(start: usize, end: usize, which: Records) -> Vec<Taken> {
    let Ok(mut state) = STATE.lock() else {
        return Vec::new();
    };
    let ranges: &mut BTreeMap<usize, usize> = match which {
        Records::Mapped => &mut state.gpu_mapped,
        Records::Reserved => &mut state.gpu_reserved,
        Records::Placeholders => &mut state.placeholders,
    };
    let hit: Vec<(usize, usize)> = ranges
        .range(..end)
        .filter(|(at, length)| **at + **length > start)
        .map(|(&at, &length)| (at, length))
        .collect();
    let mut covered = Vec::new();
    for (at, length) in hit {
        ranges.remove(&at);
        let (low, high) = (at.max(start), (at + length).min(end));
        if at < low {
            ranges.insert(at, low - at);
        }
        if high < at + length {
            ranges.insert(high, at + length - high);
        }
        covered.push(Taken {
            record: (at, length),
            covered: (low, high - low),
        });
    }
    covered
}

/// Windows refused: the GPU hears that the ranges it was told to forget are mapped after all.
fn restore(hooks: Option<&dyn GpuMemoryHooks>, unmapped: &[Taken]) {
    if let Ok(mut state) = STATE.lock() {
        for taken in unmapped {
            let (at, length) = taken.record;
            state
                .gpu_mapped
                .retain(|&start, _| start < at || start >= at + length);
            state.gpu_mapped.insert(at, length);
        }
    }
    let Some(hooks) = hooks else {
        return;
    };
    for &Taken {
        covered: (at, length),
        ..
    } in unmapped
    {
        if let Err(e) = hooks.mapped(at as u64, length as u64, SyncPolicy::Always) {
            note(format!(
                "the GPU cannot map {at:#x}+{length:#x} again after VirtualFree refused: {e}"
            ));
        }
    }
}

/// Brings the placeholder and reservation records in line with a call that succeeded.
fn record(start: usize, end: usize, free_type: u32) {
    let released = free_type & MEM_RELEASE != 0;
    let preserved = free_type & MEM_PRESERVE_PLACEHOLDER != 0;
    if released && !preserved {
        take_ranges(start, end, Records::Reserved);
        take_ranges(start, end, Records::Placeholders);
        return;
    }
    if !released {
        return;
    }
    if free_type & MEM_COALESCE_PLACEHOLDERS != 0 {
        take_ranges(start, end, Records::Placeholders);
        if let Ok(mut state) = STATE.lock() {
            state.placeholders.insert(start, end - start);
        }
        return;
    }
    if let Ok(mut state) = STATE.lock() {
        let held = state
            .placeholders
            .range(..=start)
            .next_back()
            .is_some_and(|(&base, &length)| end <= base + length);
        if held {
            imp::record_split(&mut state, start, end - start);
        } else {
            state.placeholders.insert(start, end - start);
        }
    }
}
