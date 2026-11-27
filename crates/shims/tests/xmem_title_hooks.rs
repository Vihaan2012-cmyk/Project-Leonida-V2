//! `XMemAlloc` and `XMemFree` through the title's `__XMemAllocationHooks__` export, found in a synthetic image
//! whose table holds hooks written here. The image stands in for the title, as `set_main_module` names it.
#![cfg(windows)]
#![allow(unsafe_code)]

mod common;

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use common::{GIB, Installed, PAGE, shim, shim_of, without_hooks};
use shims::loader::set_main_module;
use xbox_d3d12::heap::console::{ALLOCATOR_ID, DESCRIPTOR_HEAP_ATTRIBUTES, OBJECT_TYPE};
use xbox_d3d12::heap::{ConsolePlacement, HeapAllocator, HeapPlacement, LowBlock, LowMemory};

type Alloc = unsafe extern "win64" fn(usize, u64) -> *mut c_void;
type Free = unsafe extern "win64" fn(*mut c_void, u64);
type VirtualAlloc = unsafe extern "win64" fn(*mut c_void, usize, u32, u64, u32) -> *mut c_void;
type VirtualFree = unsafe extern "win64" fn(*mut c_void, usize, u32) -> i32;
type Allocate = unsafe extern "win64" fn(usize, *mut usize, *mut usize) -> i32;
type Map = unsafe extern "win64" fn(*mut c_void, usize, *const usize) -> i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Alloc(usize, u64),
    Free(usize, u64),
}

static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());
/// The scoped override the title would have pushed: its address and byte size.
static OVERRIDE: Mutex<Option<(usize, usize)>> = Mutex::new(None);

fn calls() -> Vec<Call> {
    std::mem::take(&mut *CALLS.lock().unwrap())
}

fn set_override(to: Option<(usize, usize)>) {
    *OVERRIDE.lock().unwrap() = to;
}

/// The title's allocation hook in miniature: its override answers a descriptor heap request of its size, and
/// everything else reaches `XMemAllocDefault`, as the static node of every bucket does.
extern "win64" fn alloc_hook(size: usize, attributes: u64) -> *mut c_void {
    CALLS.lock().unwrap().push(Call::Alloc(size, attributes));
    let answer = *OVERRIDE.lock().unwrap();
    if let Some((address, bytes)) = answer
        && attributes & 0x3fff == 4
        && size == bytes
    {
        return address as *mut c_void;
    }
    let default: Alloc = shim("XMemAllocDefault");
    // SAFETY: the shim's own signature.
    unsafe { default(size, attributes) }
}

extern "win64" fn free_hook(address: *mut c_void, attributes: u64) {
    CALLS
        .lock()
        .unwrap()
        .push(Call::Free(address as usize, attributes));
    if OVERRIDE
        .lock()
        .unwrap()
        .is_some_and(|(owned, _)| owned == address as usize)
    {
        return;
    }
    let default: Free = shim("XMemFreeDefault");
    // SAFETY: the shim's own signature.
    unsafe { default(address, attributes) }
}

