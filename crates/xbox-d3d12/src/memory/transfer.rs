//! A queue of its own for the uploads of a mirrored window, so they run while the main queue renders the
//! previous frame. Uploads are ordered with the work that reads them by a timeline semaphore.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ash::vk;

use super::MemoryError;

/// The queue uploads are copied on. A family other than the main queue's (a dedicated DMA family) lets the copy
/// engine run beside rendering; a second queue of the main family may or may not run concurrently.
#[derive(Clone)]
pub struct TransferQueue {
    pub queue: vk::Queue,
    pub family: u32,
    /// Held while submitting, for code that shares the queue.
    pub lock: Arc<Mutex<()>>,
}

/// What the submission that uses an upload must wait for: `value` of the timeline `semaphore`, at any stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineWait {
    pub semaphore: vk::Semaphore,
    pub value: u64,
}

/// A queue the device must be created with for `AddressSpace::new_with_transfer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferPick {
    pub family: u32,
    /// The queue index inside the family: 1 when the family is the main one, so a second queue is needed.
    pub queue_index: u32,
}

/// The queue family uploads should be copied on: a transfer-only family if the GPU has one, else a second queue
/// of the main family when it has two. `None` when neither exists. The device must be created with one queue
/// at `queue_index` of `family` (so `queue_index + 1` queues when it is in the main family) and with Vulkan
/// 1.2's `timelineSemaphore` feature.
pub fn pick_transfer_family(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    main_family: u32,
) -> Option<TransferPick> {
    // SAFETY: `physical` came from `instance`.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    let dedicated = families
        .iter()
        .enumerate()
        .filter(|(i, f)| {
            *i as u32 != main_family
                && f.queue_flags.contains(vk::QueueFlags::TRANSFER)
                && !f
                    .queue_flags
                    .intersects(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                && f.queue_flags.as_raw().count_ones() <= 2
        })
        .min_by_key(|(_, f)| f.queue_flags.as_raw().count_ones())
        .map(|(i, _)| TransferPick {
            family: i as u32,
            queue_index: 0,
        });
    dedicated.or_else(|| {
        let main = families.get(main_family as usize)?;
        (main.queue_count >= 2).then_some(TransferPick {
            family: main_family,
            queue_index: 1,
        })
    })
}

/// The copies for one batch: `(source, destination, regions)`.
pub(super) type Copies = Vec<(vk::Buffer, vk::Buffer, Vec<vk::BufferCopy>)>;

pub(super) struct Transfer {
    queue: TransferQueue,
    pool: vk::CommandPool,
    semaphore: vk::Semaphore,
    submitted: u64,
    busy: VecDeque<(vk::CommandBuffer, u64)>,
    free: Vec<vk::CommandBuffer>,
}

impl Transfer {
    pub(super) fn new(device: &ash::Device, queue: TransferQueue) -> Result<Transfer, MemoryError> {
        let mut kind = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(0);
        // SAFETY: fresh objects, destroyed by `destroy`.
        unsafe {
            let pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let semaphore = match device.create_semaphore(
                &vk::SemaphoreCreateInfo::default().push_next(&mut kind),
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    device.destroy_command_pool(pool, None);
                    return Err(e.into());
                }
            };
            Ok(Transfer {
                queue,
                pool,
                semaphore,
                submitted: 0,
                busy: VecDeque::new(),
                free: Vec::new(),
            })
        }
    }

    /// Records `copies` into a command buffer, submits it, and says what later work must wait for.
    pub(super) fn submit(
        &mut self,
        device: &ash::Device,
        copies: &Copies,
    ) -> Result<TimelineWait, MemoryError> {
        // SAFETY: the command buffers are this transfer's own, finished ones are recycled, and the queue is used
        // under its lock.
        unsafe {
            let done = device
                .get_semaphore_counter_value(self.semaphore)
                .unwrap_or(0);
            while self.busy.front().is_some_and(|(_, v)| *v <= done) {
                let (command, _) = self.busy.pop_front().expect("checked above");
                self.free.push(command);
            }
            let command = match self.free.pop() {
                Some(c) => c,
                None => device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.pool)
                        .command_buffer_count(1),
                )?[0],
            };
            device.begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let barrier = [vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::TRANSFER_READ)];
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &barrier,
                &[],
                &[],
            );
            for (src, dst, regions) in copies {
                device.cmd_copy_buffer(command, *src, *dst, regions);
            }
            device.end_command_buffer(command)?;
            let value = self.submitted + 1;
            let commands = [command];
            let signals = [self.semaphore];
            let values = [value];
            let mut timeline =
                vk::TimelineSemaphoreSubmitInfo::default().signal_semaphore_values(&values);
            let info = [vk::SubmitInfo::default()
                .command_buffers(&commands)
                .signal_semaphores(&signals)
                .push_next(&mut timeline)];
            {
                let _guard = self.queue.lock.lock().unwrap_or_else(|e| e.into_inner());
                device.queue_submit(self.queue.queue, &info, vk::Fence::null())?;
            }
            self.submitted = value;
            self.busy.push_back((command, value));
            Ok(TimelineWait {
                semaphore: self.semaphore,
                value,
            })
        }
    }

    pub(super) fn destroy(self, device: &ash::Device) {
        // SAFETY: the owner waited for the GPU to be idle.
        unsafe {
            device.destroy_command_pool(self.pool, None);
            device.destroy_semaphore(self.semaphore, None);
        }
    }
}
