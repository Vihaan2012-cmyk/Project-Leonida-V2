use super::*;

/// A device with the features the address space needs.
pub struct TestGpu {
    _entry: ash::Entry,
    pub handles: Handles,
    pub family: u32,
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
                let priorities = [1.0];
                let queues = [vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(family as u32)
                    .queue_priorities(&priorities)];
                let base = vk::PhysicalDeviceFeatures::default()
                    .sparse_binding(true)
                    .sparse_residency_buffer(true)
                    .shader_int64(true);
                let mut v12 =
                    vk::PhysicalDeviceVulkan12Features::default().buffer_device_address(true);
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
                all.push(TestGpu {
                    _entry: entry.clone(),
                    handles: Handles {
                        instance: instance.clone(),
                        physical,
                        device,
                        queue,
                        queue_family: family as u32,
                        queue_lock: Arc::new(Mutex::new(())),
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
}

impl Region {
    pub fn new(len: u64, write_watch: bool) -> Region {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
        };
        const MEM_WRITE_WATCH: u32 = 0x0020_0000;
        let mut flags = MEM_RESERVE | MEM_COMMIT;
        if write_watch {
            flags |= MEM_WRITE_WATCH;
        }
        for step in 0..28u64 {
            let hint = (1u64 << 30) + step * (1 << 26);
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
                return Region { address: at as u64 };
            }
        }
        panic!("no free address between 1 and 3 GiB");
    }

    pub fn read(&self, index: u64) -> u32 {
        // SAFETY: the region is ours and committed; callers stay inside it.
        unsafe { *((self.address + index * 4) as *const u32) }
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
