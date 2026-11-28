//! The spill tier of a mirrored window: runs of the game's memory that live in system memory, bound straight into
//! the window, because device-local memory is short. See the module documentation of `memory`.
//!
//! A spilled run is the imported host memory itself (the window's binding there is the import, as in a direct
//! window), so it has no mirror: nothing is uploaded or downloaded for it, the sync code skips it, and the GPU and
//! the CPU share its bytes. `HostAccess` does not matter for it and `read_back` finds nothing to copy.

use super::*;

/// The memory type a spilled run is imported as: the title's own pages are cached write-back memory, and the GPU
/// reads and writes them coherently with the CPU's caches.
const SPILL_MEMORY: vk::MemoryPropertyFlags = vk::MemoryPropertyFlags::from_raw(
    vk::MemoryPropertyFlags::HOST_VISIBLE.as_raw() | vk::MemoryPropertyFlags::HOST_CACHED.as_raw(),
);

/// The most one `promote_spilled` call mirrors, so the uploads of a frame stay bounded.
const PROMOTE_PER_CALL: u64 = 256 << 20;

/// A mapped run that is one memory object over its whole length.
struct Whole {
    va: u64,
    id: u64,
    len: u64,
}

impl AddressSpace {
    /// Whether runs past the mirror's share of the budget are placed in system memory.
    pub fn spill_enabled(&self) -> bool {
        self.spill
    }

    /// Turns the spill tier on or off. It stays off where the GPU does not take imported host memory in its
    /// sparse buffers (`spill_works`), and in a direct window, which has no mirror to avoid.
    pub fn set_spill(&mut self, on: bool) {
        self.spill = on && self.backing == Backing::Mirrored && self.spill_works();
    }

    /// The bytes of the game's memory that live in system memory because device-local memory was short.
    pub fn spilled_bytes(&self) -> u64 {
        self.extents
            .0
            .values()
            .filter(|e| self.memories.get(&e.memory).is_some_and(Memory::is_spilled))
            .map(|e| e.len)
            .sum()
    }

    /// Imports `len` bytes of the game's memory for the window itself, with no device-local copy.
    pub(super) fn import_spilled(
        &self,
        host: u64,
        len: u64,
    ) -> Result<(vk::DeviceMemory, Option<Mirror>, Grant), MemoryError> {
        // SAFETY: the caller of `map_host` keeps the memory mapped until `unmap`.
        let (memory, _) = unsafe { self.import_memory(host, len, true, SPILL_MEMORY)? };
        Ok((memory, None, self.h.budget.record_spill(Class::Mirror, len)))
    }

    /// Binds a page of imported host memory into a throwaway sparse buffer made like the window and checks that
    /// the GPU copies what the CPU wrote and that the CPU sees what the GPU wrote.
    pub(super) fn spill_works(&self) -> bool {
        let layout = std::alloc::Layout::from_size_align(PAGE as usize, PAGE as usize)
            .expect("a valid layout");
        // SAFETY: a page-aligned allocation that stays alive and unmoved until the GPU is done with it.
        unsafe {
            let host = std::alloc::alloc_zeroed(layout);
            if host.is_null() {
                return false;
            }
            let ok = self
                .spill_roundtrip(host as u64, host as *mut u32)
                .unwrap_or(false);
            std::alloc::dealloc(host, layout);
            ok
        }
    }

