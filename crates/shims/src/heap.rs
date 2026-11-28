//! A private Windows heap for Leonida's own allocations.
//!
//! The title's C runtime allocates from the process heap, and a boot showed one of the title's stacks running
//! over memory that held a resource object of ours (rva 0x4ecc933: the object's first words became a stack frame
//! of the title's, while the object was still referenced). Leonida's objects live in a heap of their own, so
//! nothing the title does with the process heap can land on them.

use std::alloc::{GlobalAlloc, Layout};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::Memory::{
    HEAP_ZERO_MEMORY, HeapAlloc, HeapCreate, HeapDestroy, HeapFree, HeapReAlloc,
};

/// `MEMORY_ALLOCATION_ALIGNMENT` on x64: what a heap block is aligned to without help.
const NATURAL: usize = 16;

static HEAP: AtomicUsize = AtomicUsize::new(0);

fn heap() -> *mut c_void {
    let current = HEAP.load(Ordering::Acquire);
    if current != 0 {
        return current as *mut c_void;
    }
    // SAFETY: a growable heap with default options.
    let made = unsafe { HeapCreate(0, 0, 0) } as usize;
    match HEAP.compare_exchange(0, made, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made as *mut c_void,
        Err(other) => {
            // SAFETY: the heap just made, which nothing has used.
            unsafe { HeapDestroy(made as *mut c_void) };
            other as *mut c_void
        }
    }
}

/// Blocks at least this large are remembered with the stack that asked for them, for the memory report.
const LARGE: usize = 256 << 20;

struct Large {
    address: usize,
    size: usize,
    stack: std::backtrace::Backtrace,
}

static LARGE_BLOCKS: std::sync::Mutex<Vec<Large>> = std::sync::Mutex::new(Vec::new());

thread_local! {
    static RECORDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn remember(address: *mut u8, size: usize) {
    if address.is_null() || size < LARGE || RECORDING.with(|r| r.replace(true)) {
        return;
    }
    let stack = std::backtrace::Backtrace::force_capture();
    if let Ok(mut blocks) = LARGE_BLOCKS.lock() {
        blocks.push(Large {
            address: address as usize,
            size,
            stack,
        });
    }
    RECORDING.with(|r| r.set(false));
}

fn forget(address: *mut u8, size: usize) {
    if size < LARGE || RECORDING.with(|r| r.replace(true)) {
        return;
    }
    if let Ok(mut blocks) = LARGE_BLOCKS.lock() {
        blocks.retain(|b| b.address != address as usize);
    }
    RECORDING.with(|r| r.set(false));
}

/// The live blocks of at least 256 MiB Leonida's own code holds, each with the stack that made it.
pub fn large_blocks() -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    if RECORDING.with(|r| r.replace(true)) {
        return text;
    }
    if let Ok(blocks) = LARGE_BLOCKS.lock() {
        for block in blocks.iter() {
            let _ = writeln!(
                text,
                "block {:#x}: {:.3} GiB, made by
{}",
                block.address,
                block.size as f64 / (1u64 << 30) as f64,
                block.stack
            );
        }
    }
    RECORDING.with(|r| r.set(false));
    text
}

/// The global allocator over the private heap.
pub struct PrivateHeap;

impl PrivateHeap {
    /// Over-allocates and stores the block's start just before the aligned address.
    unsafe fn aligned(layout: Layout, flags: u32) -> *mut u8 {
        let total = layout.size() + layout.align() + std::mem::size_of::<usize>();
        // SAFETY: a heap of ours.
        let raw = unsafe { HeapAlloc(heap(), flags, total) } as usize;
        if raw == 0 {
            return std::ptr::null_mut();
        }
        let aligned = (raw + std::mem::size_of::<usize>()).next_multiple_of(layout.align());
        // SAFETY: `aligned - 8` lies inside the block, after its start.
        unsafe { ((aligned - std::mem::size_of::<usize>()) as *mut usize).write(raw) };
        aligned as *mut u8
    }
}

// SAFETY: every block comes from one heap and goes back to it, aligned as asked.
unsafe impl GlobalAlloc for PrivateHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let made = if layout.align() <= NATURAL {
            // SAFETY: a heap of ours.
            unsafe { HeapAlloc(heap(), 0, layout.size()) as *mut u8 }
        } else {
            // SAFETY: as above.
            unsafe { Self::aligned(layout, 0) }
        };
        remember(made, layout.size());
        made
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let made = if layout.align() <= NATURAL {
            // SAFETY: a heap of ours.
            unsafe { HeapAlloc(heap(), HEAP_ZERO_MEMORY, layout.size()) as *mut u8 }
        } else {
            // SAFETY: as above.
            unsafe { Self::aligned(layout, HEAP_ZERO_MEMORY) }
        };
        remember(made, layout.size());
        made
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        forget(ptr, layout.size());
        let block = if layout.align() <= NATURAL {
            ptr as *mut c_void
        } else {
            // SAFETY: `aligned` stored the block's start just before the pointer it gave.
            unsafe {
                ((ptr as usize - std::mem::size_of::<usize>()) as *const usize).read()
                    as *mut c_void
            }
        };
        // SAFETY: a block of this heap.
        unsafe { HeapFree(heap(), 0, block) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= NATURAL {
            forget(ptr, layout.size());
            // SAFETY: a block of this heap, kept at the natural alignment.
            let made = unsafe { HeapReAlloc(heap(), 0, ptr as *mut c_void, new_size) as *mut u8 };
            remember(made, new_size);
            made
        } else {
            // SAFETY: the layout is valid; copied then freed as the default does.
            unsafe {
                let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
                let new = self.alloc(new_layout);
                if !new.is_null() {
                    std::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                    self.dealloc(ptr, layout);
                }
                new
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_come_back_aligned_zeroed_and_resized() {
        let heap = PrivateHeap;
        for align in [1, 8, 16, 64, 4096] {
            let layout = Layout::from_size_align(100, align).unwrap();
            // SAFETY: a valid layout; each block is written inside its size and freed once.
            unsafe {
                let p = heap.alloc_zeroed(layout);
                assert!(!p.is_null());
                assert_eq!(p as usize % align, 0, "aligned to {align}");
                assert!(std::slice::from_raw_parts(p, 100).iter().all(|&b| b == 0));
                p.write_bytes(7, 100);
                let q = heap.realloc(p, layout, 300);
                assert_eq!(q as usize % align, 0);
                assert!(std::slice::from_raw_parts(q, 100).iter().all(|&b| b == 7));
                heap.dealloc(q, Layout::from_size_align(300, align).unwrap());
            }
        }
    }
}
