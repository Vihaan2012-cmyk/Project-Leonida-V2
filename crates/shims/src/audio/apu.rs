//! The coprocessor's address space as the title sees it: heaps it creates, blocks it allocates from them, and a
//! 32-bit coprocessor address for every CPU buffer it asks about.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::BTreeMap;

/// The first coprocessor address handed out; address 0 stays invalid.
const APU_BASE: u64 = 0x0100_0000;
const MIN_ALIGN: usize = 0x800;
const FOREIGN_PAGE: usize = 0x1000;
/// Largest CPU range one block or lookup may cover.
const MAX_SPAN: usize = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApuError {
    #[error("no heap was created")]
    NoHeap,
    #[error("the coprocessor address space is exhausted")]
    OutOfAddresses,
    #[error("the host refused {0:#x} bytes")]
    OutOfMemory(usize),
    #[error("invalid size or alignment")]
    Invalid,
}

struct Block {
    size: usize,
    apu: u32,
    layout: Layout,
}

struct Foreign {
    size: usize,
    apu: u32,
}

pub struct ApuSpace {
    heaps: Vec<(u32, usize)>,
    /// Allocated blocks by CPU address.
    blocks: BTreeMap<usize, Block>,
    /// CPU buffers the title owns itself, by start address.
    foreign: BTreeMap<usize, Foreign>,
    next: u64,
    allocated: usize,
}

impl Drop for ApuSpace {
    fn drop(&mut self) {
        for (&address, block) in &self.blocks {
            // SAFETY: allocated in `alloc` with this layout and not freed since.
            unsafe { dealloc(address as *mut u8, block.layout) };
        }
    }
}

impl Default for ApuSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl ApuSpace {
    pub fn new() -> ApuSpace {
        ApuSpace {
            heaps: Vec::new(),
            blocks: BTreeMap::new(),
            foreign: BTreeMap::new(),
            next: APU_BASE,
            allocated: 0,
        }
    }

    /// Creates a heap and returns its number (from 1). Allocations are not limited to its size.
    pub fn create_heap(&mut self, kind: u32, size: usize) -> u32 {
        self.heaps.push((kind, size));
        self.heaps.len() as u32
    }

    pub fn heap_size(&self) -> usize {
        self.heaps.iter().map(|h| h.1).sum()
    }

    /// Bytes allocated and not yet freed.
    pub fn allocated(&self) -> usize {
        self.allocated
    }

    fn take_addresses(&mut self, size: usize, align: usize) -> Result<u32, ApuError> {
        let start = self.next.next_multiple_of(align as u64);
        let end = start + size.max(1) as u64;
        if end > u64::from(u32::MAX) {
            return Err(ApuError::OutOfAddresses);
        }
        self.next = end;
        Ok(start as u32)
    }

