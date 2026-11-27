//! Keeping the device-local window and the host memory equal, for a mirrored window.
//!
//! Two kinds of change are tracked, each as address ranges:
//!
//! - **dirty**: the CPU wrote, so the host bytes are newer. Found by `mark_dirty` (exact bytes), by Windows'
//!   write watch (4 KiB pages), or by treating a whole region as written (`SyncPolicy`). They are copied to the
//!   window by `submit_uploads` (on the transfer queue) and `record_uploads` (on the main queue).
//! - **stale**: the GPU may have written, so the window bytes are newer. The caller says so with `note_gpu_write`.
//!   Stale bytes are copied back to the host memory when the CPU may read them, as `HostAccess` says.
//!
//! Rules that keep both sides from seeing old bytes:
//!
//! 1. Uploads never copy stale bytes. A page the CPU wrote that also holds GPU results nobody has read yet (a
//!    small buffer next to constants) only has its other bytes copied, so the results survive. The CPU wins over
//!    stale bytes only through `mark_dirty`, which names exactly what it wrote and discards the GPU's results there.
//!    A CPU that rewrites a range the GPU wrote without ever reading it must therefore call `mark_dirty`, unless
//!    the range is `HostAccess::Eager`, where the results are copied back after every submission.
//! 2. Dirty bytes in ranges the GPU has ever written (`note_gpu_write`) are uploaded on the main queue, in order
//!    with the GPU's writes and downloads, never on the transfer queue where they could run beside them.
//! 3. `note_gpu_write` is called after the submission's uploads are recorded, and the downloads for it are recorded
//!    after the GPU work that writes (`record_pending_downloads`).
//! 4. A CPU read of bytes that are stale and not `Eager` must be preceded by `read_back`.
//!
//! What the CPU will read decides what is downloaded. `Eager` (the default, always sound) is copied back after
//! every submission that wrote it. `OnDemand` is copied back only by `read_back`, called when the CPU maps or reads
//! it after a fence wait: a readback heap the game reads now and then. `Never` is GPU-only memory, such as a default
//! heap resource: it is never copied back, and writes the CPU makes to it are only believed through `mark_dirty`.

use ash::vk;

use super::dirty::{RangeSet, Scanner, coalesce, intersect, subtract};
use super::transfer::Copies;
use super::*;

/// Whether the CPU will read what the GPU wrote in a range, which decides when it is copied back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HostAccess {
    /// May be read at any time: copied back after every submission that wrote it.
    #[default]
    Eager,
    /// Read now and then: copied back by `read_back`, which the CPU's access must be preceded by.
    OnDemand,
    /// Never read, and not written without `mark_dirty`: never copied back.
    Never,
}

/// Dirty ranges split by the queue that will copy them.
struct Plan {
    /// To be copied in the main queue's command buffer, in order with the GPU's own work.
    inorder: Vec<(u64, u64)>,
    /// Free of the GPU's writes: may be copied on the transfer queue.
    offloaded: Vec<(u64, u64)>,
    /// Some of it is where the GPU has written before, so the copies wait for the GPU's earlier work.
    after_gpu: bool,
}

#[derive(Default)]
pub(super) struct SyncState {
    /// The CPU wrote these exact bytes: a new mapping, `mark_dirty`, a rebuilt window.
    pub(super) dirty: RangeSet,
    stale: RangeSet,
    noted: RangeSet,
    gpu_ever: RangeSet,
    on_demand: RangeSet,
    never: RangeSet,
    scanner: Scanner,
    plan: Option<Plan>,
    pool: Option<vk::CommandPool>,
}

impl SyncState {
    /// Forgets everything about `lo..hi`, which is no longer bound.
    pub(super) fn forget(&mut self, lo: u64, hi: u64) {
        for set in [
            &mut self.dirty,
            &mut self.stale,
            &mut self.noted,
            &mut self.gpu_ever,
            &mut self.on_demand,
            &mut self.never,
        ] {
            set.remove(lo, hi);
        }
        if let Some(plan) = &mut self.plan {
            for runs in [&mut plan.inorder, &mut plan.offloaded] {
                let mut gone = RangeSet::default();
                gone.add(lo, hi);
                *runs = subtract(runs, &gone);
            }
        }
    }

