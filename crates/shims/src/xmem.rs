//! The GDK's memory allocator, from its public signatures: `XMemAlloc(size, attributes)` and
//! `XMemFree(address, attributes)`, plus their `Default` forms that custom allocation hooks fall back to.
//! Allocations come straight from the system, so any alignment the attributes ask for (64 KiB at most) holds.

use crate::Shim;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };

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
}

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let alloc = imp::alloc as *const () as usize;
        let free = imp::free as *const () as usize;
        [
            ("XMemAlloc", alloc),
            ("XMemAllocDefault", alloc),
            ("XMemFree", free),
            ("XMemFreeDefault", free),
        ]
        .into_iter()
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
