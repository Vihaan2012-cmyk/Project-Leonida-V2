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
    #[error("texture data does not match its size, format and mip count")]
    BadTexture,
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
    pub descriptor_indexing: bool,
    pub storage_images: bool,
    pub min_lod: bool,
    pub gather_extended: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    Rgba32Float,
    Rgba32Uint,
    Rgba32Sint,
    Rgba8Unorm,
    Depth32Float,
}

impl TextureFormat {
    pub fn texel_bytes(self) -> usize {
        match self {
            TextureFormat::Rgba8Unorm | TextureFormat::Depth32Float => 4,
            _ => 16,
        }
    }

    fn vk(self) -> vk::Format {
        match self {
            TextureFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
            TextureFormat::Rgba32Uint => vk::Format::R32G32B32A32_UINT,
            TextureFormat::Rgba32Sint => vk::Format::R32G32B32A32_SINT,
            TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            TextureFormat::Depth32Float => vk::Format::D32_SFLOAT,
        }
    }

    fn aspect(self) -> vk::ImageAspectFlags {
        match self {
            TextureFormat::Depth32Float => vk::ImageAspectFlags::DEPTH,
            _ => vk::ImageAspectFlags::COLOR,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureView {
    D2,
    D2Array,
    D3,
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub view: TextureView,
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    /// Slices of a 3D texture, layers of an array.
    pub depth: u32,
    pub mips: u32,
    /// Every mip, largest first; within a mip, layer by layer or slice by slice, rows top to bottom.
    pub data: Vec<u8>,
}

impl Texture {
    pub fn extent(&self, mip: u32) -> [u32; 3] {
        let depth = match self.view {
            TextureView::D3 => (self.depth >> mip).max(1),
            _ => 1,
        };
        [
            (self.width >> mip).max(1),
            (self.height >> mip).max(1),
            depth,
        ]
    }

    pub fn layers(&self) -> u32 {
        match self.view {
            TextureView::D2Array => self.depth,
            _ => 1,
        }
    }

    /// Bytes of one mip, all layers included.
    pub fn mip_bytes(&self, mip: u32) -> usize {
        let [w, h, d] = self.extent(mip);
        (w * h * d * self.layers()) as usize * self.format.texel_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler {
    pub linear: bool,
    pub repeat: bool,
    /// A depth comparison, as a `vk::CompareOp` value (the hardware's DEPTH_COMPARE_FUNC uses the same order).
    pub compare: Option<i32>,
}

#[derive(Debug, Clone)]
pub enum Resource {
    Buffer(Vec<u32>),
    Samplers(Vec<Sampler>),
    Textures(Vec<Texture>),
    /// Writable textures; their data is read back after the dispatch.
    StorageTextures(Vec<Texture>),
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
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut f13)
            .push_next(&mut f12);
        instance.get_physical_device_features2(physical, &mut features);
        let int64 = features.features.shader_int64 == vk::TRUE;
        let float64 = features.features.shader_float64 == vk::TRUE;
        let write_without_format =
            features.features.shader_storage_image_write_without_format == vk::TRUE;
        let min_lod = features.features.shader_resource_min_lod == vk::TRUE;
        let gather_extended = features.features.shader_image_gather_extended == vk::TRUE;
        let descriptor_indexing = f12.runtime_descriptor_array == vk::TRUE
            && f12.shader_sampled_image_array_non_uniform_indexing == vk::TRUE;
        let storage_images = descriptor_indexing
            && f12.shader_storage_image_array_non_uniform_indexing == vk::TRUE
            && write_without_format;
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
                descriptor_indexing,
                storage_images,
                min_lod,
                gather_extended,
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
            let indexing = chosen.info.descriptor_indexing;
            let storage = chosen.info.storage_images;
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default()
                .buffer_device_address(true)
                .runtime_descriptor_array(indexing)
                .shader_sampled_image_array_non_uniform_indexing(indexing)
                .shader_storage_image_array_non_uniform_indexing(storage);
            let base = vk::PhysicalDeviceFeatures::default()
                .shader_int64(chosen.info.int64)
                .shader_float64(chosen.info.float64)
                .shader_storage_image_write_without_format(storage)
                .shader_resource_min_lod(chosen.info.min_lod)
                .shader_image_gather_extended(chosen.info.gather_extended);
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
        self.memory_type_with(
            bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
    }

    fn memory_type_with(&self, bits: u32, wanted: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.memory.memory_type_count).find(|i| {
            bits & (1 << i) != 0
                && self.memory.memory_types[*i as usize]
                    .property_flags
                    .contains(wanted)
        })
    }

    /// A host-visible buffer holding `data`.
    ///
    /// # Safety
    /// The caller destroys the returned handles after the GPU has finished with them.
    unsafe fn host_buffer(
        &self,
        data: &[u32],
        usage: vk::BufferUsageFlags,
    ) -> Result<(vk::Buffer, vk::DeviceMemory, u64), GpuError> {
        let device = &self.device;
        let size = (data.len().max(1) * 4) as u64;
        // SAFETY: the buffer and memory are fresh, bound once, and the mapping covers `size` bytes.
        unsafe {
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default().size(size).usage(usage),
                None,
            )?;
            let needs = device.get_buffer_memory_requirements(buffer);
            let Some(kind) = self.memory_type(needs.memory_type_bits) else {
                device.destroy_buffer(buffer, None);
                return Err(GpuError::NoMemory);
            };
            let mut flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind)
                    .push_next(&mut flags),
                None,
            )?;
            device.bind_buffer_memory(buffer, memory, 0)?;
            let mapped =
                device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())? as *mut u32;
            std::ptr::copy_nonoverlapping(data.as_ptr(), mapped, data.len());
            device.unmap_memory(memory);
            Ok((buffer, memory, size))
        }
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
        self.dispatch_resources(spirv, buffers, push, groups, subgroup_size, &mut [])
    }

    /// Like `dispatch_with`, with descriptor set 1 holding `resources` at their bindings.
    pub fn dispatch_resources(
        &self,
        spirv: &[u32],
        buffers: &mut [&mut [u32]],
        push: impl FnOnce(&[u64]) -> Vec<u32>,
        groups: [u32; 3],
        subgroup_size: Option<u32>,
        resources: &mut [(u32, Resource)],
    ) -> Result<(), GpuError> {
        if spirv.is_empty() {
            return Err(GpuError::BadSpirv);
        }
        let mut scratch = Scratch::new(&self.device);
        // SAFETY: every handle is created from `self.device`, recorded in `scratch` and destroyed when it drops,
        // after the fence wait below has made the GPU finish with them.
        unsafe {
            let device = &self.device;
            let storage =
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
            for data in buffers.iter() {
                scratch.buffers.push(self.host_buffer(data, storage)?);
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
            let mut extra_bindings = Vec::new();
            let mut counts = [0u32; 4];
            for (binding, resource) in resources.iter() {
                let (kind, slot, count) = match resource {
                    Resource::Buffer(data) => {
                        scratch.extra_buffers.push(self.host_buffer(data, storage)?);
                        (vk::DescriptorType::STORAGE_BUFFER, 0, 1)
                    }
                    Resource::Samplers(list) => {
                        for sampler in list {
                            scratch.samplers.push(self.sampler(*sampler)?);
                        }
                        (vk::DescriptorType::SAMPLER, 1, list.len())
                    }
                    Resource::Textures(list) => {
                        for texture in list {
                            self.texture(&mut scratch, texture, false)?;
                        }
                        (vk::DescriptorType::SAMPLED_IMAGE, 2, list.len())
                    }
                    Resource::StorageTextures(list) => {
                        for texture in list {
                            self.texture(&mut scratch, texture, true)?;
                        }
                        (vk::DescriptorType::STORAGE_IMAGE, 3, list.len())
                    }
                };
                counts[slot] += count as u32;
                extra_bindings.push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(*binding)
                        .descriptor_type(kind)
                        .descriptor_count(count as u32)
                        .stage_flags(vk::ShaderStageFlags::COMPUTE),
                );
            }
            if !resources.is_empty() {
                scratch.extra_layout = device.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&extra_bindings),
                    None,
                )?;
            }
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
            let mut set_layouts = vec![scratch.set_layout];
            if !resources.is_empty() {
                set_layouts.push(scratch.extra_layout);
            }
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
            counts[0] += buffers.len() as u32;
            let mut sizes: Vec<vk::DescriptorPoolSize> = [
                vk::DescriptorType::STORAGE_BUFFER,
                vk::DescriptorType::SAMPLER,
                vk::DescriptorType::SAMPLED_IMAGE,
                vk::DescriptorType::STORAGE_IMAGE,
            ]
            .into_iter()
            .zip(counts)
            .filter(|(_, count)| *count > 0)
            .map(|(ty, count)| {
                vk::DescriptorPoolSize::default()
                    .ty(ty)
                    .descriptor_count(count)
            })
            .collect();
            if sizes.is_empty() {
                sizes.push(
                    vk::DescriptorPoolSize::default()
                        .ty(vk::DescriptorType::STORAGE_BUFFER)
                        .descriptor_count(1),
                );
            }
            scratch.pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(set_layouts.len() as u32)
                    .pool_sizes(&sizes),
                None,
            )?;
            let sets = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(scratch.pool)
                    .set_layouts(&set_layouts),
            )?;
            let buffer_infos: Vec<[vk::DescriptorBufferInfo; 1]> = scratch
                .buffers
                .iter()
                .chain(&scratch.extra_buffers)
                .map(|(buffer, _, size)| {
                    [vk::DescriptorBufferInfo::default()
                        .buffer(*buffer)
                        .range(*size)]
                })
                .collect();
            let mut image_infos = Vec::new();
            let (mut next_sampler, mut next_view) = (0, 0);
            for (_, resource) in resources.iter() {
                image_infos.push(match resource {
                    Resource::Buffer(_) => Vec::new(),
                    Resource::Samplers(list) => {
                        let infos: Vec<_> = scratch.samplers
                            [next_sampler..next_sampler + list.len()]
                            .iter()
                            .map(|s| vk::DescriptorImageInfo::default().sampler(*s))
                            .collect();
                        next_sampler += list.len();
                        infos
                    }
                    Resource::Textures(list) | Resource::StorageTextures(list) => {
                        let layout = if matches!(resource, Resource::StorageTextures(_)) {
                            vk::ImageLayout::GENERAL
                        } else {
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                        };
                        let infos: Vec<_> = scratch.images[next_view..next_view + list.len()]
                            .iter()
                            .map(|(_, _, view)| {
                                vk::DescriptorImageInfo::default()
                                    .image_view(*view)
                                    .image_layout(layout)
                            })
                            .collect();
                        next_view += list.len();
                        infos
                    }
                });
            }
            let mut writes: Vec<_> = buffer_infos[..buffers.len()]
                .iter()
                .enumerate()
                .map(|(binding, info)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(sets[0])
                        .dst_binding(binding as u32)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(info)
                })
                .collect();
            let mut next_buffer = buffers.len();
            for ((binding, resource), images) in resources.iter().zip(&image_infos) {
                let write = vk::WriteDescriptorSet::default()
                    .dst_set(sets[1])
                    .dst_binding(*binding);
                writes.push(match resource {
                    Resource::Buffer(_) => {
                        next_buffer += 1;
                        write
                            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                            .buffer_info(&buffer_infos[next_buffer - 1])
                    }
                    Resource::Samplers(_) => write
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(images),
                    Resource::Textures(_) => write
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(images),
                    Resource::StorageTextures(_) => write
                        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                        .image_info(images),
                });
            }
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
            for upload in &scratch.uploads {
                upload.record(device, command);
            }
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, scratch.pipeline);
            device.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::COMPUTE,
                scratch.pipeline_layout,
                0,
                &sets,
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
            let mut readbacks = Vec::new();
            if scratch.uploads.iter().any(|u| u.storage) {
                let to_copy = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[to_copy],
                    &[],
                    &[],
                );
                for upload in scratch.uploads.iter().filter(|u| u.storage) {
                    let words = vec![0u32; upload.bytes.div_ceil(4)];
                    let target = self.host_buffer(&words, vk::BufferUsageFlags::TRANSFER_DST)?;
                    device.cmd_copy_image_to_buffer(
                        command,
                        upload.image,
                        vk::ImageLayout::GENERAL,
                        target.0,
                        &upload.regions,
                    );
                    readbacks.push(target);
                }
            }
            let barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ);
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
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
            let written = resources
                .iter_mut()
                .filter_map(|(_, r)| match r {
                    Resource::StorageTextures(list) => Some(list),
                    _ => None,
                })
                .flatten();
            for (texture, (_, memory, size)) in written.zip(&readbacks) {
                let mapped =
                    device.map_memory(*memory, 0, *size, vk::MemoryMapFlags::empty())? as *const u8;
                let length = texture.data.len();
                std::ptr::copy_nonoverlapping(mapped, texture.data.as_mut_ptr(), length);
                device.unmap_memory(*memory);
            }
            scratch.staging.extend(readbacks);
        }
        Ok(())
    }

    /// # Safety
    /// The caller destroys the sampler after the GPU has finished with it.
    unsafe fn sampler(&self, sampler: Sampler) -> Result<vk::Sampler, GpuError> {
        let compare = sampler.compare.map(vk::CompareOp::from_raw);
        let (filter, mipmap) = if sampler.linear {
            (vk::Filter::LINEAR, vk::SamplerMipmapMode::LINEAR)
        } else {
            (vk::Filter::NEAREST, vk::SamplerMipmapMode::NEAREST)
        };
        let address = if sampler.repeat {
            vk::SamplerAddressMode::REPEAT
        } else {
            vk::SamplerAddressMode::CLAMP_TO_EDGE
        };
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(filter)
            .min_filter(filter)
            .mipmap_mode(mipmap)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .compare_enable(compare.is_some())
            .compare_op(compare.unwrap_or(vk::CompareOp::NEVER))
            .max_lod(vk::LOD_CLAMP_NONE);
        // SAFETY: a plain sampler from valid create info.
        Ok(unsafe { self.device.create_sampler(&info, None)? })
    }

    /// Creates the image and its view, and a staged upload to record before the dispatch.
    ///
    /// # Safety
    /// `scratch` must belong to `self.device`; it owns everything created here.
    unsafe fn texture(
        &self,
        scratch: &mut Scratch<'_>,
        texture: &Texture,
        storage: bool,
    ) -> Result<(), GpuError> {
        let device = &self.device;
        let expected: usize = (0..texture.mips).map(|m| texture.mip_bytes(m)).sum();
        if texture.data.len() != expected || texture.mips == 0 {
            return Err(GpuError::BadTexture);
        }
        let (image_type, view_type) = match texture.view {
            TextureView::D2 => (vk::ImageType::TYPE_2D, vk::ImageViewType::TYPE_2D),
            TextureView::D2Array => (vk::ImageType::TYPE_2D, vk::ImageViewType::TYPE_2D_ARRAY),
            TextureView::D3 => (vk::ImageType::TYPE_3D, vk::ImageViewType::TYPE_3D),
        };
        let [width, height, depth] = texture.extent(0);
        // SAFETY: fresh handles from valid create infos; each is recorded in `scratch` straight away.
        unsafe {
            let image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(image_type)
                    .format(texture.format.vk())
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth,
                    })
                    .mip_levels(texture.mips)
                    .array_layers(texture.layers())
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(if storage {
                        vk::ImageUsageFlags::STORAGE
                            | vk::ImageUsageFlags::TRANSFER_DST
                            | vk::ImageUsageFlags::TRANSFER_SRC
                    } else {
                        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST
                    })
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            scratch
                .images
                .push((image, vk::DeviceMemory::null(), vk::ImageView::null()));
            let needs = device.get_image_memory_requirements(image);
            let kind = self
                .memory_type_with(
                    needs.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
                .or_else(|| {
                    self.memory_type_with(needs.memory_type_bits, vk::MemoryPropertyFlags::empty())
                })
                .ok_or(GpuError::NoMemory)?;
            let memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            )?;
            scratch.images.last_mut().expect("just pushed").1 = memory;
            device.bind_image_memory(image, memory, 0)?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(texture.format.aspect())
                .level_count(texture.mips)
                .layer_count(texture.layers());
            let view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(view_type)
                    .format(texture.format.vk())
                    .subresource_range(range),
                None,
            )?;
            scratch.images.last_mut().expect("just pushed").2 = view;
            let words: Vec<u32> = texture
                .data
                .chunks(4)
                .map(|c| {
                    let mut word = [0u8; 4];
                    word[..c.len()].copy_from_slice(c);
                    u32::from_le_bytes(word)
                })
                .collect();
            let staging = self.host_buffer(&words, vk::BufferUsageFlags::TRANSFER_SRC)?;
            scratch.staging.push(staging);
            let mut regions = Vec::new();
            let mut offset = 0;
            for mip in 0..texture.mips {
                let [width, height, depth] = texture.extent(mip);
                regions.push(
                    vk::BufferImageCopy::default()
                        .buffer_offset(offset as u64)
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(texture.format.aspect())
                                .mip_level(mip)
                                .layer_count(texture.layers()),
                        )
                        .image_extent(vk::Extent3D {
                            width,
                            height,
                            depth,
                        }),
                );
                offset += texture.mip_bytes(mip);
            }
            scratch.uploads.push(Upload {
                buffer: staging.0,
                image,
                range,
                regions,
                storage,
                bytes: expected,
            });
        }
        Ok(())
    }
}

