//! The GDK's memory functions.
//!
//! `XMemAlloc(size, attributes)` and `XMemFree(address, attributes)`, and their `Default` forms, follow the
//! public signatures. The rest have no public documentation; their shapes were measured from how the
//! executable calls them:
//!
//! - `XMemVirtualAlloc(address, size, allocation_type, attributes, protect)`: `VirtualAlloc` with an extra
//!   attribute word, console bits in the allocation type (bit 29 and bit 22, the 64 KiB page mode) and
//!   graphics bits in the protection (0x400 and up). A reservation without commit becomes a placeholder, so
//!   physical pages can be mapped into it; the console bits and the graphics protections are dropped.
//! - `XMemAllocatePhysicalPages(flags, &count, pages)`, `XMemMapPhysicalPages(address, count, pages)` (a null
//!   `pages` unmaps), `XMemMapPhysicalPagesScatter(addresses, count, pages)` and
//!   `XMemFreePhysicalPages(count, pages)`: Windows' AWE functions without the process handle. Each physical
//!   page is a 64 KiB section of the page file, since AWE needs a privilege ordinary users lack; its number is
//!   an index into our own table.
//!   `write_physical_page` is how DirectStorage reads land in those pages. The page-table entry of such a
//!   request is taken as a page number from `XMemAllocatePhysicalPages`, or as the address of a mapped view of
//!   a page; whether the title stores one or the other (or a console physical address) is not measured.
//! - `XMemIsToolsMemAvailable()`: there is no development kit memory, so false.
//! - `XMemGetWorkingSetStatistics(kind, stats)`: `BOOL`. The executable passes 0x2000000a or 0x20000001 and
//!   the upscaler 0x20000001, with a buffer of at least 0x80 bytes; the callers read a 64-bit size at +8.
//!   The first 16 bytes are written (the title's memory limit at +0, its working set at +8), the rest is
//!   left alone because nothing measured what it holds.
//! - `XMemTransferMemory(direction, size)`: moves budget between the title's and the system's partitions on
//!   the console; a PC has one pool, so it succeeds and moves nothing. Each distinct pair is traced.
//!
//! `XMemAlloc` and `XMemFree` run the title's allocation hooks (`title_hooks`), and `XMemAllocDefault` and
//! `XMemFreeDefault`, where the hook chains end, do the allocating.
//!
//! With `xbox_d3d12::memory::hooks` installed, memory the GPU reads is placed inside the hooks' window and
//! reported as it is committed, mapped, unmapped and freed. The title frees such memory with `VirtualFree` as
//! well; `virtual_free` serves that import and reports what it releases. A placeholder reserve that may back a
//! descriptor heap is placed below 4 GiB instead (`backs_descriptor_heap`). Memory counts as GPU-visible when the attribute word has bit 0x2 or the
//! protection has a `PAGE_GRAPHICS_*` bit: bit 0x2 is set in every one of the 23 measured calls whose protection
//! has one (and in the placeholder the physical reserves are carved from) and clear in the two that have none
//! (0x181 and 0x21). That is a correlation across the measured constants, not a documented meaning.

use crate::Shim;

#[cfg(windows)]
mod title_hooks;
#[cfg(windows)]
mod virtual_free;

#[cfg(windows)]
pub use imp::write_physical_page;
#[cfg(windows)]
pub use title_hooks::{EXPORT_NAME, TitleHeapAllocator, title_heap_allocator};

pub const PAGE: usize = 64 * 1024;

#[cfg(windows)]
mod imp {
    use std::collections::{BTreeMap, HashSet};
    use std::ffi::c_void;
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_WRITE, MEM_ADDRESS_REQUIREMENTS, MEM_COMMIT, MEM_DECOMMIT,
        MEM_EXTENDED_PARAMETER, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEM_REPLACE_PLACEHOLDER,
        MEM_RESERVE, MEM_RESERVE_PLACEHOLDER, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        MapViewOfFile3, MemExtendedParameterAddressRequirements, PAGE_NOACCESS, PAGE_READWRITE,
        SEC_COMMIT, UnmapViewOfFile, UnmapViewOfFile2, VirtualAlloc, VirtualAlloc2, VirtualFree,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use xbox_d3d12::memory::hooks::{self, GpuMemoryHooks};
    use xbox_d3d12::memory::{LOW_LIMIT, SyncPolicy};

    use super::{PAGE, title_hooks};
    use crate::trace::note;

