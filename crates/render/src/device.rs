use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ash::vk;
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};

use crate::error::{RenderError, Result};

static VALIDATION_ERRORS: AtomicUsize = AtomicUsize::new(0);

/// Validation-layer errors reported since the process started (needs `DeviceOptions::validation`).
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

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// How to create a [`Device`].
#[derive(Debug, Clone)]
pub struct DeviceOptions {
    /// Turn on the Khronos validation layer when it is installed. Default: set when `LEONIDA_VALIDATION` is.
    pub validation: bool,
    /// Enable the surface and swapchain extensions so a [`crate::Swapchain`] can be made. Headless runs leave it off.
    pub present: bool,
    /// Pick the GPU whose name contains this text (case-insensitive). Default: `LEONIDA_GPU`.
    pub adapter: Option<String>,
    /// How many queues to ask the graphics family for (at least one is always made).
    pub queues: u32,
    /// A saved pipeline cache blob to start from.
    pub pipeline_cache: Vec<u8>,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        DeviceOptions {
            validation: std::env::var_os("LEONIDA_VALIDATION").is_some(),
            present: false,
            adapter: std::env::var("LEONIDA_GPU").ok(),
            queues: 2,
            pipeline_cache: Vec::new(),
        }
    }
}

/// Update-after-bind limits that size the bindless set.
#[derive(Debug, Clone, Copy, Default)]
pub struct DescriptorLimits {
    pub max_per_stage_samplers: u32,
    pub max_per_stage_sampled_images: u32,
    pub max_per_stage_storage_images: u32,
    pub max_per_stage_storage_buffers: u32,
    pub max_per_stage_resources: u32,
    pub max_sampler_allocation_count: u32,
}

/// What this GPU and driver offer beyond the Vulkan 1.3 baseline `render` requires.
#[derive(Debug, Clone)]
pub struct Features {
    /// Everything the bindless set needs: runtime arrays, non-uniform indexing, partially-bound update-after-bind.
    pub bindless: bool,
    /// `VK_EXT_external_memory_host`: GPU buffers can alias host memory.
    pub host_import: bool,
    /// Pointer and size alignment `host_import` demands.
    pub host_import_alignment: u64,
    /// The surface and swapchain extensions are enabled.
    pub swapchain: bool,
    /// BC1-BC7 textures can be created.
    pub bc_textures: bool,
    /// `VK_EXT_swapchain_colorspace` is enabled, so HDR surface formats can be listed.
    pub hdr_colorspace: bool,
    pub non_coherent_atom_size: u64,
    pub max_push_constants: u32,
    pub queue_count: u32,
    pub descriptor_limits: DescriptorLimits,
}

struct QueueSlot {
    handle: vk::Queue,
    timeline: vk::Semaphore,
    submitted: Mutex<u64>,
}

