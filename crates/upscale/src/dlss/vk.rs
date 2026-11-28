//! The Vulkan side of DLSS: a device on an NVIDIA GPU created with the extensions NGX names (or the renderer's own
//! device, borrowed), and the images and buffers the CPU-image upscaler path needs.

use std::ffi::{CStr, CString, c_char};

use ash::vk;

pub const NVIDIA_VENDOR_ID: u32 = 0x10DE;

/// Raw handles of a device the renderer owns.
#[derive(Debug, Clone, Copy)]
pub struct VkHandles {
    pub instance: vk::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: vk::Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
}

pub struct VkContext {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub device_name: String,
    owned: bool,
}

fn name_of(raw: &[c_char]) -> String {
    // SAFETY: Vulkan name arrays are NUL-terminated.
    unsafe { CStr::from_ptr(raw.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

impl VkContext {
    /// Creates an instance and device on an NVIDIA GPU with the given extensions enabled. Fails with a sentence
    /// naming the missing piece.
    pub fn create_nvidia(
        instance_extensions: &[String],
        device_extensions: &[String],
    ) -> Result<VkContext, String> {
        // SAFETY: loading the system Vulkan loader; every handle created below is destroyed in Drop or on error.
        unsafe {
            let entry = ash::Entry::load().map_err(|e| format!("no Vulkan loader: {e}"))?;
            let have = entry
                .enumerate_instance_extension_properties(None)
                .map_err(|e| format!("Vulkan: {e}"))?;
            let have: Vec<String> = have.iter().map(|e| name_of(&e.extension_name)).collect();
            if let Some(missing) = instance_extensions.iter().find(|e| !have.contains(e)) {
                return Err(format!(
                    "the Vulkan instance lacks {missing}, which NGX needs"
                ));
            }
            let instance_names: Vec<CString> = instance_extensions
                .iter()
                .map(|e| CString::new(e.as_str()).expect("extension names have no NUL"))
                .collect();
            let instance_ptrs: Vec<*const c_char> =
                instance_names.iter().map(|n| n.as_ptr()).collect();
            let app = vk::ApplicationInfo::default()
                .application_name(c"leonida")
                .api_version(vk::API_VERSION_1_3);
            let create = vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_extension_names(&instance_ptrs);
            let instance = entry
                .create_instance(&create, None)
                .map_err(|e| format!("Vulkan instance: {e}"))?;
            match Self::finish(entry, instance.clone(), device_extensions) {
                Ok(context) => Ok(context),
                Err(error) => {
                    instance.destroy_instance(None);
                    Err(error)
                }
            }
        }
    }

    /// SAFETY: `instance` is a live instance created from `entry`; on success the returned context owns it.
    unsafe fn finish(
        entry: ash::Entry,
        instance: ash::Instance,
        device_extensions: &[String],
    ) -> Result<VkContext, String> {
        // SAFETY: handles come from `instance`; property structs outlive the calls.
        unsafe {
            let physicals = instance
                .enumerate_physical_devices()
                .map_err(|e| format!("Vulkan: {e}"))?;
            let mut nvidia: Vec<_> = physicals
                .into_iter()
                .filter(|p| {
                    let properties = instance.get_physical_device_properties(*p);
                    properties.vendor_id == NVIDIA_VENDOR_ID
                        && properties.api_version >= vk::API_VERSION_1_1
                })
                .collect();
            nvidia.sort_by_key(|p| {
                instance.get_physical_device_properties(*p).device_type
                    != vk::PhysicalDeviceType::DISCRETE_GPU
            });
            let Some(physical) = nvidia.first().copied() else {
                return Err("no NVIDIA GPU with Vulkan was found".into());
            };
            let properties = instance.get_physical_device_properties(physical);
            let device_name = name_of(&properties.device_name);
            let have = instance
                .enumerate_device_extension_properties(physical)
                .map_err(|e| format!("Vulkan: {e}"))?;
            let have: Vec<String> = have.iter().map(|e| name_of(&e.extension_name)).collect();
            if let Some(missing) = device_extensions.iter().find(|e| !have.contains(e)) {
                return Err(format!(
                    "{device_name} lacks the device extension {missing}, which NGX needs"
                ));
            }
            let families = instance.get_physical_device_queue_family_properties(physical);
            let queue_family = families
                .iter()
                .position(|f| {
                    f.queue_flags
                        .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                })
                .or_else(|| {
                    families
                        .iter()
                        .position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE))
                })
                .ok_or("the GPU has no compute queue")? as u32;
            let names: Vec<CString> = device_extensions
                .iter()
                .map(|e| CString::new(e.as_str()).expect("extension names have no NUL"))
                .collect();
            let ptrs: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&priorities)];
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut supported12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut query = vk::PhysicalDeviceFeatures2::default().push_next(&mut supported12);
            instance.get_physical_device_features2(physical, &mut query);
            f12 = f12
                .buffer_device_address(supported12.buffer_device_address == vk::TRUE)
                .timeline_semaphore(supported12.timeline_semaphore == vk::TRUE)
                .shader_float16(supported12.shader_float16 == vk::TRUE);
            let create = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&ptrs)
                .push_next(&mut f12);
            let device = instance
                .create_device(physical, &create, None)
                .map_err(|e| format!("{device_name}: device creation failed: {e}"))?;
            let queue = device.get_device_queue(queue_family, 0);
            let memory = instance.get_physical_device_memory_properties(physical);
            Ok(VkContext {
                entry,
                instance,
                physical,
                device,
                queue,
                queue_family,
                memory,
                device_name,
                owned: true,
            })
        }
    }

    /// Wraps the renderer's own device. The context never destroys it.
    ///
    /// # Safety
    /// The handles must be live and belong together, and the queue must only be used by this context while it
    /// records and submits (the renderer serialises queue access).
    pub unsafe fn borrow(handles: VkHandles) -> Result<VkContext, String> {
        // SAFETY: loading the system Vulkan loader; the caller guarantees the handles are live and the function
        // tables come from them.
        unsafe {
            let entry = ash::Entry::load().map_err(|e| format!("no Vulkan loader: {e}"))?;
            let instance = ash::Instance::load(entry.static_fn(), handles.instance);
            let device = ash::Device::load(instance.fp_v1_0(), handles.device);
            let properties = instance.get_physical_device_properties(handles.physical);
            let memory = instance.get_physical_device_memory_properties(handles.physical);
            Ok(VkContext {
                entry,
                instance,
                physical: handles.physical,
                device,
                queue: handles.queue,
                queue_family: handles.queue_family,
                memory,
                device_name: name_of(&properties.device_name),
                owned: false,
            })
        }
    }

    pub fn handles(&self) -> VkHandles {
        VkHandles {
            instance: self.instance.handle(),
            physical: self.physical,
            device: self.device.handle(),
            queue: self.queue,
            queue_family: self.queue_family,
        }
    }

    pub fn vendor_id(&self) -> u32 {
        // SAFETY: `physical` belongs to `instance`.
        unsafe { self.instance.get_physical_device_properties(self.physical) }.vendor_id
    }

    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.memory.memory_type_count).find(|&i| {
            bits & (1 << i) != 0
                && self.memory.memory_types[i as usize]
                    .property_flags
                    .contains(flags)
        })
    }

    pub fn format_supports(&self, format: vk::Format, features: vk::FormatFeatureFlags) -> bool {
        // SAFETY: `physical` belongs to `instance`.
        let properties = unsafe {
            self.instance
                .get_physical_device_format_properties(self.physical, format)
        };
        properties.optimal_tiling_features.contains(features)
    }

    pub fn create_buffer(
        &self,
        size: u64,
        usage: vk::BufferUsageFlags,
    ) -> Result<HostBuffer, String> {
        // SAFETY: plain creation calls on the live device; partial results are destroyed on error.
        unsafe {
            let buffer = self
                .device
                .create_buffer(
                    &vk::BufferCreateInfo::default().size(size).usage(usage),
                    None,
                )
                .map_err(|e| format!("buffer: {e}"))?;
            let requirements = self.device.get_buffer_memory_requirements(buffer);
            let Some(memory_type) = self.memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            ) else {
                self.device.destroy_buffer(buffer, None);
                return Err("no host-visible memory".into());
            };
            let memory = match self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type),
                None,
            ) {
                Ok(memory) => memory,
                Err(e) => {
                    self.device.destroy_buffer(buffer, None);
                    return Err(format!("buffer memory: {e}"));
                }
            };
            let mapped = self
                .device
                .bind_buffer_memory(buffer, memory, 0)
                .and_then(|()| {
                    self.device
                        .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                });
            match mapped {
                Ok(ptr) => Ok(HostBuffer {
                    buffer,
                    memory,
                    size,
                    ptr: ptr.cast(),
                }),
                Err(e) => {
                    self.device.free_memory(memory, None);
                    self.device.destroy_buffer(buffer, None);
                    Err(format!("buffer bind: {e}"))
                }
            }
        }
    }

    pub fn create_image(
        &self,
        format: vk::Format,
        size: [u32; 2],
        usage: vk::ImageUsageFlags,
    ) -> Result<GpuImage, String> {
        // SAFETY: plain creation calls on the live device; partial results are destroyed on error.
        unsafe {
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D {
                    width: size[0],
                    height: size[1],
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            let image = self
                .device
                .create_image(&info, None)
                .map_err(|e| format!("image: {e}"))?;
            let requirements = self.device.get_image_memory_requirements(image);
            let Some(memory_type) = self.memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            ) else {
                self.device.destroy_image(image, None);
                return Err("no device-local memory".into());
            };
            let memory = match self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type),
                None,
            ) {
                Ok(memory) => memory,
                Err(e) => {
                    self.device.destroy_image(image, None);
                    return Err(format!("image memory: {e}"));
                }
            };
            let range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let view = self
                .device
                .bind_image_memory(image, memory, 0)
                .and_then(|()| {
                    self.device.create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(image)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(format)
                            .subresource_range(range),
                        None,
                    )
                });
            match view {
                Ok(view) => Ok(GpuImage {
                    image,
                    view,
                    memory,
                    format,
                    size,
                    range,
                }),
                Err(e) => {
                    self.device.free_memory(memory, None);
                    self.device.destroy_image(image, None);
                    Err(format!("image view: {e}"))
                }
            }
        }
    }

    pub fn destroy_buffer(&self, buffer: HostBuffer) {
        // SAFETY: the buffer belongs to this device and the GPU is no longer using it.
        unsafe {
            self.device.unmap_memory(buffer.memory);
            self.device.destroy_buffer(buffer.buffer, None);
            self.device.free_memory(buffer.memory, None);
        }
    }

    pub fn destroy_image(&self, image: GpuImage) {
        // SAFETY: the image belongs to this device and the GPU is no longer using it.
        unsafe {
            self.device.destroy_image_view(image.view, None);
            self.device.destroy_image(image.image, None);
            self.device.free_memory(image.memory, None);
        }
    }
}