    pub(super) fn destroy(&mut self, device: &ash::Device) {
        if let Some(pool) = self.pool.take() {
            // SAFETY: the GPU is idle.
            unsafe { device.destroy_command_pool(pool, None) };
        }
    }
}

impl AddressSpace {
    /// Tells a mirrored window the CPU wrote `va..va + len`. The CPU's bytes win over the GPU's results there.
    pub fn mark_dirty(&mut self, va: u64, len: u64) {
        if self.backing == Backing::Mirrored {
            let state = &mut self.sync;
            state.dirty.add(va, va + len);
            state.stale.remove(va, va + len);
            state.noted.remove(va, va + len);
        }
    }

    /// Says the commands being recorded may write `va..va + len`. Call it after the submission's uploads were
    /// recorded.
    pub fn note_gpu_write(&mut self, va: u64, len: u64) {
        if self.backing == Backing::Mirrored {
            self.sync.noted.add(va, va + len);
        }
    }

    /// Says whether the CPU will read what the GPU wrote in `va..va + len`; see `HostAccess`.
    pub fn set_host_access(&mut self, va: u64, len: u64, access: HostAccess) {
        let state = &mut self.sync;
        state.never.remove(va, va + len);
        state.on_demand.remove(va, va + len);
        match access {
            HostAccess::Eager => {}
            HostAccess::OnDemand => state.on_demand.add(va, va + len),
            HostAccess::Never => state.never.add(va, va + len),
        }
    }

    /// The bytes the GPU wrote that the host memory has not received yet.
    pub fn stale_bytes(&self) -> u64 {
        self.sync
            .stale
            .0
            .iter()
            .chain(self.sync.noted.0.iter())
            .map(|(lo, hi)| hi - lo)
            .sum()
    }

    /// Everything the CPU wrote since the last call, as sorted ranges of game addresses, without what the GPU
    /// wrote and the CPU has not seen.
    fn collect_dirty(&mut self) -> Vec<(u64, u64)> {
        let state = &mut self.sync;
        let mut explicit = state.dirty.take();
        let mut implicit = Vec::new();
        let mut host_runs = Vec::new();
        for (&va, extent) in &self.extents.0 {
            let Some(memory) = self.memories.get_mut(&extent.memory) else {
                continue;
            };
            match memory.policy {
                SyncPolicy::Marked => {}
                SyncPolicy::Always => implicit.push((va, va + extent.len)),
                SyncPolicy::WriteWatch => {
                    let host = memory.host + extent.memory_offset;
                    host_runs.clear();
                    if state.scanner.scan(
                        &mut memory.tiles,
                        host,
                        extent.len,
                        extent.memory_offset,
                        &mut host_runs,
                    ) {
                        implicit.extend(host_runs.iter().map(|&(lo, hi)| {
                            (va + (lo - host), (va + (hi - host)).min(va + extent.len))
                        }));
                    } else {
                        implicit.push((va, va + extent.len));
                    }
                }
            }
        }
        coalesce(&mut implicit);
        let mut runs = subtract(&implicit, &state.never);
        runs.append(&mut explicit);
        coalesce(&mut runs);
        subtract(&runs, &state.stale)
    }

    /// Decides what this submission copies, once: the first of `submit_uploads` and `record_uploads` does it.
    fn plan_uploads(&mut self) {
        if self.sync.plan.is_some() {
            return;
        }
        let runs = self.collect_dirty();
        let state = &mut self.sync;
        let touched = intersect(&runs, &state.gpu_ever);
        let plan = if self.transfer.is_some() {
            let offloaded = subtract(&runs, &state.gpu_ever);
            Plan {
                after_gpu: !touched.is_empty(),
                inorder: touched,
                offloaded,
            }
        } else {
            Plan {
                after_gpu: !touched.is_empty(),
                inorder: runs,
                offloaded: Vec::new(),
            }
        };
        self.merge_noted();
        self.sync.plan = Some(plan);
    }

    fn merge_noted(&mut self) {
        let state = &mut self.sync;
        for (lo, hi) in state.noted.take() {
            state.stale.add(lo, hi);
            state.gpu_ever.add(lo, hi);
        }
    }

