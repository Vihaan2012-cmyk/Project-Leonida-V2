//! The one Vulkan instance and device every part of the layer shares.
//!
//! It is Vulkan 1.3 with what the parts ask for between them: dynamic rendering, synchronization2, buffer device
//! addresses, 64-bit shader integers and timeline semaphores for replay and pipelines; sparse binding with residency
//! and `VK_EXT_external_memory_host` for the memory part (`memory::Support::check` decides); the swap chain and
//! surface extensions for presentation; and descriptor indexing for the bindless image set. Fragment barycentrics,
//! mesh shaders, pixel interlock and 64-bit image atomics are enabled when the GPU has them.

use std::collections::HashSet;
use std::ffi::{CStr, c_char};
use std::sync::{Arc, Mutex};

use ash::vk;

use crate::execute::{Features, VkContext};
use crate::memory::{self, Handles};
use crate::present::VulkanContext;
use crate::present::vulkan as present_vulkan;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("Vulkan loader: {0}")]
    Loader(String),
    #[error("Vulkan: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("no GPU can host the layer: {0}")]
    NoGpu(String),
}

/// What the caller wants of the device beyond what the parts require.
#[derive(Debug, Clone, Default)]
pub struct GpuConfig {
    /// Open the surface and swap chain extensions as required rather than as a courtesy.
    pub window: bool,
    /// A substring of the GPU's name (`LEONIDA_GPU`), compared in lower case.
    pub gpu: Option<String>,
    /// Enable the Khronos validation layer when it is installed (`LEONIDA_VALIDATION`).
    pub validation: bool,
}

impl GpuConfig {
    pub fn from_env(window: bool) -> GpuConfig {
        GpuConfig {
            window,
            gpu: std::env::var("LEONIDA_GPU").ok().map(|n| n.to_lowercase()),
            validation: std::env::var_os("LEONIDA_VALIDATION").is_some(),
        }
    }
}

/// The device and the handles the parts are built from.
pub struct Gpu {
    pub entry: ash::Entry,
    pub name: String,
    /// What replay and pipelines use.
    pub ctx: Arc<VkContext>,
    /// What presentation and an upscaler use. `queue` is the lock every submit to the main queue takes.
    pub vulkan: VulkanContext,
    /// What the memory part binds sparse memory through.
    pub handles: Handles,
    /// Whether sparse binding goes through a queue of its own (the family offered a second queue).
    pub separate_bind_queue: bool,
    /// Largest sampled and storage image arrays the bindless set may hold, from the device limits.
    pub image_array: u32,
    pub sampler_array: u32,
    pub partially_bound: bool,
    pub sampler_min_max: bool,
    pub optional: OptionalExtensions,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OptionalExtensions {
    pub barycentric: bool,
    pub mesh_shader: bool,
    pub pixel_interlock: bool,
    pub image_atomics_64: bool,
}

fn has(extensions: &[vk::ExtensionProperties], name: &CStr) -> bool {
    extensions
        .iter()
        .any(|e| e.extension_name_as_c_str() == Ok(name))
}

struct Choice {
    physical: vk::PhysicalDevice,
    family: u32,
    queues: u32,
    name: String,
    discrete: bool,
}

impl Gpu {
    pub fn create(config: &GpuConfig) -> Result<Gpu, GpuError> {
        // SAFETY: loads the system Vulkan loader; every handle made below lives as long as the returned `Gpu`
        // (the process, for the device the title uses).
        unsafe {
            let entry = ash::Entry::load().map_err(|e| GpuError::Loader(e.to_string()))?;
            let have = entry.enumerate_instance_extension_properties(None)?;
            let mut names: Vec<*const c_char> = Vec::new();
            let mut wanted: Vec<&CStr> = present_vulkan::instance_extensions();
            wanted.push(present_vulkan::COLORSPACE_EXTENSION);
            for name in wanted {
                if has(&have, name) {
                    names.push(name.as_ptr());
                } else if config.window && name != present_vulkan::COLORSPACE_EXTENSION {
                    return Err(GpuError::NoGpu(format!(
                        "the instance lacks {}",
                        name.to_string_lossy()
                    )));
                }
            }
            let layers: Vec<*const c_char> = if config.validation
                && entry
                    .enumerate_instance_layer_properties()?
                    .iter()
                    .any(|l| l.layer_name_as_c_str() == Ok(c"VK_LAYER_KHRONOS_validation"))
            {
                vec![c"VK_LAYER_KHRONOS_validation".as_ptr()]
            } else {
                Vec::new()
            };
            let app = vk::ApplicationInfo::default()
                .application_name(c"leonida")
                .api_version(vk::API_VERSION_1_3);
            let instance = entry.create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_extension_names(&names)
                    .enabled_layer_names(&layers),
                None,
            )?;
            match Self::on_instance(entry, instance.clone(), config) {
                Ok(gpu) => Ok(gpu),
                Err(e) => {
                    instance.destroy_instance(None);
                    Err(e)
                }
            }
        }
    }