impl Drop for VkContext {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: the context created these and every child object was destroyed before it.
            unsafe {
                self.device.device_wait_idle().ok();
                if !super::ngx::left_initialised() {
                    self.device.destroy_device(None);
                    self.instance.destroy_instance(None);
                }
            }
        }
    }
}

pub struct HostBuffer {
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pub size: u64,
    ptr: *mut u8,
}

impl HostBuffer {
    /// The mapped memory. Host-coherent, so writes are visible to the next submit.
    pub fn mapped(&self) -> *mut u8 {
        self.ptr
    }

    /// The mapped bytes. Host-coherent, so writes are visible to the next submit.
    pub fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: `ptr` maps `size` bytes of memory that lives until the buffer is destroyed.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size as usize) }
    }
}

pub struct GpuImage {
    pub image: vk::Image,
    pub view: vk::ImageView,
    memory: vk::DeviceMemory,
    pub format: vk::Format,
    pub size: [u32; 2],
    pub range: vk::ImageSubresourceRange,
}

impl GpuImage {
    pub fn resource(&self, read_write: bool) -> super::ffi::ResourceVk {
        super::ffi::ResourceVk::image_view(
            self.view,
            self.image,
            self.range,
            self.format,
            self.size[0],
            self.size[1],
            read_write,
        )
    }
}

/// Records one image layout transition with full-stage dependencies on both sides.
pub fn transition(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: &GpuImage,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image.image)
        .subresource_range(image.range);
    // SAFETY: `cmd` is recording on `device` and the image belongs to it.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}
