//! The renderer: draws a source image into a target with a blit, or through the compose pass when the colour space
//! changes, and presents it.

use std::collections::HashMap;

use ash::vk;

use super::PresentError;
use super::convert::{COMPOSE_WGSL, Params, Rect, ScaleMode, SdrEotf, Space, fit_rect, place};
use super::vulkan::{
    Acquire, SourceImage, Target, TargetWant, VulkanContext, color_layers, color_range,
    find_memory, is_displayable, is_srgb,
};

/// What one frame draws: a source and where it goes. No source is a black frame.
pub struct Draw {
    pub source: SourceImage,
    /// Texels of the source to show; the whole image when `None`.
    pub src_rect: Option<Rect>,
    /// `pDestPlacementBase`: fractions of the output.
    pub placement: Option<[f32; 4]>,
    pub space: Space,
    pub point_filter: bool,
    pub scale: ScaleMode,
    pub params: Params,
    /// Whether the values read from the source are the linear form of an sRGB-encoded picture. `None` takes it from
    /// the source's format; a generated frame, held in half floats, says it explicitly.
    pub srgb_values: Option<bool>,
}

impl Draw {
    pub fn values_are_srgb(&self) -> bool {
        self.srgb_values
            .unwrap_or_else(|| is_srgb(self.source.format))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    Blank,
    Blit,
    Compose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub shown: bool,
    pub path: Path,
    pub dest: Option<Rect>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ComposeParams {
    src: [f32; 4],
    dst: [f32; 4],
    out_size: [f32; 2],
    src_space: u32,
    out_space: u32,
    point: u32,
    src_srgb: u32,
    gamma22: u32,
    pad: u32,
    sdr_white: f32,
    hdr_peak: f32,
    pad2: [f32; 2],
}

struct Compose {
    module: vk::ShaderModule,
    set_layout: vk::DescriptorSetLayout,
    pipe_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

struct Mid {
    image: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
    extent: (u32, u32),
}

struct Slot {
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    acquire: vk::Semaphore,
    pool: vk::DescriptorPool,
    uniform: vk::Buffer,
    uniform_memory: vk::DeviceMemory,
    mapped: usize,
    mid: Option<Mid>,
    views: Vec<vk::ImageView>,
}

/// Draws frames into targets. One renderer per device; frames in flight are two.
pub struct Renderer {
    ctx: VulkanContext,
    pool: vk::CommandPool,
    slots: Vec<Slot>,
    next: usize,
    compose: Option<Compose>,
    features: HashMap<vk::Format, vk::FormatFeatureFlags>,
    mem: vk::PhysicalDeviceMemoryProperties,
}

const SLOTS: usize = 2;

impl Renderer {
    pub fn new(ctx: &VulkanContext) -> Result<Renderer, PresentError> {
        let device = &ctx.device;
        // SAFETY: handles made here are owned by the renderer and destroyed in Drop.
        unsafe {
            let pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(ctx.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let mem = ctx
                .instance
                .get_physical_device_memory_properties(ctx.physical);
            let mut r = Renderer {
                ctx: ctx.clone(),
                pool,
                slots: Vec::new(),
                next: 0,
                compose: None,
                features: HashMap::new(),
                mem,
            };
            for _ in 0..SLOTS {
                let slot = r.make_slot()?;
                r.slots.push(slot);
            }
            Ok(r)
        }
    }

    unsafe fn make_slot(&self) -> Result<Slot, PresentError> {
        let device = &self.ctx.device;
        // SAFETY: plain object creation; the caller pushes the slot into `slots`, which Drop tears down.
        unsafe {
            let cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .command_buffer_count(1),
            )?[0];
            let fence = device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )?;
            let acquire = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
            let sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(1),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1),
            ];
            let pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )?;
            let size = std::mem::size_of::<ComposeParams>() as u64;
            let uniform = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                None,
            )?;
            let needs = device.get_buffer_memory_requirements(uniform);
            let kind = find_memory(
                &self.mem,
                needs.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or_else(|| {
                PresentError::Vulkan("no host memory for the compose parameters".into())
            })?;
            let uniform_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            )?;
            device.bind_buffer_memory(uniform, uniform_memory, 0)?;
            let mapped =
                device.map_memory(uniform_memory, 0, size, vk::MemoryMapFlags::empty())? as usize;
            Ok(Slot {
                cmd,
                fence,
                acquire,
                pool,
                uniform,
                uniform_memory,
                mapped,
                mid: None,
                views: Vec::new(),
            })
        }
    }

    fn features_of(&mut self, format: vk::Format) -> vk::FormatFeatureFlags {
        if let Some(f) = self.features.get(&format) {
            return *f;
        }
        // SAFETY: a plain query on the physical device of this context.
        let props = unsafe {
            self.ctx
                .instance
                .get_physical_device_format_properties(self.ctx.physical, format)
        };
        self.features.insert(format, props.optimal_tiling_features);
        props.optimal_tiling_features
    }

    fn compose_pipeline(&mut self) -> Result<(), PresentError> {
        if self.compose.is_some() {
            return Ok(());
        }
        let module = naga::front::wgsl::parse_str(COMPOSE_WGSL)
            .map_err(|e| PresentError::Shader(e.to_string()))?;
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .map_err(|e| PresentError::Shader(e.to_string()))?;
        let words = naga::back::spv::write_vec(
            &module,
            &info,
            &naga::back::spv::Options::default(),
            Some(&naga::back::spv::PipelineOptions {
                shader_stage: naga::ShaderStage::Compute,
                entry_point: "main".into(),
            }),
        )
        .map_err(|e| PresentError::Shader(e.to_string()))?;
        let device = &self.ctx.device;
        // SAFETY: handles made here are owned by `Compose` and destroyed in Drop.
        unsafe {
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)?;
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
            ];
            let set_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let layouts = [set_layout];
            let pipe_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                None,
            )?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            let pipeline = device
                .create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::ComputePipelineCreateInfo::default()
                        .stage(stage)
                        .layout(pipe_layout)],
                    None,
                )
                .map_err(|(_, e)| e)?[0];
            self.compose = Some(Compose {
                module,
                set_layout,
                pipe_layout,
                pipeline,
            });
        }
        Ok(())
    }

    /// Whether `draw` can be shown, and by which path.
    pub fn plan(
        &mut self,
        draw: &Draw,
        target_format: vk::Format,
        target_hdr: bool,
    ) -> Result<Path, PresentError> {
        let src = &draw.source;
        let unsupported = |why: &str| Err(PresentError::Unsupported(why.to_string()));
        if src.samples != vk::SampleCountFlags::TYPE_1 {
            return unsupported("a multisampled back buffer is not resolved");
        }
        if !is_displayable(src.format) {
            return unsupported("the back buffer format is not a colour format that can be shown");
        }
        if src.layout == vk::ImageLayout::UNDEFINED {
            return unsupported("the back buffer has no defined layout");
        }
        if !self
            .features_of(target_format)
            .contains(vk::FormatFeatureFlags::BLIT_DST)
        {
            return unsupported("the GPU cannot blit into the output format");
        }
        let out_space = if target_hdr { Space::Hdr10 } else { Space::Sdr };
        if draw.space != out_space {
            if !src.usage.contains(vk::ImageUsageFlags::SAMPLED) {
                return unsupported(
                    "the back buffer cannot be sampled for a colour space conversion",
                );
            }
            if !self
                .features_of(src.format)
                .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
            {
                return unsupported("the back buffer format cannot be sampled");
            }
            return Ok(Path::Compose);
        }
        if !src.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC) {
            return unsupported("the back buffer cannot be copied from");
        }
        if !self
            .features_of(src.format)
            .contains(vk::FormatFeatureFlags::BLIT_SRC)
        {
            return unsupported("the GPU cannot blit from the back buffer format");
        }
        Ok(Path::Blit)
    }

    /// Draws one frame into `target` and presents it. Returns without drawing when the target has no area.
    pub fn render(
        &mut self,
        target: &mut dyn Target,
        draw: Option<&Draw>,
        want: &TargetWant,
    ) -> Result<Outcome, PresentError> {
        let device = self.ctx.device.clone();
        let index = self.next;
        // SAFETY: the fence guards the slot's objects; it is waited before any of them is reused.
        unsafe {
            device.wait_for_fences(&[self.slots[index].fence], true, u64::MAX)?;
            let slot = &mut self.slots[index];
            for v in slot.views.drain(..) {
                device.destroy_image_view(v, None);
            }
        }
        target.configure(want)?;
        let mut tries = 0;
        let (image_index, target_image) = loop {
            match target.acquire(self.slots[index].acquire)? {
                Acquire::Image(i, image) => break (i, image),
                Acquire::Skip => {
                    return Ok(Outcome {
                        shown: false,
                        path: Path::Blank,
                        dest: None,
                    });
                }
                Acquire::Recreated => {
                    tries += 1;
                    if tries > 3 {
                        return Err(PresentError::Vulkan(
                            "the swap chain keeps going out of date".into(),
                        ));
                    }
                }
            }
        };
        let (tw, th) = target.extent();
        let path = match draw {
            None => Path::Blank,
            Some(d) => self.plan(d, target.format(), target.hdr10())?,
        };
        if path == Path::Compose {
            self.compose_pipeline()?;
        }
        let mut dest_out = None;
        // SAFETY: the slot's command buffer is idle (fence waited); every handle recorded stays alive until the fence.
        unsafe {
            let cmd = self.slots[index].cmd;
            device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            barrier(
                &device,
                cmd,
                target_image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                (vk::PipelineStageFlags::TRANSFER, vk::AccessFlags::empty()),
                (
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_WRITE,
                ),
            );
            match (path, draw) {
                (Path::Blit, Some(d)) => {
                    dest_out = self.record_blit(cmd, target_image, (tw, th), d);
                }
                (Path::Compose, Some(d)) => {
                    dest_out =
                        self.record_compose(cmd, index, target_image, (tw, th), d, target.hdr10())?;
                }
                _ => {
                    let black = vk::ClearColorValue::default();
                    device.cmd_clear_color_image(
                        cmd,
                        target_image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &black,
                        &[color_range()],
                    );
                }
            }
            barrier(
                &device,
                cmd,
                target_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                target.final_layout(),
                (
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_WRITE,
                ),
                (
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::AccessFlags::empty(),
                ),
            );
            device.end_command_buffer(cmd)?;
            device.reset_fences(&[self.slots[index].fence])?;
            let mut waits = Vec::new();
            let mut values = Vec::new();
            let mut stages = Vec::new();
            let acquire = self.slots[index].acquire;
            let swap = target.done_semaphore(image_index) != vk::Semaphore::null();
            if swap {
                waits.push(acquire);
                values.push(0);
                stages.push(vk::PipelineStageFlags::TRANSFER);
            }
            if let Some(d) = draw {
                for (s, v) in &d.source.waits {
                    waits.push(*s);
                    values.push(*v);
                    stages.push(vk::PipelineStageFlags::ALL_COMMANDS);
                }
            }
            let signal = [target.done_semaphore(image_index)];
            let signal_values = [0u64];
            let signals: &[vk::Semaphore] = if swap { &signal } else { &[] };
            let signal_values: &[u64] = if swap { &signal_values } else { &[] };
            let cmds = [cmd];
            let mut timeline = vk::TimelineSemaphoreSubmitInfo::default()
                .wait_semaphore_values(&values)
                .signal_semaphore_values(signal_values);
            let mut submit = vk::SubmitInfo::default()
                .wait_semaphores(&waits)
                .wait_dst_stage_mask(&stages)
                .command_buffers(&cmds)
                .signal_semaphores(signals);
            if values.iter().any(|v| *v != 0) {
                submit = submit.push_next(&mut timeline);
            }
            let queue = self.ctx.queue.lock().unwrap_or_else(|e| e.into_inner());
            device.queue_submit(*queue, &[submit], self.slots[index].fence)?;
            target.present(*queue, image_index)?;
        }
        self.next = (self.next + 1) % SLOTS;
        Ok(Outcome {
            shown: true,
            path,
            dest: dest_out,
        })
    }

    fn geometry(d: &Draw, target: (u32, u32)) -> ((i32, i32, i32, i32), Rect) {
        let (sw, sh) = d.source.extent;
        let whole = Rect {
            left: 0,
            top: 0,
            right: sw as i32,
            bottom: sh as i32,
        };
        let r = d.src_rect.unwrap_or(whole);
        let r = Rect {
            left: r.left.clamp(0, sw as i32),
            top: r.top.clamp(0, sh as i32),
            right: r.right.clamp(0, sw as i32),
            bottom: r.bottom.clamp(0, sh as i32),
        };
        let r = if r.is_empty() { whole } else { r };
        let canvas = match d.placement {
            None => fit_rect(
                r.width() as u32,
                r.height() as u32,
                target.0,
                target.1,
                d.scale,
            ),
            Some(_) => {
                let mode = if d.scale == ScaleMode::Integer {
                    ScaleMode::Fit
                } else {
                    d.scale
                };
                fit_rect(1920, 1080, target.0, target.1, mode)
            }
        };
        let dest = d.placement.and_then(|f| place(canvas, f)).unwrap_or(canvas);
        ((r.left, r.top, r.right, r.bottom), dest)
    }

    unsafe fn record_blit(
        &mut self,
        cmd: vk::CommandBuffer,
        target: vk::Image,
        size: (u32, u32),
        d: &Draw,
    ) -> Option<Rect> {
        let device = self.ctx.device.clone();
        let (src, dest) = Self::geometry(d, size);
        // SAFETY: recording into the slot's open command buffer on images that stay alive until its fence.
        unsafe {
            let black = vk::ClearColorValue::default();
            device.cmd_clear_color_image(
                cmd,
                target,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &black,
                &[color_range()],
            );
            if dest.is_empty() {
                return None;
            }
            barrier(
                &device,
                cmd,
                d.source.image,
                d.source.layout,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                (
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::AccessFlags::MEMORY_WRITE,
                ),
                (
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_READ,
                ),
            );
            let linear = !d.point_filter
                && self
                    .features_of(d.source.format)
                    .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR);
            let region = vk::ImageBlit::default()
                .src_subresource(color_layers())
                .src_offsets([
                    vk::Offset3D {
                        x: src.0,
                        y: src.1,
                        z: 0,
                    },
                    vk::Offset3D {
                        x: src.2,
                        y: src.3,
                        z: 1,
                    },
                ])
                .dst_subresource(color_layers())
                .dst_offsets([
                    vk::Offset3D {
                        x: dest.left,
                        y: dest.top,
                        z: 0,
                    },
                    vk::Offset3D {
                        x: dest.right,
                        y: dest.bottom,
                        z: 1,
                    },
                ]);
            device.cmd_blit_image(
                cmd,
                d.source.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                target,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
                if linear {
                    vk::Filter::LINEAR
                } else {
                    vk::Filter::NEAREST
                },
            );
            barrier(
                &device,
                cmd,
                d.source.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                d.source.layout,
                (
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_READ,
                ),
                (
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                ),
            );
        }
        Some(dest)
    }

    unsafe fn record_compose(
        &mut self,
        cmd: vk::CommandBuffer,
        slot_index: usize,
        target: vk::Image,
        size: (u32, u32),
        d: &Draw,
        out_hdr: bool,
    ) -> Result<Option<Rect>, PresentError> {
        let device = self.ctx.device.clone();
        let (src, dest) = Self::geometry(d, size);
        let Some(compose) = self.compose.as_ref() else {
            return Err(PresentError::Shader(
                "the compose pipeline is not built".into(),
            ));
        };
        let (pipeline, pipe_layout, set_layout) =
            (compose.pipeline, compose.pipe_layout, compose.set_layout);
        // SAFETY: recording into the slot's open command buffer; the slot fence guards everything made here.
        unsafe {
            let slot = &mut self.slots[slot_index];
            if slot.mid.as_ref().is_none_or(|m| m.extent != size) {
                if let Some(m) = slot.mid.take() {
                    device.destroy_image_view(m.view, None);
                    device.destroy_image(m.image, None);
                    device.free_memory(m.memory, None);
                }
                let image = device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R16G16B16A16_SFLOAT)
                        .extent(vk::Extent3D {
                            width: size.0,
                            height: size.1,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )?;
                let needs = device.get_image_memory_requirements(image);
                let kind = find_memory(
                    &self.mem,
                    needs.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
                .or_else(|| {
                    find_memory(
                        &self.mem,
                        needs.memory_type_bits,
                        vk::MemoryPropertyFlags::empty(),
                    )
                })
                .ok_or_else(|| PresentError::Vulkan("no memory for the compose image".into()))?;
                let memory = device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind),
                    None,
                )?;
                device.bind_image_memory(image, memory, 0)?;
                let view = device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(vk::Format::R16G16B16A16_SFLOAT)
                        .subresource_range(color_range()),
                    None,
                )?;
                slot.mid = Some(Mid {
                    image,
                    view,
                    memory,
                    extent: size,
                });
            }
            let (mid_image, mid_view) = {
                let m = slot
                    .mid
                    .as_ref()
                    .ok_or_else(|| PresentError::Vulkan("no compose image".into()))?;
                (m.image, m.view)
            };
            let src_view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(d.source.image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(d.source.format)
                    .subresource_range(color_range()),
                None,
            )?;
            slot.views.push(src_view);
            let params = ComposeParams {
                src: [src.0 as f32, src.1 as f32, src.2 as f32, src.3 as f32],
                dst: [
                    dest.left as f32,
                    dest.top as f32,
                    dest.right as f32,
                    dest.bottom as f32,
                ],
                out_size: [size.0 as f32, size.1 as f32],
                src_space: u32::from(d.space == Space::Hdr10),
                out_space: u32::from(out_hdr),
                point: u32::from(d.point_filter),
                src_srgb: u32::from(d.values_are_srgb()),
                gamma22: u32::from(d.params.sdr_eotf == SdrEotf::Gamma22),
                pad: 0,
                sdr_white: d.params.sdr_white_nits,
                hdr_peak: d.params.hdr_peak_nits,
                pad2: [0.0; 2],
            };
            std::ptr::copy_nonoverlapping(
                (&params as *const ComposeParams).cast::<u8>(),
                slot.mapped as *mut u8,
                std::mem::size_of::<ComposeParams>(),
            );
            device.reset_descriptor_pool(slot.pool, vk::DescriptorPoolResetFlags::empty())?;
            let layouts = [set_layout];
            let set = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(slot.pool)
                    .set_layouts(&layouts),
            )?[0];
            let src_info = [vk::DescriptorImageInfo::default()
                .image_view(src_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let out_info = [vk::DescriptorImageInfo::default()
                .image_view(mid_view)
                .image_layout(vk::ImageLayout::GENERAL)];
            let buf_info = [vk::DescriptorBufferInfo::default()
                .buffer(slot.uniform)
                .range(std::mem::size_of::<ComposeParams>() as u64)];
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&src_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                        .image_info(&out_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(&buf_info),
                ],
                &[],
            );
            barrier(
                &device,
                cmd,
                d.source.image,
                d.source.layout,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                (
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::AccessFlags::MEMORY_WRITE,
                ),
                (
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::AccessFlags::SHADER_READ,
                ),
            );
            barrier(
                &device,
                cmd,
                mid_image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
                (
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::AccessFlags::empty(),
                ),
                (
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::AccessFlags::SHADER_WRITE,
                ),
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipe_layout,
                0,
                &[set],
                &[],
            );
            device.cmd_dispatch(cmd, size.0.div_ceil(8), size.1.div_ceil(8), 1);
            barrier(
                &device,
                cmd,
                mid_image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                (
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::AccessFlags::SHADER_WRITE,
                ),
                (
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_READ,
                ),
            );
            barrier(
                &device,
                cmd,
                d.source.image,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                d.source.layout,
                (
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::AccessFlags::SHADER_READ,
                ),
                (
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                ),
            );
            let region = vk::ImageBlit::default()
                .src_subresource(color_layers())
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: size.0 as i32,
                        y: size.1 as i32,
                        z: 1,
                    },
                ])
                .dst_subresource(color_layers())
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: size.0 as i32,
                        y: size.1 as i32,
                        z: 1,
                    },
                ]);
            device.cmd_blit_image(
                cmd,
                mid_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                target,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
                vk::Filter::NEAREST,
            );
        }
        Ok((!dest.is_empty()).then_some(dest))
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn barrier(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
    src: (vk::PipelineStageFlags, vk::AccessFlags),
    dst: (vk::PipelineStageFlags, vk::AccessFlags),
) {
    let b = vk::ImageMemoryBarrier::default()
        .src_access_mask(src.1)
        .dst_access_mask(dst.1)
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range());
    // SAFETY: recording into an open command buffer; the caller owns the image.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src.0,
            dst.0,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[b],
        );
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let device = &self.ctx.device;
        // SAFETY: the device is idle before anything the slots own is destroyed.
        unsafe {
            let _ = device.device_wait_idle();
            for slot in self.slots.drain(..) {
                for v in slot.views {
                    device.destroy_image_view(v, None);
                }
                if let Some(m) = slot.mid {
                    device.destroy_image_view(m.view, None);
                    device.destroy_image(m.image, None);
                    device.free_memory(m.memory, None);
                }
                device.destroy_buffer(slot.uniform, None);
                device.free_memory(slot.uniform_memory, None);
                device.destroy_descriptor_pool(slot.pool, None);
                device.destroy_semaphore(slot.acquire, None);
                device.destroy_fence(slot.fence, None);
            }
            if let Some(c) = self.compose.take() {
                device.destroy_pipeline(c.pipeline, None);
                device.destroy_pipeline_layout(c.pipe_layout, None);
                device.destroy_descriptor_set_layout(c.set_layout, None);
                device.destroy_shader_module(c.module, None);
            }
            device.destroy_command_pool(self.pool, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_parameters_match_the_shader_block() {
        assert_eq!(std::mem::size_of::<ComposeParams>(), 80);
    }
}