pub(crate) struct Inner {
    pub(crate) entry: ash::Entry,
    pub(crate) instance: ash::Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) device: ash::Device,
    pub(crate) surface_ext: Option<ash::khr::surface::Instance>,
    pub(crate) swapchain_ext: Option<ash::khr::swapchain::Device>,
    pub(crate) host_ext: Option<ash::ext::external_memory_host::Device>,
    pub(crate) allocator: Mutex<Option<Allocator>>,
    queues: Vec<QueueSlot>,
    pub(crate) family: u32,
    pub(crate) memory: vk::PhysicalDeviceMemoryProperties,
    pub(crate) limits: vk::PhysicalDeviceLimits,
    pub(crate) info: gpu::DeviceInfo,
    pub(crate) features: Features,
    pipeline_cache: vk::PipelineCache,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: every object holding a handle owns a `Device` clone, so none is alive when `Inner` drops; the
        // handles destroyed here were created in `Device::new` and are destroyed once.
        unsafe {
            let _ = self.device.device_wait_idle();
            drop(self.allocator.get_mut().ok().and_then(Option::take));
            self.device
                .destroy_pipeline_cache(self.pipeline_cache, None);
            for queue in &self.queues {
                self.device.destroy_semaphore(queue.timeline, None);
            }
            self.device.destroy_device(None);
            if let Some((utils, messenger)) = &self.debug {
                utils.destroy_debug_utils_messenger(*messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// The Vulkan device and everything long-lived around it: queues, the allocator and the pipeline cache. Cheap to
/// clone; every resource keeps its device alive.
#[derive(Clone)]
pub struct Device {
    pub(crate) inner: Arc<Inner>,
}

struct Candidate {
    physical: vk::PhysicalDevice,
    family: u32,
    queue_count: u32,
    info: gpu::DeviceInfo,
    features: Features,
    required_stages: vk::ShaderStageFlags,
    bindless: bool,
}

fn has_extension(list: &[vk::ExtensionProperties], name: &CStr) -> bool {
    list.iter().any(|e| e.extension_name_as_c_str() == Ok(name))
}

fn describe(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    present: bool,
    hdr_colorspace: bool,
) -> Option<Candidate> {
    // SAFETY: `physical` came from this instance and the property structs outlive the calls.
    unsafe {
        let base = instance.get_physical_device_properties(physical);
        if base.api_version < vk::API_VERSION_1_3 {
            return None;
        }
        let families = instance.get_physical_device_queue_family_properties(physical);
        let want = vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE;
        let family = families.iter().position(|f| f.queue_flags.contains(want))? as u32;
        let queue_count = families[family as usize].queue_count;
        let extensions = instance
            .enumerate_device_extension_properties(physical)
            .unwrap_or_default();
        let has = |name: &CStr| has_extension(&extensions, name);
        let mut v11 = vk::PhysicalDeviceVulkan11Properties::default();
        let mut v12 = vk::PhysicalDeviceVulkan12Properties::default();
        let mut v13 = vk::PhysicalDeviceVulkan13Properties::default();
        let mut host_props = vk::PhysicalDeviceExternalMemoryHostPropertiesEXT::default();
        let has_host = has(ash::ext::external_memory_host::NAME);
        let mut properties = vk::PhysicalDeviceProperties2::default()
            .push_next(&mut v11)
            .push_next(&mut v12)
            .push_next(&mut v13);
        if has_host {
            properties = properties.push_next(&mut host_props);
        }
        instance.get_physical_device_properties2(physical, &mut properties);
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut f11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut atomics64 = vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default();
        let mut barycentric = vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR::default();
        let mut mesh = vk::PhysicalDeviceMeshShaderFeaturesEXT::default();
        let mut interlock = vk::PhysicalDeviceFragmentShaderInterlockFeaturesEXT::default();
        let has_atomics64 = has(ash::ext::shader_image_atomic_int64::NAME);
        let has_barycentric = has(ash::khr::fragment_shader_barycentric::NAME);
        let has_mesh = has(ash::ext::mesh_shader::NAME);
        let has_interlock = has(ash::ext::fragment_shader_interlock::NAME);
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut f13)
            .push_next(&mut f12)
            .push_next(&mut f11);
        if has_atomics64 {
            features = features.push_next(&mut atomics64);
        }
        if has_barycentric {
            features = features.push_next(&mut barycentric);
        }
        if has_mesh {
            features = features.push_next(&mut mesh);
        }
        if has_interlock {
            features = features.push_next(&mut interlock);
        }
        instance.get_physical_device_features2(physical, &mut features);
        let base_features = features.features;
        if f12.timeline_semaphore != vk::TRUE
            || f12.buffer_device_address != vk::TRUE
            || f13.synchronization2 != vk::TRUE
            || f13.dynamic_rendering != vk::TRUE
        {
            return None;
        }
        let write_without_format =
            base_features.shader_storage_image_write_without_format == vk::TRUE;
        let fragment_stores = base_features.fragment_stores_and_atomics == vk::TRUE
            && base_features.vertex_pipeline_stores_and_atomics == vk::TRUE;
        let descriptor_indexing = f12.runtime_descriptor_array == vk::TRUE
            && f12.shader_sampled_image_array_non_uniform_indexing == vk::TRUE;
        let storage_images = descriptor_indexing
            && f12.shader_storage_image_array_non_uniform_indexing == vk::TRUE
            && write_without_format;
        let bindless = storage_images
            && f12.descriptor_binding_partially_bound == vk::TRUE
            && f12.descriptor_binding_sampled_image_update_after_bind == vk::TRUE
            && f12.descriptor_binding_storage_image_update_after_bind == vk::TRUE
            && f12.descriptor_binding_storage_buffer_update_after_bind == vk::TRUE
            && f12.descriptor_binding_update_unused_while_pending == vk::TRUE;
        let needed = vk::SubgroupFeatureFlags::BASIC
            | vk::SubgroupFeatureFlags::BALLOT
            | vk::SubgroupFeatureFlags::SHUFFLE;
        let name = base
            .device_name_as_c_str()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let pixel_shaders = has_barycentric
            && barycentric.fragment_shader_barycentric == vk::TRUE
            && fragment_stores
            && f11.shader_draw_parameters == vk::TRUE;
        let info = gpu::DeviceInfo {
            name,
            vendor: base.vendor_id,
            discrete: base.device_type == vk::PhysicalDeviceType::DISCRETE_GPU,
            subgroup_size: v11.subgroup_size,
            min_subgroup_size: v13.min_subgroup_size,
            max_subgroup_size: v13.max_subgroup_size,
            subgroup_size_control: f13.subgroup_size_control == vk::TRUE
                && v13
                    .required_subgroup_size_stages
                    .contains(vk::ShaderStageFlags::COMPUTE),
            full_subgroups: f13.compute_full_subgroups == vk::TRUE,
            ballot_and_shuffle: v11.subgroup_supported_operations.contains(needed)
                && v11
                    .subgroup_supported_stages
                    .contains(vk::ShaderStageFlags::COMPUTE),
            int64: base_features.shader_int64 == vk::TRUE,
            float64: base_features.shader_float64 == vk::TRUE,
            denorm_preserve_32: v12.shader_denorm_preserve_float32 == vk::TRUE,
            denorm_flush_32: v12.shader_denorm_flush_to_zero_float32 == vk::TRUE,
            nan_preserve_32: v12.shader_signed_zero_inf_nan_preserve_float32 == vk::TRUE,
            max_shared_memory: base.limits.max_compute_shared_memory_size,
            descriptor_indexing,
            storage_images,
            min_lod: base_features.shader_resource_min_lod == vk::TRUE,
            gather_extended: base_features.shader_image_gather_extended == vk::TRUE,
            image_int64_atomics: has_atomics64 && atomics64.shader_image_int64_atomics == vk::TRUE,
            pixel_shaders,
            mesh_shaders: has_mesh && mesh.mesh_shader == vk::TRUE,
            quad_operations: v11
                .subgroup_supported_operations
                .contains(vk::SubgroupFeatureFlags::QUAD),
            pixel_interlock: has_interlock && interlock.fragment_shader_pixel_interlock == vk::TRUE,
            geometry_shaders: base_features.geometry_shader == vk::TRUE,
        };
        let features = Features {
            bindless,
            host_import: has_host,
            host_import_alignment: host_props.min_imported_host_pointer_alignment.max(1),
            swapchain: present && has(ash::khr::swapchain::NAME),
            bc_textures: base_features.texture_compression_bc == vk::TRUE,
            hdr_colorspace,
            non_coherent_atom_size: base.limits.non_coherent_atom_size.max(1),
            max_push_constants: base.limits.max_push_constants_size,
            queue_count,
            descriptor_limits: DescriptorLimits {
                max_per_stage_samplers: v12.max_per_stage_descriptor_update_after_bind_samplers,
                max_per_stage_sampled_images: v12
                    .max_per_stage_descriptor_update_after_bind_sampled_images,
                max_per_stage_storage_images: v12
                    .max_per_stage_descriptor_update_after_bind_storage_images,
                max_per_stage_storage_buffers: v12
                    .max_per_stage_descriptor_update_after_bind_storage_buffers,
                max_per_stage_resources: v12.max_per_stage_update_after_bind_resources,
                max_sampler_allocation_count: base.limits.max_sampler_allocation_count,
            },
        };
        if present && !features.swapchain {
            return None;
        }
        Some(Candidate {
            physical,
            family,
            queue_count,
            info,
            features,
            required_stages: v13.required_subgroup_size_stages,
            bindless,
        })
    }
}

fn platform_surface_extensions(available: &[vk::ExtensionProperties]) -> Vec<&'static CStr> {
    [
        ash::khr::win32_surface::NAME,
        ash::khr::xlib_surface::NAME,
        ash::khr::xcb_surface::NAME,
        ash::khr::wayland_surface::NAME,
    ]
    .into_iter()
    .filter(|name| has_extension(available, name))
    .collect()
}

impl Device {
    /// Creates the instance, picks the GPU (a discrete one first, `options.adapter` if given), creates the
    /// device with every feature `gpu::DeviceInfo` reports that the GPU has, the allocator and the queues.
    pub fn new(options: &DeviceOptions) -> Result<Device> {
        // SAFETY: loading the system Vulkan loader; every handle created below is destroyed by `Inner::drop`
        // (or on the early-error paths here).
        unsafe {
            let entry = ash::Entry::load().map_err(|e| RenderError::Loader(e.to_string()))?;
            let validate = options.validation
                && entry
                    .enumerate_instance_layer_properties()?
                    .iter()
                    .any(|layer| layer.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
            let available = entry.enumerate_instance_extension_properties(None)?;
            let app = vk::ApplicationInfo::default()
                .application_name(c"leonida")
                .api_version(vk::API_VERSION_1_3);
            let layers = if validate {
                vec![VALIDATION_LAYER.as_ptr()]
            } else {
                Vec::new()
            };
            let mut extensions: Vec<*const i8> = Vec::new();
            let debug_utils = validate && has_extension(&available, ash::ext::debug_utils::NAME);
            if debug_utils {
                extensions.push(ash::ext::debug_utils::NAME.as_ptr());
            }
            let mut hdr_colorspace = false;
            let mut surface = false;
            if options.present {
                let platform = platform_surface_extensions(&available);
                if has_extension(&available, ash::khr::surface::NAME) && !platform.is_empty() {
                    surface = true;
                    extensions.push(ash::khr::surface::NAME.as_ptr());
                    extensions.extend(platform.iter().map(|n| n.as_ptr()));
                    if has_extension(&available, ash::ext::swapchain_colorspace::NAME) {
                        hdr_colorspace = true;
                        extensions.push(ash::ext::swapchain_colorspace::NAME.as_ptr());
                    }
                }
            }
            if options.present && !surface {
                return Err(RenderError::Unsupported("the Vulkan surface extensions"));
            }
            let create = vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_layer_names(&layers)
                .enabled_extension_names(&extensions);
            let instance = entry.create_instance(&create, None)?;
            let debug = if debug_utils {
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
                match utils.create_debug_utils_messenger(&info, None) {
                    Ok(messenger) => Some((utils, messenger)),
                    Err(e) => {
                        instance.destroy_instance(None);
                        return Err(e.into());
                    }
                }
            } else {
                None
            };
            let fail = |error: RenderError| {
                if let Some((utils, messenger)) = &debug {
                    utils.destroy_debug_utils_messenger(*messenger, None);
                }
                instance.destroy_instance(None);
                Err(error)
            };
            let wanted = options.adapter.as_ref().map(|n| n.to_lowercase());
            let physicals = match instance.enumerate_physical_devices() {
                Ok(list) => list,
                Err(e) => return fail(e.into()),
            };
            let mut candidates: Vec<Candidate> = physicals
                .into_iter()
                .filter_map(|p| describe(&instance, p, surface, hdr_colorspace))
                .filter(|c| {
                    wanted
                        .as_ref()
                        .is_none_or(|w| c.info.name.to_lowercase().contains(w))
                })
                .collect();
            candidates.sort_by_key(|c| !c.info.discrete);
            let Some(chosen) = candidates.into_iter().next() else {
                return fail(RenderError::NoDevice);
            };
            let queue_count = options.queues.clamp(1, chosen.queue_count);
            let priorities = vec![1.0; queue_count as usize];
            let queue_infos = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(chosen.family)
                .queue_priorities(&priorities)];
            let info = &chosen.info;
            let indexing = info.descriptor_indexing;
            let storage = info.storage_images;
            let bindless = chosen.bindless;
            let mut f13 = vk::PhysicalDeviceVulkan13Features::default()
                .synchronization2(true)
                .dynamic_rendering(true)
                .subgroup_size_control(info.subgroup_size_control)
                .compute_full_subgroups(info.full_subgroups);
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default()
                .timeline_semaphore(true)
                .buffer_device_address(true)
                .runtime_descriptor_array(indexing)
                .shader_sampled_image_array_non_uniform_indexing(indexing)
                .shader_storage_image_array_non_uniform_indexing(storage)
                .descriptor_binding_partially_bound(bindless)
                .descriptor_binding_sampled_image_update_after_bind(bindless)
                .descriptor_binding_storage_image_update_after_bind(bindless)
                .descriptor_binding_storage_buffer_update_after_bind(bindless)
                .descriptor_binding_update_unused_while_pending(bindless);
            let mut f11 = vk::PhysicalDeviceVulkan11Features::default()
                .shader_draw_parameters(info.pixel_shaders);
            let base = vk::PhysicalDeviceFeatures::default()
                .shader_int64(info.int64)
                .shader_float64(info.float64)
                .shader_storage_image_write_without_format(storage)
                .shader_resource_min_lod(info.min_lod)
                .shader_image_gather_extended(info.gather_extended)
                .fragment_stores_and_atomics(info.pixel_shaders)
                .vertex_pipeline_stores_and_atomics(info.pixel_shaders)
                .geometry_shader(info.geometry_shaders)
                .sampler_anisotropy(true)
                .texture_compression_bc(chosen.features.bc_textures);
            let mut atomics64 = vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default()
                .shader_image_int64_atomics(true);
            let mut barycentric = vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR::default()
                .fragment_shader_barycentric(true);
            let mut mesh = vk::PhysicalDeviceMeshShaderFeaturesEXT::default().mesh_shader(true);
            let mut interlock = vk::PhysicalDeviceFragmentShaderInterlockFeaturesEXT::default()
                .fragment_shader_pixel_interlock(true);
            let mut features = vk::PhysicalDeviceFeatures2::default()
                .features(base)
                .push_next(&mut f13)
                .push_next(&mut f12)
                .push_next(&mut f11);
            let mut device_extensions: Vec<*const i8> = Vec::new();
            if info.image_int64_atomics {
                features = features.push_next(&mut atomics64);
                device_extensions.push(ash::ext::shader_image_atomic_int64::NAME.as_ptr());
            }
            if info.pixel_shaders {
                features = features.push_next(&mut barycentric);
                device_extensions.push(ash::khr::fragment_shader_barycentric::NAME.as_ptr());
            }
            if info.mesh_shaders {
                features = features.push_next(&mut mesh);
                device_extensions.push(ash::ext::mesh_shader::NAME.as_ptr());
            }
            if info.pixel_interlock {
                features = features.push_next(&mut interlock);
                device_extensions.push(ash::ext::fragment_shader_interlock::NAME.as_ptr());
            }
            if chosen.features.swapchain {
                device_extensions.push(ash::khr::swapchain::NAME.as_ptr());
            }
            if chosen.features.host_import {
                device_extensions.push(ash::ext::external_memory_host::NAME.as_ptr());
            }
            let create = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_infos)
                .enabled_extension_names(&device_extensions)
                .push_next(&mut features);
            let device = match instance.create_device(chosen.physical, &create, None) {
                Ok(device) => device,
                Err(e) => return fail(e.into()),
            };
            let mut queues: Vec<QueueSlot> = Vec::new();
            let destroy_queues = |device: &ash::Device, queues: &[QueueSlot]| {
                for q in queues {
                    device.destroy_semaphore(q.timeline, None);
                }
            };
            for index in 0..queue_count {
                let mut kind = vk::SemaphoreTypeCreateInfo::default()
                    .semaphore_type(vk::SemaphoreType::TIMELINE)
                    .initial_value(0);
                let semaphore = match device.create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut kind),
                    None,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        destroy_queues(&device, &queues);
                        device.destroy_device(None);
                        return fail(e.into());
                    }
                };
                queues.push(QueueSlot {
                    handle: device.get_device_queue(chosen.family, index),
                    timeline: semaphore,
                    submitted: Mutex::new(0),
                });
            }
            let cache = match device.create_pipeline_cache(
                &vk::PipelineCacheCreateInfo::default().initial_data(&options.pipeline_cache),
                None,
            ) {
                Ok(cache) => cache,
                Err(_) => device
                    .create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None)
                    .unwrap_or_default(),
            };
            let allocator = Allocator::new(&AllocatorCreateDesc {
                instance: instance.clone(),
                device: device.clone(),
                physical_device: chosen.physical,
                debug_settings: Default::default(),
                buffer_device_address: true,
                allocation_sizes: Default::default(),
            });
            let allocator = match allocator {
                Ok(a) => a,
                Err(e) => {
                    destroy_queues(&device, &queues);
                    device.destroy_pipeline_cache(cache, None);
                    device.destroy_device(None);
                    return fail(e.into());
                }
            };
            let memory = instance.get_physical_device_memory_properties(chosen.physical);
            let limits = instance
                .get_physical_device_properties(chosen.physical)
                .limits;
            let mut info = chosen.info;
            if !chosen
                .required_stages
                .contains(vk::ShaderStageFlags::COMPUTE)
            {
                info.subgroup_size_control = false;
            }
            let swapchain_ext = chosen
                .features
                .swapchain
                .then(|| ash::khr::swapchain::Device::new(&instance, &device));
            let surface_ext = (surface && chosen.features.swapchain)
                .then(|| ash::khr::surface::Instance::new(&entry, &instance));
            let host_ext = chosen
                .features
                .host_import
                .then(|| ash::ext::external_memory_host::Device::new(&instance, &device));
            let mut features = chosen.features;
            features.queue_count = queue_count;
            Ok(Device {
                inner: Arc::new(Inner {
                    entry,
                    instance,
                    debug,
                    physical: chosen.physical,
                    device,
                    surface_ext,
                    swapchain_ext,
                    host_ext,
                    allocator: Mutex::new(Some(allocator)),
                    queues,
                    family: chosen.family,
                    memory,
                    limits,
                    info,
                    features,
                    pipeline_cache: cache,
                }),
            })
        }
    }

    /// What the GPU can do, in the shape `gpu` and the translator use.
    pub fn info(&self) -> &gpu::DeviceInfo {
        &self.inner.info
    }

    pub fn features(&self) -> &Features {
        &self.inner.features
    }

    pub fn limits(&self) -> &vk::PhysicalDeviceLimits {
        &self.inner.limits
    }

    /// The `ash` device, for calls `render` has no wrapper for.
    pub fn raw(&self) -> &ash::Device {
        &self.inner.device
    }

    pub fn instance(&self) -> &ash::Instance {
        &self.inner.instance
    }

    pub fn physical_device(&self) -> vk::PhysicalDevice {
        self.inner.physical
    }

    pub fn queue_family(&self) -> u32 {
        self.inner.family
    }

    pub fn pipeline_cache(&self) -> vk::PipelineCache {
        self.inner.pipeline_cache
    }

    /// The pipeline cache's contents, to save and pass back through `DeviceOptions::pipeline_cache`.
    pub fn pipeline_cache_data(&self) -> Result<Vec<u8>> {
        // SAFETY: the cache belongs to this device.
        Ok(unsafe {
            self.inner
                .device
                .get_pipeline_cache_data(self.inner.pipeline_cache)?
        })
    }

    pub fn queue_count(&self) -> usize {
        self.inner.queues.len()
    }

    /// Queue `index`; queue 0 is the main one. All queues are in one family, so resources need no ownership
    /// transfers between them.
    pub fn queue(&self, index: usize) -> Queue {
        Queue {
            device: self.clone(),
            index: index.min(self.inner.queues.len() - 1),
        }
    }

    pub fn wait_idle(&self) -> Result<()> {
        let _guards: Vec<_> = self
            .inner
            .queues
            .iter()
            .map(|q| lock(&q.submitted))
            .collect();
        // SAFETY: every queue is locked, so no submission races the wait.
        unsafe { self.inner.device.device_wait_idle()? };
        Ok(())
    }

    pub(crate) fn memory_type(
        &self,
        bits: u32,
        wanted: vk::MemoryPropertyFlags,
        avoid: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        let memory = &self.inner.memory;
        let find = |avoid: vk::MemoryPropertyFlags| {
            (0..memory.memory_type_count).find(|i| {
                let flags = memory.memory_types[*i as usize].property_flags;
                bits & (1 << i) != 0 && flags.contains(wanted) && !flags.intersects(avoid)
            })
        };
        find(avoid).or_else(|| find(vk::MemoryPropertyFlags::empty()))
    }

    pub(crate) fn allocator(&self) -> MutexGuard<'_, Option<Allocator>> {
        lock(&self.inner.allocator)
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A semaphore a submission waits on.
#[derive(Debug, Clone, Copy)]
pub struct WaitSemaphore {
    pub semaphore: vk::Semaphore,
    /// The timeline value to wait for; 0 for a binary semaphore.
    pub value: u64,
    pub stage: vk::PipelineStageFlags2,
}

