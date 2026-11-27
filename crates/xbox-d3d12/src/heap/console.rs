//! Where the title expects a descriptor heap to start.
//!
//! Measured statically with `leonida exe disasm`, `decompile` and `xrefs`; every address below is an RVA in
//! the 1.0.2.1 executable, and nothing was run. This is the answer to upstream's stop 13
//! (`retail_boot_chain.md`): the title does not compute the start handle, it chooses it, and the runtime has
//! to honour the choice through the title's allocation hooks.
//!
//! # The title's heap routine (0x1cf3280)
//!
//! `(object, type, address, count, flags)`. It reads `GetDescriptorHandleIncrementSize(type)` (device slot
//! 15), builds `D3D12_DESCRIPTOR_HEAP_DESC {type, count, flags, node mask 1}` on the stack and calls
//! `CreateDescriptorHeap` (slot 14) with the heap IID. Its byte size is `count * increment`.
//!
//! - When `address` is not zero it first pushes a scoped override on its own hook list (0x5c8f990, callback
//!   0x5eb2730, argument `{address, bytes}`) and removes it after the call (0x5c8fa70).
//! - After the call it stores the heap, then calls heap slot 9 (0x1cf3358) and keeps the result as the start.
//!   **The assertion is `address != 0 && start != address`: the next instruction is a breakpoint (0x1cf336c).**
//!   With `address == 0` nothing is compared.
//! - The end is `start + bytes`, the increment is kept, and the GPU side is `slot 10 - start` when the flags
//!   have the shader-visible bit (0x1cf3392), else the constant `0xBADADD0000000000` without calling slot 10.
//!   Slot 10 is compared with nothing at creation; the title keeps the difference and adds it to CPU
//!   addresses later. On the console the two handles are the same address, so the difference is 0.
//!
//! # Where the address comes from
//!
//! The device bring-up (0x2478440) calls this routine five times, in this order, from 0x2478eb5 and 0x2478edd
//! to 0x2478f2b:
//!
//! | Heap | Type | Descriptors | Flags | Address |
//! |---|---|---|---|---|
//! | 1 | CBV_SRV_UAV | `(a1 + a2 + a3) / increment`, sizes below | shader visible | `[0x75df188]` |
//! | 2 | SAMPLER | 0x80 | shader visible | 0 |
//! | 3 | SAMPLER | 0x80 | none | 0 |
//! | 4 | RTV | 0x1000 | none | 0 |
//! | 5 | DSV | 0x800 | none | 0 |
//!
//! Only heap 1 carries an address, so it is the first `CreateDescriptorHeap` and the one that stopped the
//! boot. Its address and size come from the memory setup at 0x5c5ff00 and 0x1e86440, which is the 185 MiB
//! reserve of stops 10 and 11: `XMemVirtualAlloc(0, total, 0x42000, 7, 1)` returns the base (stored at
//! 0x75df188, the first two ranges' size at 0x75df190), the base is split with `VirtualFree` into five ranges
//! and each is re-reserved with `0x20406000` and attributes 0x1004, 0x1404, 0x8404, 0x2404, 0x2004. The first
//! three ranges are the descriptor heap: `a1` and `a2` are descriptor counts from the title's config
//! (0xa94393c + 0xa943940, 0xa943944) times the type 0 increment, each rounded up to 64 KiB, and `a3`
//! (`[0xa943910]` or 0x40000, rounded to 64 KiB, times `[0xa943914]` or 0x20) is the third. Heap 1's size is
//! `a1 + a2 + a3` (5cabb00 divides it by the increment, 1cf3280 multiplies it back), so the increment must
//! divide a 64 KiB multiple for the hook to match. The title maps physical pages into those ranges itself
//! (`XMemAllocatePhysicalPages` with 0x280, then `XMemMapPhysicalPages`), so the memory is already there and
//! writable when the heap is created.
//!
//! # How the runtime is meant to honour it
//!
//! The executable carries a two-pointer table at 0x6844380, named by the string `__XMemAllocationHooks__`
//! (0x6791a9b): an allocation hook (0x4deb420, body 0x4deb430) and a free hook (0x4e38ec0, body 0x4e38ed0).
//! `XMemAlloc` calls the allocation hook as `(size, attributes)`. It:
//!
//! 1. picks a bucket by attribute bits 16 to 23 (`bextr` with start 16, length 8; bucket table at 0x6843370,
//!    the free hook's at 0x6843b80);
//! 2. keeps the nodes `{function, argument, thread tag, next}` whose thread tag is -1 (every thread) or the
//!    calling thread (the per-thread object from 0x4de8620);
//! 3. calls the newest kept node as `function(size, attributes, argument, next node)`.
//!
//! The title's scoped override sits in bucket 0x80 (head at 0x6843770; the matching free override, 0x5c8fa00,
//! at 0x6843f80), so 0x80 is the allocator id the D3D12 runtime uses. The callback (0x5eb2730) returns the
//! override's address when `attributes & 0x3fff == 4` and `size` equals the override's byte size, and
//! otherwise forwards to the next node. So a console runtime that allocates a descriptor heap's memory with
//! `XMemAlloc(count * increment, attributes with allocator id 0x80 and object type 4)` gets the title's
//! address back, puts the heap there, and slot 9 returns it.
//!
//! `ConsolePlacement` does exactly that: it asks a `HeapAllocator` (the XMem layer, running the title's
//! allocation hook) for the memory, and uses the address it returns as both handles' base.
//!
//! # Increments
//!
//! The 10 call sites of device slot 15 in functions that use the device (0x1c7456b, 0x1cf32be, 0x1f75fdd,
//! 0x2478e42 to 0x2478e9a, 0x5c5ff2f, 0x5cabb18) all take the value from the call: the title never writes a
//! literal stride there. The staging heaps at 0x1c744f0 and 0x1f75f60 (type 0, 3128 descriptors, from the
//! description at 0x61ecfb0) hand out slot `i` as `start + i * increment` for 408, 1840 and 880 slots. Types 0
//! to 4 are queried at 0x2478e42 to 0x2478e9a; that function keeps only type 0's answer (0x2478e4c) and drops the rest, type 4's included. So the 32 bytes
//! in `increment_size` is free to be right or wrong as long as the device and the heaps agree and it divides
//! 64 KiB.
//!
//! # Not measured
//!
//! - The attribute bits other than allocator id 0x80 and object type 4: the hook ignores them, and the
//!   runtime's own value is not visible in the executable. `DESCRIPTOR_HEAP_ATTRIBUTES` has just those two.
//! - Whether the console runtime frees a heap's memory through the free hook; `Hooked::release` is `None`
//!   for the title's own ranges and the allocator decides for the rest.
//! - Where the title's range lies: it has to be under 4 GiB for shaders to reach a shader-visible heap, and
//!   that is the XMem layer's choice for the 0x42000 reserve. A range above the limit is used as given and
//!   reported as `PlacementNote::AboveLowLimit`.
//!
//! `leonida exe xrefs --import` does not see calls made through the executable's import thunks (they lie
//! outside the exception table); `xrefs` on the thunk's own address does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{HeapPlacement, Placed};
use crate::memory::LOW_LIMIT;

