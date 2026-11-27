//! Recording several command lists at once, one worker thread each, into primary command buffers that are
//! submitted in list order.
//!
//! A D3D12 command list starts from no state, whatever ran before it on the queue, so lists record independently:
//! each gets its own `Replayer` and its own command buffer from a pool no other thread touches. Order lives in
//! the submission, which takes the returned buffers as they are.

use std::sync::atomic::{AtomicUsize, Ordering};

use ash::vk;

use super::command::Command;
use super::{ReplayEnv, ReplayError, ReplayReport, Replayer, VkContext};

struct Pool {
    pool: vk::CommandPool,
    buffers: Vec<vk::CommandBuffer>,
    used: usize,
}

/// Command pools for recording, one per worker, reused from frame to frame.
pub struct RecordPools {
    device: ash::Device,
    pools: Vec<Pool>,
}

impl RecordPools {
    pub fn new(ctx: &VkContext, workers: usize) -> Result<RecordPools, ReplayError> {
        let mut pools = RecordPools {
            device: ctx.device.clone(),
            pools: Vec::new(),
        };
        for _ in 0..workers.max(1) {
            // SAFETY: a plain create call on a live device; the pool is destroyed in Drop.
            let pool = unsafe {
                ctx.device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.family),
                    None,
                )?
            };
            pools.pools.push(Pool {
                pool,
                buffers: Vec::new(),
                used: 0,
            });
        }
        Ok(pools)
    }

    pub fn workers(&self) -> usize {
        self.pools.len()
    }

    /// Takes back every buffer handed out, for the next batch of lists.
    ///
    /// # Safety
    /// No submission that uses a buffer recorded from these pools may still be pending.
    pub unsafe fn reset(&mut self) -> Result<(), ReplayError> {
        for pool in &mut self.pools {
            // SAFETY: the caller guarantees nothing uses the pool's buffers.
            unsafe {
                self.device
                    .reset_command_pool(pool.pool, vk::CommandPoolResetFlags::empty())?
            };
            pool.used = 0;
        }
        Ok(())
    }
}

impl Drop for RecordPools {
    fn drop(&mut self) {
        for pool in &self.pools {
            // SAFETY: the owner finished with the buffers before dropping the pools.
            unsafe { self.device.destroy_command_pool(pool.pool, None) };
        }
    }
}

/// One recorded list: a command buffer ready to submit, and what the replay did.
#[derive(Debug)]
pub struct Recorded {
    pub buffer: vk::CommandBuffer,
    pub report: ReplayReport,
}

fn record_one<'a>(
    ctx: &'a VkContext,
    pool: &mut Pool,
    env: ReplayEnv<'a>,
    commands: &[Command],
) -> Result<Recorded, ReplayError> {
    let d = &ctx.device;
    // SAFETY: the pool belongs to this worker alone; the buffer is recorded and ended before it is returned.
    unsafe {
        if pool.used == pool.buffers.len() {
            let more = d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool.pool)
                    .command_buffer_count(1),
            )?;
            pool.buffers.extend(more);
        }
        let buffer = pool.buffers[pool.used];
        pool.used += 1;
        d.begin_command_buffer(
            buffer,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        let report = Replayer::new(ctx, env, buffer).record(commands)?;
        d.end_command_buffer(buffer)?;
        Ok(Recorded { buffer, report })
    }
}

/// Records each of `lists` into its own command buffer, in parallel on up to `pools.workers()` threads, and
/// returns them in list order. `make_env` builds the environment of list `i` on the thread that records it, so
/// each list can have its own spill space.
pub fn record_lists<'e>(
    ctx: &'e VkContext,
    pools: &mut RecordPools,
    lists: &[&[Command]],
    make_env: &(dyn Fn(usize) -> ReplayEnv<'e> + Sync),
) -> Result<Vec<Recorded>, ReplayError> {
    let workers = pools.pools.len().min(lists.len());
    if workers <= 1 {
        let Some(pool) = pools.pools.first_mut() else {
            return Ok(Vec::new());
        };
        return lists
            .iter()
            .enumerate()
            .map(|(i, commands)| record_one(ctx, pool, make_env(i), commands))
            .collect();
    }
    let next = AtomicUsize::new(0);
    let mut slots: Vec<Option<Result<Recorded, ReplayError>>> = Vec::new();
    slots.resize_with(lists.len(), || None);
    let done = std::thread::scope(|scope| {
        let handles: Vec<_> = pools.pools[..workers]
            .iter_mut()
            .map(|pool| {
                let next = &next;
                scope.spawn(move || {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= lists.len() {
                            break;
                        }
                        out.push((i, record_one(ctx, pool, make_env(i), lists[i])));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a recording thread panicked"))
            .collect::<Vec<_>>()
    });
    for (i, result) in done.into_iter().flatten() {
        slots[i] = Some(result);
    }
    slots
        .into_iter()
        .map(|slot| slot.expect("every list is recorded by some worker"))
        .collect()
}