    /// Copies, on the transfer queue, what the CPU wrote since the last call and the GPU has not written, and
    /// returns what the submission that reads it must wait for. `None` when there is nothing to copy or no
    /// transfer queue; `record_uploads` then (or also, for bytes the GPU writes) copies in the command buffer.
    pub fn submit_uploads(&mut self) -> Result<Option<TimelineWait>, MemoryError> {
        if self.backing == Backing::Direct || self.transfer.is_none() {
            return Ok(None);
        }
        self.plan_uploads();
        let runs = std::mem::take(&mut self.sync.plan.as_mut().expect("planned above").offloaded);
        if runs.is_empty() {
            return Ok(None);
        }
        let copies = self.copies_for(&runs, true);
        let transfer = self.transfer.as_mut().expect("checked above");
        match transfer.submit(&self.h.device, &copies) {
            Ok(wait) => Ok(Some(wait)),
            Err(e) => {
                for (lo, hi) in runs {
                    self.sync.dirty.add(lo, hi);
                }
                Err(e)
            }
        }
    }

    /// Records into `command` the copies to the window of what the CPU wrote since the last call, so the commands
    /// recorded after these see it. Nothing is recorded when the window is direct. Returns the bytes copied.
    ///
    /// With a transfer queue, `submit_uploads` has taken what it can; this copies the rest, which is what the GPU
    /// writes too.
    pub fn record_uploads(&mut self, command: vk::CommandBuffer) -> u64 {
        if self.backing == Backing::Direct {
            return 0;
        }
        self.plan_uploads();
        let plan = self.sync.plan.take().expect("planned above");
        let mut runs = plan.inorder;
        runs.extend(plan.offloaded);
        coalesce(&mut runs);
        if runs.is_empty() {
            return 0;
        }
        if plan.after_gpu {
            self.barrier(
                command,
                vk::AccessFlags::MEMORY_WRITE,
                vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::TRANSFER_READ,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
            );
        }
        let bytes = self.record_copies(command, &runs, true);
        if bytes > 0 {
            self.barrier(
                command,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
            );
        }
        bytes
    }

    /// Copies what the GPU wrote in `va..va + len` back to the host memory, whether or not it was noted. The CPU
    /// may read it once the commands have finished. Nothing is recorded when the window is direct. Returns the
    /// bytes copied.
    pub fn record_downloads(&mut self, command: vk::CommandBuffer, va: u64, len: u64) -> u64 {
        if self.backing == Backing::Direct {
            return 0;
        }
        self.sync.stale.remove(va, va + len);
        self.sync.noted.remove(va, va + len);
        self.record_download_runs(command, &[(va, va + len)])
    }

    /// Copies back what the GPU wrote and the CPU may read (`HostAccess::Eager`): the ranges noted with
    /// `note_gpu_write` and not yet downloaded. Record it after the commands that write. Returns the bytes copied.
    pub fn record_pending_downloads(&mut self, command: vk::CommandBuffer) -> u64 {
        if self.backing == Backing::Direct {
            return 0;
        }
        self.merge_noted();
        let state = &mut self.sync;
        let stale: Vec<(u64, u64)> = state.stale.0.iter().map(|(a, b)| (*a, *b)).collect();
        let eager = subtract(&subtract(&stale, &state.never), &state.on_demand);
        for &(lo, hi) in &eager {
            state.stale.remove(lo, hi);
        }
        self.record_download_runs(command, &eager)
    }

    fn record_download_runs(&self, command: vk::CommandBuffer, runs: &[(u64, u64)]) -> u64 {
        if runs.is_empty() {
            return 0;
        }
        self.barrier(
            command,
            vk::AccessFlags::MEMORY_WRITE,
            vk::AccessFlags::TRANSFER_READ,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::TRANSFER,
        );
        let bytes = self.record_copies(command, runs, false);
        self.barrier(
            command,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::HOST_READ,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::HOST,
        );
        bytes
    }