/// The allocator id (attribute bits 16 to 23) of the D3D12 runtime's allocations.
pub const ALLOCATOR_ID: u64 = 0x80;
/// The object type (attribute bits 0 to 13) the title's hook accepts for a descriptor heap.
pub const OBJECT_TYPE: u64 = 4;
pub const OBJECT_TYPE_MASK: u64 = 0x3fff;
/// The attributes of a descriptor heap's `XMemAlloc`: only the two fields the title's hook reads.
pub const DESCRIPTOR_HEAP_ATTRIBUTES: u64 = (ALLOCATOR_ID << 16) | OBJECT_TYPE;

/// Memory the title's allocation hook answered with.
pub struct Hooked {
    pub address: u64,
    /// What to do when the heap dies, for memory the allocator owns (its free hook, `XMemFree`). `None` for
    /// memory the title owns.
    pub release: Option<Box<dyn FnOnce() + Send + Sync>>,
}

/// `XMemAlloc` as the runtime calls it: through the title's allocation hook.
pub trait HeapAllocator: Send + Sync {
    /// The address the hook chain returned, or `None` when no hook is installed or it allocated nothing.
    fn allocate(&self, bytes: u64, attributes: u64) -> Option<Hooked>;
}

/// No hooks installed: every heap is placed in low memory.
pub struct NoAllocator;

impl HeapAllocator for NoAllocator {
    fn allocate(&self, _bytes: u64, _attributes: u64) -> Option<Hooked> {
        None
    }
}