fn put16(image: &mut [u8], at: usize, value: u16) {
    image[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(image: &mut [u8], at: usize, value: u32) {
    image[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_text(image: &mut [u8], at: usize, text: &str) {
    image[at..at + text.len()].copy_from_slice(text.as_bytes());
}

const TABLE_RVA: usize = 0x2000;

/// A PE32+ image in memory with two exports, `Other` and `export`, the second naming a two-pointer table.
/// Returns its base; the memory is kept for the life of the process.
fn image_with_export(export: &str, hooks: [usize; 2]) -> usize {
    let mut image = vec![0u8; 0x3000];
    put16(&mut image, 0, 0x5A4D);
    put32(&mut image, 0x3C, 0x80);
    put32(&mut image, 0x80, 0x0000_4550);
    put16(&mut image, 0x84, 0x8664);
    put16(&mut image, 0x94, 0xF0);
    put16(&mut image, 0x98, 0x20B);
    put32(&mut image, 0x98 + 56, 0x3000);
    put32(&mut image, 0x98 + 108, 16);
    put32(&mut image, 0x98 + 112, 0x1000);
    put32(&mut image, 0x98 + 116, 0x100);
    put32(&mut image, 0x1000 + 16, 1);
    put32(&mut image, 0x1000 + 20, 2);
    put32(&mut image, 0x1000 + 24, 2);
    put32(&mut image, 0x1000 + 28, 0x1040);
    put32(&mut image, 0x1000 + 32, 0x1050);
    put32(&mut image, 0x1000 + 36, 0x1060);
    put32(&mut image, 0x1040, 0x2100);
    put32(&mut image, 0x1044, TABLE_RVA as u32);
    put32(&mut image, 0x1050, 0x1070);
    put32(&mut image, 0x1054, 0x1090);
    put16(&mut image, 0x1062, 1);
    put_text(&mut image, 0x1070, "Other");
    put_text(&mut image, 0x1090, export);
    for (k, hook) in hooks.into_iter().enumerate() {
        let at = TABLE_RVA + 8 * k;
        image[at..at + 8].copy_from_slice(&(hook as u64).to_le_bytes());
    }
    let image: &'static mut [u8] = Box::leak(image.into_boxed_slice());
    image.as_ptr() as usize
}

fn title_with_hooks() -> usize {
    let base = image_with_export(
        shims::xmem::EXPORT_NAME,
        [
            alloc_hook as *const () as usize,
            free_hook as *const () as usize,
        ],
    );
    set_main_module(base);
    calls();
    set_override(None);
    base
}

#[test]
fn xmem_alloc_calls_the_hook_with_size_and_attributes_and_returns_its_address() {
    let _lock = without_hooks();
    title_with_hooks();
    let alloc: Alloc = shim("XMemAlloc");
    let answer = 0x1234_0000usize;
    set_override(Some((answer, 0x4000)));
    // SAFETY: the hook answers with an address it does not hand to anything else.
    let got = unsafe { alloc(0x4000, DESCRIPTOR_HEAP_ATTRIBUTES) };
    assert_eq!(got as usize, answer);
    assert_eq!(calls(), vec![Call::Alloc(0x4000, 0x0080_0004)]);
    assert_eq!(
        DESCRIPTOR_HEAP_ATTRIBUTES,
        (ALLOCATOR_ID << 16) | OBJECT_TYPE
    );
    set_main_module(0);
}

#[test]
fn a_request_the_hook_does_not_answer_ends_in_the_default_allocation() {
    let _lock = without_hooks();
    title_with_hooks();
    let alloc: Alloc = shim("XMemAlloc");
    let free: Free = shim("XMemFree");
    set_override(Some((0x1234_0000, 0x4000)));
    // SAFETY: a plain allocation, written and freed through the same entry points.
    unsafe {
        let got = alloc(0x2000, 0xf081_4000);
        assert!(!got.is_null());
        (got as *mut u8).add(0x1FFF).write_volatile(1);
        assert_eq!(calls(), vec![Call::Alloc(0x2000, 0xf081_4000)]);
        free(got, 0xf081_4000);
        assert_eq!(calls(), vec![Call::Free(got as usize, 0xf081_4000)]);
    }
    set_main_module(0);
}

#[test]
fn xmem_free_runs_the_free_hook_and_an_override_keeps_the_title_memory() {
    let _lock = without_hooks();
    title_with_hooks();
    let free: Free = shim("XMemFree");
    set_override(Some((0x1234_0000, 0x4000)));
    // SAFETY: the hook returns without touching the address when it is the override's.
    unsafe { free(0x1234_0000 as *mut c_void, DESCRIPTOR_HEAP_ATTRIBUTES) };
    assert_eq!(calls(), vec![Call::Free(0x1234_0000, 0x0080_0004)]);
    set_main_module(0);
}

#[test]
fn the_default_functions_do_not_run_the_hook_again() {
    let _lock = without_hooks();
    title_with_hooks();
    let alloc: Alloc = shim("XMemAllocDefault");
    let free: Free = shim("XMemFreeDefault");
    // SAFETY: a plain allocation.
    unsafe {
        let got = alloc(0x1000, 0xf081_4000);
        assert!(!got.is_null());
        free(got, 0xf081_4000);
    }
    assert_eq!(calls(), vec![]);
    set_main_module(0);
}

#[test]
fn xmem_allocates_itself_where_the_title_has_no_hook() {
    let _lock = without_hooks();
    let alloc: Alloc = shim("XMemAlloc");
    let free: Free = shim("XMemFree");
    let check = |what: &str| {
        // SAFETY: a plain allocation, freed again.
        unsafe {
            let got = alloc(0x3000, 0xf081_4000);
            assert!(!got.is_null(), "{what}");
            (got as *mut u8).add(0x2FFF).write_volatile(1);
            free(got, 0xf081_4000);
        }
        assert_eq!(calls(), vec![], "{what}");
    };
    calls();
    set_main_module(0);
    check("no title");
    let hooks = [
        alloc_hook as *const () as usize,
        free_hook as *const () as usize,
    ];
    set_main_module(image_with_export("SomethingElse", hooks));
    check("an image without the export");
    set_main_module(image_with_export(shims::xmem::EXPORT_NAME, [0, 0]));
    check("null entries in the table");
    set_main_module(0);
}

struct Low {
    address: u64,
    asked: Mutex<Vec<u64>>,
}

impl LowMemory for Low {
    fn alloc_low(&self, bytes: u64) -> Option<LowBlock> {
        self.asked.lock().unwrap().push(bytes);
        Some(LowBlock {
            address: self.address,
            keep: Box::new(()),
        })
    }
}

/// The title's first descriptor heap: reserve, split, carve, map, then the heap at the address it stored, which
/// it asserts the heap's start handle equals.
#[test]
fn the_console_placement_puts_the_first_heap_where_the_title_says() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    title_with_hooks();
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let split: VirtualFree = shim_of("api-ms-win-core-memory-l1-1-0.dll", "VirtualFree");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    let page = PAGE as usize;
    let heap_bytes = 4 * page;
    // SAFETY: the shapes the executable uses; every pointer is valid.
    let stored = unsafe {
        let base = reserve(std::ptr::null_mut(), 6 * page, 0x4_2000, 0x7, 0x1);
        assert!(!base.is_null());
        assert_eq!(split(base, heap_bytes, 0x8002), 1);
        assert_eq!(
            reserve(base, heap_bytes, 0x2040_6000, 0x27, 0x1004),
            base,
            "carved from the split the title made itself"
        );
        let mut count = 4usize;
        let mut pages = [0usize; 4];
        assert_eq!(allocate(0x280, &mut count, pages.as_mut_ptr()), 1);
        assert_eq!(map(base, 4, pages.as_ptr()), 1);
        base as u64
    };
    assert!(stored + 6 * PAGE <= 1 << 32, "the reserve is under 4 GiB");
    hooks.take();
    set_override(Some((stored as usize, heap_bytes)));
    calls();

    let low = Arc::new(Low {
        address: 0x7000_0000,
        asked: Mutex::new(Vec::new()),
    });
    let placement = ConsolePlacement::new(shims::xmem::title_heap_allocator(), low.clone());
    let placed = placement.place(heap_bytes, true).expect("placed");
    assert_eq!(
        placed.cpu, stored,
        "the start handle equals the title's address"
    );
    assert_eq!(placed.gpu, stored, "one address serves both on the console");
    assert_eq!(calls(), vec![Call::Alloc(heap_bytes, 0x0080_0004)]);
    assert!(low.asked.lock().unwrap().is_empty());

    let other = placement
        .place(0x1000, false)
        .expect("placed in low memory");
    assert_eq!(
        other.cpu, 0x7000_0000,
        "no override: the heap goes to low memory"
    );
    assert_eq!(*low.asked.lock().unwrap(), vec![0x1000]);
    assert_eq!(
        calls(),
        vec![Call::Alloc(0x1000, 0x0080_0004)],
        "the hook chain's own allocation is not the heap's"
    );
    drop(placed);
    drop(other);
    assert_eq!(
        calls(),
        vec![],
        "the title's memory is not freed with the heap"
    );
    set_main_module(0);
}

#[test]
fn the_allocator_answers_nothing_without_a_hook() {
    let _lock = without_hooks();
    set_main_module(0);
    let allocator = shims::xmem::title_heap_allocator();
    assert!(
        allocator
            .allocate(0x4000, DESCRIPTOR_HEAP_ATTRIBUTES)
            .is_none()
    );
}
