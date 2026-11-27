//! The default placement: host memory at fixed addresses under 4 GiB.

use std::ffi::c_void;
use std::sync::Mutex;

use super::console::{LowBlock, LowMemory};
use super::{HeapPlacement, Placed};
use crate::com;

const WINDOW_START: usize = 0x3000_0000;
#[cfg(windows)]
const LOW_END: usize = 0xF000_0000;
const WINDOW_ALIGN: usize = 64 << 10;

/// Hands out heap addresses from a fixed window, in creation order, and falls back to any address when the
/// window's address is taken.
pub struct FixedWindow {
    next: Mutex<usize>,
}

impl Default for FixedWindow {
    fn default() -> Self {
        FixedWindow {
            next: Mutex::new(WINDOW_START),
        }
    }
}

struct Block {
    base: usize,
    len: usize,
    #[cfg(not(windows))]
    _own: Vec<u8>,
}

#[cfg(windows)]
impl Drop for Block {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
        // SAFETY: `base` came from `VirtualAlloc` and is released once.
        unsafe { VirtualFree(self.base as *mut c_void, 0, MEM_RELEASE) };
        let _ = self.len;
    }
}

#[cfg(windows)]
fn reserve(at: usize, len: usize) -> Option<Block> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
    };
    // SAFETY: a fresh allocation; `at` is a hint address that fails when taken.
    let base = unsafe {
        VirtualAlloc(
            at as *const c_void,
            len,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    (!base.is_null()).then_some(Block {
        base: base as usize,
        len,
    })
}

#[cfg(not(windows))]
fn reserve(_at: usize, len: usize) -> Option<Block> {
    let own = vec![0u8; len + WINDOW_ALIGN];
    let base = (own.as_ptr() as usize + WINDOW_ALIGN - 1) & !(WINDOW_ALIGN - 1);
    Some(Block {
        base,
        len,
        _own: own,
    })
}

impl FixedWindow {
    /// The next free range of the window, or, when its address is taken, the first free range below 4 GiB
    /// after it; only when none is left, memory anywhere.
    #[cfg(windows)]
    fn block(&self, bytes: usize) -> Option<Block> {
        let len = bytes.max(1).div_ceil(WINDOW_ALIGN) * WINDOW_ALIGN;
        let mut next = self.next.lock().ok()?;
        let mut at = *next;
        while at + len <= LOW_END {
            if let Some(block) = reserve(at, len).filter(|b| b.base == at) {
                *next = at + len;
                return Some(block);
            }
            at += len;
        }
        com::note_once(
            "no descriptor heap address under 4 GiB is free; the heap is placed elsewhere",
        );
        reserve(0, len)
    }

    #[cfg(not(windows))]
    fn block(&self, bytes: usize) -> Option<Block> {
        let _ = &self.next;
        reserve(0, bytes.max(1).div_ceil(WINDOW_ALIGN) * WINDOW_ALIGN)
    }
}

impl HeapPlacement for FixedWindow {
    fn place(&self, bytes: usize, shader_visible: bool) -> Option<Placed> {
        let block = self.block(bytes)?;
        let cpu = block.base as u64;
        Some(Placed {
            cpu,
            gpu: if shader_visible { cpu } else { 0 },
            keep: Box::new(BlockOwner(block)),
        })
    }
}

impl LowMemory for FixedWindow {
    fn alloc_low(&self, bytes: u64) -> Option<LowBlock> {
        let block = self.block(usize::try_from(bytes).ok()?)?;
        Some(LowBlock {
            address: block.base as u64,
            keep: Box::new(BlockOwner(block)),
        })
    }
}

struct BlockOwner(#[allow(dead_code)] Block);

// SAFETY: a `Block` is an address range this heap alone owns; nothing is read through the owner.
unsafe impl Send for BlockOwner {}
// SAFETY: as above.
unsafe impl Sync for BlockOwner {}