    /// # Safety
    /// `host` is `PAGE` bytes of memory at `words`, stable until this returns.
    unsafe fn spill_roundtrip(&self, host: u64, words: *mut u32) -> Result<bool, vk::Result> {
        const COPIED: u32 = 0xA5A5_0000;
        const FILLED: u32 = 0x5EED_5EED;
        let device = &self.h.device;
        // SAFETY: as the caller promises.
        unsafe {
            for i in 0..1024 {
                words.add(i).write_volatile(COPIED | i as u32);
            }
        }
        // SAFETY: as the caller promises.
        let Ok((memory, _)) = (unsafe { self.import_memory(host, PAGE, true, SPILL_MEMORY) })
        else {
            return Ok(false);
        };
        let Ok(probe) = Self::make_window(&self.h, 0, PAGE, false, &self.families) else {
            // SAFETY: just made, never bound.
            unsafe { device.free_memory(memory, None) };
            return Ok(false);
        };
        let run = || -> Result<(), vk::Result> {
            // SAFETY: fresh objects, destroyed before this returns; the queue is used under its lock and each
            // fence is waited on and destroyed here.
            unsafe {
                let binds = [vk::SparseMemoryBind::default().size(PAGE).memory(memory)];
                let buffer_binds = [vk::SparseBufferMemoryBindInfo::default()
                    .buffer(probe.buffer)
                    .binds(&binds)];
                let info = [vk::BindSparseInfo::default().buffer_binds(&buffer_binds)];
                let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
                let bound = {
                    let _guard = self.h.queue_lock.lock().unwrap_or_else(|e| e.into_inner());
                    device.queue_bind_sparse(self.h.queue, &info, fence)
                };
                let waited = bound.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
                device.destroy_fence(fence, None);
                waited?;
                let pool = device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(self.h.queue_family),
                    None,
                )?;
                let done = (|| -> Result<(), vk::Result> {
                    let command = device.allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .command_buffer_count(1),
                    )?[0];
                    device.begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())?;
                    device.cmd_copy_buffer(
                        command,
                        probe.buffer,
                        probe.buffer,
                        &[vk::BufferCopy::default().dst_offset(4096).size(4096)],
                    );
                    device.cmd_fill_buffer(command, probe.buffer, 8192, 4096, FILLED);
                    device.end_command_buffer(command)?;
                    let commands = [command];
                    let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
                    let submitted = {
                        let _guard = self.h.queue_lock.lock().unwrap_or_else(|e| e.into_inner());
                        device.queue_submit(
                            self.h.queue,
                            &[vk::SubmitInfo::default().command_buffers(&commands)],
                            fence,
                        )
                    };
                    let waited =
                        submitted.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
                    device.destroy_fence(fence, None);
                    waited
                })();
                device.destroy_command_pool(pool, None);
                done
            }
        };
        let result = run();
        // SAFETY: the GPU is done with both.
        unsafe {
            device.destroy_buffer(probe.buffer, None);
            device.free_memory(memory, None);
        }
        result?;
        // SAFETY: as the caller promises.
        let seen = unsafe {
            (0..1024).all(|i| words.add(1024 + i).read_volatile() == COPIED | i as u32)
                && (0..1024).all(|i| words.add(2048 + i).read_volatile() == FILLED)
        };
        Ok(seen)
    }

    /// The runs that are one memory object over their whole length, in address order.
    fn whole_runs(&self) -> Vec<Whole> {
        let mut count: HashMap<u64, u32> = HashMap::new();
        for e in self.extents.0.values() {
            *count.entry(e.memory).or_default() += 1;
        }
        self.extents
            .0
            .iter()
            .filter(|(_, e)| {
                count[&e.memory] == 1
                    && e.memory_offset == 0
                    && self
                        .memories
                        .get(&e.memory)
                        .is_some_and(|m| m.live == e.len)
            })
            .map(|(va, e)| Whole {
                va: *va,
                id: e.memory,
                len: e.len,
            })
            .collect()
    }

    /// Mirrors spilled runs again, those the resource layer called hot first and then lowest address first, while
    /// the budget has room for them, and returns the bytes done. The copies to device-local memory are made by the
    /// next `record_uploads`. Call it when device-local memory has been freed; `reclaim` does.
    pub fn promote_spilled(&mut self) -> Result<u64, MemoryError> {
        let mut done = 0;
        let mut room = self.h.budget.mirror_room();
        self.promote_more = false;
        let mut waiting: Vec<Whole> = self
            .whole_runs()
            .into_iter()
            .filter(|r| {
                self.memories
                    .get(&r.id)
                    .is_some_and(|m| m.is_spilled() && !m.pinned)
            })
            .collect();
        waiting.sort_by_key(|r| (!self.temp.is_hot(r.va, r.va + r.len), r.va));
        for run in waiting {
            if run.len > room {
                continue;
            }
            if done + run.len > PROMOTE_PER_CALL {
                self.promote_more = true;
                break;
            }
            let host = self.memories[&run.id].host;
            let (handle, mirror, grant) = match self.import_mirrored(host, run.len) {
                Ok(made) => made,
                Err(MemoryError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)) => break,
                Err(e) => return Err(e),
            };
            let memory = self.memories.get_mut(&run.id).expect("found above");
            self.retired_memory
                .push(std::mem::replace(&mut memory.handle, handle));
            if let Some(spilled) = memory.grant.replace(grant) {
                self.h.budget.release(&spilled);
            }
            memory.mirror = mirror;
            memory.tiles = Tiles::default();
            self.pending.push(Bind {
                offset: run.va - self.window.base,
                size: run.len,
                memory: handle,
                memory_offset: 0,
            });
            self.sync.dirty.add(run.va, run.va + run.len);
            done += run.len;
            room -= run.len;
        }
        self.commit()?;
        Ok(done)
    }
}

impl Memory {
    /// Whether the run is system memory bound into the window, with no device-local copy.
    pub(super) fn is_spilled(&self) -> bool {
        self.grant
            .is_some_and(|g| g.placement == Placement::Spilled)
    }
}