struct Upload {
    buffer: vk::Buffer,
    image: vk::Image,
    range: vk::ImageSubresourceRange,
    regions: Vec<vk::BufferImageCopy>,
    storage: bool,
    bytes: usize,
}

impl Upload {
    /// # Safety
    /// `command` is recording, and the buffer and image outlive its execution.
    unsafe fn record(&self, device: &ash::Device, command: vk::CommandBuffer) {
        let to_transfer = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.image)
            .subresource_range(self.range);
        let to_shader = to_transfer
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(if self.storage {
                vk::ImageLayout::GENERAL
            } else {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            });
        // SAFETY: guaranteed by the caller.
        unsafe {
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            device.cmd_copy_buffer_to_image(
                command,
                self.buffer,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &self.regions,
            );
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_shader],
            );
        }
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
    extra_buffers: Vec<(vk::Buffer, vk::DeviceMemory, u64)>,
    staging: Vec<(vk::Buffer, vk::DeviceMemory, u64)>,
    images: Vec<(vk::Image, vk::DeviceMemory, vk::ImageView)>,
    samplers: Vec<vk::Sampler>,
    uploads: Vec<Upload>,
    set_layout: vk::DescriptorSetLayout,
    extra_layout: vk::DescriptorSetLayout,
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
            extra_buffers: Vec::new(),
            staging: Vec::new(),
            images: Vec::new(),
            samplers: Vec::new(),
            uploads: Vec::new(),
            set_layout: vk::DescriptorSetLayout::null(),
            extra_layout: vk::DescriptorSetLayout::null(),
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
            d.destroy_descriptor_set_layout(self.extra_layout, None);
            let all = self.buffers.iter().chain(&self.extra_buffers);
            for (buffer, memory, _) in all.chain(&self.staging) {
                d.destroy_buffer(*buffer, None);
                d.free_memory(*memory, None);
            }
            for (image, memory, view) in &self.images {
                d.destroy_image_view(*view, None);
                d.destroy_image(*image, None);
                d.free_memory(*memory, None);
            }
            for sampler in &self.samplers {
                d.destroy_sampler(*sampler, None);
            }
        }
    }
}
