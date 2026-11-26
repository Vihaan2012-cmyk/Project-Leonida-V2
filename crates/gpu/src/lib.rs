#![allow(unsafe_code)]

use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use ash::vk;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("the Vulkan loader could not be opened: {0}")]
    Loader(String),
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("no Vulkan 1.3 GPU with a compute queue was found")]
    NoDevice,
    #[error("no host-visible memory for a buffer")]
    NoMemory,
    #[error("SPIR-V must be a whole number of 32-bit words")]
    BadSpirv,
}

static VALIDATION_ERRORS: AtomicUsize = AtomicUsize::new(0);

pub fn validation_errors() -> usize {
    VALIDATION_ERRORS.load(Ordering::Relaxed)
}

unsafe extern "system" fn report(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        VALIDATION_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    // SAFETY: the loader passes a valid callback-data pointer for the duration of the call.
    let message = unsafe { data.as_ref() }
        .and_then(|data| unsafe { data.message_as_c_str() })
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default();
    eprintln!("vulkan: {message}");
    vk::FALSE
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub vendor: u32,
    pub discrete: bool,
    pub subgroup_size: u32,
    pub min_subgroup_size: u32,
    pub max_subgroup_size: u32,
    pub subgroup_size_control: bool,
    pub full_subgroups: bool,
    pub ballot_and_shuffle: bool,
    pub int64: bool,
    pub float64: bool,
    pub denorm_preserve_32: bool,
    pub denorm_flush_32: bool,
    pub nan_preserve_32: bool,
    pub max_shared_memory: u32,
}

pub struct Gpu {
    _entry: ash::Entry,
    instance: ash::Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    device: ash::Device,
    queue: vk::Queue,
    family: u32,
    memory: vk::PhysicalDeviceMemoryProperties,
    pub info: DeviceInfo,
}

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

struct Candidate {
    physical: vk::PhysicalDevice,
    family: u32,
    info: DeviceInfo,
    required_stages: vk::ShaderStageFlags,
}

fn describe(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Option<Candidate> {
    // SAFETY: `physical` came from this instance and the property structs outlive the calls.
    unsafe {
        let base = instance.get_physical_device_properties(physical);
        if base.api_version < vk::API_VERSION_1_3 {
            return None;
        }
        let family = instance
            .get_physical_device_queue_family_properties(physical)
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::COMPUTE))?
            as u32;
        let mut v11 = vk::PhysicalDeviceVulkan11Properties::default();
        let mut v12 = vk::PhysicalDeviceVulkan12Properties::default();
        let mut v13 = vk::PhysicalDeviceVulkan13Properties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default()
            .push_next(&mut v11)
            .push_next(&mut v12)
            .push_next(&mut v13);
        instance.get_physical_device_properties2(physical, &mut properties);
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut f13);
        instance.get_physical_device_features2(physical, &mut features);
        let int64 = features.features.shader_int64 == vk::TRUE;
        let float64 = features.features.shader_float64 == vk::TRUE;
        let size_control = f13.subgroup_size_control == vk::TRUE;
        let needed = vk::SubgroupFeatureFlags::BASIC
            | vk::SubgroupFeatureFlags::BALLOT
            | vk::SubgroupFeatureFlags::SHUFFLE;
        let name = base
            .device_name_as_c_str()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Some(Candidate {
            physical,
            family,
            required_stages: v13.required_subgroup_size_stages,
            info: DeviceInfo {
                name,
                vendor: base.vendor_id,
                discrete: base.device_type == vk::PhysicalDeviceType::DISCRETE_GPU,
                subgroup_size: v11.subgroup_size,
                min_subgroup_size: v13.min_subgroup_size,
                max_subgroup_size: v13.max_subgroup_size,
                subgroup_size_control: size_control,
                full_subgroups: f13.compute_full_subgroups == vk::TRUE,
                ballot_and_shuffle: v11.subgroup_supported_operations.contains(needed)
                    && v11
                        .subgroup_supported_stages
                        .contains(vk::ShaderStageFlags::COMPUTE),
                int64,
                float64,
                denorm_preserve_32: v12.shader_denorm_preserve_float32 == vk::TRUE,
                denorm_flush_32: v12.shader_denorm_flush_to_zero_float32 == vk::TRUE,
                nan_preserve_32: v12.shader_signed_zero_inf_nan_preserve_float32 == vk::TRUE,
                max_shared_memory: base.limits.max_compute_shared_memory_size,
            },
        })
    }
}

