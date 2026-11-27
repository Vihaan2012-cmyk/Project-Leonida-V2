//! The Vulkan device that pipelines are built on and command streams are replayed on.
//!
//! The device layer hands its own handles over with [`VkContext::from_parts`]; [`VkContext::standalone`] opens a
//! headless device with the features translated shaders need, for tests and tools.

use std::ffi::CStr;

use ash::vk;

use super::ReplayError;

/// What the device can do beyond Vulkan 1.3's baseline, as far as pipelines and replay care.
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    /// Dynamic rendering, fragment barycentrics and fragment stores: what translated pixel shaders need.
    pub pixel_shaders: bool,
    pub mesh_shaders: bool,
    pub geometry_shaders: bool,
    pub depth_clamp: bool,
    pub logic_op: bool,
    pub depth_bounds: bool,
    pub multi_draw_indirect: bool,
    pub subgroup_size_control: bool,
    pub full_subgroups: bool,
    pub min_subgroup_size: u32,
    pub max_subgroup_size: u32,
}

pub struct VkContext {
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub family: u32,
    pub properties: vk::PhysicalDeviceProperties,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub features: Features,
    pub mesh: Option<ash::ext::mesh_shader::Device>,
    owned: Option<Owned>,
}

struct Owned {
    _entry: ash::Entry,
}

impl VkContext {
    /// Wraps a device someone else created and will destroy. The device must be Vulkan 1.3 with dynamic
    /// rendering, synchronization2 and buffer device addresses on; `mesh` is the extension's loader when it is
    /// enabled.
    pub fn from_parts(
        instance: ash::Instance,
        physical: vk::PhysicalDevice,
        device: ash::Device,
        queue: vk::Queue,
        family: u32,
        features: Features,
    ) -> VkContext {
        // SAFETY: `physical` came from `instance`.
        let (properties, memory) = unsafe {
            (
                instance.get_physical_device_properties(physical),
                instance.get_physical_device_memory_properties(physical),
            )
        };
        let mesh = features
            .mesh_shaders
            .then(|| ash::ext::mesh_shader::Device::new(&instance, &device));
        VkContext {
            instance,
            physical,
            device,
            queue,
            family,
            properties,
            memory,
            features,
            mesh,
            owned: None,
        }
    }