    /// Copies back, now and waiting for it, what the GPU wrote in `va..va + len` that the host has not received,
    /// so the CPU can read it. The GPU commands that write it must have been submitted. Returns the bytes copied.
    pub fn read_back(&mut self, va: u64, len: u64) -> Result<u64, MemoryError> {
        if self.backing == Backing::Direct {
            return Ok(0);
        }
        self.merge_noted();
        let runs = self.sync.stale.pieces(va, va + len);
        if runs.is_empty() {
            return Ok(0);
        }
        let device = &self.h.device;
        // SAFETY: the pool and the command buffer are made and freed here; the queue is used under its lock and
        // the fence is waited on and destroyed here.
        unsafe {
            let pool = match self.sync.pool {
                Some(pool) => pool,
                None => {
                    let pool = device.create_command_pool(
                        &vk::CommandPoolCreateInfo::default()
                            .queue_family_index(self.h.queue_family)
                            .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                        None,
                    )?;
                    self.sync.pool = Some(pool);
                    pool
                }
            };
            let command = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )?[0];
            let commands = [command];
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let waited = (|| -> Result<(), vk::Result> {
                device.begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                self.record_download_runs(command, &runs);
                device.end_command_buffer(command)?;
                {
                    let _guard = self.h.queue_lock.lock().unwrap_or_else(|e| e.into_inner());
                    device.queue_submit(
                        self.h.queue,
                        &[vk::SubmitInfo::default().command_buffers(&commands)],
                        fence,
                    )?;
                }
                device.wait_for_fences(&[fence], true, u64::MAX)
            })();
            device.destroy_fence(fence, None);
            device.free_command_buffers(pool, &commands);
            waited?;
        }
        for &(lo, hi) in &runs {
            self.sync.stale.remove(lo, hi);
        }
        Ok(runs.iter().map(|(lo, hi)| hi - lo).sum())
    }

    pub(super) fn barrier(
        &self,
        command: vk::CommandBuffer,
        from: vk::AccessFlags,
        to: vk::AccessFlags,
        src: vk::PipelineStageFlags,
        dst: vk::PipelineStageFlags,
    ) {
        let barrier = [vk::MemoryBarrier::default()
            .src_access_mask(from)
            .dst_access_mask(to)];
        // SAFETY: the command buffer is in the recording state, as the caller promises.
        unsafe {
            self.h.device.cmd_pipeline_barrier(
                command,
                src,
                dst,
                vk::DependencyFlags::empty(),
                &barrier,
                &[],
                &[],
            );
        }
    }

    /// The copy regions for `ranges` (sorted game addresses), grouped by the host buffer they use.
    fn copies_for(&self, ranges: &[(u64, u64)], upload: bool) -> Copies {
        let mut by_buffer: Copies = Vec::new();
        for &(lo, hi) in ranges {
            for (start, extent, from, to) in self.extents.overlapping(lo, hi) {
                let Some(mirror) = self
                    .memories
                    .get(&extent.memory)
                    .and_then(|m| m.mirror.as_ref())
                else {
                    continue;
                };
                let host_offset = extent.memory_offset + (from - start);
                let window_offset = from - self.window.base;
                let (src, dst) = if upload {
                    (host_offset, window_offset)
                } else {
                    (window_offset, host_offset)
                };
                let copy = vk::BufferCopy::default()
                    .src_offset(src)
                    .dst_offset(dst)
                    .size(to - from);
                let (from_buffer, to_buffer) = if upload {
                    (mirror.buffer, self.window.buffer)
                } else {
                    (self.window.buffer, mirror.buffer)
                };
                match by_buffer
                    .iter_mut()
                    .rev()
                    .take(2)
                    .find(|(s, d, _)| *s == from_buffer && *d == to_buffer)
                {
                    Some((_, _, copies)) => copies.push(copy),
                    None => by_buffer.push((from_buffer, to_buffer, vec![copy])),
                }
            }
        }
        by_buffer
    }

    fn record_copies(
        &self,
        command: vk::CommandBuffer,
        ranges: &[(u64, u64)],
        upload: bool,
    ) -> u64 {
        let mut bytes = 0;
        for (src, dst, copies) in self.copies_for(ranges, upload) {
            bytes += copies.iter().map(|c| c.size).sum::<u64>();
            // SAFETY: the command buffer is recording; both buffers are alive and the offsets are in range.
            unsafe { self.h.device.cmd_copy_buffer(command, src, dst, &copies) };
        }
        bytes
    }
}
