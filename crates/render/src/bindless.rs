//! The bindless descriptor set the translated shaders read (`shader_translate::images`), set `images::SET`.
//!
//! Bindings, as the translator declares them: 0 and 1 are the texture and sampler lookup tables (storage
//! buffers), 2 the sampler array, 3..=11 the sampled images (2D, 2D array, 3D times float, uint, sint), 12..=20
//! the same images as storage images, 21 the samplers again with depth comparison on, and 22..=27 single-channel
//! 32- and 64-bit unsigned storage images for atomics. Index 0 of every array is a fallback (zeros, a nearest
//! clamping sampler) because a lookup miss returns slot 0. Every descriptor is written with `GENERAL` layout, so
//! images in the set must be in `GENERAL` when shaders read them.

use std::collections::HashMap;

use ash::vk;
use shader_translate::images::{self, NUMBERS, Number, VIEWS, View};

use crate::buffer::{Buffer, BufferDesc, MemoryLocation};
use crate::command::CommandBuffer;
use crate::device::{Device, Queue};
use crate::error::{RenderError, Result};
use crate::format::Format;
use crate::image::{Image, ImageDesc, ImageKind, ImageView, ViewDesc};
use crate::pipeline::{PipelineLayout, SetLayout};
use crate::sampler::{Sampler, SamplerDesc};

const BINDINGS: u32 = 28;
const VIEW_NAMES: [&str; 3] = ["2D image", "2D array image", "3D image"];

/// How many indices each array holds. Index 0 is the fallback, so the usable count is one less.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindlessSizes {
    /// Indices per view type: 2D, 2D array, 3D. The storage arrays are as long as the sampled ones.
    pub images: [u32; 3],
    pub samplers: u32,
}

impl Default for BindlessSizes {
    fn default() -> Self {
        BindlessSizes {
            images: [4096, 1024, 256],
            samplers: 1024,
        }
    }
}

impl BindlessSizes {
    /// The defaults, halved until the set fits the device's update-after-bind limits.
    pub fn for_device(device: &Device) -> BindlessSizes {
        BindlessSizes::default().fitted(device)
    }

    pub fn fitted(mut self, device: &Device) -> BindlessSizes {
        let limits = &device.features().descriptor_limits;
        let fits = |s: &BindlessSizes| {
            let sum: u64 = s.images.iter().map(|&n| u64::from(n)).sum();
            let samplers = 2 * u64::from(s.samplers);
            let sampled = 3 * sum;
            let storage = 5 * sum;
            sampled <= u64::from(limits.max_per_stage_sampled_images)
                && storage <= u64::from(limits.max_per_stage_storage_images)
                && samplers <= u64::from(limits.max_per_stage_samplers)
                && sampled + storage + samplers + 2 <= u64::from(limits.max_per_stage_resources)
                && samplers + 64 <= u64::from(limits.max_sampler_allocation_count)
        };
        while !fits(&self) && (self.samplers > 16 || self.images.iter().any(|&n| n > 16)) {
            self.samplers = (self.samplers / 2).max(16);
            self.images = self.images.map(|n| (n / 2).max(16));
        }
        self
    }
}

struct Slots {
    next: u32,
    free: Vec<u32>,
    capacity: u32,
}

impl Slots {
    fn new(capacity: u32) -> Slots {
        Slots {
            next: 1,
            free: Vec::new(),
            capacity,
        }
    }

    fn alloc(&mut self) -> Option<u32> {
        if let Some(index) = self.free.pop() {
            return Some(index);
        }
        (self.next < self.capacity).then(|| {
            self.next += 1;
            self.next - 1
        })
    }

    fn release(&mut self, index: u32) {
        self.free.push(index);
    }
}

/// What part of a texture a table entry shows, as its T# describes it.
#[derive(Debug, Clone, Copy)]
pub struct BindlessView {
    pub view: View,
    /// The T#'s format, when it differs from the image's (the image needs `mutable_format`). Its number type
    /// chooses which array the texture goes in.
    pub format: Option<Format>,
    pub base_mip: u32,
    pub mips: Option<u32>,
    pub base_layer: u32,
    pub layers: Option<u32>,
    pub swizzle: vk::ComponentMapping,
}

impl BindlessView {
    pub fn new(view: View) -> BindlessView {
        BindlessView {
            view,
            format: None,
            base_mip: 0,
            mips: None,
            base_layer: 0,
            layers: None,
            swizzle: vk::ComponentMapping::default(),
        }
    }