    const TRUE: i32 = 1;
    const FALSE: i32 = 0;
    const E_FAIL: i32 = 0x8000_4005u32 as i32;
    const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
    const MEM_TOP_DOWN: u32 = 0x10_0000;
    const MEM_PHYSICAL: u32 = 0x40_0000;
    const MEM_WRITE_WATCH: u32 = 0x20_0000;

    /// Set in the attribute word of every measured call that maps graphics memory or reserves what physical pages
    /// are mapped into, and clear in the two calls that only hold CPU data (0x181, 0x21). Inferred from those
    /// 23 call sites; nothing documents it.
    const GPU_ATTRIBUTE: u64 = 0x2;
    /// `PAGE_GRAPHICS_NOACCESS`, `_READONLY`, `_READWRITE`, `_EXECUTE_READ`, `_COHERENT` and `_NOCACHE`.
    const GRAPHICS_PROTECT: u32 = 0x800 | 0x1000 | 0x2000 | 0x8000 | 0x2_0000 | 0x4_0000;
    /// The last address a process can use.
    const USER_END: u64 = 0x7FFF_FFFE_FFFF;

    /// Whether the title meant the GPU to read this memory.
    fn gpu_visible(attributes: u64, protect: u32) -> bool {
        attributes & GPU_ATTRIBUTE != 0 || protect & GRAPHICS_PROTECT != 0
    }

    pub(super) struct State {
        /// Placeholder reservations: start to length.
        pub(super) placeholders: BTreeMap<usize, usize>,
        /// Physical pages by number less one; `None` once freed.
        pages: Vec<Option<usize>>,
        /// Mapped addresses and the page there.
        views: BTreeMap<usize, usize>,
        /// Reservations of GPU-visible memory made while hooks were installed: start to length.
        pub(super) gpu_reserved: BTreeMap<usize, usize>,
        /// Committed regions reported to the hooks as mapped, to report them unmapped when freed.
        pub(super) gpu_mapped: BTreeMap<usize, usize>,
    }

    pub(super) static STATE: Mutex<State> = Mutex::new(State {
        placeholders: BTreeMap::new(),
        pages: Vec::new(),
        views: BTreeMap::new(),
        gpu_reserved: BTreeMap::new(),
        gpu_mapped: BTreeMap::new(),
    });

    /// `XMemAlloc`: the title's allocation hook when the executable has one, else the allocation the hook chain
    /// ends in. The hook is what a console `XMemAlloc` runs (`title_hooks` has the measurements).
    pub extern "win64" fn alloc(size: usize, attributes: u64) -> *mut c_void {
        match title_hooks::current().and_then(|hooks| hooks.alloc) {
            // SAFETY: a hook of the loaded title, called as the console's `XMemAlloc` calls it.
            Some(hook) => unsafe { hook(size, attributes) },
            None => alloc_default(size, attributes),
        }
    }

    /// `XMemFree`: through the title's free hook when there is one.
    pub extern "win64" fn free(address: *mut c_void, attributes: u64) {
        match title_hooks::current().and_then(|hooks| hooks.free) {
            // SAFETY: as in `alloc`.
            Some(hook) => unsafe { hook(address, attributes) },
            None => free_default(address, attributes),
        }
    }

    /// `XMemAllocDefault`: the allocation itself, which the last node of every hook chain calls.
    pub extern "win64" fn alloc_default(size: usize, attributes: u64) -> *mut c_void {
        let address = allocate(size, attributes);
        title_hooks::note_default_allocation(address as usize);
        address
    }