/// Writable host memory under 4 GiB and what keeps it alive.
pub struct LowBlock {
    pub address: u64,
    pub keep: Box<dyn Send + Sync>,
}

/// Memory for heaps the title did not place. `resource::Resources::alloc_low` gives such memory;
/// `FixedWindow` does without a GPU.
pub trait LowMemory: Send + Sync {
    fn alloc_low(&self, bytes: u64) -> Option<LowBlock>;
}

/// Things `ConsolePlacement` could not settle from the measurements.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlacementNote {
    #[error(
        "descriptor heap memory is asked for with allocator id {ALLOCATOR_ID:#x} and object type {OBJECT_TYPE} \
         ({attributes:#x}); the runtime's other attribute bits are not measured"
    )]
    UnmeasuredAttributes { attributes: u64 },
    #[error(
        "shader-visible descriptor heap {address:#x}+{bytes:#x} lies above 4 GiB and is the title's own memory; \
         shaders truncate descriptor table addresses to 32 bits"
    )]
    AboveLowLimit { address: u64, bytes: u64 },
    #[error("descriptor heap memory {from:#x} lay above 4 GiB and was replaced by {to:#x}")]
    MovedBelowLimit { from: u64, to: u64 },
    #[error("no memory under 4 GiB for a descriptor heap of {bytes:#x} bytes")]
    OutOfLowMemory { bytes: u64 },
}

type NoteFn = dyn Fn(&PlacementNote) + Send + Sync;

/// Places each descriptor heap where the title's allocation hook says, else in low memory.
///
/// A shader-visible heap's GPU handle is its CPU handle (one address serves both on the console); any other
/// heap has GPU handle 0.
pub struct ConsolePlacement {
    allocator: Arc<dyn HeapAllocator>,
    low: Arc<dyn LowMemory>,
    notes: Arc<NoteFn>,
    attributes_noted: AtomicBool,
}

impl ConsolePlacement {
    pub fn new(allocator: Arc<dyn HeapAllocator>, low: Arc<dyn LowMemory>) -> ConsolePlacement {
        ConsolePlacement {
            allocator,
            low,
            notes: Arc::new(|note| crate::com::note_once(note.to_string())),
            attributes_noted: AtomicBool::new(false),
        }
    }

    /// Sends the notes to `notes` instead of the trace.
    pub fn with_notes(mut self, notes: impl Fn(&PlacementNote) + Send + Sync + 'static) -> Self {
        self.notes = Arc::new(notes);
        self
    }

    fn note(&self, note: PlacementNote) {
        (self.notes)(&note);
    }

    fn low_block(&self, bytes: u64) -> Option<(u64, Box<dyn Send + Sync>)> {
        match self.low.alloc_low(bytes) {
            Some(block) => Some((block.address, block.keep)),
            None => {
                self.note(PlacementNote::OutOfLowMemory { bytes });
                None
            }
        }
    }
}

struct Release(Option<Box<dyn FnOnce() + Send + Sync>>);

impl Drop for Release {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            release();
        }
    }
}

impl HeapPlacement for ConsolePlacement {
    fn place(&self, bytes: usize, shader_visible: bool) -> Option<Placed> {
        let bytes = bytes as u64;
        let hooked = self
            .allocator
            .allocate(bytes, DESCRIPTOR_HEAP_ATTRIBUTES)
            .filter(|h| h.address != 0);
        let (cpu, keep): (u64, Box<dyn Send + Sync>) = match hooked {
            Some(hooked) => {
                if !self.attributes_noted.swap(true, Ordering::Relaxed) {
                    self.note(PlacementNote::UnmeasuredAttributes {
                        attributes: DESCRIPTOR_HEAP_ATTRIBUTES,
                    });
                }
                let too_high = shader_visible && hooked.address.saturating_add(bytes) > LOW_LIMIT;
                if too_high && hooked.release.is_some() {
                    let from = hooked.address;
                    drop(Release(hooked.release));
                    let (to, keep) = self.low_block(bytes)?;
                    self.note(PlacementNote::MovedBelowLimit { from, to });
                    (to, keep)
                } else {
                    if too_high {
                        self.note(PlacementNote::AboveLowLimit {
                            address: hooked.address,
                            bytes,
                        });
                    }
                    (hooked.address, Box::new(Release(hooked.release)))
                }
            }
            None => self.low_block(bytes)?,
        };
        Some(Placed {
            cpu,
            gpu: if shader_visible { cpu } else { 0 },
            keep,
        })
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
