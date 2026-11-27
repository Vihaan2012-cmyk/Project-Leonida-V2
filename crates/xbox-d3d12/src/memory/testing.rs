use super::*;

/// A device with the features the address space needs.
pub struct TestGpu {
    _entry: ash::Entry,
    pub handles: Handles,
    pub family: u32,
    /// A queue of a transfer-only family, where the GPU has one.
    pub dedicated: Option<TransferQueue>,
    /// A second queue of the main family, where the GPU has one.
    pub sibling: Option<TransferQueue>,
}

impl TestGpu {
    /// One for each GPU that can host an address space.
    pub fn open_all() -> Vec<TestGpu> {
        // SAFETY: loads the system loader; every object lives as long as the returned values.
        unsafe {
            let Ok(entry) = ash::Entry::load() else {
                return Vec::new();
            };
            let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
            let Ok(instance) = entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            ) else {
                return Vec::new();
            };
            let mut all = Vec::new();
            for physical in instance.enumerate_physical_devices().unwrap_or_default() {
                if Support::query(&instance, physical).check().is_err() {
                    continue;
                }
                let families = instance.get_physical_device_queue_family_properties(physical);
                let want = vk::QueueFlags::COMPUTE | vk::QueueFlags::SPARSE_BINDING;
                let Some(family) = families.iter().position(|f| f.queue_flags.contains(want))
                else {
                    continue;
                };
                let priorities = [1.0, 1.0];
                let main_count = families[family].queue_count.min(2);
                let dedicated_family = pick_transfer_family(&instance, physical, family as u32)
                    .filter(|pick| pick.family != family as u32)
                    .map(|pick| pick.family);
                let mut queues = vec![
                    vk::DeviceQueueCreateInfo::default()
                        .queue_family_index(family as u32)
                        .queue_priorities(&priorities[..main_count as usize]),
                ];
                if let Some(other) = dedicated_family {
                    queues.push(
                        vk::DeviceQueueCreateInfo::default()
                            .queue_family_index(other)
                            .queue_priorities(&priorities[..1]),
                    );
                }
                let base = vk::PhysicalDeviceFeatures::default()
                    .sparse_binding(true)
                    .sparse_residency_buffer(true)
                    .shader_int64(true);
                let timeline = Support::query(&instance, physical).timeline_semaphores;
                let mut v12 = vk::PhysicalDeviceVulkan12Features::default()
                    .buffer_device_address(true)
                    .timeline_semaphore(timeline);
                let mut features = vk::PhysicalDeviceFeatures2::default()
                    .features(base)
                    .push_next(&mut v12);
                let extensions = DEVICE_EXTENSIONS.map(CStr::as_ptr);
                let Ok(device) = instance.create_device(
                    physical,
                    &vk::DeviceCreateInfo::default()
                        .queue_create_infos(&queues)
                        .enabled_extension_names(&extensions)
                        .push_next(&mut features),
                    None,
                ) else {
                    continue;
                };
                let queue = device.get_device_queue(family as u32, 0);
                let lock = Arc::new(Mutex::new(()));
                let sibling = (timeline && main_count > 1).then(|| TransferQueue {
                    queue: device.get_device_queue(family as u32, 1),
                    family: family as u32,
                    lock: Arc::new(Mutex::new(())),
                });
                let dedicated = dedicated_family
                    .filter(|_| timeline)
                    .map(|other| TransferQueue {
                        queue: device.get_device_queue(other, 0),
                        family: other,
                        lock: Arc::new(Mutex::new(())),
                    });
                all.push(TestGpu {
                    dedicated,
                    sibling,
                    _entry: entry.clone(),
                    handles: Handles {
                        instance: instance.clone(),
                        physical,
                        device,
                        queue,
                        queue_family: family as u32,
                        queue_lock: lock,
                    },
                    family: family as u32,
                });
            }
            all
        }
    }

    pub fn name(&self) -> String {
        // SAFETY: the physical device came from this instance.
        unsafe {
            self.handles
                .instance
                .get_physical_device_properties(self.handles.physical)
                .device_name_as_c_str()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        }
    }
}

impl Drop for TestGpu {
    fn drop(&mut self) {
        // SAFETY: the test owns the device and has finished with it. The instance is shared with the other
        // devices and is released with the process.
        unsafe { self.handles.device.destroy_device(None) };
    }
}

/// Committed host memory at an address between 1 and 3 GiB, standing in for an `XMemVirtualAlloc` region.
pub struct Region {
    pub address: u64,
    pub len: u64,
}

impl Region {
    pub fn new(len: u64, write_watch: bool) -> Region {
        Region::new_in(len, write_watch, 1 << 30, 1 << 26, 28)
    }

    /// Committed memory at the first of `tries` addresses, `step` apart from `first`, that is free.
    pub fn new_in(len: u64, write_watch: bool, first: u64, step: u64, tries: u64) -> Region {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
        };
        const MEM_WRITE_WATCH: u32 = 0x0020_0000;
        let mut flags = MEM_RESERVE | MEM_COMMIT;
        if write_watch {
            flags |= MEM_WRITE_WATCH;
        }
        for n in 0..tries {
            let hint = first + n * step;
            // SAFETY: a fresh allocation, at the hint or nowhere.
            let at = unsafe {
                VirtualAlloc(
                    hint as usize as *const c_void,
                    len as usize,
                    flags,
                    PAGE_READWRITE,
                )
            };
            if !at.is_null() {
                return Region {
                    address: at as u64,
                    len,
                };
            }
        }
        panic!("no free address from {first:#x}");
    }

    pub fn read(&self, index: u64) -> u32 {
        // SAFETY: the region is ours and committed; callers stay inside it.
        unsafe { *((self.address + index * 4) as *const u32) }
    }

    /// Writes one word at byte `offset`, which dirties its page.
    pub fn touch(&self, offset: u64, value: u32) {
        debug_assert!(offset + 4 <= self.len);
        // SAFETY: inside the region, which is ours and committed.
        unsafe { *((self.address + offset) as *mut u32) = value };
    }

    pub fn write(&self, index: u64, value: u32) {
        // SAFETY: as in `read`.
        unsafe { *((self.address + index * 4) as *mut u32) = value };
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
        // SAFETY: allocated in `new`.
        unsafe { VirtualFree(self.address as usize as *mut c_void, 0, MEM_RELEASE) };
    }
}

