//! Writing bytes into physical pages from outside the game's code, as DirectStorage reads do.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;

use shims::xmem::write_physical_page;

const PAGE: usize = 64 * 1024;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;

type VirtualAlloc = unsafe extern "win64" fn(*mut c_void, usize, u32, u64, u32) -> *mut c_void;
type Allocate = unsafe extern "win64" fn(usize, *mut usize, *mut usize) -> i32;
type Map = unsafe extern "win64" fn(*mut c_void, usize, *const usize) -> i32;
type FreePages = unsafe extern "win64" fn(usize, *const usize) -> i32;

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

fn bytes<'a>(at: usize, offset: usize, length: usize) -> &'a [u8] {
    // SAFETY: the callers read mapped pages of their own reservations.
    unsafe { std::slice::from_raw_parts((at + offset) as *const u8, length) }
}

#[test]
fn a_write_shows_in_every_view_of_the_page() {
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        let cpu = reserve(std::ptr::null_mut(), 2 * PAGE, 0x2040_2000, 0x23, 0x2004) as usize;
        let graphics = reserve(std::ptr::null_mut(), 2 * PAGE, 0x2040_2000, 0x23, 0x2004) as usize;
        assert!(cpu != 0 && graphics != 0);
        let mut count = 2usize;
        let mut pages = [0usize; 2];
        assert_eq!(allocate(0x280, &mut count, pages.as_mut_ptr()), 1);
        assert_eq!(map(cpu as *mut c_void, 2, pages.as_ptr()), 1);
        assert_eq!(map(graphics as *mut c_void, 2, pages.as_ptr()), 1);

        let data: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8 + 1).collect();
        assert_eq!(
            write_physical_page(pages[1] as u64, 0xFF00, &data[..0x100]),
            0
        );
        assert_eq!(
            write_physical_page(pages[1] as u64, 0xFF80, &data[..0x80]),
            0
        );
        assert_eq!(bytes(cpu, PAGE + 0xFF00, 0x80), &data[..0x80]);
        assert_eq!(bytes(graphics, PAGE + 0xFF00, 0x80), &data[..0x80]);
        assert_eq!(bytes(cpu, PAGE + 0xFF80, 0x80), &data[..0x80]);
        assert_eq!(bytes(graphics, PAGE + 0xFF80, 0x80), &data[..0x80]);
        assert_eq!(bytes(cpu, 0, 16), &[0; 16], "the other page is untouched");

        // The same page named by the address of a view.
        assert_eq!(write_physical_page(cpu as u64, 8, &[0xAB; 4]), 0);
        assert_eq!(bytes(graphics, 8, 4), &[0xAB; 4]);
    }
}

#[test]
fn a_page_with_no_view_is_written_through_its_section() {
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        let base = reserve(std::ptr::null_mut(), PAGE, 0x2040_2000, 0x23, 0x2004);
        let mut count = 1usize;
        let mut page = [0usize; 1];
        assert_eq!(allocate(0x280, &mut count, page.as_mut_ptr()), 1);
        assert_eq!(write_physical_page(page[0] as u64, 100, &[5, 6, 7]), 0);
        assert_eq!(map(base, 1, page.as_ptr()), 1);
        assert_eq!(bytes(base as usize, 100, 3), &[5, 6, 7]);
    }
}

#[test]
fn entries_that_name_no_page_and_ranges_past_the_page_fail() {
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let free: FreePages = shim("XMemFreePhysicalPages");
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        let mut count = 1usize;
        let mut page = [0usize; 1];
        assert_eq!(allocate(0x280, &mut count, page.as_mut_ptr()), 1);
        let live = page[0] as u64;
        assert_eq!(write_physical_page(0, 0, &[1]), E_INVALIDARG);
        assert_eq!(write_physical_page(0xFFFF_0000_0000, 0, &[1]), E_INVALIDARG);
        assert_eq!(write_physical_page(live, PAGE - 1, &[1, 2]), E_INVALIDARG);
        assert_eq!(write_physical_page(live, PAGE + 1, &[]), E_INVALIDARG);
        assert_eq!(write_physical_page(live, PAGE - 1, &[1]), 0);
        assert_eq!(write_physical_page(live, 0, &[]), 0);
        assert_eq!(free(1, page.as_ptr()), 1);
        assert_eq!(write_physical_page(live, 0, &[1]), E_INVALIDARG, "freed");
    }
}
