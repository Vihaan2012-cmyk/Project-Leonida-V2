#![allow(unsafe_code, dead_code)]

use ash::vk;

/// A bare Vulkan device for the tests: `None` from `Ctx::new` means there is no usable GPU and a test skips.
pub struct Ctx {
    _entry: ash::Entry,
    instance: ash::Instance,
    pub device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    memory: vk::PhysicalDeviceMemoryProperties,
}

pub struct Buf {
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pub size: usize,
}

impl Ctx {
    pub fn new() -> Option<Ctx> {
        // SAFETY: plain instance, device and pool creation; every handle is destroyed in Drop.
        unsafe {
            let entry = ash::Entry::load().ok()?;
            let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
            let instance = entry
                .create_instance(
                    &vk::InstanceCreateInfo::default().application_info(&app),
                    None,
                )
                .ok()?;
            let wanted = std::env::var("LEONIDA_GPU").ok().map(|n| n.to_lowercase());
            let mut found = None;
            for physical in instance.enumerate_physical_devices().ok()? {
                let props = instance.get_physical_device_properties(physical);
                let name = props
                    .device_name_as_c_str()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if wanted.as_ref().is_some_and(|w| !name.contains(w)) {
                    continue;
                }
                let family = instance
                    .get_physical_device_queue_family_properties(physical)
                    .iter()
                    .position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE));
                if let Some(family) = family {
                    let discrete = props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
                    if found.is_none() || discrete {
                        found = Some((physical, family as u32));
                    }
                }
            }
            let (physical, family) = found?;
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(&priorities)];
            let device = instance
                .create_device(
                    physical,
                    &vk::DeviceCreateInfo::default().queue_create_infos(&queues),
                    None,
                )
                .ok()?;
            let queue = device.get_device_queue(family, 0);
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                    None,
                )
                .ok()?;
            let memory = instance.get_physical_device_memory_properties(physical);
            Some(Ctx {
                _entry: entry,
                instance,
                device,
                queue,
                pool,
                memory,
            })
        }
    }

    pub fn buffer(&self, data: &[u8]) -> Buf {
        let size = data.len().next_multiple_of(4).max(4);
        // SAFETY: a fresh buffer bound to fresh host-visible memory, mapped for the copy only.
        unsafe {
            let buffer = self
                .device
                .create_buffer(
                    &vk::BufferCreateInfo::default().size(size as u64).usage(
                        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
                    ),
                    None,
                )
                .unwrap();
            let needs = self.device.get_buffer_memory_requirements(buffer);
            let wanted =
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let kind = (0..self.memory.memory_type_count)
                .find(|i| {
                    needs.memory_type_bits & (1 << i) != 0
                        && self.memory.memory_types[*i as usize]
                            .property_flags
                            .contains(wanted)
                })
                .unwrap();
            let memory = self
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind),
                    None,
                )
                .unwrap();
            self.device.bind_buffer_memory(buffer, memory, 0).unwrap();
            let map = self
                .device
                .map_memory(memory, 0, size as u64, vk::MemoryMapFlags::empty())
                .unwrap() as *mut u8;
            std::ptr::write_bytes(map, 0, size);
            std::ptr::copy_nonoverlapping(data.as_ptr(), map, data.len());
            self.device.unmap_memory(memory);
            Buf {
                buffer,
                memory,
                size,
            }
        }
    }

    pub fn read(&self, buf: &Buf, len: usize) -> Vec<u8> {
        // SAFETY: the memory is host-visible and coherent, and the work that wrote it has finished.
        unsafe {
            let map = self
                .device
                .map_memory(buf.memory, 0, buf.size as u64, vk::MemoryMapFlags::empty())
                .unwrap() as *const u8;
            let out = std::slice::from_raw_parts(map, len).to_vec();
            self.device.unmap_memory(buf.memory);
            out
        }
    }

    pub fn free(&self, buf: Buf) {
        // SAFETY: the work using the buffer has finished.
        unsafe {
            self.device.destroy_buffer(buf.buffer, None);
            self.device.free_memory(buf.memory, None);
        }
    }

    /// Records with `f`, submits and waits.
    pub fn run(&self, f: impl FnOnce(vk::CommandBuffer)) {
        // SAFETY: one command buffer recorded, submitted and waited on before it is freed.
        unsafe {
            let cmd = self
                .device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.pool)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            self.device
                .begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .unwrap();
            f(cmd);
            self.device.end_command_buffer(cmd).unwrap();
            let fence = self
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            let cmds = [cmd];
            self.device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&cmds)],
                    fence,
                )
                .unwrap();
            self.device
                .wait_for_fences(&[fence], true, u64::MAX)
                .unwrap();
            self.device.destroy_fence(fence, None);
            self.device.free_command_buffers(self.pool, &cmds);
        }
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        // SAFETY: every object made from the device is gone by now.
        unsafe {
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// A small deterministic byte generator.
pub fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}
