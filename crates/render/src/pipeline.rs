use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ash::vk;

use crate::buffer::Buffer;
use crate::device::{Device, lock};
use crate::error::{RenderError, Result};

struct SetLayoutInner {
    device: Device,
    handle: vk::DescriptorSetLayout,
    bindings: Vec<(u32, vk::DescriptorType, u32)>,
    update_after_bind: bool,
}

impl Drop for SetLayoutInner {
    fn drop(&mut self) {
        // SAFETY: the layout belongs to this device; pipeline layouts built from it keep a clone alive.
        unsafe {
            self.device
                .raw()
                .destroy_descriptor_set_layout(self.handle, None);
        }
    }
}

/// A descriptor set layout. Clones share it.
#[derive(Clone)]
pub struct SetLayout {
    inner: Arc<SetLayoutInner>,
}

impl SetLayout {
    /// Makes a layout. `flags`, when given, holds one binding-flags entry per binding and turns on
    /// update-after-bind pools for the layout when any entry has `UPDATE_AFTER_BIND`.
    pub fn new(
        device: &Device,
        bindings: &[vk::DescriptorSetLayoutBinding<'_>],
        flags: Option<&[vk::DescriptorBindingFlags]>,
    ) -> Result<SetLayout> {
        let update_after_bind = flags.is_some_and(|f| {
            f.iter()
                .any(|f| f.contains(vk::DescriptorBindingFlags::UPDATE_AFTER_BIND))
        });
        let mut binding_flags = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
            .binding_flags(flags.unwrap_or(&[]));
        let mut info = vk::DescriptorSetLayoutCreateInfo::default().bindings(bindings);
        if update_after_bind {
            info = info.flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL);
        }
        if flags.is_some() {
            info = info.push_next(&mut binding_flags);
        }
        // SAFETY: plain creation on a live device.
        let handle = unsafe { device.raw().create_descriptor_set_layout(&info, None)? };
        Ok(SetLayout {
            inner: Arc::new(SetLayoutInner {
                device: device.clone(),
                handle,
                bindings: bindings
                    .iter()
                    .map(|b| (b.binding, b.descriptor_type, b.descriptor_count))
                    .collect(),
                update_after_bind,
            }),
        })
    }

    /// A layout of `count` storage buffers at bindings `0..count`, visible to every stage.
    pub fn storage_buffers(device: &Device, count: u32) -> Result<SetLayout> {
        let bindings: Vec<_> = (0..count)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::ALL)
            })
            .collect();
        SetLayout::new(device, &bindings, None)
    }

    pub fn handle(&self) -> vk::DescriptorSetLayout {
        self.inner.handle
    }

    pub fn device(&self) -> &Device {
        &self.inner.device
    }
}

struct PipelineLayoutInner {
    device: Device,
    handle: vk::PipelineLayout,
    _sets: Vec<SetLayout>,
    push_bytes: u32,
}

impl Drop for PipelineLayoutInner {
    fn drop(&mut self) {
        // SAFETY: the layout belongs to this device; pipelines made with it keep a clone alive.
        unsafe { self.device.raw().destroy_pipeline_layout(self.handle, None) };
    }
}

/// A pipeline layout. Clones share it.
#[derive(Clone)]
pub struct PipelineLayout {
    inner: Arc<PipelineLayoutInner>,
}

impl PipelineLayout {
    /// `sets` in set-number order, and `push_bytes` of push constants visible to every stage.
    pub fn new(device: &Device, sets: &[SetLayout], push_bytes: u32) -> Result<PipelineLayout> {
        let handles: Vec<_> = sets.iter().map(SetLayout::handle).collect();
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::ALL)
            .offset(0)
            .size(push_bytes)];
        let mut info = vk::PipelineLayoutCreateInfo::default().set_layouts(&handles);
        if push_bytes > 0 {
            info = info.push_constant_ranges(&ranges);
        }
        // SAFETY: plain creation on a live device from live set layouts.
        let handle = unsafe { device.raw().create_pipeline_layout(&info, None)? };
        Ok(PipelineLayout {
            inner: Arc::new(PipelineLayoutInner {
                device: device.clone(),
                handle,
                _sets: sets.to_vec(),
                push_bytes,
            }),
        })
    }

    pub fn handle(&self) -> vk::PipelineLayout {
        self.inner.handle
    }

    pub fn push_bytes(&self) -> u32 {
        self.inner.push_bytes
    }
}

struct PipelineInner {
    device: Device,
    handle: vk::Pipeline,
    _layout: PipelineLayout,
}

