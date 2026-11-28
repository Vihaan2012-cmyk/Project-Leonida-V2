//! What the title does with the physical page entries `XMemAllocatePhysicalPages` returns.
//!
//! The title keeps `entry >> 4` in a 21-bit table and rebuilds `(stored << 4) & 0x1fffff0` when it maps a page
//! again (RVAs 0x1e6c110, 0x55aa6a0, 0xf2c470), so the identity of a page has to live in bits 4 and up.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;

const PAGE: usize = 64 * 1024;
const TITLE_MASK: usize = 0x1FF_FFF0;

type VirtualAlloc = unsafe extern "win64" fn(*mut c_void, usize, u32, u64, u32) -> *mut c_void;
type Allocate = unsafe extern "win64" fn(usize, *mut usize, *mut usize) -> i32;
type Map = unsafe extern "win64" fn(*mut c_void, usize, *const usize) -> i32;

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

/// The title's table: the stored value, then the entry rebuilt from it.
fn through_the_title(entry: usize) -> usize {
    let stored = (entry >> 4) & 0xFFFF_FFFF;
    (stored << 4) & TITLE_MASK
}

#[test]
fn an_entry_comes_back_unchanged_from_the_title_table() {
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let mut count = 70usize;
    let mut pages = [0usize; 70];
    // SAFETY: the shape the executable uses; the array has room for the count.
    assert_eq!(
        unsafe { allocate(0x180, &mut count, pages.as_mut_ptr()) },
        1
    );
    assert_eq!(count, 70);
    let mut seen = std::collections::HashSet::new();
    for &entry in &pages {
        assert_ne!(entry, 0, "0 names no page");
        assert_eq!(entry & 0xF, 0, "{entry:#x}: the low nibble is not identity");
        assert_eq!(through_the_title(entry), entry);
        assert!(seen.insert(entry), "{entry:#x} was handed out twice");
    }
}

#[test]
fn pages_mapped_again_through_the_title_table_stay_distinct() {
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    const COUNT: usize = 40;
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        let base = reserve(std::ptr::null_mut(), COUNT * PAGE, 0x2040_2000, 0x21, 4) as usize;
        assert_ne!(base, 0);
        let mut count = COUNT;
        let mut pages = [0usize; COUNT];
        assert_eq!(allocate(0x180, &mut count, pages.as_mut_ptr()), 1);
        for (k, &entry) in pages.iter().enumerate() {
            let again = [through_the_title(entry)];
            assert_eq!(map((base + k * PAGE) as *mut c_void, 1, again.as_ptr()), 1);
            ((base + k * PAGE) as *mut u8).write(k as u8 + 1);
        }
        for k in 0..COUNT {
            assert_eq!(
                ((base + k * PAGE) as *const u8).read(),
                k as u8 + 1,
                "page {k} shows another page's byte"
            );
        }
    }
}

#[test]
fn an_entry_of_zero_unmaps_and_succeeds() {
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        let base = reserve(std::ptr::null_mut(), PAGE, 0x2040_2000, 0x21, 4);
        let mut count = 1usize;
        let mut page = [0usize; 1];
        assert_eq!(allocate(0x180, &mut count, page.as_mut_ptr()), 1);
        assert_eq!(map(base, 1, page.as_ptr()), 1);
        (base as *mut u8).write(5);
        assert_eq!(map(base, 1, [0usize].as_ptr()), 1);
        assert_eq!(map(base, 1, page.as_ptr()), 1);
        assert_eq!((base as *const u8).read(), 5, "the page keeps its bytes");
    }
}
