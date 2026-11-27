//! The image descriptor set translated shaders sample through (set 1).
//!
//! Each image descriptor (T#) and sampler descriptor (S#) the game can hand a shader is registered under its own
//! words and gets a slot (`descriptor::registry`). This set holds the Vulkan side: for slot `i`, a sampled image
//! view in the array of its view and number type, the same view as a storage image when the texture can be written,
//! the sampler (and a comparison twin) at index `i`, and the two lookup tables as storage buffers. Replay runs one
//! list at a time and waits for the GPU, so the set is rewritten between submissions and needs no update-after-bind.

use std::sync::Mutex;

use ash::vk;
use shader_translate::images::{self, View};

use super::gpu::Gpu;
use super::views::ViewCache;
use crate::com;
use crate::descriptor::Descriptors;
use crate::descriptor::gfx10::ImageType;
use crate::descriptor::vulkan::{SamplerParams, sampler_params};
use crate::pipeline::bindless_bindings;
use crate::resource::{Resources, desc};

struct TableBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    ptr: usize,
    capacity: u64,
}

#[derive(Default)]
struct State {
    pending_textures: Vec<u32>,
    pending_samplers: Vec<u32>,
    tables_version: Option<u64>,
    textures: Option<TableBuffer>,
    samplers: Option<TableBuffer>,
    made_samplers: Vec<vk::Sampler>,
}

pub struct Bindless {
    device: ash::Device,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    pub layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    /// Entries in each image and sampler array: slot indices below this have somewhere to go.
    pub count: u32,
    max_bias: f32,
    reduction: bool,
    anisotropy: bool,
    state: Mutex<State>,
}

fn view_of(ty: ImageType) -> Option<View> {
    Some(match ty {
        ImageType::D2 | ImageType::D2Msaa => View::D2,
        ImageType::D2Array | ImageType::Cube | ImageType::D2MsaaArray => View::D2Array,
        ImageType::D3 => View::D3,
        ImageType::D1 | ImageType::D1Array => return None,
    })
}

impl Bindless {
    pub fn new(gpu: &Gpu) -> Result<Bindless, vk::Result> {
        let device = gpu.ctx.device.clone();
        let count = gpu.image_array.min(gpu.sampler_array).max(2);
        let bindings = bindless_bindings(count);
        let flags: Vec<vk::DescriptorBindingFlags> = bindings
            .iter()
            .map(|b| {
                if b.descriptor_count > 1 {
                    vk::DescriptorBindingFlags::PARTIALLY_BOUND
                } else {
                    vk::DescriptorBindingFlags::empty()
                }
            })
            .collect();
        let mut flag_info =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
        // SAFETY: plain create calls on a live device; everything is destroyed in Drop.
        unsafe {
            let layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(&bindings)
                    .push_next(&mut flag_info),
                None,
            )?;
            let sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(9 * count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(15 * count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(2 * count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(2),
            ];
            let pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?;
            let layouts = [layout];
            let set = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )?[0];
            Ok(Bindless {
                device,
                instance: gpu.ctx.instance.clone(),
                physical: gpu.ctx.physical,
                layout,
                pool,
                set,
                count,
                max_bias: gpu.ctx.properties.limits.max_sampler_lod_bias,
                reduction: gpu.sampler_min_max,
                anisotropy: gpu.ctx.properties.limits.max_sampler_anisotropy > 1.0,
                state: Mutex::new(State::default()),
            })
        }
    }