    /// The view type the shader's array of this kind needs.
    pub fn of_image(image: &Image) -> BindlessView {
        BindlessView::new(match (image.kind(), image.layers()) {
            (ImageKind::D3, _) => View::D3,
            (_, 1) => View::D2,
            _ => View::D2Array,
        })
    }
}

fn view_type(view: View) -> vk::ImageViewType {
    match view {
        View::D2 => vk::ImageViewType::TYPE_2D,
        View::D2Array => vk::ImageViewType::TYPE_2D_ARRAY,
        View::D3 => vk::ImageViewType::TYPE_3D,
    }
}

struct TextureEntry {
    view: View,
    number: Number,
    index: u32,
    _image: Image,
    sampled: Option<ImageView>,
    storage: Option<ImageView>,
}

struct SamplerEntry {
    index: u32,
    _plain: Sampler,
    _compare: Sampler,
}

struct Table {
    buffer: Buffer,
    words: Vec<u32>,
    entries: Vec<([u32; 8], u32)>,
    dirty: bool,
}

struct Fallbacks {
    _images: Vec<Image>,
    _views: Vec<ImageView>,
    _samplers: [Sampler; 2],
}

/// What a flush had to do to the lookup tables, for callers that track GPU work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlushReport {
    pub texture_table_rewritten: bool,
    pub sampler_table_rewritten: bool,
    /// The device was idled because a live slot changed or a table grew.
    pub waited_idle: bool,
}

/// The bindless set, its lookup tables, and the index allocators behind them.
///
/// Adding a texture or sampler writes its descriptors at once into an index no running work can be reading
/// (descriptors are update-after-bind and partially bound). The lookup tables change at [`BindlessSet::flush`]:
/// adding keys to empty slots needs no wait, while removing a key, growing a table, or moving a live entry idles the
/// device first. Dropping the set waits for the device to go idle.
pub struct BindlessSet {
    device: Device,
    sizes: BindlessSizes,
    layout: SetLayout,
    empty: SetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    image_slots: [Slots; 3],
    sampler_slots: Slots,
    textures: HashMap<[u32; 8], TextureEntry>,
    samplers: HashMap<[u32; 4], SamplerEntry>,
    texture_table: Table,
    sampler_table: Table,
    retired: Vec<(Vec<u64>, ImageView)>,
    fallbacks: Option<Fallbacks>,
}

