//! The console memory functions with GPU memory hooks installed: what they tell the hooks and where they place
//! GPU-visible memory. The hooks are global, so every test holds `LOCK`.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard};

use xbox_d3d12::memory::hooks::{self, GpuMemoryHooks};
use xbox_d3d12::memory::{MemoryError, SyncPolicy};

const PAGE: u64 = 64 * 1024;

type VirtualAlloc = unsafe extern "win64" fn(*mut c_void, usize, u32, u64, u32) -> *mut c_void;
type Alloc = unsafe extern "win64" fn(usize, u64) -> *mut c_void;
type Free = unsafe extern "win64" fn(*mut c_void, u64);
type Allocate = unsafe extern "win64" fn(usize, *mut usize, *mut usize) -> i32;
type Map = unsafe extern "win64" fn(*mut c_void, usize, *const usize) -> i32;
type Scatter = unsafe extern "win64" fn(*const usize, usize, *const usize) -> i32;
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Mapped(u64, u64, SyncPolicy),
    Pages(u64, u64, SyncPolicy),
    Scatter(Vec<u64>, SyncPolicy),
    Unmapped(u64, u64),
}

struct Recorder {
    events: Mutex<Vec<Event>>,
    window: Mutex<(u64, u64)>,
    fail: Mutex<bool>,
}

impl Recorder {
    fn record(&self, event: Event) -> Result<(), MemoryError> {
        if *self.fail.lock().unwrap() {
            return Err(MemoryError::Unsupported("a test failure"));
        }
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

impl GpuMemoryHooks for Recorder {
    fn mapped(&self, va: u64, len: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.record(Event::Mapped(va, len, policy))
    }
    fn pages_mapped(&self, va: u64, count: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.record(Event::Pages(va, count, policy))
    }
    fn pages_mapped_scatter(&self, a: &[u64], policy: SyncPolicy) -> Result<(), MemoryError> {
        self.record(Event::Scatter(a.to_vec(), policy))
    }
    fn unmapped(&self, va: u64, len: u64) -> Result<(), MemoryError> {
        self.record(Event::Unmapped(va, len))
    }
    fn window(&self) -> (u64, u64) {
        *self.window.lock().unwrap()
    }
}

static LOCK: Mutex<()> = Mutex::new(());

/// Holds the lock, with a recorder installed over `window` until dropped.
struct Installed {
    recorder: Arc<Recorder>,
    _lock: MutexGuard<'static, ()>,
}

impl Installed {
    fn new(window: (u64, u64)) -> Installed {
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let recorder = Arc::new(Recorder {
            events: Mutex::new(Vec::new()),
            window: Mutex::new(window),
            fail: Mutex::new(false),
        });
        hooks::install(recorder.clone());
        Installed {
            recorder,
            _lock: lock,
        }
    }

    fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.recorder.events.lock().unwrap())
    }

    fn fail(&self, on: bool) {
        *self.recorder.fail.lock().unwrap() = on;
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        hooks::uninstall();
    }
}

const GIB: u64 = 1 << 30;

fn inside(window: (u64, u64), base: *mut c_void, len: u64) -> bool {
    let base = base as u64;
    base >= window.0 && base + len <= window.0 + window.1
}

fn below_4_gib(base: *mut c_void, len: u64) -> bool {
    let base = base as u64;
    base >= PAGE && base + len <= 4 * GIB
}

/// The title's 185 MiB reserve holds its first descriptor heap, which shaders reach with 32-bit pointers.
#[test]
fn the_placeholder_the_title_carves_its_descriptor_heap_from_is_placed_below_4_gib() {
    let window = (64 * GIB, 64 * GIB);
    let hooks = Installed::new(window);
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    // SAFETY: the shapes the executable uses.
    unsafe {
        let size = 185 << 20;
        let heap = reserve(std::ptr::null_mut(), size, 0x4_2000, 0x7, 0x1);
        assert!(below_4_gib(heap, size as u64), "{heap:p}");
        assert_eq!(heap as u64 % PAGE, 0);

        let physical = reserve(
            std::ptr::null_mut(),
            4 * PAGE as usize,
            0x2040_2000,
            0x23,
            0x2004,
        );
        assert!(
            inside(window, physical, 4 * PAGE),
            "{physical:p}: a reserve without the placeholder bit"
        );
        let committed = reserve(
            std::ptr::null_mut(),
            2 * PAGE as usize,
            0x2040_3000,
            0x183,
            0x2004,
        );
        assert!(inside(window, committed, 2 * PAGE));
        let cpu_only = reserve(std::ptr::null_mut(), 2 * PAGE as usize, 0x4_2000, 0x1, 0x1);
        assert!(
            !below_4_gib(cpu_only, 2 * PAGE),
            "not GPU-visible: left to Windows"
        );

        let large = reserve(
            std::ptr::null_mut(),
            (1 << 30) + PAGE as usize,
            0x4_2000,
            0x7,
            0x1,
        );
        assert!(
            inside(window, large, (1 << 30) + PAGE),
            "{large:p}: too large to share the low gigabytes"
        );
        assert_eq!(
            hooks.take(),
            vec![Event::Mapped(
                committed as u64,
                2 * PAGE,
                SyncPolicy::WriteWatch
            )]
        );
    }
}