    /// A descriptor was registered under `index`; its view is made on the next `update`.
    pub fn note_texture(&self, index: u32) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending_textures
            .push(index);
    }

    pub fn note_sampler(&self, index: u32) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending_samplers
            .push(index);
    }

    fn table_buffer(
        &self,
        slot: &mut Option<TableBuffer>,
        words: &[u32],
    ) -> Result<vk::Buffer, vk::Result> {
        let bytes = (words.len() * 4) as u64;
        if slot.as_ref().is_none_or(|t| t.capacity < bytes) {
            self.release_table(slot);
            let capacity = bytes.next_power_of_two().max(4096);
            // SAFETY: fresh objects; the mapping lives until `release_table`.
            unsafe {
                let buffer = self.device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(capacity)
                        .usage(vk::BufferUsageFlags::STORAGE_BUFFER),
                    None,
                )?;
                let needs = self.device.get_buffer_memory_requirements(buffer);
                let types = self
                    .instance
                    .get_physical_device_memory_properties(self.physical);
                let want =
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
                let Some(kind) = (0..types.memory_type_count).find(|i| {
                    needs.memory_type_bits & (1 << i) != 0
                        && types.memory_types[*i as usize]
                            .property_flags
                            .contains(want)
                }) else {
                    self.device.destroy_buffer(buffer, None);
                    return Err(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY);
                };
                let memory = self.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind),
                    None,
                )?;
                self.device.bind_buffer_memory(buffer, memory, 0)?;
                let ptr =
                    self.device
                        .map_memory(memory, 0, needs.size, vk::MemoryMapFlags::empty())?
                        as usize;
                *slot = Some(TableBuffer {
                    buffer,
                    memory,
                    ptr,
                    capacity,
                });
            }
        }
        let table = slot.as_ref().expect("made above");
        // SAFETY: the mapping holds `capacity >= bytes` bytes of host-coherent memory no submitted work reads now.
        unsafe {
            std::ptr::copy_nonoverlapping(
                words.as_ptr() as *const u8,
                table.ptr as *mut u8,
                bytes as usize,
            )
        };
        Ok(table.buffer)
    }

    fn release_table(&self, slot: &mut Option<TableBuffer>) {
        if let Some(t) = slot.take() {
            // SAFETY: no submitted work uses the table: replay waits for the GPU before the next update.
            unsafe {
                self.device.unmap_memory(t.memory);
                self.device.destroy_buffer(t.buffer, None);
                self.device.free_memory(t.memory, None);
            }
        }
    }

    fn make_sampler(
        &self,
        params: &SamplerParams,
        compare: bool,
    ) -> Result<vk::Sampler, vk::Result> {
        let mut info = params.create_info();
        if !self.anisotropy {
            info = info.anisotropy_enable(false);
        }
        if compare && !params.unnormalized {
            info = info
                .compare_enable(true)
                .compare_op(params.compare.unwrap_or(vk::CompareOp::LESS_OR_EQUAL));
        }
        let mut reduction =
            vk::SamplerReductionModeCreateInfo::default().reduction_mode(params.reduction);
        if self.reduction && params.reduction != vk::SamplerReductionMode::WEIGHTED_AVERAGE {
            info = info.push_next(&mut reduction);
        }
        // SAFETY: a plain create call on a live device; destroyed in Drop.
        unsafe { self.device.create_sampler(&info, None) }
    }

    /// Brings the set up to date with what has been registered since the last call.
    pub fn update(
        &self,
        descriptors: &mut Descriptors,
        resources: &Resources,
        views: &ViewCache,
    ) -> Result<(), vk::Result> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut infos: Vec<(u32, u32, vk::DescriptorType, vk::DescriptorImageInfo)> = Vec::new();
        let pending = std::mem::take(&mut state.pending_textures);
        let mut retry = Vec::new();
        for index in pending {
            if index >= self.count {
                com::note_once(format!(
                    "image descriptor slot {index} is beyond the {} entries of the bindless set",
                    self.count
                ));
                continue;
            }
            let Some(entry) = descriptors.registry.texture(index).copied() else {
                continue;
            };
            let Some(view_kind) = view_of(entry.image.ty) else {
                com::note_once("a 1D texture descriptor was registered; 1D views are not bound");
                continue;
            };
            let Some((resource, info)) = resources.view_for(&entry.image) else {
                retry.push(index);
                continue;
            };
            let Some(gpu_image) = resource.image.as_ref() else {
                retry.push(index);
                continue;
            };
            let color = gpu_image.aspect == vk::ImageAspectFlags::COLOR;
            let aspect = if color {
                vk::ImageAspectFlags::COLOR
            } else {
                vk::ImageAspectFlags::DEPTH
            };
            let view = match views.get(gpu_image.image, &info, aspect, info.components) {
                Ok(v) => v,
                Err(e) => {
                    com::note_once(format!("an image view could not be made: {e}"));
                    continue;
                }
            };
            let image_info = vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::GENERAL);
            infos.push((
                images::binding(view_kind, entry.number),
                index,
                vk::DescriptorType::SAMPLED_IMAGE,
                image_info,
            ));
            if color && resource.desc.has_flag(desc::ALLOW_UNORDERED_ACCESS) {
                infos.push((
                    images::storage_binding(view_kind, entry.number),
                    index,
                    vk::DescriptorType::STORAGE_IMAGE,
                    image_info,
                ));
            }
        }
        retry.truncate(1 << 16);
        state.pending_textures = retry;
        let pending = std::mem::take(&mut state.pending_samplers);
        for index in pending {
            if index >= self.count {
                continue;
            }
            let Some(entry) = descriptors.registry.sampler(index).copied() else {
                continue;
            };
            let params = sampler_params(&entry.sampler, self.max_bias);
            for compare in [false, true] {
                match self.make_sampler(&params, compare) {
                    Ok(sampler) => {
                        state.made_samplers.push(sampler);
                        infos.push((
                            if compare {
                                images::COMPARE_SAMPLERS
                            } else {
                                images::SAMPLERS
                            },
                            index,
                            vk::DescriptorType::SAMPLER,
                            vk::DescriptorImageInfo::default().sampler(sampler),
                        ));
                    }
                    Err(e) => com::note_once(format!("a sampler could not be made: {e}")),
                }
            }
        }
        let version = descriptors.registry.version();
        let mut buffers = Vec::new();
        if state.tables_version != Some(version) {
            let tables = match descriptors.tables() {
                Ok(t) => t,
                Err(e) => {
                    com::note_once(format!("the descriptor lookup tables failed: {e}"));
                    return Ok(());
                }
            };
            let (textures, samplers) = (tables.textures.clone(), tables.samplers.clone());
            let mut slot = state.textures.take();
            let first = self.table_buffer(&mut slot, &textures);
            state.textures = slot;
            let mut slot = state.samplers.take();
            let second = self.table_buffer(&mut slot, &samplers);
            state.samplers = slot;
            buffers.push((images::TEXTURE_TABLE, first?, (textures.len() * 4) as u64));
            buffers.push((images::SAMPLER_TABLE, second?, (samplers.len() * 4) as u64));
            state.tables_version = Some(version);
        }
        if infos.is_empty() && buffers.is_empty() {
            return Ok(());
        }
        let buffer_infos: Vec<[vk::DescriptorBufferInfo; 1]> = buffers
            .iter()
            .map(|(_, buffer, bytes)| {
                [vk::DescriptorBufferInfo::default()
                    .buffer(*buffer)
                    .range((*bytes).max(4))]
            })
            .collect();
        let image_infos: Vec<[vk::DescriptorImageInfo; 1]> =
            infos.iter().map(|(_, _, _, i)| [*i]).collect();
        let mut writes = Vec::with_capacity(infos.len() + buffers.len());
        for (i, (binding, _, _)) in buffers.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(*binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&buffer_infos[i]),
            );
        }
        for (i, (binding, element, ty, _)) in infos.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(*binding)
                    .dst_array_element(*element)
                    .descriptor_type(*ty)
                    .image_info(&image_infos[i]),
            );
        }
        // SAFETY: the set is not in use by any submitted work; the infos outlive the call.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        Ok(())
    }
}

impl Drop for Bindless {
    fn drop(&mut self) {
        let mut state = std::mem::take(self.state.get_mut().unwrap_or_else(|e| e.into_inner()));
        let (mut textures, mut samplers) = (state.textures.take(), state.samplers.take());
        self.release_table(&mut textures);
        self.release_table(&mut samplers);
        // SAFETY: the owner drops this once the GPU is idle.
        unsafe {
            for sampler in state.made_samplers.drain(..) {
                self.device.destroy_sampler(sampler, None);
            }
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}
