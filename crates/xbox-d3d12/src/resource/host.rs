//! Whether game memory is present in the host's address space before Leonida reads or writes it itself.
//!
//! A placed resource names an address the title reserved; its pages are mapped only once the title maps them, and a
//! boot showed the title making a view of a Bink frame texture whose memory was not mapped yet: the texel read
//! faulted inside the view call (rva 0x3a16b40, the frame texture readiness check), the title caught it, and the
//! movie never became ready.

#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

/// Whether every byte of `address..address + len` is committed and readable, and writable when `write` is set.
#[cfg(windows)]
pub fn backed(address: u64, len: u64, write: bool) -> bool {
    let Some(end) = address.checked_add(len) else {
        return false;
    };
    if address == 0 || len == 0 {
        return false;
    }
    let writable =
        PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    let mut at = address;
    while at < end {
        // SAFETY: an all-zero structure is a valid value for VirtualQuery to overwrite.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: VirtualQuery only inspects the address space.
        let got = unsafe {
            VirtualQuery(
                at as usize as *const _,
                &mut info,
                std::mem::size_of_val(&info),
            )
        };
        if got == 0
            || info.State != MEM_COMMIT
            || info.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
            || info.Protect == 0
            || (write && info.Protect & writable == 0)
        {
            return false;
        }
        let next = info.BaseAddress as u64 + info.RegionSize as u64;
        if next <= at {
            return false;
        }
        at = next;
    }
    true
}

#[cfg(not(windows))]
pub fn backed(address: u64, len: u64, _write: bool) -> bool {
    address != 0 && len != 0
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn heap_memory_is_backed_and_a_reserved_range_is_not() {
        let block = vec![0u8; 3 * 4096];
        assert!(backed(block.as_ptr() as u64, block.len() as u64, true));
        assert!(!backed(0, 16, false));
        // SAFETY: reserves address space without committing it, released below.
        let reserved = unsafe {
            windows_sys::Win32::System::Memory::VirtualAlloc(
                std::ptr::null(),
                1 << 16,
                windows_sys::Win32::System::Memory::MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        assert!(!reserved.is_null());
        assert!(!backed(reserved as u64, 4096, false));
        // SAFETY: the reservation made above.
        unsafe {
            windows_sys::Win32::System::Memory::VirtualFree(
                reserved,
                0,
                windows_sys::Win32::System::Memory::MEM_RELEASE,
            )
        };
    }
}