impl Drop for PipelineInner {
    fn drop(&mut self) {
        // SAFETY: the pipeline belongs to this device; command buffers using it keep a clone alive.
        unsafe { self.device.raw().destroy_pipeline(self.handle, None) };
    }
}

/// A compute pipeline built through the device's pipeline cache. Clones share it.
#[derive(Clone)]
pub struct ComputePipeline {
    inner: Arc<PipelineInner>,
}

impl ComputePipeline {
    /// Builds a compute pipeline from SPIR-V whose entry point is `main`. `subgroup_size`, when the GPU
    /// supports it, forces that subgroup width.
    pub fn new(
        device: &Device,
        layout: &PipelineLayout,
        spirv: &[u32],
        subgroup_size: Option<u32>,
    ) -> Result<ComputePipeline> {
        let raw = device.raw();
        let force = subgroup_size.filter(|_| device.info().subgroup_size_control);
        // SAFETY: the module is destroyed after the pipeline is created, which copies what it needs.
        unsafe {
            let module =
                raw.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(spirv), None)?;
            let mut required = vk::PipelineShaderStageRequiredSubgroupSizeCreateInfo::default()
                .required_subgroup_size(force.unwrap_or(0));
            let mut stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            if force.is_some() {
                stage = stage.push_next(&mut required);
            }
            let info = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout.handle());
            let result = raw.create_compute_pipelines(device.pipeline_cache(), &[info], None);
            raw.destroy_shader_module(module, None);
            let handle = result.map_err(|(_, e)| RenderError::Vulkan(e))?[0];
            Ok(ComputePipeline {
                inner: Arc::new(PipelineInner {
                    device: device.clone(),
                    handle,
                    _layout: layout.clone(),
                }),
            })
        }
    }

    pub fn handle(&self) -> vk::Pipeline {
        self.inner.handle
    }
}

struct DescriptorSetInner {
    device: Device,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    _layout: SetLayout,
    buffers: Mutex<HashMap<u32, Buffer>>,
}

impl Drop for DescriptorSetInner {
    fn drop(&mut self) {
        // SAFETY: destroying the pool frees its set; command buffers using the set keep a clone alive.
        unsafe { self.device.raw().destroy_descriptor_pool(self.pool, None) };
    }
}

/// One descriptor set in a pool of its own, for the few sets (storage buffers, uniform data) outside the
/// bindless set. Clones share it.
#[derive(Clone)]
pub struct DescriptorSet {
    inner: Arc<DescriptorSetInner>,
}

impl DescriptorSet {
    pub fn new(layout: &SetLayout) -> Result<DescriptorSet> {
        let device = layout.device();
        let mut sizes: HashMap<vk::DescriptorType, u32> = HashMap::new();
        for (_, kind, count) in &layout.inner.bindings {
            *sizes.entry(*kind).or_default() += count;
        }
        let sizes: Vec<_> = sizes
            .into_iter()
            .map(|(ty, descriptor_count)| vk::DescriptorPoolSize {
                ty,
                descriptor_count,
            })
            .collect();
        let mut flags = vk::DescriptorPoolCreateFlags::empty();
        if layout.inner.update_after_bind {
            flags |= vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND;
        }
        let layouts = [layout.handle()];
        // SAFETY: the pool is destroyed with the set, and the set is allocated from it once.
        unsafe {
            let pool = device.raw().create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .flags(flags)
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?;
            let set = match device.raw().allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            ) {
                Ok(sets) => sets[0],
                Err(e) => {
                    device.raw().destroy_descriptor_pool(pool, None);
                    return Err(e.into());
                }
            };
            Ok(DescriptorSet {
                inner: Arc::new(DescriptorSetInner {
                    device: device.clone(),
                    pool,
                    set,
                    _layout: layout.clone(),
                    buffers: Mutex::new(HashMap::new()),
                }),
            })
        }
    }

    pub fn handle(&self) -> vk::DescriptorSet {
        self.inner.set
    }

    /// Points a storage-buffer binding at `buffer` (the whole of it).
    pub fn write_storage_buffer(&self, binding: u32, buffer: &Buffer) {
        let info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer.handle())
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.inner.set)
            .dst_binding(binding)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&info);
        // SAFETY: the set and buffer belong to this device; the set keeps the buffer alive below.
        unsafe {
            self.inner
                .device
                .raw()
                .update_descriptor_sets(&[write], &[])
        };
        lock(&self.inner.buffers).insert(binding, buffer.clone());
    }
}
