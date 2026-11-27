//! `VirtualFree` as the title calls it on memory `XMemVirtualAlloc` gave it: forwarded to Windows, with what the
//! GPU was told about the memory kept true.
#![cfg(windows)]
#![allow(unsafe_code)]

mod common;

use std::ffi::c_void;

use common::{Event, GIB, Installed, PAGE, shim, shim_of, without_hooks};
use xbox_d3d12::memory::SyncPolicy;

type VirtualAlloc = unsafe extern "win64" fn(*mut c_void, usize, u32, u64, u32) -> *mut c_void;
type VirtualFree = unsafe extern "win64" fn(*mut c_void, usize, u32) -> i32;

const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;
const MEM_PRESERVE_PLACEHOLDER: u32 = 0x2;

fn virtual_free() -> VirtualFree {
    shim_of("api-ms-win-core-memory-l1-1-0.dll", "VirtualFree")
}

const PAGE_USIZE: usize = PAGE as usize;

#[test]
fn it_is_served_for_kernel32_and_the_api_sets_the_title_imports_it_from() {
    for dll in [
        "kernel32.dll",
        "kernelbase.dll",
        "api-ms-win-core-memory-l1-1-0.dll",
    ] {
        let found = shims::builtin()
            .into_iter()
            .any(|s| s.dll == dll && s.name == "VirtualFree");
        assert!(found, "{dll}");
    }
}

#[test]
fn releasing_gpu_memory_reports_it_unmapped_and_frees_it() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let free = virtual_free();
    // SAFETY: the shapes the executable uses.
    unsafe {
        let base = reserve(
            std::ptr::null_mut(),
            4 * PAGE_USIZE,
            0x2040_3000,
            0x183,
            0x2004,
        );
        assert!(!base.is_null());
        (base as *mut u8).write_volatile(1);
        assert_eq!(
            hooks.take(),
            vec![Event::Mapped(base as u64, 4 * PAGE, SyncPolicy::WriteWatch)]
        );
        assert_eq!(free(base, 0, MEM_RELEASE), 1);
        assert_eq!(hooks.take(), vec![Event::Unmapped(base as u64, 4 * PAGE)]);
        assert_eq!(free(base, 0, MEM_RELEASE), 0, "gone");
        assert_eq!(hooks.take(), vec![], "and nothing is left to report");
    }
}

#[test]
fn decommitting_part_of_it_reports_that_part_and_the_rest_later() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let free = virtual_free();
    // SAFETY: the shapes the executable uses.
    unsafe {
        let base = reserve(
            std::ptr::null_mut(),
            4 * PAGE_USIZE,
            0x2040_3000,
            0x183,
            0x2004,
        ) as u64;
        hooks.take();
        let middle = (base + PAGE) as *mut c_void;
        assert_eq!(free(middle, PAGE_USIZE, MEM_DECOMMIT), 1);
        assert_eq!(hooks.take(), vec![Event::Unmapped(base + PAGE, PAGE)]);
        assert_eq!(free(base as *mut c_void, 0, MEM_RELEASE), 1);
        assert_eq!(
            hooks.take(),
            vec![
                Event::Unmapped(base, PAGE),
                Event::Unmapped(base + 2 * PAGE, 2 * PAGE)
            ]
        );
    }
}

#[test]
fn memory_the_gpu_never_saw_is_freed_without_a_report() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let free = virtual_free();
    // SAFETY: the shapes the executable uses.
    unsafe {
        let base = reserve(
            std::ptr::null_mut(),
            2 * PAGE_USIZE,
            0x2040_3000,
            0x181,
            0x4,
        );
        assert!(!base.is_null());
        assert_eq!(free(base, 0, MEM_RELEASE), 1);
    }
    assert_eq!(hooks.take(), vec![]);
}

#[test]
fn a_call_windows_refuses_leaves_the_memory_mapped_for_the_gpu() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let free = virtual_free();
    // SAFETY: the shapes the executable uses; the refused call frees nothing.
    unsafe {
        let base = reserve(
            std::ptr::null_mut(),
            2 * PAGE_USIZE,
            0x2040_3000,
            0x183,
            0x2004,
        ) as u64;
        hooks.take();
        assert_eq!(free(base as *mut c_void, PAGE_USIZE, MEM_RELEASE), 0);
        assert_eq!(
            hooks.take(),
            vec![
                Event::Unmapped(base, PAGE),
                Event::Mapped(base, PAGE, SyncPolicy::Always)
            ]
        );
        assert_eq!(free(base as *mut c_void, 0, MEM_RELEASE), 1);
        assert_eq!(hooks.take(), vec![Event::Unmapped(base, 2 * PAGE)]);
    }
}

#[test]
fn the_titles_split_of_a_placeholder_leaves_pieces_xmem_virtual_alloc_can_carve() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let free = virtual_free();
    let sizes = [4 * PAGE_USIZE, 2 * PAGE_USIZE, 3 * PAGE_USIZE];
    // SAFETY: the sequence of 0x1e86440: reserve, split off each range, re-reserve each.
    unsafe {
        let total: usize = sizes.iter().sum::<usize>() + PAGE_USIZE;
        let base = reserve(std::ptr::null_mut(), total, 0x4_2000, 0x7, 0x1);
        assert!(!base.is_null());
        let mut at = base as usize;
        let mut pieces = Vec::new();
        for size in sizes {
            assert_eq!(
                free(
                    at as *mut c_void,
                    size,
                    MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER
                ),
                1
            );
            pieces.push((at, size));
            at += size;
        }
        for (at, size) in pieces {
            assert_eq!(
                reserve(at as *mut c_void, size, 0x2040_6000, 0x27, 0x1004),
                at as *mut c_void
            );
        }
        let last = at as *mut c_void;
        assert_eq!(reserve(last, PAGE_USIZE, 0x2040_6000, 0x27, 0x2004), last);
    }
    assert_eq!(hooks.take(), vec![], "reserving maps nothing");
}

#[test]
fn without_hooks_it_is_the_hosts_virtual_free() {
    let _lock = without_hooks();
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let free = virtual_free();
    // SAFETY: the shapes the executable uses.
    unsafe {
        let base = reserve(std::ptr::null_mut(), PAGE_USIZE, 0x2040_3000, 0x183, 0x2004);
        assert!(!base.is_null());
        assert_eq!(free(base, 0, MEM_RELEASE), 1);
        assert_eq!(free(base, 0, MEM_RELEASE), 0);
    }
}