#[test]
fn gpu_allocations_are_placed_in_the_window_reported_and_unreported_on_free() {
    let window = (64 * GIB, 64 * GIB);
    let hooks = Installed::new(window);
    let alloc: Alloc = shim("XMemAlloc");
    let free: Free = shim("XMemFree");
    // SAFETY: plain sizes; the allocation is ours until freed.
    unsafe {
        let base = alloc(100_000, 0x3);
        assert!(inside(window, base, 2 * PAGE), "{base:p}");
        assert_eq!(base as u64 % PAGE, 0);
        (base as *mut u8).add(100_000 - 1).write_volatile(9);
        assert_eq!(
            hooks.take(),
            vec![Event::Mapped(base as u64, 2 * PAGE, SyncPolicy::WriteWatch)]
        );
        free(base, 0x3);
        assert_eq!(hooks.take(), vec![Event::Unmapped(base as u64, 2 * PAGE)]);

        let plain = alloc(4096, 0xf081_4000);
        assert!(!plain.is_null());
        free(plain, 0xf081_4000);
        assert_eq!(hooks.take(), vec![], "a CPU allocation is not the GPU's");
    }
}

#[test]
fn placement_stays_under_the_end_of_a_window_from_zero() {
    let window = (0, 1 << 40);
    let hooks = Installed::new(window);
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let free: Free = shim("XMemFree");
    // SAFETY: the shapes the executable uses.
    unsafe {
        let placeholder = reserve(std::ptr::null_mut(), 16 * PAGE as usize, 0x4_2000, 0x7, 0x1);
        assert!(below_4_gib(placeholder, 16 * PAGE));
        let committed = reserve(
            std::ptr::null_mut(),
            3 * PAGE as usize,
            0x2040_3000,
            0x183,
            0x22004,
        );
        assert!(inside(window, committed, 3 * PAGE));
        (committed as *mut u64).write_volatile(1);
        assert_eq!(
            hooks.take(),
            vec![Event::Mapped(
                committed as u64,
                3 * PAGE,
                SyncPolicy::WriteWatch
            )]
        );
        free(committed, 0x183);
    }
}

#[test]
fn a_window_too_small_fails_the_call_with_a_note() {
    let hooks = Installed::new((GIB << 2, PAGE));
    let alloc: Alloc = shim("XMemAlloc");
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    // SAFETY: plain sizes.
    unsafe {
        assert!(alloc(0x7_0000, 0x3).is_null());
        assert!(
            reserve(std::ptr::null_mut(), 0x9_0000, 0x2040_2000, 0x23, 0x2004).is_null(),
            "a placeholder too"
        );
        assert!(
            reserve(std::ptr::null_mut(), 0xB_0000, 0x2040_3000, 0x283, 0x2004).is_null(),
            "and a committed region"
        );
    }
    assert_eq!(hooks.take(), vec![]);
    let notes = shims::trace::notes().join("\n");
    for size in ["0x70000", "0x90000", "0xb0000"] {
        assert!(
            notes.contains(&format!("no room for {size} bytes of GPU-visible memory")),
            "{notes}"
        );
    }
}

#[test]
fn memory_for_the_cpu_alone_is_left_where_windows_puts_it() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    // SAFETY: as above.
    unsafe {
        let base = reserve(
            std::ptr::null_mut(),
            3 * PAGE as usize,
            0x2040_3000,
            0x181,
            0x4,
        );
        assert!(!base.is_null());
        (base as *mut u64).write_volatile(1);
        let cpu_only = reserve(
            std::ptr::null_mut(),
            2 * PAGE as usize,
            0x2040_2000,
            0x21,
            0x4,
        );
        assert!(!cpu_only.is_null());
    }
    assert_eq!(hooks.take(), vec![]);
}