// SPIR-V of a compute shader (GLSL, buffer_reference) that stores `*(heap + src) + 1` at `heap + dst`, with
// `heap`, `src` and `dst` three 64-bit push constants.
const INCREMENT: &[u32] = include!("test_shader.in");

pub struct Kernel {
    module: vk::ShaderModule,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pool: vk::CommandPool,
}

impl Kernel {
    pub fn new(gpu: &TestGpu) -> Kernel {
        let device = &gpu.handles.device;
        // SAFETY: test-only Vulkan use; `finish` destroys everything.
        unsafe {
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(INCREMENT), None)
                .unwrap();
            let range = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .size(24)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&range),
                    None,
                )
                .unwrap();
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            let pipeline = device
                .create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::ComputePipelineCreateInfo::default()
                        .stage(stage)
                        .layout(layout)],
                    None,
                )
                .unwrap()[0];
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.family),
                    None,
                )
                .unwrap();
            Kernel {
                module,
                layout,
                pipeline,
                pool,
            }
        }
    }

    /// One submission: uploads, then each `(src, dst)` increment, then downloads of `download`. Returns the
    /// bytes the uploads copied.
    pub fn run(
        &self,
        gpu: &TestGpu,
        space: &mut AddressSpace,
        jobs: &[(u64, u64)],
        download: (u64, u64),
    ) -> u64 {
        let device = &gpu.handles.device;
        // SAFETY: test-only Vulkan use; the command buffer completes before returning.
        unsafe {
            let command = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.pool)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            device
                .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
                .unwrap();
            let uploaded = space.record_uploads(command);
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            for &(src, dst) in jobs {
                let mut push = Vec::new();
                for word in [space.heap_base(), src, dst] {
                    push.extend_from_slice(&word.to_le_bytes());
                }
                device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    &push,
                );
                device.cmd_dispatch(command, 1, 1, 1);
                let barrier = [vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)];
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &barrier,
                    &[],
                    &[],
                );
            }
            space.record_downloads(command, download.0, download.1);
            device.end_command_buffer(command).unwrap();
            let fence = device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            let commands = [command];
            device
                .queue_submit(
                    gpu.handles.queue,
                    &[vk::SubmitInfo::default().command_buffers(&commands)],
                    fence,
                )
                .unwrap();
            device.wait_for_fences(&[fence], true, u64::MAX).unwrap();
            device.destroy_fence(fence, None);
            device.free_command_buffers(self.pool, &commands);
            uploaded
        }
    }

    /// One submission built from the layer's own calls: `submit_uploads`, `record_uploads`, each `(src, dst)`
    /// increment with `note_gpu_write` for its destination word, then `record_pending_downloads`. Returns the
    /// bytes the in-queue uploads copied and whether a transfer-queue submission was waited for.
    pub fn run_tracked(
        &self,
        gpu: &TestGpu,
        space: &mut AddressSpace,
        jobs: &[(u64, u64)],
    ) -> (u64, bool) {
        let device = &gpu.handles.device;
        let wait = space.submit_uploads().unwrap();
        // SAFETY: test-only Vulkan use; the command buffer completes before returning.
        unsafe {
            let command = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.pool)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            device
                .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
                .unwrap();
            let uploaded = space.record_uploads(command);
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            for &(src, dst) in jobs {
                let mut push = Vec::new();
                for word in [space.heap_base(), src, dst] {
                    push.extend_from_slice(&word.to_le_bytes());
                }
                device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    &push,
                );
                device.cmd_dispatch(command, 1, 1, 1);
                let barrier = [vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)];
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &barrier,
                    &[],
                    &[],
                );
                space.note_gpu_write(dst, 4);
            }
            space.record_pending_downloads(command);
            device.end_command_buffer(command).unwrap();
            let fence = device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            let commands = [command];
            let semaphores: Vec<vk::Semaphore> = wait.iter().map(|w| w.semaphore).collect();
            let values: Vec<u64> = wait.iter().map(|w| w.value).collect();
            let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; semaphores.len()];
            let mut timeline =
                vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&values);
            device
                .queue_submit(
                    gpu.handles.queue,
                    &[vk::SubmitInfo::default()
                        .command_buffers(&commands)
                        .wait_semaphores(&semaphores)
                        .wait_dst_stage_mask(&stages)
                        .push_next(&mut timeline)],
                    fence,
                )
                .unwrap();
            device.wait_for_fences(&[fence], true, u64::MAX).unwrap();
            device.destroy_fence(fence, None);
            device.free_command_buffers(self.pool, &commands);
            (uploaded, wait.is_some())
        }
    }

    pub fn finish(self, gpu: &TestGpu) {
        let device = &gpu.handles.device;
        // SAFETY: nothing is in flight.
        unsafe {
            device.destroy_command_pool(self.pool, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_shader_module(self.module, None);
        }
    }
}