impl BindlessSet {
    pub fn new(queue: &Queue, sizes: BindlessSizes) -> Result<BindlessSet> {
        let device = queue.device();
        if !device.features().bindless {
            return Err(RenderError::Unsupported(
                "update-after-bind, partially bound descriptor arrays",
            ));
        }
        let stages = vk::ShaderStageFlags::ALL;
        let mut bindings = Vec::new();
        for binding in 0..BINDINGS {
            let (kind, count) = match binding {
                images::TEXTURE_TABLE | images::SAMPLER_TABLE => {
                    (vk::DescriptorType::STORAGE_BUFFER, 1)
                }
                images::SAMPLERS | images::COMPARE_SAMPLERS => {
                    (vk::DescriptorType::SAMPLER, sizes.samplers)
                }
                3..=11 => (
                    vk::DescriptorType::SAMPLED_IMAGE,
                    sizes.images[(binding as usize - 3) / 3],
                ),
                12..=20 => (
                    vk::DescriptorType::STORAGE_IMAGE,
                    sizes.images[(binding as usize - 12) / 3],
                ),
                _ => (
                    vk::DescriptorType::STORAGE_IMAGE,
                    sizes.images[(binding as usize - 22) % 3],
                ),
            };
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(kind)
                    .descriptor_count(count)
                    .stage_flags(stages),
            );
        }
        let flags = vec![
            vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::PARTIALLY_BOUND
                | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING;
            BINDINGS as usize
        ];
        let layout = SetLayout::new(device, &bindings, Some(&flags))?;
        let empty = SetLayout::new(device, &[], None)?;
        let image_total: u32 = sizes.images.iter().sum();
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 2,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: 2 * sizes.samplers,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: 3 * image_total,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 5 * image_total,
            },
        ];
        let layouts = [layout.handle()];
        // SAFETY: the pool and set are destroyed in `Drop`; the set is allocated once from a one-set pool.
        let (pool, set) = unsafe {
            let pool = device.raw().create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )?;
            match device.raw().allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            ) {
                Ok(sets) => (pool, sets[0]),
                Err(e) => {
                    device.raw().destroy_descriptor_pool(pool, None);
                    return Err(e.into());
                }
            }
        };
        let make_table = |name: &str| -> Result<Table> {
            let words = images::table(&[])?;
            let buffer = Buffer::new(
                device,
                &BufferDesc {
                    name,
                    size: words.len() as u64 * 4,
                    usage: vk::BufferUsageFlags::STORAGE_BUFFER,
                    location: MemoryLocation::Upload,
                },
            )?;
            buffer.write(0, as_bytes(&words))?;
            Ok(Table {
                buffer,
                words,
                entries: Vec::new(),
                dirty: false,
            })
        };
        let mut this = BindlessSet {
            device: device.clone(),
            sizes,
            layout,
            empty,
            pool,
            set,
            image_slots: sizes.images.map(Slots::new),
            sampler_slots: Slots::new(sizes.samplers),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            texture_table: make_table("texture table")?,
            sampler_table: make_table("sampler table")?,
            retired: Vec::new(),
            fallbacks: None,
        };
        this.write_table_descriptor(images::TEXTURE_TABLE, &this.texture_table.buffer.clone());
        this.write_table_descriptor(images::SAMPLER_TABLE, &this.sampler_table.buffer.clone());
        this.fallbacks = Some(this.make_fallbacks(queue)?);
        Ok(this)
    }

    pub fn sizes(&self) -> BindlessSizes {
        self.sizes
    }

    /// The layout of the set, for building pipeline layouts yourself.
    pub fn set_layout(&self) -> &SetLayout {
        &self.layout
    }

    /// A pipeline layout with `set0` (or an empty set) at set 0, this set at `images::SET`, and `push_bytes` of
    /// push constants.
    pub fn pipeline_layout(
        &self,
        set0: Option<&SetLayout>,
        push_bytes: u32,
    ) -> Result<PipelineLayout> {
        let first = set0.unwrap_or(&self.empty);
        PipelineLayout::new(
            &self.device,
            &[first.clone(), self.layout.clone()],
            push_bytes,
        )
    }

    /// Binds the set at `images::SET`. The `BindlessSet` must outlive the work.
    pub fn bind(
        &self,
        cb: &mut CommandBuffer,
        bind_point: vk::PipelineBindPoint,
        layout: &PipelineLayout,
    ) {
        cb.bind_raw_descriptor_set(bind_point, layout, images::SET, self.set);
    }

    pub fn texture_table_buffer(&self) -> &Buffer {
        &self.texture_table.buffer
    }

    pub fn sampler_table_buffer(&self) -> &Buffer {
        &self.sampler_table.buffer
    }

    /// The CPU copy of the texture table as the shader reads it, once flushed.
    pub fn texture_table_words(&self) -> &[u32] {
        &self.texture_table.words
    }

    pub fn sampler_table_words(&self) -> &[u32] {
        &self.sampler_table.words
    }

    pub fn texture_index(&self, key: &[u32; 8]) -> Option<u32> {
        self.textures.get(key).map(|t| t.index)
    }

    pub fn sampler_index(&self, key: &[u32; 4]) -> Option<u32> {
        self.samplers.get(key).map(|s| s.index)
    }

    fn write_table_descriptor(&self, binding: u32, buffer: &Buffer) {
        let info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer.handle())
            .offset(0)
            .range(vk::WHOLE_SIZE)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(binding)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&info);
        // SAFETY: the set and buffer belong to this device; the buffer is held by the table.
        unsafe { self.device.raw().update_descriptor_sets(&[write], &[]) };
    }

    fn write_image(&self, binding: u32, index: u32, kind: vk::DescriptorType, view: vk::ImageView) {
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(binding)
            .dst_array_element(index)
            .descriptor_type(kind)
            .image_info(&info);
        // SAFETY: the set and view belong to this device; the caller keeps the view alive.
        unsafe { self.device.raw().update_descriptor_sets(&[write], &[]) };
    }

    fn write_sampler(&self, binding: u32, index: u32, sampler: &Sampler) {
        let info = [vk::DescriptorImageInfo::default().sampler(sampler.handle())];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(binding)
            .dst_array_element(index)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&info);
        // SAFETY: the set and sampler belong to this device; the caller keeps the sampler alive.
        unsafe { self.device.raw().update_descriptor_sets(&[write], &[]) };
    }

    fn make_fallbacks(&self, queue: &Queue) -> Result<Fallbacks> {
        let device = &self.device;
        let wide = device.info().image_int64_atomics;
        let mut images = Vec::new();
        let mut views = Vec::new();
        let make = |view: View,
                    format: Format,
                    usage: vk::ImageUsageFlags,
                    images: &mut Vec<Image>|
         -> Result<Image> {
            let kind = if view == View::D3 {
                ImageKind::D3
            } else {
                ImageKind::D2
            };
            let image = Image::new(
                device,
                &ImageDesc {
                    kind,
                    ..ImageDesc::new_2d("bindless fallback", format, 1, 1, usage)
                },
            )?;
            images.push(image.clone());
            Ok(image)
        };
        let both = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE;
        let mut plan = Vec::new();
        for view in VIEWS {
            for number in NUMBERS {
                let format = match number {
                    Number::Float => Format::Rgba8Unorm,
                    Number::Uint => Format::Rgba8Uint,
                    Number::Sint => Format::Rgba8Sint,
                };
                let image = make(view, format, both, &mut images)?;
                let v = image.create_view(&ViewDesc::new(view_type(view)))?;
                plan.push((
                    vec![
                        (
                            images::binding(view, number),
                            vk::DescriptorType::SAMPLED_IMAGE,
                        ),
                        (
                            images::storage_binding(view, number),
                            vk::DescriptorType::STORAGE_IMAGE,
                        ),
                    ],
                    v,
                ));
            }
            for (format, is_wide) in [(Format::R32Uint, false), (Format::R64Uint, true)] {
                if is_wide && !wide {
                    continue;
                }
                let image = make(view, format, vk::ImageUsageFlags::STORAGE, &mut images)?;
                let v = image.create_view(&ViewDesc::new(view_type(view)))?;
                plan.push((
                    vec![(
                        images::atomic_binding(view, is_wide),
                        vk::DescriptorType::STORAGE_IMAGE,
                    )],
                    v,
                ));
            }
        }
        queue.immediate(|cb| {
            for image in &images {
                cb.transition(image, vk::ImageLayout::UNDEFINED, vk::ImageLayout::GENERAL);
                cb.clear_color_image(
                    image,
                    vk::ImageLayout::GENERAL,
                    vk::ClearColorValue::default(),
                    &[image.full_range(vk::ImageAspectFlags::COLOR)],
                );
            }
            Ok(())
        })?;
        for (targets, view) in plan {
            for (binding, kind) in targets {
                self.write_image(binding, 0, kind, view.handle());
            }
            views.push(view);
        }
        let sampler = Sampler::new(device, &SamplerDesc::default())?;
        let compare = Sampler::new(
            device,
            &SamplerDesc {
                compare: Some(vk::CompareOp::LESS_OR_EQUAL),
                ..SamplerDesc::default()
            },
        )?;
        self.write_sampler(images::SAMPLERS, 0, &sampler);
        self.write_sampler(images::COMPARE_SAMPLERS, 0, &compare);
        Ok(Fallbacks {
            _images: images,
            _views: views,
            _samplers: [sampler, compare],
        })
    }

    fn retire(&mut self, view: ImageView) {
        let snapshot = (0..self.device.queue_count())
            .map(|i| self.device.queue(i).submitted())
            .collect();
        self.retired.push((snapshot, view));
    }

    fn collect(&mut self) -> Result<()> {
        let device = self.device.clone();
        let mut keep = Vec::new();
        for (snapshot, view) in self.retired.drain(..) {
            let done = snapshot
                .iter()
                .enumerate()
                .all(|(i, &v)| device.queue(i).completed().is_ok_and(|c| c >= v));
            if !done {
                keep.push((snapshot, view));
            }
        }
        self.retired = keep;
        Ok(())
    }

    /// Registers a texture under its T# bytes `key` and writes it into the arrays; returns its index. A key that
    /// is already registered keeps its index and has its descriptors rewritten. The new key reaches shaders at the
    /// next [`BindlessSet::flush`].
    pub fn add_texture(
        &mut self,
        key: [u32; 8],
        image: &Image,
        view: &BindlessView,
    ) -> Result<u32> {
        let format = view.format.unwrap_or(image.format());
        let number = format.number();
        let existing = self.textures.get(&key).map(|t| (t.index, t.view));
        let index = match existing {
            Some((index, old_view)) if old_view == view.view => index,
            Some(_) => {
                return Err(RenderError::Invalid(
                    "a registered descriptor cannot change its view type".into(),
                ));
            }
            None => self.image_slots[view.view as usize]
                .alloc()
                .ok_or(RenderError::BindlessFull(VIEW_NAMES[view.view as usize]))?,
        };
        let base = ViewDesc {
            view_type: view_type(view.view),
            format: view.format,
            aspect: None,
            base_mip: view.base_mip,
            mips: view.mips,
            base_layer: view.base_layer,
            layers: view.layers,
            swizzle: view.swizzle,
        };
        let created = (|| -> Result<(Option<ImageView>, Option<ImageView>)> {
            let sampled = if image.usage().contains(vk::ImageUsageFlags::SAMPLED) {
                Some(image.create_view(&base)?)
            } else {
                None
            };
            let storage_ok = image.usage().contains(vk::ImageUsageFlags::STORAGE)
                && !format.is_depth()
                && !format.is_compressed();
            let storage = if storage_ok {
                Some(image.create_view(&ViewDesc {
                    mips: Some(1),
                    swizzle: vk::ComponentMapping::default(),
                    ..base
                })?)
            } else {
                None
            };
            Ok((sampled, storage))
        })();
        let (sampled, storage) = match created {
            Ok(views) => views,
            Err(e) => {
                if existing.is_none() {
                    self.image_slots[view.view as usize].release(index);
                }
                return Err(e);
            }
        };
        if let Some(sampled) = &sampled {
            self.write_image(
                images::binding(view.view, number),
                index,
                vk::DescriptorType::SAMPLED_IMAGE,
                sampled.handle(),
            );
        }
        if let Some(storage) = &storage {
            self.write_image(
                images::storage_binding(view.view, number),
                index,
                vk::DescriptorType::STORAGE_IMAGE,
                storage.handle(),
            );
            let atomic = match format {
                Format::R32Uint => Some(false),
                Format::R64Uint if self.device.info().image_int64_atomics => Some(true),
                _ => None,
            };
            if let Some(wide) = atomic {
                self.write_image(
                    images::atomic_binding(view.view, wide),
                    index,
                    vk::DescriptorType::STORAGE_IMAGE,
                    storage.handle(),
                );
            }
        }
        let entry = TextureEntry {
            view: view.view,
            number,
            index,
            _image: image.clone(),
            sampled,
            storage,
        };
        let slot = images::slot(index, number);
        match self.textures.insert(key, entry) {
            Some(old) => {
                if let Some(sampled) = old.sampled {
                    self.retire(sampled);
                }
                if let Some(storage) = old.storage {
                    self.retire(storage);
                }
                if old.number != number {
                    let position = self.texture_table.entries.iter().position(|e| e.0 == key);
                    if let Some(position) = position {
                        self.texture_table.entries[position].1 = slot;
                        self.texture_table.dirty = true;
                    }
                }
            }
            None => {
                self.texture_table.entries.push((key, slot));
                self.texture_table.dirty = true;
            }
        }
        Ok(index)
    }

    /// Forgets a texture: its table entry goes at the next flush and its index is reusable at once.
    pub fn remove_texture(&mut self, key: &[u32; 8]) -> bool {
        let Some(entry) = self.textures.remove(key) else {
            return false;
        };
        self.image_slots[entry.view as usize].release(entry.index);
        self.texture_table.entries.retain(|e| &e.0 != key);
        self.texture_table.dirty = true;
        if let Some(sampled) = entry.sampled {
            self.retire(sampled);
        }
        if let Some(storage) = entry.storage {
            self.retire(storage);
        }
        true
    }

    /// Registers a sampler under its S# bytes and the ordinary and depth-comparison `VkSampler`s for it.
    pub fn add_sampler(
        &mut self,
        key: [u32; 4],
        plain: &Sampler,
        compare: &Sampler,
    ) -> Result<u32> {
        let index = match self.samplers.get(&key) {
            Some(entry) => entry.index,
            None => self
                .sampler_slots
                .alloc()
                .ok_or(RenderError::BindlessFull("sampler"))?,
        };
        self.write_sampler(images::SAMPLERS, index, plain);
        self.write_sampler(images::COMPARE_SAMPLERS, index, compare);
        let entry = SamplerEntry {
            index,
            _plain: plain.clone(),
            _compare: compare.clone(),
        };
        if self.samplers.insert(key, entry).is_none() {
            self.sampler_table.entries.push((
                images::sampler_key(&key),
                images::slot(index, Number::Float),
            ));
            self.sampler_table.dirty = true;
        }
        Ok(index)
    }

    pub fn remove_sampler(&mut self, key: &[u32; 4]) -> bool {
        let Some(entry) = self.samplers.remove(key) else {
            return false;
        };
        self.sampler_slots.release(entry.index);
        let table_key = images::sampler_key(key);
        self.sampler_table.entries.retain(|e| e.0 != table_key);
        self.sampler_table.dirty = true;
        true
    }

    /// Writes changed lookup tables to their buffers and frees retired views the GPU has finished with.
    pub fn flush(&mut self) -> Result<FlushReport> {
        self.collect()?;
        let mut report = FlushReport::default();
        for which in [images::TEXTURE_TABLE, images::SAMPLER_TABLE] {
            let table = if which == images::TEXTURE_TABLE {
                &self.texture_table
            } else {
                &self.sampler_table
            };
            if !table.dirty {
                continue;
            }
            let words = images::table(&table.entries)?;
            let (replace_buffer, idle) = plan_upload(table, &words);
            if idle {
                self.device.wait_idle()?;
                report.waited_idle = true;
            }
            let device = self.device.clone();
            let table = if which == images::TEXTURE_TABLE {
                &mut self.texture_table
            } else {
                &mut self.sampler_table
            };
            if replace_buffer {
                table.buffer = Buffer::new(
                    &device,
                    &BufferDesc {
                        name: "bindless table",
                        size: words.len() as u64 * 4,
                        usage: vk::BufferUsageFlags::STORAGE_BUFFER,
                        location: MemoryLocation::Upload,
                    },
                )?;
                table.buffer.write(0, as_bytes(&words))?;
            } else if table.words.len() != words.len() {
                table.buffer.write(0, as_bytes(&words))?;
            } else {
                let first = words.iter().zip(&table.words).position(|(a, b)| a != b);
                let last = words.iter().zip(&table.words).rposition(|(a, b)| a != b);
                if let (Some(first), Some(last)) = (first, last) {
                    table
                        .buffer
                        .write(first as u64 * 4, as_bytes(&words[first..=last]))?;
                }
            }
            table.words = words;
            table.dirty = false;
            let buffer = table.buffer.clone();
            if replace_buffer {
                self.write_table_descriptor(which, &buffer);
            }
            if which == images::TEXTURE_TABLE {
                report.texture_table_rewritten = true;
            } else {
                report.sampler_table_rewritten = true;
            }
        }
        Ok(report)
    }
}

