//! Memory the layer itself puts in the GPU's address space: translated shader programs, the memory behind
//! `AllocateUploadMemoryX`, and the array a root signature spills into. Each is committed host memory in the window
//! (an upload buffer), so the CPU pointer is the console address the GPU reads.

use std::sync::{Arc, Mutex, PoisonError};

use crate::execute::SpillAllocator;
use crate::list::UploadMemory;
use crate::pipeline::{CodeStore, PipelineError};
use crate::resource::desc::ResourceDesc;
use crate::resource::{HeapKind, ResourceId, Resources};

struct Chunk {
    id: ResourceId,
    base: u64,
    size: u64,
    used: u64,
}

fn new_chunk(resources: &Mutex<Resources>, size: u64) -> Option<Chunk> {
    let mut resources = resources.lock().unwrap_or_else(PoisonError::into_inner);
    let id = resources
        .create_committed(&ResourceDesc::buffer(size), HeapKind::Upload)
        .ok()?;
    let r = resources.get(id)?;
    Some(Chunk {
        id,
        base: r.address,
        size: r.size,
        used: 0,
    })
}

fn bump(chunk: &mut Chunk, size: u64, align: u64) -> Option<u64> {
    let at = (chunk.base + chunk.used).next_multiple_of(align);
    let end = at.checked_add(size)?;
    if end > chunk.base + chunk.size {
        return None;
    }
    chunk.used = end - chunk.base;
    Some(at)
}

const CODE_CHUNK: u64 = 16 << 20;

/// Where translated programs live. They are kept for the life of the device: a program's address is part of its
/// pipeline.
pub struct CodeArena {
    resources: Arc<Mutex<Resources>>,
    chunks: Mutex<Vec<Chunk>>,
}

impl CodeArena {
    pub fn new(resources: Arc<Mutex<Resources>>) -> CodeArena {
        CodeArena {
            resources,
            chunks: Mutex::new(Vec::new()),
        }
    }
}

impl CodeStore for CodeArena {
    fn upload(&self, bytes: &[u8]) -> Result<u64, PipelineError> {
        let mut chunks = self.chunks.lock().unwrap_or_else(PoisonError::into_inner);
        let size = bytes.len() as u64;
        let at = match chunks.last_mut().and_then(|c| bump(c, size, 256)) {
            Some(at) => at,
            None => {
                let chunk = new_chunk(&self.resources, CODE_CHUNK.max(size + 256))
                    .ok_or_else(|| PipelineError::Upload("no memory for shader code".into()))?;
                chunks.push(chunk);
                bump(chunks.last_mut().expect("just pushed"), size, 256)
                    .ok_or_else(|| PipelineError::Upload("shader code does not fit".into()))?
            }
        };
        // SAFETY: `at..at + size` is inside a committed upload chunk of this arena, which nothing else writes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as usize as *mut u8, bytes.len())
        };
        Ok(at)
    }
}

const UPLOAD_CHUNK: u64 = 32 << 20;
const UPLOAD_CHUNKS: usize = 8;

/// The memory `AllocateUploadMemoryX` hands out: a ring of chunks. How long the title keeps such memory is not
/// measured; a chunk is taken back once the ring has gone round, which is `UPLOAD_CHUNKS * UPLOAD_CHUNK` bytes of
/// later allocations.
pub struct UploadArena {
    resources: Arc<Mutex<Resources>>,
    ring: Mutex<Ring>,
}

#[derive(Default)]
struct Ring {
    chunks: Vec<Chunk>,
    current: usize,
}

impl UploadArena {
    pub fn new(resources: Arc<Mutex<Resources>>) -> UploadArena {
        UploadArena {
            resources,
            ring: Mutex::new(Ring::default()),
        }
    }
}

impl UploadMemory for UploadArena {
    fn allocate(&self, size: usize, alignment: usize) -> Option<usize> {
        let (size, align) = (size as u64, alignment.max(16) as u64);
        let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        if ring.chunks.is_empty() {
            ring.chunks.push(new_chunk(&self.resources, UPLOAD_CHUNK)?);
        }
        let current = ring.current;
        if let Some(at) = bump(&mut ring.chunks[current], size, align) {
            return Some(at as usize);
        }
        if size + align > UPLOAD_CHUNK {
            let mut big = new_chunk(&self.resources, size + align)?;
            let at = bump(&mut big, size, align)?;
            let slot = ring.current;
            let old = std::mem::replace(&mut ring.chunks[slot], big);
            self.retire(old);
            return Some(at as usize);
        }
        let next = (current + 1) % UPLOAD_CHUNKS;
        if next >= ring.chunks.len() {
            let chunk = new_chunk(&self.resources, UPLOAD_CHUNK)?;
            ring.chunks.push(chunk);
        }
        ring.current = next;
        ring.chunks[next].used = 0;
        bump(&mut ring.chunks[next], size, align).map(|a| a as usize)
    }
}

impl UploadArena {
    fn retire(&self, chunk: Chunk) {
        let _ = self
            .resources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .destroy(chunk.id);
    }
}

const SPILL_BYTES: u64 = 8 << 20;

/// The array root arguments spill into, reset at the start of every submission: replay waits for the GPU, so
/// nothing reads the last submission's arrays any more.
pub struct SpillRing {
    chunk: Option<Chunk>,
}

impl SpillRing {
    pub fn new(resources: &Mutex<Resources>) -> SpillRing {
        SpillRing {
            chunk: new_chunk(resources, SPILL_BYTES),
        }
    }

    pub fn reset(&mut self) {
        if let Some(c) = &mut self.chunk {
            c.used = 0;
        }
    }
}

impl SpillAllocator for SpillRing {
    fn store(&mut self, bytes: &[u8]) -> Option<u64> {
        let chunk = self.chunk.as_mut()?;
        let at = bump(chunk, bytes.len() as u64, 16)?;
        // SAFETY: `at..at + len` is inside the committed upload chunk this ring owns.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as usize as *mut u8, bytes.len())
        };
        Some(at)
    }
}