    /// # Safety
    /// `instance` is a live instance made from `entry`, destroyed by the caller on failure.
    unsafe fn on_instance(
        entry: ash::Entry,
        instance: ash::Instance,
        config: &GpuConfig,
    ) -> Result<Gpu, GpuError> {
        // SAFETY: plain queries and creation on objects of `instance`; see the caller.
        unsafe {
            let mut reasons = Vec::new();
            let mut candidates = Vec::new();
            for physical in instance.enumerate_physical_devices()? {
                let props = instance.get_physical_device_properties(physical);
                let name = props
                    .device_name_as_c_str()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if config
                    .gpu
                    .as_ref()
                    .is_some_and(|w| !name.to_lowercase().contains(w.as_str()))
                {
                    continue;
                }
                match Self::suitable(&instance, physical, &props, config) {
                    Ok((family, queues)) => candidates.push(Choice {
                        physical,
                        family,
                        queues,
                        discrete: props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU,
                        name,
                    }),
                    Err(why) => reasons.push(format!("{name}: {why}")),
                }
            }
            candidates.sort_by_key(|c| !c.discrete);
            let Some(choice) = candidates.into_iter().next() else {
                return Err(GpuError::NoGpu(if reasons.is_empty() {
                    "no Vulkan device".into()
                } else {
                    reasons.join("; ")
                }));
            };
            Self::open(entry, instance, choice, config)
        }
    }

    /// The queue family to use and how many queues it has, or why the GPU will not do.
    fn suitable(
        instance: &ash::Instance,
        physical: vk::PhysicalDevice,
        props: &vk::PhysicalDeviceProperties,
        config: &GpuConfig,
    ) -> Result<(u32, u32), String> {
        if props.api_version < vk::API_VERSION_1_3 {
            return Err("older than Vulkan 1.3".into());
        }
        let support = memory::Support::query(instance, physical);
        support.check().map_err(|e| e.to_string())?;
        // SAFETY: queries on a physical device of `instance`.
        let (extensions, families, v12) = unsafe {
            let mut v12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
            let mut features = vk::PhysicalDeviceFeatures2::default()
                .push_next(&mut v12)
                .push_next(&mut v13);
            instance.get_physical_device_features2(physical, &mut features);
            if v13.dynamic_rendering != vk::TRUE || v13.synchronization2 != vk::TRUE {
                return Err("no dynamic rendering or synchronization2".into());
            }
            (
                instance
                    .enumerate_device_extension_properties(physical)
                    .unwrap_or_default(),
                instance.get_physical_device_queue_family_properties(physical),
                v12,
            )
        };
        if v12.timeline_semaphore != vk::TRUE {
            return Err("no timeline semaphores".into());
        }
        if v12.runtime_descriptor_array != vk::TRUE
            || v12.descriptor_binding_partially_bound != vk::TRUE
            || v12.shader_sampled_image_array_non_uniform_indexing != vk::TRUE
        {
            return Err("no descriptor indexing for the bindless image set".into());
        }
        for name in memory::DEVICE_EXTENSIONS {
            if !has(&extensions, name) {
                return Err(format!("no {}", name.to_string_lossy()));
            }
        }
        if config.window {
            for name in present_vulkan::device_extensions() {
                if !has(&extensions, name) {
                    return Err(format!("no {}", name.to_string_lossy()));
                }
            }
        }
        let want =
            vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::SPARSE_BINDING;
        families
            .iter()
            .position(|f| f.queue_flags.contains(want))
            .map(|i| (i as u32, families[i].queue_count))
            .ok_or_else(|| "no graphics queue family with sparse binding".to_string())
    }

