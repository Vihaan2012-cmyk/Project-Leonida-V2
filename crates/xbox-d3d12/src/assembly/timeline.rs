//! Fences as Vulkan timeline semaphores.
//!
//! The value the title reads and waits on lives in host memory (`HostTimeline`: the console hands out the address of
//! the completed value, which the title polls), and each change is mirrored into a timeline semaphore so work that
//! is ordered on the GPU can wait on the fence. Executing a list returns only once the GPU has finished it, so a
//! `Signal` queued behind it is ordered after its work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ash::vk;

use crate::queue::{HostTimeline, Timeline, TimelineFactory};

pub struct VkTimeline {
    device: ash::Device,
    semaphore: vk::Semaphore,
    mirrored: AtomicU64,
    host: HostTimeline,
}

impl VkTimeline {
    fn new(device: &ash::Device, initial: u64) -> Option<VkTimeline> {
        let mut kind = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(initial);
        // SAFETY: a plain create call on a live device; the semaphore is destroyed in Drop.
        let semaphore = unsafe {
            device
                .create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut kind),
                    None,
                )
                .ok()?
        };
        Some(VkTimeline {
            device: device.clone(),
            semaphore,
            mirrored: AtomicU64::new(initial),
            host: HostTimeline::new(initial),
        })
    }

    /// The semaphore that follows this fence's value, for GPU work that waits on it.
    pub fn semaphore(&self) -> vk::Semaphore {
        self.semaphore
    }

    fn mirror(&self, value: u64) {
        if self.mirrored.fetch_max(value, Ordering::AcqRel) >= value {
            return;
        }
        let info = vk::SemaphoreSignalInfo::default()
            .semaphore(self.semaphore)
            .value(value);
        // SAFETY: the semaphore is a live timeline semaphore of this device; the value only rises.
        let _ = unsafe { self.device.signal_semaphore(&info) };
    }
}

impl Drop for VkTimeline {
    fn drop(&mut self) {
        // SAFETY: nothing submits work that waits on a fence the title has released.
        unsafe { self.device.destroy_semaphore(self.semaphore, None) };
    }
}

impl Timeline for VkTimeline {
    fn value(&self) -> u64 {
        self.host.value()
    }

    fn signal(&self, value: u64) {
        self.host.signal(value);
        self.mirror(value);
    }

    fn wait(&self, value: u64, abort: &AtomicBool) -> bool {
        self.host.wait(value, abort)
    }

    fn on_reached(&self, value: u64, f: Box<dyn FnOnce() + Send>) {
        self.host.on_reached(value, f);
    }

    fn cell(&self) -> &AtomicU64 {
        self.host.cell()
    }
}

pub struct VkTimelines {
    device: ash::Device,
}

impl VkTimelines {
    pub fn new(device: ash::Device) -> VkTimelines {
        VkTimelines { device }
    }
}

impl TimelineFactory for VkTimelines {
    fn create(&self, initial: u64) -> Arc<dyn Timeline> {
        match VkTimeline::new(&self.device, initial) {
            Some(timeline) => Arc::new(timeline),
            None => {
                crate::com::note_once(
                    "a timeline semaphore could not be made; the fence is host only",
                );
                Arc::new(HostTimeline::new(initial))
            }
        }
    }
}