/// The boot sequence of `tests/xmem.rs`, with each step's reports.
#[test]
fn physical_pages_are_reported_where_mapped_remapped_unmapped_and_freed() {
    let window = (64 * GIB, 64 * GIB);
    let hooks = Installed::new(window);
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    let scatter: Scatter = shim("XMemMapPhysicalPagesScatter");
    let free_pages: FreePages = shim("XMemFreePhysicalPages");
    // SAFETY: the shapes the executable uses; every pointer is valid.
    unsafe {
        let base = reserve(std::ptr::null_mut(), 16 * PAGE as usize, 0x4_2000, 0x7, 0x1);
        assert!(below_4_gib(base, 16 * PAGE));
        let start = base as u64;
        let inner = (start + 4 * PAGE) as *mut c_void;
        assert_eq!(
            reserve(inner, 2 * PAGE as usize, 0x2040_6000, 0x27, 0x2404),
            inner
        );
        assert_eq!(hooks.take(), vec![], "reserving and carving map nothing");

        let mut count = 2usize;
        let mut pages = [0usize; 2];
        assert_eq!(allocate(0x380, &mut count, pages.as_mut_ptr()), 1);
        assert_eq!(map(inner, 2, pages.as_ptr()), 1);
        let inner_va = inner as u64;
        assert_eq!(
            hooks.take(),
            vec![
                Event::Unmapped(inner_va, 2 * PAGE),
                Event::Pages(inner_va, 2, SyncPolicy::Always)
            ]
        );

        assert_eq!(map(inner, 2, std::ptr::null()), 1);
        assert_eq!(hooks.take(), vec![Event::Unmapped(inner_va, 2 * PAGE)]);
        assert_eq!(map(inner, 2, pages.as_ptr()), 1);
        hooks.take();

        let alias = start + 12 * PAGE;
        let alias_address = alias as usize;
        assert_eq!(scatter(&alias_address, 1, pages.as_ptr()), 1);
        assert_eq!(
            hooks.take(),
            vec![
                Event::Unmapped(alias, PAGE),
                Event::Scatter(vec![alias], SyncPolicy::Always)
            ]
        );

        let other = (start + 10 * PAGE) as *mut c_void;
        assert_eq!(reserve(other, PAGE as usize, 0x6000, 0x3, 0x1), other);
        assert_eq!(hooks.take(), vec![], "a reservation maps nothing");
        assert_eq!(reserve(other, PAGE as usize, 0x1000, 0x3, 0x4), other);
        assert_eq!(
            hooks.take(),
            vec![Event::Mapped(other as u64, PAGE, SyncPolicy::WriteWatch)]
        );

        assert_eq!(free_pages(2, pages.as_ptr()), 1);
        assert_eq!(
            hooks.take(),
            vec![
                Event::Unmapped(inner_va, 2 * PAGE),
                Event::Unmapped(alias, PAGE)
            ],
            "freed pages leave the GPU"
        );
    }
}

#[test]
fn committing_inside_a_placeholder_is_reported_and_undone_if_the_gpu_refuses() {
    let window = (64 * GIB, 64 * GIB);
    let hooks = Installed::new(window);
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    // SAFETY: the shapes the executable uses.
    unsafe {
        let base = reserve(
            std::ptr::null_mut(),
            4 * PAGE as usize,
            0x2040_2000,
            0x23,
            0x2004,
        );
        assert!(inside(window, base, 4 * PAGE));
        let at = (base as u64 + 2 * PAGE) as *mut c_void;

        hooks.fail(true);
        assert!(reserve(at, PAGE as usize, 0x2040_1000, 0x23, 0x4).is_null());
        hooks.fail(false);
        assert!(
            shims::trace::notes()
                .iter()
                .any(|n| n.contains("the GPU cannot map"))
        );

        assert_eq!(
            reserve(at, PAGE as usize, 0x2040_1000, 0x23, 0x4),
            at,
            "the placeholder is back"
        );
        (at as *mut u32).write_volatile(5);
        assert_eq!(
            hooks.take(),
            vec![Event::Mapped(at as u64, PAGE, SyncPolicy::Always)]
        );
    }
}

#[test]
fn a_failing_gpu_fails_an_allocation_and_gives_the_memory_back() {
    let hooks = Installed::new((64 * GIB, 64 * GIB));
    let alloc: Alloc = shim("XMemAlloc");
    hooks.fail(true);
    // SAFETY: a plain size.
    assert!(unsafe { alloc(PAGE as usize, 0x3) }.is_null());
}

#[test]
fn nothing_is_reported_or_moved_without_hooks() {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    assert!(hooks::current().is_none());
    let alloc: Alloc = shim("XMemAlloc");
    let free: Free = shim("XMemFree");
    let reserve: VirtualAlloc = shim("XMemVirtualAlloc");
    // SAFETY: plain sizes.
    unsafe {
        let base = alloc(100_000, 0x3);
        assert!(!base.is_null());
        (base as *mut u8).add(99_999).write_volatile(1);
        free(base, 0x3);
        let region = reserve(
            std::ptr::null_mut(),
            2 * PAGE as usize,
            0x2040_3000,
            0x283,
            0x2004,
        );
        assert!(!region.is_null());
        (region as *mut u8).write_volatile(1);
    }
}

#[test]
fn a_window_that_neither_starts_at_zero_nor_ends_on_a_power_of_two_is_honoured() {
    let window = (5 * GIB, 7 * GIB);
    let hooks = Installed::new(window);
    let alloc: Alloc = shim("XMemAlloc");
    let free: Free = shim("XMemFree");
    let mut held = Vec::new();
    // SAFETY: plain sizes; each allocation is freed below.
    unsafe {
        for _ in 0..4 {
            let base = alloc(256 * PAGE as usize, 0x3);
            assert!(inside(window, base, 256 * PAGE), "{base:p}");
            held.push(base);
        }
        for base in held {
            free(base, 0x3);
        }
    }
    assert_eq!(hooks.take().len(), 8);
}