    /// A zeroed block of host memory with a coprocessor address: `(cpu address, apu address)`.
    pub fn alloc(&mut self, size: usize, align: usize) -> Result<(usize, u32), ApuError> {
        if self.heaps.is_empty() {
            return Err(ApuError::NoHeap);
        }
        if size == 0 || size > MAX_SPAN || (align != 0 && !align.is_power_of_two()) {
            return Err(ApuError::Invalid);
        }
        let align = align.max(MIN_ALIGN);
        let layout = Layout::from_size_align(size, align).map_err(|_| ApuError::Invalid)?;
        let apu = self.take_addresses(size, align)?;
        // SAFETY: the layout has a non-zero size.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(ApuError::OutOfMemory(size));
        }
        let address = ptr as usize;
        self.blocks.insert(address, Block { size, apu, layout });
        self.allocated += size;
        Ok((address, apu))
    }

    /// Frees a block by its CPU address; false when it is not one.
    pub fn free(&mut self, address: usize) -> bool {
        let Some(block) = self.blocks.remove(&address) else {
            return false;
        };
        // SAFETY: allocated in `alloc` with this layout.
        unsafe { dealloc(address as *mut u8, block.layout) };
        self.allocated -= block.size;
        true
    }

    fn containing(&self, address: usize) -> Option<(usize, &Block)> {
        let (&base, block) = self.blocks.range(..=address).next_back()?;
        (address < base + block.size).then_some((base, block))
    }

    /// Whether the coprocessor can see `address`: any non-null buffer, ours or the title's own.
    pub fn is_valid(&self, address: usize) -> bool {
        address != 0
    }

    /// The coprocessor address of a CPU address, registering the buffer when it is not one of our blocks.
    pub fn map_virtual(&mut self, address: usize, size: usize) -> Option<u32> {
        if address == 0 {
            return None;
        }
        if let Some((base, block)) = self.containing(address) {
            return Some(block.apu + (address - base) as u32);
        }
        if let Some((&base, f)) = self.foreign.range(..=address).next_back()
            && address + size.max(1) <= base + f.size
        {
            return Some(f.apu + (address - base) as u32);
        }
        let start = address & !(FOREIGN_PAGE - 1);
        let span = (address - start + size.max(1)).clamp(1, MAX_SPAN);
        let span = span.next_multiple_of(FOREIGN_PAGE);
        let apu = self.take_addresses(span, FOREIGN_PAGE).ok()?;
        self.foreign.insert(start, Foreign { size: span, apu });
        Some(apu + (address - start) as u32)
    }

    /// The CPU address of a coprocessor address, `None` when nothing was mapped there.
    pub fn map_apu(&self, apu: u32) -> Option<usize> {
        let apu = u64::from(apu);
        let hit = |base: usize, start: u32, size: usize| {
            let start = u64::from(start);
            (start..start + size as u64)
                .contains(&apu)
                .then(|| base + (apu - start) as usize)
        };
        self.blocks
            .iter()
            .find_map(|(&base, b)| hit(base, b.apu, b.size))
            .or_else(|| {
                self.foreign
                    .iter()
                    .find_map(|(&base, f)| hit(base, f.apu, f.size))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_aligned_zeroed_and_map_both_ways() {
        let mut space = ApuSpace::new();
        assert_eq!(space.alloc(64, 0x800), Err(ApuError::NoHeap));
        assert_eq!(space.create_heap(0, 0x226e00), 1);
        let (cpu, apu) = space.alloc(0x1000, 0x800).unwrap();
        assert_eq!(cpu % 0x800, 0);
        assert_ne!(apu, 0);
        assert_eq!(apu % 0x800, 0);
        // SAFETY: the block is 0x1000 zeroed bytes we own.
        let bytes = unsafe { std::slice::from_raw_parts(cpu as *const u8, 0x1000) };
        assert!(bytes.iter().all(|b| *b == 0));
        assert_eq!(space.map_virtual(cpu + 0x10, 0x40), Some(apu + 0x10));
        assert_eq!(space.map_apu(apu + 0x10), Some(cpu + 0x10));
        let (cpu2, apu2) = space.alloc(0x100, 0).unwrap();
        assert!(apu2 >= apu + 0x1000);
        assert_eq!(space.map_apu(apu2), Some(cpu2));
        assert_eq!(space.allocated(), 0x1100);
        assert!(space.free(cpu));
        assert!(!space.free(cpu));
        assert_eq!(space.map_apu(apu), None);
        assert_eq!(space.allocated(), 0x100);
    }

    #[test]
    fn the_titles_own_buffers_get_stable_addresses() {
        let mut space = ApuSpace::new();
        let buffer = vec![0u8; 0x3000];
        let at = buffer.as_ptr() as usize;
        assert!(space.is_valid(at));
        assert!(!space.is_valid(0));
        let a = space.map_virtual(at + 0x20, 0x80).unwrap();
        let b = space.map_virtual(at + 0x20, 0x80).unwrap();
        assert_eq!(a, b);
        assert_eq!(space.map_apu(a), Some(at + 0x20));
        let c = space.map_virtual(at + 0x40, 0x40).unwrap();
        assert_eq!(c, a + 0x20);
        assert_eq!(space.map_virtual(0, 8), None);
    }

    #[test]
    fn bad_requests_fail() {
        let mut space = ApuSpace::new();
        space.create_heap(0, 1);
        assert_eq!(space.alloc(0, 0x800), Err(ApuError::Invalid));
        assert_eq!(space.alloc(8, 3), Err(ApuError::Invalid));
    }
}
