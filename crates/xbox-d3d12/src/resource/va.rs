//! Game addresses handed out for resources the game did not place itself, and the host memory behind them.

use std::collections::BTreeMap;
use std::ffi::c_void;

/// First-fit allocator over an address range.
pub struct VaAllocator {
    free: BTreeMap<u64, u64>,
}

impl VaAllocator {
    pub fn new(lo: u64, hi: u64) -> VaAllocator {
        let mut free = BTreeMap::new();
        if hi > lo {
            free.insert(lo, hi - lo);
        }
        VaAllocator { free }
    }

    /// `size` bytes at a multiple of `align` (a power of two).
    pub fn alloc(&mut self, size: u64, align: u64) -> Option<u64> {
        let size = size.max(1);
        let hit = self.free.iter().find_map(|(start, len)| {
            let at = start.next_multiple_of(align);
            (at + size <= start + len).then_some((*start, *len, at))
        })?;
        let (start, len, at) = hit;
        self.free.remove(&start);
        if at > start {
            self.free.insert(start, at - start);
        }
        if at + size < start + len {
            self.free.insert(at + size, start + len - at - size);
        }
        Some(at)
    }

    /// Marks `start..start + size` as taken, for a range the game chose. False if any of it is not free.
    pub fn reserve(&mut self, start: u64, size: u64) -> bool {
        let Some((&from, &len)) = self.free.range(..=start).next_back() else {
            return false;
        };
        if start + size > from + len {
            return false;
        }
        self.free.remove(&from);
        if start > from {
            self.free.insert(from, start - from);
        }
        if start + size < from + len {
            self.free.insert(start + size, from + len - start - size);
        }
        true
    }

    pub fn free(&mut self, start: u64, size: u64) {
        let (mut start, mut size) = (start, size);
        if let Some((&before, &len)) = self.free.range(..start).next_back()
            && before + len == start
        {
            self.free.remove(&before);
            start = before;
            size += len;
        }
        if let Some(len) = self.free.remove(&(start + size)) {
            size += len;
        }
        self.free.insert(start, size);
    }

    pub fn available(&self) -> u64 {
        self.free.values().sum()
    }
}

/// Committed host memory at exactly `va`, readable and writable, as the CPU side of a resource the GPU sees at
/// the same address. With `write_watch` the system reports which pages are written.
#[cfg(windows)]
pub fn commit_host(va: u64, len: u64, write_watch: bool) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
    };
    const MEM_WRITE_WATCH: u32 = 0x0020_0000;
    let flags = MEM_RESERVE | MEM_COMMIT | if write_watch { MEM_WRITE_WATCH } else { 0 };
    // SAFETY: asks for a fixed address and gets it or fails; nothing else is touched.
    let got = unsafe {
        VirtualAlloc(
            va as usize as *const c_void,
            len as usize,
            flags,
            PAGE_READWRITE,
        )
    };
    got as u64 == va
}

#[cfg(windows)]
pub fn release_host(va: u64) {
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
    // SAFETY: `va` is the base of an allocation `commit_host` made.
    unsafe { VirtualFree(va as usize as *mut c_void, 0, MEM_RELEASE) };
}

#[cfg(not(windows))]
pub fn commit_host(_va: u64, _len: u64, _write_watch: bool) -> bool {
    false
}

#[cfg(not(windows))]
pub fn release_host(_va: u64) {}