    fn allocate(size: usize, attributes: u64) -> *mut c_void {
        if attributes & GPU_ATTRIBUTE != 0
            && let Some(hooks) = hooks::current()
        {
            return gpu_virtual_alloc(
                hooks.as_ref(),
                std::ptr::null_mut(),
                size.max(1),
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            );
        }
        // SAFETY: a plain allocation; null on failure, as XMemAlloc reports it.
        unsafe {
            VirtualAlloc(
                std::ptr::null(),
                size.max(1),
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        }
    }

    /// `XMemFreeDefault`.
    pub extern "win64" fn free_default(address: *mut c_void, _attributes: u64) {
        if address.is_null() {
            return;
        }
        let mapped = STATE
            .lock()
            .ok()
            .and_then(|mut state| state.gpu_mapped.remove(&(address as usize)));
        if let Some(length) = mapped
            && let Some(hooks) = hooks::current()
        {
            report_unmapped(hooks.as_ref(), address as usize, length);
        }
        // SAFETY: `address` came from `alloc`, which only hands out allocation bases.
        unsafe { VirtualFree(address, 0, MEM_RELEASE) };
    }

    pub(super) fn report_mapped(
        hooks: &dyn GpuMemoryHooks,
        at: usize,
        length: usize,
        policy: SyncPolicy,
    ) -> bool {
        match hooks.mapped(at as u64, length as u64, policy) {
            Ok(()) => true,
            Err(e) => {
                note(format!(
                    "the GPU cannot map {at:#x}+{length:#x} of game memory: {e}"
                ));
                false
            }
        }
    }

    pub(super) fn report_unmapped(hooks: &dyn GpuMemoryHooks, at: usize, length: usize) {
        if let Err(e) = hooks.unmapped(at as u64, length as u64) {
            note(format!(
                "the GPU cannot unmap {at:#x}+{length:#x} of game memory: {e}"
            ));
        }
    }

    /// Reserves `size` bytes inside the window; null if there is no room.
    fn place(window: (u64, u64), size: usize, flags: u32, protect: u32) -> *mut c_void {
        let (first, length) = window;
        if length == 0 || size as u64 > length {
            return std::ptr::null_mut();
        }
        let last = first.saturating_add(length - 1).min(USER_END);
        let mut requirements = MEM_ADDRESS_REQUIREMENTS {
            LowestStartingAddress: first as usize as *mut c_void,
            HighestEndingAddress: last as usize as *mut c_void,
            Alignment: 0,
        };
        let mut parameter = MEM_EXTENDED_PARAMETER::default();
        parameter.Anonymous1._bitfield = MemExtendedParameterAddressRequirements as u64;
        parameter.Anonymous2.Pointer = (&mut requirements as *mut MEM_ADDRESS_REQUIREMENTS).cast();
        // SAFETY: `requirements` and `parameter` outlive the call, which only reads them. Top down keeps the low
        // addresses, where descriptor heaps must go, free.
        let base = unsafe {
            VirtualAlloc2(
                GetCurrentProcess(),
                std::ptr::null(),
                size,
                flags | MEM_TOP_DOWN,
                protect,
                &mut parameter,
                1,
            )
        };
        if !base.is_null() && base as u64 + size as u64 - 1 > last {
            // SAFETY: the allocation just made.
            unsafe { VirtualFree(base, 0, MEM_RELEASE) };
            return std::ptr::null_mut();
        }
        base
    }

    /// The allocation type a title passes to ask for a placeholder it carves itself (`MEM_RESERVE_PLACEHOLDER`).
    const TITLE_PLACEHOLDER: u32 = MEM_RESERVE_PLACEHOLDER;
    /// The largest reserve moved below 4 GiB. A judgement, not a measurement: the one measured reserve is 185 MiB
    /// and the whole low gigabytes are shared with every descriptor heap.
    const LOW_RESERVE_LIMIT: usize = 1 << 30;

    /// Whether a GPU-visible reserve may back a descriptor heap, which the shaders reach with 32-bit pointers.
    ///
    /// Of the 23 `XMemVirtualAlloc` call sites in the 1.0.2.1 executable, exactly one asks for a placeholder
    /// (allocation type 0x42000, attributes 7, protection 1; `sub_01e86440`). It is the 185 MiB reserve the
    /// title splits with `VirtualFree` into five ranges and re-reserves with `0x20406000`; the heap of the first
    /// `CreateDescriptorHeap` is its first three ranges, at the address stored at 0x75df188. Every other site
    /// reserves for itself, commits, or carves inside a placeholder that already exists. The rule is therefore
    /// the placeholder bit, with no console bits besides, up to `LOW_RESERVE_LIMIT`; anything else keeps the
    /// window placement.
    fn backs_descriptor_heap(allocation_type: u32, size: usize) -> bool {
        allocation_type & TITLE_PLACEHOLDER != 0
            && allocation_type & (MEM_COMMIT | MEM_PHYSICAL) == 0
            && size <= LOW_RESERVE_LIMIT
    }

    /// Reserves below 4 GiB, from the top, away from where the resource step hands out its own low addresses.
    fn place_low(size: usize, flags: u32, protect: u32) -> *mut c_void {
        let base = place((PAGE as u64, LOW_LIMIT - PAGE as u64), size, flags, protect);
        if base.is_null() {
            note(format!(
                "no room below 4 GiB for {size:#x} bytes the title may put a descriptor heap in"
            ));
        }
        base
    }

    fn place_gpu(hooks: &dyn GpuMemoryHooks, size: usize, flags: u32, protect: u32) -> *mut c_void {
        let window = hooks.window();
        let base = place(window, size, flags, protect);
        if base.is_null() {
            note(format!(
                "no room for {size:#x} bytes of GPU-visible memory in the window {:#x}+{:#x}",
                window.0, window.1
            ));
        }
        base
    }

    /// An allocation the GPU reads: inside the window, whole 64 KiB pages when the address is ours to choose,
    /// reported mapped once committed.
    fn gpu_virtual_alloc(
        hooks: &dyn GpuMemoryHooks,
        address: *mut c_void,
        size: usize,
        kind: u32,
        access: u32,
    ) -> *mut c_void {
        // A commit with no address reserves too (Windows does so implicitly), so it is placed like a reserve.
        let reserve = kind & MEM_RESERVE != 0 || address.is_null();
        let flags = if reserve {
            kind | MEM_RESERVE | MEM_WRITE_WATCH
        } else {
            kind
        };
        let placed = address.is_null() && reserve;
        let length = if address.is_null() {
            size.next_multiple_of(PAGE)
        } else {
            size
        };
        let base = if placed {
            place_gpu(hooks, length, flags, access)
        } else {
            // SAFETY: an ordinary allocation.
            unsafe { VirtualAlloc(address, length, flags, access) }
        };
        if base.is_null() {
            return base;
        }
        let at = if address.is_null() {
            base as usize
        } else {
            address as usize
        };
        if kind & MEM_COMMIT != 0 {
            if !report_mapped(hooks, at, length, SyncPolicy::WriteWatch) {
                // SAFETY: undoes the allocation just made.
                unsafe {
                    if reserve {
                        VirtualFree(base, 0, MEM_RELEASE);
                    } else {
                        VirtualFree(address, length, MEM_DECOMMIT);
                    }
                }
                return std::ptr::null_mut();
            }
            if let Ok(mut state) = STATE.lock() {
                state.gpu_mapped.insert(at, length);
            }
        }
        note_gpu_reserved(at, length);
        base
    }

    fn note_gpu_reserved(start: usize, size: usize) {
        if !gpu_reserved(start, size)
            && let Ok(mut state) = STATE.lock()
        {
            state.gpu_reserved.insert(start, size);
        }
    }

    fn gpu_reserved(start: usize, size: usize) -> bool {
        STATE
            .lock()
            .is_ok_and(|state| covers(&state.gpu_reserved, start, size))
    }

    fn covers(ranges: &BTreeMap<usize, usize>, start: usize, size: usize) -> bool {
        ranges
            .range(..=start)
            .next_back()
            .is_some_and(|(&base, &length)| start + size <= base + length)
    }

    /// Turns a committed region that replaced part of a placeholder back into a placeholder.
    fn release_to_placeholder(start: usize, size: usize) {
        // SAFETY: undoes a replacement of our own placeholder.
        unsafe {
            VirtualFree(
                start as *mut c_void,
                size,
                MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
            )
        };
        if let Ok(mut state) = STATE.lock() {
            state.placeholders.insert(start, size);
        }
    }

    pub extern "win64" fn virtual_alloc(
        address: *mut c_void,
        size: usize,
        allocation_type: u32,
        attributes: u64,
        protect: u32,
    ) -> *mut c_void {
        let hooks = hooks::current();
        let gpu = hooks.is_some() && gpu_visible(attributes, protect);
        let kind = allocation_type & (MEM_COMMIT | MEM_RESERVE | MEM_TOP_DOWN);
        let access = match protect & 0xFF {
            0 => PAGE_READWRITE,
            access => access,
        };
        let reserve_only = kind & MEM_COMMIT == 0 && kind & MEM_RESERVE != 0;
        if reserve_only && !address.is_null() && inside_placeholder(address as usize, size) {
            if allocation_type & MEM_PHYSICAL != 0 {
                return if carve(address as usize, size) {
                    if gpu {
                        note_gpu_reserved(address as usize, size);
                    }
                    address
                } else {
                    std::ptr::null_mut()
                };
            }
            if carve(address as usize, size) {
                // SAFETY: a reservation replacing a placeholder carved to exactly this range.
                let base = unsafe {
                    VirtualAlloc2(
                        GetCurrentProcess(),
                        address,
                        size,
                        MEM_RESERVE | MEM_REPLACE_PLACEHOLDER,
                        PAGE_NOACCESS,
                        std::ptr::null_mut(),
                        0,
                    )
                };
                replaced(base, address);
                if gpu && !base.is_null() {
                    note_gpu_reserved(address as usize, size);
                }
                return base;
            }
        }
        if reserve_only && size.is_multiple_of(PAGE) && (address as usize).is_multiple_of(PAGE) {
            let base = match hooks.as_deref() {
                Some(hooks) if gpu && address.is_null() => {
                    let flags = MEM_RESERVE | MEM_RESERVE_PLACEHOLDER;
                    let low = if backs_descriptor_heap(allocation_type, size) {
                        place_low(size, flags, PAGE_NOACCESS)
                    } else {
                        std::ptr::null_mut()
                    };
                    if low.is_null() {
                        place_gpu(hooks, size, flags, PAGE_NOACCESS)
                    } else {
                        low
                    }
                }
                // SAFETY: a placeholder reservation, at the program's address if it named one.
                _ => unsafe {
                    VirtualAlloc2(
                        GetCurrentProcess(),
                        address,
                        size,
                        MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                        PAGE_NOACCESS,
                        std::ptr::null_mut(),
                        0,
                    )
                },
            };
            if !base.is_null() {
                if let Ok(mut state) = STATE.lock() {
                    state.placeholders.insert(base as usize, size);
                }
                if gpu {
                    note_gpu_reserved(base as usize, size);
                }
                return base;
            }
        }
        if kind & MEM_COMMIT != 0
            && !address.is_null()
            && inside_placeholder(address as usize, size)
            && carve(address as usize, size)
        {
            // SAFETY: committed memory replacing a placeholder carved to exactly this range.
            let base = unsafe {
                VirtualAlloc2(
                    GetCurrentProcess(),
                    address,
                    size,
                    MEM_RESERVE | MEM_COMMIT | MEM_REPLACE_PLACEHOLDER,
                    access,
                    std::ptr::null_mut(),
                    0,
                )
            };
            replaced(base, address);
            if !base.is_null()
                && let Some(hooks) = hooks.as_deref()
                && (gpu || gpu_reserved(address as usize, size))
            {
                if !report_mapped(hooks, address as usize, size, SyncPolicy::Always) {
                    release_to_placeholder(address as usize, size);
                    return std::ptr::null_mut();
                }
                if let Ok(mut state) = STATE.lock() {
                    state.gpu_mapped.insert(address as usize, size);
                }
            }
            return base;
        }
        if let Some(hooks) = hooks.as_deref()
            && (gpu || (!address.is_null() && gpu_reserved(address as usize, size)))
        {
            let base = gpu_virtual_alloc(hooks, address, size, kind, access);
            if base.is_null() {
                note(format!(
                    "XMemVirtualAlloc of {size:#x} bytes at {address:p} (type {allocation_type:#x}, attributes                      {attributes:#x}, protect {protect:#x}) failed"
                ));
            }
            return base;
        }
        // SAFETY: an ordinary allocation.
        let base = unsafe { VirtualAlloc(address, size, kind, access) };
        if base.is_null() {
            note(format!(
                "XMemVirtualAlloc of {size:#x} bytes at {address:p} (type {allocation_type:#x}, attributes \
                 {attributes:#x}, protect {protect:#x}) failed"
            ));
        }
        base
    }

    /// Forgets the placeholder at `address` once an allocation replaced it.
    fn replaced(base: *mut c_void, address: *mut c_void) {
        if !base.is_null()
            && let Ok(mut state) = STATE.lock()
        {
            state.placeholders.remove(&(address as usize));
        }
    }

    fn inside_placeholder(start: usize, size: usize) -> bool {
        STATE.lock().is_ok_and(|state| {
            state
                .placeholders
                .range(..=start)
                .next_back()
                .is_some_and(|(&base, &length)| start + size <= base + length)
        })
    }

    /// Splits the placeholder holding `start..start + size` so that range is a placeholder of its own.
    fn carve(start: usize, size: usize) -> bool {
        let Ok(mut state) = STATE.lock() else {
            return false;
        };
        let Some((&base, &length)) = state.placeholders.range(..=start).next_back() else {
            return false;
        };
        if start + size > base + length {
            return false;
        }
        // SAFETY: splits our own placeholder reservation.
        let split = |at: usize, part: usize| unsafe {
            VirtualFree(
                at as *mut c_void,
                part,
                MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
            ) != 0
        };
        if start > base && !split(base, start - base) {
            return false;
        }
        if start + size < base + length && !split(start, size) {
            return false;
        }
        record_split(&mut state, start, size);
        true
    }

    /// Records that `start..start + size` is a placeholder of its own, split out of the one that held it.
    pub(super) fn record_split(state: &mut State, start: usize, size: usize) {
        let Some((&base, &length)) = state.placeholders.range(..=start).next_back() else {
            return;
        };
        if start + size > base + length {
            return;
        }
        state.placeholders.remove(&base);
        if start > base {
            state.placeholders.insert(base, start - base);
        }
        if start + size < base + length {
            state
                .placeholders
                .insert(start + size, base + length - start - size);
        }
        state.placeholders.insert(start, size);
    }

    /// # Safety
    /// `count` points at the wanted count and gets the count made; `pages` has room for that many numbers.
    pub unsafe extern "win64" fn allocate_physical_pages(
        _flags: usize,
        count: *mut usize,
        pages: *mut usize,
    ) -> i32 {
        if count.is_null() || pages.is_null() {
            return FALSE;
        }
        let Ok(mut state) = STATE.lock() else {
            return FALSE;
        };
        // SAFETY: as the caller promises.
        let wanted = unsafe { *count };
        let mut made = 0;
        while made < wanted {
            // SAFETY: a 64 KiB section backed by the page file.
            let section = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE | SEC_COMMIT,
                    0,
                    PAGE as u32,
                    std::ptr::null(),
                )
            };
            if section.is_null() {
                break;
            }
            state.pages.push(Some(section as usize));
            // SAFETY: `pages` has room for `wanted` numbers.
            unsafe { *pages.add(made) = state.pages.len() };
            made += 1;
        }
        // SAFETY: as the caller promises.
        unsafe { *count = made };
        if made == wanted { TRUE } else { FALSE }
    }

    fn unmap(state: &mut State, at: usize) {
        if state.views.remove(&at).is_some() {
            // SAFETY: a view this module mapped at `at`; the range returns to a placeholder.
            unsafe {
                UnmapViewOfFile2(
                    GetCurrentProcess(),
                    MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: at as *mut c_void,
                    },
                    MEM_PRESERVE_PLACEHOLDER,
                )
            };
            state.placeholders.insert(at, PAGE);
        }
    }

    fn map_one(at: usize, page: Option<usize>) -> bool {
        if let Ok(mut state) = STATE.lock() {
            unmap(&mut state, at);
        }
        let Some(page) = page else {
            return true;
        };
        if !carve(at, PAGE) {
            note(format!(
                "XMemMapPhysicalPages at {at:#x}: not inside a reservation it made"
            ));
            return false;
        }
        let Ok(mut state) = STATE.lock() else {
            return false;
        };
        let Some(Some(section)) = page
            .checked_sub(1)
            .and_then(|i| state.pages.get(i))
            .copied()
        else {
            return false;
        };
        // SAFETY: maps one of our sections over the placeholder carved at `at`.
        let view = unsafe {
            MapViewOfFile3(
                section as HANDLE,
                GetCurrentProcess(),
                at as *const c_void,
                0,
                PAGE,
                MEM_REPLACE_PLACEHOLDER,
                PAGE_READWRITE,
                std::ptr::null_mut(),
                0,
            )
        };
        if view.Value.is_null() {
            return false;
        }
        state.placeholders.remove(&at);
        state.views.insert(at, page);
        true
    }

    /// Maps (or, with null `pages`, unmaps) a page at each address, and tells the hooks about the ones in
    /// GPU-visible reservations: the unmap first, while the old pages are still there, then what was mapped.
    ///
    /// # Safety
    /// `pages` is null or holds one page number for each address.
    unsafe fn map_all(addresses: &[usize], pages: *const usize, scatter: bool) -> i32 {
        let hooks = hooks::current();
        let tracked: Vec<bool> = match hooks {
            Some(_) => addresses.iter().map(|&at| gpu_reserved(at, PAGE)).collect(),
            None => vec![false; addresses.len()],
        };
        let watched = |wanted: bool| -> Vec<u64> {
            addresses
                .iter()
                .zip(&tracked)
                .filter(|(_, tracked)| **tracked == wanted)
                .map(|(&at, _)| at as u64)
                .collect()
        };
        if let Some(hooks) = hooks.as_deref() {
            let before = watched(true);
            if !before.is_empty() {
                let result = if !scatter && contiguous(&before) {
                    hooks.unmapped(before[0], before.len() as u64 * PAGE as u64)
                } else {
                    hooks.unmapped_scatter(&before)
                };
                if let Err(e) = result {
                    note(format!("the GPU cannot unmap {} pages: {e}", before.len()));
                }
            }
        }
        let mut ok = true;
        let mut mapped = Vec::new();
        for (k, &at) in addresses.iter().enumerate() {
            // SAFETY: as the caller promises.
            let page = (!pages.is_null()).then(|| unsafe { *pages.add(k) });
            let done = map_one(at, page);
            ok &= done;
            if done && page.is_some() && tracked[k] {
                mapped.push(at as u64);
            }
        }
        if let Some(hooks) = hooks.as_deref()
            && !mapped.is_empty()
        {
            let result = if !scatter && contiguous(&mapped) {
                hooks.pages_mapped(mapped[0], mapped.len() as u64, SyncPolicy::Always)
            } else {
                hooks.pages_mapped_scatter(&mapped, SyncPolicy::Always)
            };
            if let Err(e) = result {
                note(format!("the GPU cannot map {} pages: {e}", mapped.len()));
                ok = false;
            }
        }
        if ok { TRUE } else { FALSE }
    }

    /// Whether `addresses` are consecutive pages.
    fn contiguous(addresses: &[u64]) -> bool {
        addresses
            .windows(2)
            .all(|pair| pair[1] == pair[0] + PAGE as u64)
    }

    /// # Safety
    /// `pages` is null or holds `count` page numbers.
    pub unsafe extern "win64" fn map_physical_pages(
        address: *mut c_void,
        count: usize,
        pages: *const usize,
    ) -> i32 {
        let addresses: Vec<usize> = (0..count).map(|k| address as usize + k * PAGE).collect();
        // SAFETY: as the caller promises.
        unsafe { map_all(&addresses, pages, false) }
    }

    /// # Safety
    /// `addresses` holds `count` addresses and `pages` is null or holds `count` page numbers.
    pub unsafe extern "win64" fn map_physical_pages_scatter(
        addresses: *const usize,
        count: usize,
        pages: *const usize,
    ) -> i32 {
        if addresses.is_null() {
            return FALSE;
        }
        // SAFETY: as the caller promises.
        let addresses = unsafe { std::slice::from_raw_parts(addresses, count) };
        // SAFETY: as the caller promises.
        unsafe { map_all(addresses, pages, true) }
    }

    /// # Safety
    /// `pages` holds `count` page numbers this module made.
    pub unsafe extern "win64" fn free_physical_pages(count: usize, pages: *const usize) -> i32 {
        if pages.is_null() {
            return FALSE;
        }
        if let Some(hooks) = hooks::current() {
            // SAFETY: as the caller promises.
            let freed: HashSet<usize> = unsafe { std::slice::from_raw_parts(pages, count) }
                .iter()
                .copied()
                .collect();
            let mapped: Vec<u64> = STATE.lock().map_or_else(
                |_| Vec::new(),
                |state| {
                    state
                        .views
                        .iter()
                        .filter(|(at, page)| {
                            freed.contains(page) && covers(&state.gpu_reserved, **at, PAGE)
                        })
                        .map(|(&at, _)| at as u64)
                        .collect()
                },
            );
            if !mapped.is_empty()
                && let Err(e) = hooks.unmapped_scatter(&mapped)
            {
                note(format!(
                    "the GPU cannot unmap {} freed pages: {e}",
                    mapped.len()
                ));
            }
        }
        let Ok(mut state) = STATE.lock() else {
            return FALSE;
        };
        for k in 0..count {
            // SAFETY: as the caller promises.
            let page = unsafe { *pages.add(k) };
            if let Some(slot) = page.checked_sub(1).and_then(|i| state.pages.get_mut(i))
                && let Some(section) = slot.take()
            {
                // SAFETY: a section this module made; mapped views keep their own reference.
                unsafe { CloseHandle(section as HANDLE) };
            }
        }
        TRUE
    }

    /// The page a DirectStorage page-table entry names: a page number `XMemAllocatePhysicalPages` returned, or
    /// the 64 KiB-aligned address of a view of one. The two cannot be told apart only for the value 0x10000,
    /// which would need 65,536 live pages; the number wins.
    fn page_of_entry(state: &State, entry: u64) -> Option<usize> {
        if let Ok(number) = usize::try_from(entry)
            && number >= 1
            && matches!(state.pages.get(number - 1), Some(Some(_)))
        {
            return Some(number);
        }
        let at = usize::try_from(entry).ok().filter(|at| at % PAGE == 0)?;
        state.views.get(&at).copied()
    }

    /// Writes `data` at `offset` into the physical page `entry` names (see `page_of_entry`), for DirectStorage
    /// reads that land in physical pages. The write goes through a view of the page when one is mapped, else
    /// through a temporary view of its section, so the CPU and every other view of the page see it. Views the
    /// GPU reads are reported to the memory hooks as written by the host. Returns an `HRESULT`.
    pub fn write_physical_page(entry: u64, offset: usize, data: &[u8]) -> i32 {
        if offset > PAGE || data.len() > PAGE - offset {
            return E_INVALIDARG;
        }
        if data.is_empty() {
            return 0;
        }
        let Ok(state) = STATE.lock() else {
            return E_FAIL;
        };
        let Some(page) = page_of_entry(&state, entry) else {
            note(format!(
                "page write: entry {entry:#x} is neither a live physical page number nor a mapped page address"
            ));
            return E_INVALIDARG;
        };
        let mut visible = Vec::new();
        let mut through = None;
        for (&at, &mapped) in &state.views {
            if mapped == page {
                through.get_or_insert(at);
                if covers(&state.gpu_reserved, at, PAGE) {
                    visible.push(at + offset);
                }
            }
        }
        match through {
            // SAFETY: a view of this page mapped by `map_one` and kept mapped by the lock held here; the range
            // is inside the 64 KiB page.
            Some(at) => unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), (at + offset) as *mut u8, data.len());
            },
            None => {
                let Some(Some(section)) = state.pages.get(page - 1).copied() else {
                    return E_INVALIDARG;
                };
                // SAFETY: maps a section this module made, writes inside the view and unmaps it.
                let view = unsafe { MapViewOfFile(section as HANDLE, FILE_MAP_WRITE, 0, 0, PAGE) };
                if view.Value.is_null() {
                    return E_FAIL;
                }
                // SAFETY: the view is 64 KiB and the range is inside it.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        view.Value.cast::<u8>().add(offset),
                        data.len(),
                    );
                    UnmapViewOfFile(view);
                }
            }
        }
        drop(state);
        if !visible.is_empty()
            && let Some(hooks) = hooks::current()
        {
            for va in visible {
                hooks.host_written(va as u64, data.len() as u64);
            }
        }
        0
    }

    pub extern "win64" fn tools_memory_available() -> i32 {
        FALSE
    }

    const DEFAULT_LIMIT: u64 = 12 << 30;

    /// # Safety
    /// `stats` is null or points at a buffer of at least 16 bytes.
    pub unsafe extern "win64" fn get_working_set_statistics(kind: u32, stats: *mut u64) -> i32 {
        if stats.is_null() {
            return FALSE;
        }
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: fills a plain counters structure for this process.
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } == 0
        {
            return FALSE;
        }
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: fills a plain status structure.
        let physical =
            if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 && status.ullTotalPhys != 0 {
                status.ullTotalPhys
            } else {
                DEFAULT_LIMIT
            };
        // SAFETY: the callers' buffers are at least 0x80 bytes.
        unsafe {
            stats.write_unaligned(physical);
            stats.add(1).write_unaligned(counters.WorkingSetSize as u64);
        }
        if let Ok(mut seen) = SEEN_KINDS.lock()
            && !seen.contains(&kind)
        {
            seen.push(kind);
            note(format!("xmem_working_set kind {kind:#x}"));
        }
        TRUE
    }

    static SEEN_KINDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    static SEEN_TRANSFERS: Mutex<Vec<(u32, usize)>> = Mutex::new(Vec::new());

    pub extern "win64" fn transfer_memory(direction: u32, size: usize) -> i32 {
        if let Ok(mut seen) = SEEN_TRANSFERS.lock()
            && !seen.contains(&(direction, size))
        {
            seen.push((direction, size));
            note(format!(
                "xmem_transfer direction {direction} size {size:#x}"
            ));
        }
        TRUE
    }
}

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let list: [(&str, usize); 12] = [
            ("XMemAlloc", imp::alloc as *const () as usize),
            ("XMemAllocDefault", imp::alloc_default as *const () as usize),
            ("XMemFree", imp::free as *const () as usize),
            ("XMemFreeDefault", imp::free_default as *const () as usize),
            ("XMemVirtualAlloc", imp::virtual_alloc as *const () as usize),
            (
                "XMemAllocatePhysicalPages",
                imp::allocate_physical_pages as *const () as usize,
            ),
            (
                "XMemMapPhysicalPages",
                imp::map_physical_pages as *const () as usize,
            ),
            (
                "XMemMapPhysicalPagesScatter",
                imp::map_physical_pages_scatter as *const () as usize,
            ),
            (
                "XMemFreePhysicalPages",
                imp::free_physical_pages as *const () as usize,
            ),
            (
                "XMemIsToolsMemAvailable",
                imp::tools_memory_available as *const () as usize,
            ),
            (
                "XMemGetWorkingSetStatistics",
                imp::get_working_set_statistics as *const () as usize,
            ),
            (
                "XMemTransferMemory",
                imp::transfer_memory as *const () as usize,
            ),
        ];
        let mut shims: Vec<Shim> = list
            .into_iter()
            .map(|(name, address)| Shim {
                dll: "xmem.dll",
                name,
                address,
            })
            .collect();
        shims.extend(virtual_free::shims());
        shims
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