impl Gpu {
    pub fn new() -> Result<Gpu, GpuError> {
        // SAFETY: loading the system Vulkan loader; every handle created below is destroyed in Drop.
        unsafe {
            let entry = ash::Entry::load().map_err(|e| GpuError::Loader(e.to_string()))?;
            let validate = std::env::var_os("LEONIDA_VALIDATION").is_some()
                && entry
                    .enumerate_instance_layer_properties()?
                    .iter()
                    .any(|layer| layer.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
            let app = vk::ApplicationInfo::default()
                .application_name(c"leonida")
                .api_version(vk::API_VERSION_1_3);
            let layers = if validate {
                vec![VALIDATION_LAYER.as_ptr()]
            } else {
                Vec::new()
            };
            let extensions = if validate {
                vec![ash::ext::debug_utils::NAME.as_ptr()]
            } else {
                Vec::new()
            };
            let create = vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_layer_names(&layers)
                .enabled_extension_names(&extensions);
            let instance = entry.create_instance(&create, None)?;
            let debug = if validate {
                let utils = ash::ext::debug_utils::Instance::new(&entry, &instance);
                let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                    .message_severity(
                        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                            | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                    )
                    .message_type(
                        vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                            | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                    )
                    .pfn_user_callback(Some(report));
                let messenger = utils.create_debug_utils_messenger(&info, None)?;
                Some((utils, messenger))
            } else {
                None
            };
            let wanted = std::env::var("LEONIDA_GPU").ok().map(|n| n.to_lowercase());
            let mut candidates: Vec<Candidate> = instance
                .enumerate_physical_devices()?
                .into_iter()
                .filter_map(|physical| describe(&instance, physical))
                .filter(|c| {
                    wanted
                        .as_ref()
                        .is_none_or(|w| c.info.name.to_lowercase().contains(w))
                })
                .collect();
            candidates.sort_by_key(|c| !c.info.discrete);
            let Some(chosen) = candidates.into_iter().next() else {
                if let Some((utils, messenger)) = &debug {
                    utils.destroy_debug_utils_messenger(*messenger, None);
                }
                instance.destroy_instance(None);
                return Err(GpuError::NoDevice);
            };
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(chosen.family)
                .queue_priorities(&priorities)];
            let mut f13 = vk::PhysicalDeviceVulkan13Features::default()
                .subgroup_size_control(chosen.info.subgroup_size_control)
                .compute_full_subgroups(chosen.info.full_subgroups);
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default().buffer_device_address(true);
            let base = vk::PhysicalDeviceFeatures::default()
                .shader_int64(chosen.info.int64)
                .shader_float64(chosen.info.float64);
            let mut features = vk::PhysicalDeviceFeatures2::default()
                .features(base)
                .push_next(&mut f13)
                .push_next(&mut f12);
            let create = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .push_next(&mut features);
            let device = instance.create_device(chosen.physical, &create, None)?;
            let queue = device.get_device_queue(chosen.family, 0);
            let memory = instance.get_physical_device_memory_properties(chosen.physical);
            let mut info = chosen.info;
            if !chosen
                .required_stages
                .contains(vk::ShaderStageFlags::COMPUTE)
            {
                info.subgroup_size_control = false;
            }
            Ok(Gpu {
                _entry: entry,
                instance,
                debug,
                device,
                queue,
                family: chosen.family,
                memory,
                info,
            })
        }
    }

