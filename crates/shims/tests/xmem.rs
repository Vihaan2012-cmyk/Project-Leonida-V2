//! The console memory functions, called the way the executable calls them.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;

const PAGE: usize = 64 * 1024;

type VirtualAlloc = unsafe extern "win64" fn(*mut c_void, usize, u32, u64, u32) -> *mut c_void;
type Allocate = unsafe extern "win64" fn(usize, *mut usize, *mut usize) -> i32;
type Map = unsafe extern "win64" fn(*mut c_void, usize, *const usize) -> i32;
type Scatter = unsafe extern "win64" fn(*const usize, usize, *const usize) -> i32;
type Free = unsafe extern "win64" fn(usize, *const usize) -> i32;

fn shim<T: Copy>(name: &str) -> T {
    let address = shims::builtin()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("{name} registered"))
        .address;
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `T` is the function pointer type matching the shim's measured shape.
    unsafe { std::mem::transmute_copy(&address) }
}

#[test]
fn physical_pages_map_into_reservations_and_alias() {
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    let scatter: Scatter = shim("XMemMapPhysicalPagesScatter");
    let free: Free = shim("XMemFreePhysicalPages");
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        let base = reserve(std::ptr::null_mut(), 4 * PAGE, 0x2040_2000, 0x23, 0x2004);
        assert!(!base.is_null());
        let mut count = 2usize;
        let mut pages = [0usize; 2];
        assert_eq!(allocate(0x280, &mut count, pages.as_mut_ptr()), 1);
        assert_eq!(count, 2);
        assert_eq!(map(base, 2, pages.as_ptr()), 1);
        let first = base as *mut u32;
        first.write_volatile(0xC0FFEE);
        first.add(PAGE / 4).write_volatile(0xBEEF);
        let alias = base as usize + 3 * PAGE;
        assert_eq!(scatter(&alias, 1, pages.as_ptr()), 1);
        assert_eq!(
            (alias as *const u32).read_volatile(),
            0xC0FFEE,
            "the same page"
        );
        assert_eq!(map(base, 2, std::ptr::null()), 1);
        assert_eq!(map(base, 2, pages.as_ptr()), 1);
        assert_eq!(
            first.add(PAGE / 4).read_volatile(),
            0xBEEF,
            "pages keep their contents"
        );
        let committed = base as usize + 2 * PAGE;
        let got = reserve(committed as *mut c_void, PAGE, 0x2040_1000, 0x23, 0x4);
        assert_eq!(got as usize, committed, "commit inside the reservation");
        (got as *mut u32).write_volatile(5);
        assert_eq!(free(2, pages.as_ptr()), 1);
    }
}

#[test]
fn ordinary_allocations_drop_console_bits() {
    let alloc: VirtualAlloc = shim("XMemVirtualAlloc");
    // SAFETY: as above.
    unsafe {
        let base = alloc(std::ptr::null_mut(), 3 * PAGE, 0x2040_3000, 0x181, 0x4);
        assert!(!base.is_null());
        (base as *mut u64).write_volatile(1);
    }
    let tools: unsafe extern "win64" fn() -> i32 = shim("XMemIsToolsMemAvailable");
    // SAFETY: no arguments.
    assert_eq!(unsafe { tools() }, 0);
}