/// A semaphore a submission signals.
#[derive(Debug, Clone, Copy)]
pub struct SignalSemaphore {
    pub semaphore: vk::Semaphore,
    pub value: u64,
    pub stage: vk::PipelineStageFlags2,
}

/// One submitted batch, named by its queue's timeline value.
#[derive(Clone)]
pub struct Submission {
    queue: Queue,
    value: u64,
}

impl Submission {
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn value(&self) -> u64 {
        self.value
    }

    pub fn is_done(&self) -> Result<bool> {
        Ok(self.queue.completed()? >= self.value)
    }

    pub fn wait(&self) -> Result<()> {
        self.queue.wait(self.value, u64::MAX)?;
        Ok(())
    }

    /// A wait another queue's submission can use to run after this one.
    pub fn as_wait(&self, stage: vk::PipelineStageFlags2) -> WaitSemaphore {
        WaitSemaphore {
            semaphore: self.queue.timeline(),
            value: self.value,
            stage,
        }
    }
}

/// One `VkQueue` and its timeline semaphore. Submissions and presents on a queue are serialised by a lock, so a
/// queue can be shared between threads.
#[derive(Clone)]
pub struct Queue {
    device: Device,
    index: usize,
}

impl Queue {
    fn slot(&self) -> &QueueSlot {
        &self.device.inner.queues[self.index]
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn timeline(&self) -> vk::Semaphore {
        self.slot().timeline
    }

    /// The last timeline value any submission on this queue will signal.
    pub fn submitted(&self) -> u64 {
        *lock(&self.slot().submitted)
    }

    /// The highest timeline value the GPU has reached.
    pub fn completed(&self) -> Result<u64> {
        // SAFETY: the semaphore belongs to this device.
        Ok(unsafe {
            self.device
                .inner
                .device
                .get_semaphore_counter_value(self.slot().timeline)?
        })
    }

    /// Waits for the timeline to reach `value`; returns false on timeout (nanoseconds).
    pub fn wait(&self, value: u64, timeout: u64) -> Result<bool> {
        let semaphores = [self.slot().timeline];
        let values = [value];
        let info = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        // SAFETY: the semaphore belongs to this device.
        match unsafe { self.device.inner.device.wait_semaphores(&info, timeout) } {
            Ok(()) => Ok(true),
            Err(vk::Result::TIMEOUT) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub fn wait_idle(&self) -> Result<()> {
        self.wait(self.submitted(), u64::MAX)?;
        Ok(())
    }

    /// Submits `command_buffers` as one batch that waits for `waits` and signals `signals` plus this queue's
    /// timeline.
    pub fn submit(
        &self,
        command_buffers: &[vk::CommandBuffer],
        waits: &[WaitSemaphore],
        signals: &[SignalSemaphore],
    ) -> Result<Submission> {
        let slot = self.slot();
        let mut submitted = lock(&slot.submitted);
        let value = *submitted + 1;
        let waits: Vec<_> = waits
            .iter()
            .map(|w| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(w.semaphore)
                    .value(w.value)
                    .stage_mask(w.stage)
            })
            .collect();
        let mut signal_infos: Vec<_> = signals
            .iter()
            .map(|s| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(s.semaphore)
                    .value(s.value)
                    .stage_mask(s.stage)
            })
            .collect();
        signal_infos.push(
            vk::SemaphoreSubmitInfo::default()
                .semaphore(slot.timeline)
                .value(value)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        );
        let buffers: Vec<_> = command_buffers
            .iter()
            .map(|&cb| vk::CommandBufferSubmitInfo::default().command_buffer(cb))
            .collect();
        let info = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&waits)
            .command_buffer_infos(&buffers)
            .signal_semaphore_infos(&signal_infos);
        // SAFETY: the queue lock is held, and the handles in the infos were created from this device.
        unsafe {
            self.device
                .inner
                .device
                .queue_submit2(slot.handle, &[info], vk::Fence::null())?;
        }
        *submitted = value;
        Ok(Submission {
            queue: self.clone(),
            value,
        })
    }

    /// Runs `f` on a transient command buffer and waits for it. For setup and tools, not for frames.
    pub fn immediate(&self, f: impl FnOnce(&mut crate::CommandBuffer) -> Result<()>) -> Result<()> {
        let mut stream = crate::CommandStream::new(self)?;
        let mut cb = stream.begin()?;
        f(&mut cb)?;
        stream.submit(cb, &[], &[])?.wait()?;
        stream.reclaim()
    }

    /// Presents on this queue, holding its lock.
    pub(crate) fn present(
        &self,
        info: &vk::PresentInfoKHR<'_>,
    ) -> std::result::Result<bool, vk::Result> {
        let _guard = lock(&self.slot().submitted);
        let ext = self
            .device
            .inner
            .swapchain_ext
            .as_ref()
            .expect("present needs a swapchain device");
        // SAFETY: the queue lock is held and the info refers to live swapchain objects.
        unsafe { ext.queue_present(self.slot().handle, info) }
    }
}