    fn memory_type(&self, bits: u32) -> Option<u32> {
        let wanted = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        (0..self.memory.memory_type_count).find(|i| {
            bits & (1 << i) != 0
                && self.memory.memory_types[*i as usize]
                    .property_flags
                    .contains(wanted)
        })
    }

    pub fn dispatch(
        &self,
        spirv: &[u32],
        buffers: &mut [&mut [u32]],
        groups: [u32; 3],
        subgroup_size: Option<u32>,
    ) -> Result<(), GpuError> {
        self.dispatch_with(spirv, buffers, |_| Vec::new(), groups, subgroup_size)
    }

    /// Like `dispatch`, with push constants built from the buffers' device addresses.
    pub fn dispatch_with(
        &self,
        spirv: &[u32],
        buffers: &mut [&mut [u32]],
        push: impl FnOnce(&[u64]) -> Vec<u32>,
        groups: [u32; 3],
        subgroup_size: Option<u32>,
    ) -> Result<(), GpuError> {
        if spirv.is_empty() {
            return Err(GpuError::BadSpirv);
        }
        let mut scratch = Scratch::new(&self.device);
        // SAFETY: every handle is created from `self.device`, recorded in `scratch` and destroyed when it drops,
        // after the fence wait below has made the GPU finish with them.
        unsafe {
            let device = &self.device;
            for data in buffers.iter() {
                let size = (data.len().max(1) * 4) as u64;
                let buffer = device.create_buffer(
                    &vk::BufferCreateInfo::default().size(size).usage(
                        vk::BufferUsageFlags::STORAGE_BUFFER
                            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
                    ),
                    None,
                )?;
                scratch
                    .buffers
                    .push((buffer, vk::DeviceMemory::null(), size));
                let needs = device.get_buffer_memory_requirements(buffer);
                let kind = self
                    .memory_type(needs.memory_type_bits)
                    .ok_or(GpuError::NoMemory)?;
                let mut flags = vk::MemoryAllocateFlagsInfo::default()
                    .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
                let memory = device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind)
                        .push_next(&mut flags),
                    None,
                )?;
                scratch.buffers.last_mut().expect("just pushed").1 = memory;
                device.bind_buffer_memory(buffer, memory, 0)?;
                let mapped =
                    device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())? as *mut u32;
                std::ptr::copy_nonoverlapping(data.as_ptr(), mapped, data.len());
                device.unmap_memory(memory);
            }
            let bindings: Vec<_> = (0..buffers.len() as u32)
                .map(|binding| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(binding)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::COMPUTE)
                })
                .collect();
            scratch.set_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let addresses: Vec<u64> = scratch
                .buffers
                .iter()
                .map(|(buffer, _, _)| {
                    device.get_buffer_device_address(
                        &vk::BufferDeviceAddressInfo::default().buffer(*buffer),
                    )
                })
                .collect();
            let constants = push(&addresses);
            let ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .size((constants.len() * 4) as u32)];
            let set_layouts = [scratch.set_layout];
            let mut layout = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
            if !constants.is_empty() {
                layout = layout.push_constant_ranges(&ranges);
            }
            scratch.pipeline_layout = device.create_pipeline_layout(&layout, None)?;
            scratch.module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(spirv), None)?;
            let mut required = vk::PipelineShaderStageRequiredSubgroupSizeCreateInfo::default()
                .required_subgroup_size(subgroup_size.unwrap_or(self.info.subgroup_size));
            let mut stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(scratch.module)
                .name(c"main");
            if let Some(size) = subgroup_size
                && self.info.subgroup_size_control
                && (self.info.min_subgroup_size..=self.info.max_subgroup_size).contains(&size)
            {
                stage = stage.push_next(&mut required);
                if self.info.full_subgroups {
                    stage = stage.flags(vk::PipelineShaderStageCreateFlags::REQUIRE_FULL_SUBGROUPS);
                }
            }
            let create = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(scratch.pipeline_layout);
            scratch.pipeline = device
                .create_compute_pipelines(vk::PipelineCache::null(), &[create], None)
                .map_err(|(_, error)| error)?[0];
            let sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(buffers.len().max(1) as u32)];
            scratch.pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?;
            let set = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(scratch.pool)
                    .set_layouts(&set_layouts),
            )?[0];
            let infos: Vec<[vk::DescriptorBufferInfo; 1]> = scratch
                .buffers
                .iter()
                .map(|(buffer, _, size)| {
                    [vk::DescriptorBufferInfo::default()
                        .buffer(*buffer)
                        .range(*size)]
                })
                .collect();
            let writes: Vec<_> = infos
                .iter()
                .enumerate()
                .map(|(binding, info)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(binding as u32)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(info)
                })
                .collect();
            device.update_descriptor_sets(&writes, &[]);
            scratch.command_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(self.family),
                None,
            )?;
            let command = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(scratch.command_pool)
                    .command_buffer_count(1),
            )?[0];
            device.begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, scratch.pipeline);
            device.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::COMPUTE,
                scratch.pipeline_layout,
                0,
                &[set],
                &[],
            );
            if !constants.is_empty() {
                let bytes: Vec<u8> = constants.iter().flat_map(|w| w.to_le_bytes()).collect();
                device.cmd_push_constants(
                    command,
                    scratch.pipeline_layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    &bytes,
                );
            }
            device.cmd_dispatch(command, groups[0], groups[1], groups[2]);
            let barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ);
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
            device.end_command_buffer(command)?;
            scratch.fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let commands = [command];
            device.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&commands)],
                scratch.fence,
            )?;
            device.wait_for_fences(&[scratch.fence], true, u64::MAX)?;
            for (data, (_, memory, size)) in buffers.iter_mut().zip(&scratch.buffers) {
                let mapped = device.map_memory(*memory, 0, *size, vk::MemoryMapFlags::empty())?
                    as *const u32;
                std::ptr::copy_nonoverlapping(mapped, data.as_mut_ptr(), data.len());
                device.unmap_memory(*memory);
            }
        }
        Ok(())
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: all work was waited for in `dispatch`; handles are destroyed child-first.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            if let Some((utils, messenger)) = &self.debug {
                utils.destroy_debug_utils_messenger(*messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

struct Scratch<'a> {
    device: &'a ash::Device,
    buffers: Vec<(vk::Buffer, vk::DeviceMemory, u64)>,
    set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    module: vk::ShaderModule,
    pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    command_pool: vk::CommandPool,
    fence: vk::Fence,
}

impl<'a> Scratch<'a> {
    fn new(device: &'a ash::Device) -> Scratch<'a> {
        Scratch {
            device,
            buffers: Vec::new(),
            set_layout: vk::DescriptorSetLayout::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            module: vk::ShaderModule::null(),
            pipeline: vk::Pipeline::null(),
            pool: vk::DescriptorPool::null(),
            command_pool: vk::CommandPool::null(),
            fence: vk::Fence::null(),
        }
    }
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        // SAFETY: the handles were created from `self.device`; null handles are ignored by Vulkan.
        unsafe {
            let d = self.device;
            let _ = d.device_wait_idle();
            d.destroy_fence(self.fence, None);
            d.destroy_command_pool(self.command_pool, None);
            d.destroy_descriptor_pool(self.pool, None);
            d.destroy_pipeline(self.pipeline, None);
            d.destroy_shader_module(self.module, None);
            d.destroy_pipeline_layout(self.pipeline_layout, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
            for (buffer, memory, _) in &self.buffers {
                d.destroy_buffer(*buffer, None);
                d.free_memory(*memory, None);
            }
        }
    }
}