/// Whether writing `new` over the table needs a bigger buffer, and whether the device must idle first because
/// a shader could be reading something the write changes.
fn plan_upload(table: &Table, new: &[u32]) -> (bool, bool) {
    let old = &table.words;
    let replace = new.len() as u64 * 4 > table.buffer.size();
    if old.len() != new.len() {
        return (replace, true);
    }
    let live_changed = (0..(old.len() - 1) / images::ENTRY_WORDS).any(|entry| {
        let slot = 1 + entry * images::ENTRY_WORDS;
        let words = slot..slot + images::ENTRY_WORDS;
        old[slot + 8] != images::EMPTY && old[words.clone()] != new[words]
    });
    (replace, live_changed)
}

fn as_bytes(words: &[u32]) -> &[u8] {
    // SAFETY: a `u32` slice is `len * 4` initialised bytes and `u8` has no alignment requirement.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), words.len() * 4) }
}

impl Drop for BindlessSet {
    fn drop(&mut self) {
        let _ = self.device.wait_idle();
        self.retired.clear();
        // SAFETY: the device is idle, so nothing reads the set; destroying the pool frees it.
        unsafe { self.device.raw().destroy_descriptor_pool(self.pool, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_start_at_one_and_reuse_released_indices() {
        let mut slots = Slots::new(4);
        assert_eq!(slots.alloc(), Some(1));
        assert_eq!(slots.alloc(), Some(2));
        assert_eq!(slots.alloc(), Some(3));
        assert_eq!(slots.alloc(), None);
        slots.release(2);
        assert_eq!(slots.alloc(), Some(2));
        assert_eq!(slots.alloc(), None);
    }
}