    /// # Safety
    /// `instance` and `choice` are as `on_instance` made them.
    unsafe fn open(
        entry: ash::Entry,
        instance: ash::Instance,
        choice: Choice,
        config: &GpuConfig,
    ) -> Result<Gpu, GpuError> {
        // SAFETY: feature queries and device creation on `choice.physical`; the structures outlive the calls.
        unsafe {
            let physical = choice.physical;
            let extensions = instance.enumerate_device_extension_properties(physical)?;
            let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut f11 = vk::PhysicalDeviceVulkan11Features::default();
            let mut bary = vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR::default();
            let mut mesh = vk::PhysicalDeviceMeshShaderFeaturesEXT::default();
            let mut interlock = vk::PhysicalDeviceFragmentShaderInterlockFeaturesEXT::default();
            let mut atomics64 = vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default();
            let (has_bary, has_mesh, has_interlock, has_atomics) = (
                has(&extensions, ash::khr::fragment_shader_barycentric::NAME),
                has(&extensions, ash::ext::mesh_shader::NAME),
                has(&extensions, ash::ext::fragment_shader_interlock::NAME),
                has(&extensions, ash::ext::shader_image_atomic_int64::NAME),
            );
            let mut query = vk::PhysicalDeviceFeatures2::default()
                .push_next(&mut f13)
                .push_next(&mut f12)
                .push_next(&mut f11);
            if has_bary {
                query = query.push_next(&mut bary);
            }
            if has_mesh {
                query = query.push_next(&mut mesh);
            }
            if has_interlock {
                query = query.push_next(&mut interlock);
            }
            if has_atomics {
                query = query.push_next(&mut atomics64);
            }
            instance.get_physical_device_features2(physical, &mut query);
            let base = query.features;
            let mut v13 = vk::PhysicalDeviceVulkan13Properties::default();
            let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut v13);
            instance.get_physical_device_properties2(physical, &mut props2);
            let limits = props2.properties.limits;

            let pixel_shaders = has_bary
                && bary.fragment_shader_barycentric == vk::TRUE
                && base.fragment_stores_and_atomics == vk::TRUE
                && base.vertex_pipeline_stores_and_atomics == vk::TRUE
                && f11.shader_draw_parameters == vk::TRUE;
            let mesh_shaders = has_mesh && mesh.mesh_shader == vk::TRUE;
            let size_control = f13.subgroup_size_control == vk::TRUE
                && v13
                    .required_subgroup_size_stages
                    .contains(vk::ShaderStageFlags::COMPUTE);
            let features = Features {
                pixel_shaders,
                mesh_shaders,
                geometry_shaders: base.geometry_shader == vk::TRUE,
                depth_clamp: base.depth_clamp == vk::TRUE,
                fill_mode_non_solid: base.fill_mode_non_solid == vk::TRUE,
                logic_op: base.logic_op == vk::TRUE,
                depth_bounds: base.depth_bounds == vk::TRUE,
                multi_draw_indirect: base.multi_draw_indirect == vk::TRUE,
                subgroup_size_control: size_control,
                full_subgroups: f13.compute_full_subgroups == vk::TRUE,
                min_subgroup_size: v13.min_subgroup_size,
                max_subgroup_size: v13.max_subgroup_size,
            };
            let indexing = f12.runtime_descriptor_array == vk::TRUE
                && f12.shader_sampled_image_array_non_uniform_indexing == vk::TRUE
                && f12.descriptor_binding_partially_bound == vk::TRUE;
            let storage = indexing
                && f12.shader_storage_image_array_non_uniform_indexing == vk::TRUE
                && base.shader_storage_image_write_without_format == vk::TRUE;
            let min_max = f12.sampler_filter_minmax == vk::TRUE;

            let queue_count = choice.queues.clamp(1, 2);
            let priorities = [1.0, 1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(choice.family)
                .queue_priorities(&priorities[..queue_count as usize])];
            let mut e13 = vk::PhysicalDeviceVulkan13Features::default()
                .dynamic_rendering(true)
                .synchronization2(true)
                .subgroup_size_control(f13.subgroup_size_control == vk::TRUE)
                .compute_full_subgroups(f13.compute_full_subgroups == vk::TRUE);
            let mut e12 = vk::PhysicalDeviceVulkan12Features::default()
                .buffer_device_address(true)
                .timeline_semaphore(true)
                .runtime_descriptor_array(indexing)
                .descriptor_binding_partially_bound(indexing)
                .shader_sampled_image_array_non_uniform_indexing(indexing)
                .shader_storage_image_array_non_uniform_indexing(storage)
                .sampler_filter_minmax(min_max)
                .draw_indirect_count(f12.draw_indirect_count == vk::TRUE);
            let mut e11 = vk::PhysicalDeviceVulkan11Features::default()
                .shader_draw_parameters(f11.shader_draw_parameters == vk::TRUE);
            let enabled = vk::PhysicalDeviceFeatures::default()
                .sparse_binding(true)
                .sparse_residency_buffer(true)
                .shader_int64(true)
                .shader_float64(base.shader_float64 == vk::TRUE)
                .shader_storage_image_write_without_format(storage)
                .shader_resource_min_lod(base.shader_resource_min_lod == vk::TRUE)
                .shader_image_gather_extended(base.shader_image_gather_extended == vk::TRUE)
                .fragment_stores_and_atomics(pixel_shaders)
                .vertex_pipeline_stores_and_atomics(pixel_shaders)
                .geometry_shader(features.geometry_shaders)
                .depth_clamp(features.depth_clamp)
                .logic_op(features.logic_op)
                .depth_bounds(features.depth_bounds)
                .multi_draw_indirect(features.multi_draw_indirect)
                .independent_blend(base.independent_blend == vk::TRUE)
                .dual_src_blend(base.dual_src_blend == vk::TRUE)
                .fill_mode_non_solid(base.fill_mode_non_solid == vk::TRUE)
                .sampler_anisotropy(base.sampler_anisotropy == vk::TRUE)
                .wide_lines(false);
            let mut e_bary = vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR::default()
                .fragment_shader_barycentric(true);
            let mut e_mesh = vk::PhysicalDeviceMeshShaderFeaturesEXT::default().mesh_shader(true);
            let mut e_interlock = vk::PhysicalDeviceFragmentShaderInterlockFeaturesEXT::default()
                .fragment_shader_pixel_interlock(true);
            let mut e_atomics = vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default()
                .shader_image_int64_atomics(true);
            let mut chain = vk::PhysicalDeviceFeatures2::default()
                .features(enabled)
                .push_next(&mut e13)
                .push_next(&mut e12)
                .push_next(&mut e11);
            let mut seen: HashSet<&CStr> = HashSet::new();
            let mut names: Vec<*const c_char> = Vec::new();
            let mut add = |name: &'static CStr| {
                if seen.insert(name) {
                    names.push(name.as_ptr());
                }
            };
            for name in memory::DEVICE_EXTENSIONS {
                add(name);
            }
            for name in present_vulkan::device_extensions() {
                if config.window || has(&extensions, name) {
                    add(name);
                }
            }
            let mut optional = OptionalExtensions::default();
            if pixel_shaders {
                chain = chain.push_next(&mut e_bary);
                add(ash::khr::fragment_shader_barycentric::NAME);
                optional.barycentric = true;
            }
            if mesh_shaders {
                chain = chain.push_next(&mut e_mesh);
                add(ash::ext::mesh_shader::NAME);
                optional.mesh_shader = true;
            }
            if has_interlock && interlock.fragment_shader_pixel_interlock == vk::TRUE {
                chain = chain.push_next(&mut e_interlock);
                add(ash::ext::fragment_shader_interlock::NAME);
                optional.pixel_interlock = true;
            }
            if has_atomics && atomics64.shader_image_int64_atomics == vk::TRUE {
                chain = chain.push_next(&mut e_atomics);
                add(ash::ext::shader_image_atomic_int64::NAME);
                optional.image_atomics_64 = true;
            }
            let device = instance.create_device(
                physical,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queues)
                    .enabled_extension_names(&names)
                    .push_next(&mut chain),
                None,
            )?;
            let main_queue = device.get_device_queue(choice.family, 0);
            let separate_bind_queue = queue_count > 1;
            let bind_queue = if separate_bind_queue {
                device.get_device_queue(choice.family, 1)
            } else {
                main_queue
            };
            let ctx = Arc::new(VkContext::from_parts(
                instance.clone(),
                physical,
                device.clone(),
                main_queue,
                choice.family,
                features,
            ));
            let vulkan = VulkanContext {
                entry: entry.clone(),
                instance: instance.clone(),
                physical,
                device: device.clone(),
                queue: Arc::new(Mutex::new(main_queue)),
                family: choice.family,
            };
            let handles = Handles {
                instance,
                physical,
                device,
                queue: bind_queue,
                queue_family: choice.family,
                queue_lock: Arc::new(Mutex::new(())),
            };
            let per_stage_images = limits
                .max_per_stage_descriptor_sampled_images
                .min(limits.max_descriptor_set_sampled_images);
            let per_stage_storage = limits
                .max_per_stage_descriptor_storage_images
                .min(limits.max_descriptor_set_storage_images);
            let per_stage_samplers = limits
                .max_per_stage_descriptor_samplers
                .min(limits.max_descriptor_set_samplers);
            Ok(Gpu {
                entry,
                name: choice.name,
                ctx,
                vulkan,
                handles,
                separate_bind_queue,
                image_array: (per_stage_images / 9)
                    .min(per_stage_storage / 15)
                    .clamp(0, 16384),
                sampler_array: (per_stage_samplers / 2).clamp(0, 16384),
                partially_bound: indexing,
                sampler_min_max: min_max,
                optional,
            })
        }
    }

    /// The index of a memory type allowed by `bits` with all of `flags`.
    pub fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        self.ctx.memory_type(bits, flags)
    }
}
