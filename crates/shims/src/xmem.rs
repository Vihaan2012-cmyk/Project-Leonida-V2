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
//! - `XMemIsToolsMemAvailable()`: there is no development kit memory, so false.

use crate::Shim;

pub const PAGE: usize = 64 * 1024;

#[cfg(windows)]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MEM_COMMIT, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE,
        MEM_REPLACE_PLACEHOLDER, MEM_RESERVE, MEM_RESERVE_PLACEHOLDER, MEMORY_MAPPED_VIEW_ADDRESS,
        MapViewOfFile3, PAGE_NOACCESS, PAGE_READWRITE, SEC_COMMIT, UnmapViewOfFile2, VirtualAlloc,
        VirtualAlloc2, VirtualFree,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use super::PAGE;
    use crate::trace::note;

    const TRUE: i32 = 1;
    const FALSE: i32 = 0;
    const MEM_TOP_DOWN: u32 = 0x10_0000;
    const MEM_PHYSICAL: u32 = 0x40_0000;

    struct State {
        /// Placeholder reservations: start to length.
        placeholders: BTreeMap<usize, usize>,
        /// Physical pages by number less one; `None` once freed.
        pages: Vec<Option<usize>>,
        /// Mapped addresses and the page there.
        views: BTreeMap<usize, usize>,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        placeholders: BTreeMap::new(),
        pages: Vec::new(),
        views: BTreeMap::new(),
    });

    pub extern "win64" fn alloc(size: usize, _attributes: u64) -> *mut c_void {
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

    pub extern "win64" fn free(address: *mut c_void, _attributes: u64) {
        if !address.is_null() {
            // SAFETY: `address` came from `alloc`, which only hands out allocation bases.
            unsafe { VirtualFree(address, 0, MEM_RELEASE) };
        }
    }

    pub extern "win64" fn virtual_alloc(
        address: *mut c_void,
        size: usize,
        allocation_type: u32,
        attributes: u64,
        protect: u32,
    ) -> *mut c_void {
        let kind = allocation_type & (MEM_COMMIT | MEM_RESERVE | MEM_TOP_DOWN);
        let access = match protect & 0xFF {
            0 => PAGE_READWRITE,
            access => access,
        };
        let reserve_only = kind & MEM_COMMIT == 0 && kind & MEM_RESERVE != 0;
        if reserve_only && !address.is_null() && inside_placeholder(address as usize, size) {
            if allocation_type & MEM_PHYSICAL != 0 {
                return if carve(address as usize, size) {
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
                return base;
            }
        }
        if reserve_only && size.is_multiple_of(PAGE) && (address as usize).is_multiple_of(PAGE) {
            // SAFETY: a placeholder reservation, at the program's address if it named one.
            let base = unsafe {
                VirtualAlloc2(
                    GetCurrentProcess(),
                    address,
                    size,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if !base.is_null() {
                if let Ok(mut state) = STATE.lock() {
                    state.placeholders.insert(base as usize, size);
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
        true
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

    /// # Safety
    /// `pages` is null or holds `count` page numbers.
    pub unsafe extern "win64" fn map_physical_pages(
        address: *mut c_void,
        count: usize,
        pages: *const usize,
    ) -> i32 {
        let mut ok = true;
        for k in 0..count {
            // SAFETY: as the caller promises.
            let page = (!pages.is_null()).then(|| unsafe { *pages.add(k) });
            ok &= map_one(address as usize + k * PAGE, page);
        }
        if ok { TRUE } else { FALSE }
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
        let mut ok = true;
        for k in 0..count {
            // SAFETY: as the caller promises.
            let (at, page) =
                unsafe { (*addresses.add(k), (!pages.is_null()).then(|| *pages.add(k))) };
            ok &= map_one(at, page);
        }
        if ok { TRUE } else { FALSE }
    }

    /// # Safety
    /// `pages` holds `count` page numbers this module made.
    pub unsafe extern "win64" fn free_physical_pages(count: usize, pages: *const usize) -> i32 {
        if pages.is_null() {
            return FALSE;
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

    pub extern "win64" fn tools_memory_available() -> i32 {
        FALSE
    }
}

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let list: [(&str, usize); 10] = [
            ("XMemAlloc", imp::alloc as *const () as usize),
            ("XMemAllocDefault", imp::alloc as *const () as usize),
            ("XMemFree", imp::free as *const () as usize),
            ("XMemFreeDefault", imp::free as *const () as usize),
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
        ];
        list.into_iter()
            .map(|(name, address)| Shim {
                dll: "xmem.dll",
                name,
                address,
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