    /// Opens a headless device on the best GPU (`LEONIDA_GPU` picks one by name), with the features translated
    /// shaders use when the GPU has them, and the validation layer when `LEONIDA_VALIDATION` is set.
    pub fn standalone() -> Result<VkContext, ReplayError> {
        // SAFETY: loading the system Vulkan loader; every handle created below is destroyed in Drop.
        unsafe {
            let entry = ash::Entry::load().map_err(|e| ReplayError::NoDevice(e.to_string()))?;
            let app = vk::ApplicationInfo::default()
                .application_name(c"leonida")
                .api_version(vk::API_VERSION_1_3);
            let layers: Vec<*const i8> = if std::env::var_os("LEONIDA_VALIDATION").is_some()
                && entry
                    .enumerate_instance_layer_properties()?
                    .iter()
                    .any(|l| l.layer_name_as_c_str() == Ok(c"VK_LAYER_KHRONOS_validation"))
            {
                vec![c"VK_LAYER_KHRONOS_validation".as_ptr()]
            } else {
                Vec::new()
            };
            let instance = entry.create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_layer_names(&layers),
                None,
            )?;
            let wanted = std::env::var("LEONIDA_GPU").ok().map(|n| n.to_lowercase());
            let mut candidates = Vec::new();
            for physical in instance.enumerate_physical_devices()? {
                let props = instance.get_physical_device_properties(physical);
                if props.api_version < vk::API_VERSION_1_3 {
                    continue;
                }
                let name = props
                    .device_name_as_c_str()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if wanted.as_ref().is_some_and(|w| !name.contains(w.as_str())) {
                    continue;
                }
                let Some(family) = instance
                    .get_physical_device_queue_family_properties(physical)
                    .iter()
                    .position(|f| {
                        f.queue_flags
                            .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                    })
                else {
                    continue;
                };
                candidates.push((
                    props.device_type != vk::PhysicalDeviceType::DISCRETE_GPU,
                    physical,
                    family as u32,
                ));
            }
            candidates.sort_by_key(|c| c.0);
            let Some(&(_, physical, family)) = candidates.first() else {
                instance.destroy_instance(None);
                return Err(ReplayError::NoDevice(
                    "no Vulkan 1.3 GPU with a graphics queue".into(),
                ));
            };
            let extensions = instance.enumerate_device_extension_properties(physical)?;
            let has = |name: &CStr| {
                extensions
                    .iter()
                    .any(|e| e.extension_name_as_c_str() == Ok(name))
            };
            let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut f11 = vk::PhysicalDeviceVulkan11Features::default();
            let mut bary = vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR::default();
            let mut mesh = vk::PhysicalDeviceMeshShaderFeaturesEXT::default();
            let mut interlock = vk::PhysicalDeviceFragmentShaderInterlockFeaturesEXT::default();
            let mut atomics64 = vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default();
            let (has_bary, has_mesh, has_interlock, has_atomics) = (
                has(ash::khr::fragment_shader_barycentric::NAME),
                has(ash::ext::mesh_shader::NAME),
                has(ash::ext::fragment_shader_interlock::NAME),
                has(ash::ext::shader_image_atomic_int64::NAME),
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
            let pixel_shaders = has_bary
                && bary.fragment_shader_barycentric == vk::TRUE
                && base.fragment_stores_and_atomics == vk::TRUE
                && base.vertex_pipeline_stores_and_atomics == vk::TRUE
                && f11.shader_draw_parameters == vk::TRUE
                && f13.dynamic_rendering == vk::TRUE;
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
                logic_op: base.logic_op == vk::TRUE,
                depth_bounds: base.depth_bounds == vk::TRUE,
                multi_draw_indirect: base.multi_draw_indirect == vk::TRUE,
                subgroup_size_control: size_control,
                full_subgroups: f13.compute_full_subgroups == vk::TRUE,
                min_subgroup_size: v13.min_subgroup_size,
                max_subgroup_size: v13.max_subgroup_size,
            };
            let indexing = f12.runtime_descriptor_array == vk::TRUE
                && f12.shader_sampled_image_array_non_uniform_indexing == vk::TRUE;
            let storage = indexing
                && f12.shader_storage_image_array_non_uniform_indexing == vk::TRUE
                && base.shader_storage_image_write_without_format == vk::TRUE;
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(&priorities)];
            let mut e13 = vk::PhysicalDeviceVulkan13Features::default()
                .dynamic_rendering(f13.dynamic_rendering == vk::TRUE)
                .synchronization2(true)
                .subgroup_size_control(f13.subgroup_size_control == vk::TRUE)
                .compute_full_subgroups(f13.compute_full_subgroups == vk::TRUE);
            let mut e12 = vk::PhysicalDeviceVulkan12Features::default()
                .buffer_device_address(true)
                .runtime_descriptor_array(indexing)
                .shader_sampled_image_array_non_uniform_indexing(indexing)
                .shader_storage_image_array_non_uniform_indexing(storage)
                .draw_indirect_count(f12.draw_indirect_count == vk::TRUE);
            let mut e11 = vk::PhysicalDeviceVulkan11Features::default()
                .shader_draw_parameters(f11.shader_draw_parameters == vk::TRUE);
            let enabled = vk::PhysicalDeviceFeatures::default()
                .shader_int64(base.shader_int64 == vk::TRUE)
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
            let mut names = Vec::new();
            if pixel_shaders {
                chain = chain.push_next(&mut e_bary);
                names.push(ash::khr::fragment_shader_barycentric::NAME.as_ptr());
            }
            if mesh_shaders {
                chain = chain.push_next(&mut e_mesh);
                names.push(ash::ext::mesh_shader::NAME.as_ptr());
            }
            if has_interlock && interlock.fragment_shader_pixel_interlock == vk::TRUE {
                chain = chain.push_next(&mut e_interlock);
                names.push(ash::ext::fragment_shader_interlock::NAME.as_ptr());
            }
            if has_atomics && atomics64.shader_image_int64_atomics == vk::TRUE {
                chain = chain.push_next(&mut e_atomics);
                names.push(ash::ext::shader_image_atomic_int64::NAME.as_ptr());
            }
            let device = instance.create_device(
                physical,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queues)
                    .enabled_extension_names(&names)
                    .push_next(&mut chain),
                None,
            )?;
            let queue = device.get_device_queue(family, 0);
            let mut context =
                VkContext::from_parts(instance, physical, device, queue, family, features);
            context.owned = Some(Owned { _entry: entry });
            Ok(context)
        }
    }

    /// The index of a memory type allowed by `bits` with all of `flags`.
    pub fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.memory.memory_type_count).find(|&i| {
            bits & (1 << i) != 0
                && self.memory.memory_types[i as usize]
                    .property_flags
                    .contains(flags)
        })
    }
}

impl Drop for VkContext {
    fn drop(&mut self) {
        if self.owned.is_some() {
            // SAFETY: the context created these handles and every object made from them is gone by now.
            unsafe {
                let _ = self.device.device_wait_idle();
                self.device.destroy_device(None);
                self.instance.destroy_instance(None);
            }
        }
    }
}
