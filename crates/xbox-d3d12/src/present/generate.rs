//! Frame generation on the presenter's Vulkan device: the passes of `upscale::framegen` run on the images the title
//! presents, with the work kept on the GPU.
//!
//! For every real frame [`FrameGen::capture`] records one submit that
//!
//! 1. copies the colour, the depth and the motion vectors (three images the title owns, read through sampled views)
//!    into float buffers (`FG_LOAD_*`),
//! 2. when a frame is to be generated, runs the key, field and interpolation passes of `upscale::framegen` between
//!    the previous frame's buffers and these (`FG_KEY`, `FG_FIELD`, `FG_INTERP`), and
//! 3. writes the real frame and the generated one into half-float images of its own (`FG_STORE`).
//!
//! Those images are what the presenter shows later, so the title can reuse its back buffer at once. The values are
//! the ones a sampled read of the title's image gives: an sRGB back buffer is held linear and flagged as such in the
//! returned [`Captured`], so it takes the same path to the screen as the title's own image would.

use std::time::Duration;

use ash::vk;
use upscale::framegen::FrameInputs;
use upscale::shaders;

use super::convert::Rect;
use super::dlssg::{self, DlssState};
use super::vulkan::{
    SourceImage, VulkanContext, color_range, find_memory, is_displayable, is_srgb,
};
use super::{FrameGenMethod, PresentError};

const SLOTS: usize = 2;
/// Held frames: the real and the generated image each have this many, so a frame can still be on its way to the
/// screen while two newer ones are captured.
const HELD: usize = 3;
const PARAM_STRIDE: u64 = 256;
const PARAM_REGIONS: u64 = 5;
const REGION_COLOR: u64 = 0;
const REGION_DEPTH: u64 = 1;
const REGION_MOTION: u64 = 2;
const REGION_MAIN: u64 = 3;
const REGION_STORE: u64 = 4;
const DEPTH_TOLERANCE: f32 = 0.03;
const HOLD_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Buffer,
    Sampled,
    Storage,
}

struct Pass {
    module: vk::ShaderModule,
    set_layout: vk::DescriptorSetLayout,
    pipe_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

struct Passes {
    load_color: Pass,
    load_depth: Pass,
    load_scalar: Pass,
    load_motion: Pass,
    key: Pass,
    field: Pass,
    interp: Pass,
    store: Pass,
}

struct Buf {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

struct Img {
    image: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
}

struct Sized {
    display: [u32; 2],
    render: [u32; 2],
    color: [Buf; 2],
    depth: [Buf; 2],
    motion: Buf,
    key: Buf,
    field: Buf,
    dst: Buf,
    real: Vec<Img>,
    generated: Vec<Img>,
}

#[derive(Default)]
struct Slot {
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    pool: vk::DescriptorPool,
    params: vk::Buffer,
    params_memory: vk::DeviceMemory,
    mapped: usize,
    views: Vec<vk::ImageView>,
    queries: vk::QueryPool,
    submitted: bool,
}

/// What to capture from a real frame.
pub struct Capture<'a> {
    pub color: &'a SourceImage,
    /// The part of the colour image that is the frame; the whole image when `None`.
    pub src_rect: Option<Rect>,
    pub inputs: &'a FrameInputs,
    pub depth: &'a SourceImage,
    pub motion: &'a SourceImage,
    /// Generate a frame between the previous captured frame and this one.
    pub generate: bool,
    pub phase: f32,
    pub hud_threshold: f32,
    /// What makes the generated frame.
    pub method: FrameGenMethod,
}

/// The images a capture leaves behind, in `SHADER_READ_ONLY_OPTIMAL`, valid until two more frames are captured.
pub struct Captured {
    pub real: SourceImage,
    pub generated: Option<SourceImage>,
    /// The held values are the linear form of an sRGB-encoded frame.
    pub srgb_values: bool,
    /// Why DLSS frame generation was asked for and not used: this capture was made by Leonida's own passes.
    pub fallback: Option<String>,
}

/// Where the GPU time of the last capture went.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuTimes {
    pub loads: Duration,
    pub generation: Duration,
    pub stores: Duration,
}

impl GpuTimes {
    pub fn total(&self) -> Duration {
        self.loads + self.generation + self.stores
    }
}

pub struct FrameGen {
    ctx: VulkanContext,
    mem: vk::PhysicalDeviceMemoryProperties,
    passes: Passes,
    command_pool: vk::CommandPool,
    slots: Vec<Slot>,
    next: usize,
    sized: Option<Sized>,
    current: usize,
    have_previous: bool,
    held: usize,
    period_ns: f64,
    timestamps: bool,
    dlss: Option<DlssState>,
    dlss_failure: Option<String>,
}

fn depth_aspects(format: vk::Format) -> Option<(vk::ImageAspectFlags, vk::ImageAspectFlags)> {
    use vk::Format as F;
    match format {
        F::D16_UNORM | F::D32_SFLOAT | F::X8_D24_UNORM_PACK32 => {
            Some((vk::ImageAspectFlags::DEPTH, vk::ImageAspectFlags::DEPTH))
        }
        F::D16_UNORM_S8_UINT | F::D24_UNORM_S8_UINT | F::D32_SFLOAT_S8_UINT => Some((
            vk::ImageAspectFlags::DEPTH,
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
        )),
        _ => None,
    }
}

impl Pass {
    /// # Safety
    /// `device` is live; the pass is destroyed with [`Pass::destroy`] on the same device.
    unsafe fn new(
        device: &ash::Device,
        spirv: &[u32],
        kinds: &[Kind],
    ) -> Result<Pass, PresentError> {
        // SAFETY: plain object creation from valid create infos.
        unsafe {
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(spirv), None)?;
            let bindings: Vec<_> = kinds
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(i as u32)
                        .descriptor_type(match k {
                            Kind::Buffer => vk::DescriptorType::STORAGE_BUFFER,
                            Kind::Sampled => vk::DescriptorType::SAMPLED_IMAGE,
                            Kind::Storage => vk::DescriptorType::STORAGE_IMAGE,
                        })
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::COMPUTE)
                })
                .collect();
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
            Ok(Pass {
                module,
                set_layout,
                pipe_layout,
                pipeline,
            })
        }
    }

    /// # Safety
    /// The pass is idle and not used again.
    unsafe fn destroy(&self, device: &ash::Device) {
        // SAFETY: the caller promises the GPU is done with the pass.
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipe_layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_shader_module(self.module, None);
        }
    }
}

impl Passes {
    fn each(&self) -> [&Pass; 8] {
        [
            &self.load_color,
            &self.load_depth,
            &self.load_scalar,
            &self.load_motion,
            &self.key,
            &self.field,
            &self.interp,
            &self.store,
        ]
    }
}

#[derive(Clone, Copy)]
enum Res {
    Buffer(vk::Buffer, u64, u64),
    Sampled(vk::ImageView),
    Storage(vk::ImageView),
}

/// What a capture needs while it records.
struct Recorder<'a> {
    device: &'a ash::Device,
    cmd: vk::CommandBuffer,
    pool: vk::DescriptorPool,
    params: vk::Buffer,
}

impl Recorder<'_> {
    fn params(&self, region: u64) -> Res {
        Res::Buffer(self.params, region * PARAM_STRIDE, PARAM_STRIDE)
    }

    /// # Safety
    /// The command buffer is recording and every resource in `res` lives until its fence.
    unsafe fn run(&self, pass: &Pass, res: &[Res], size: [u32; 2]) -> Result<(), PresentError> {
        let layouts = [pass.set_layout];
        // SAFETY: recording into an open command buffer; the descriptor pool is reset only after the fence.
        unsafe {
            let set = self.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.pool)
                    .set_layouts(&layouts),
            )?[0];
            let buffers: Vec<_> = res
                .iter()
                .map(|r| match r {
                    Res::Buffer(b, offset, range) => [vk::DescriptorBufferInfo::default()
                        .buffer(*b)
                        .offset(*offset)
                        .range(*range)],
                    _ => [vk::DescriptorBufferInfo::default()],
                })
                .collect();
            let images: Vec<_> = res
                .iter()
                .map(|r| match r {
                    Res::Sampled(v) => [vk::DescriptorImageInfo::default()
                        .image_view(*v)
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)],
                    Res::Storage(v) => [vk::DescriptorImageInfo::default()
                        .image_view(*v)
                        .image_layout(vk::ImageLayout::GENERAL)],
                    Res::Buffer(..) => [vk::DescriptorImageInfo::default()],
                })
                .collect();
            let writes: Vec<_> = res
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let w = vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(i as u32);
                    match r {
                        Res::Buffer(..) => w
                            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                            .buffer_info(&buffers[i]),
                        Res::Sampled(_) => w
                            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                            .image_info(&images[i]),
                        Res::Storage(_) => w
                            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                            .image_info(&images[i]),
                    }
                })
                .collect();
            self.device.update_descriptor_sets(&writes, &[]);
            self.device
                .cmd_bind_pipeline(self.cmd, vk::PipelineBindPoint::COMPUTE, pass.pipeline);
            self.device.cmd_bind_descriptor_sets(
                self.cmd,
                vk::PipelineBindPoint::COMPUTE,
                pass.pipe_layout,
                0,
                &[set],
                &[],
            );
            self.device
                .cmd_dispatch(self.cmd, size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
        Ok(())
    }

    /// Everything the earlier passes wrote is visible to the later ones.
    ///
    /// # Safety
    /// The command buffer is recording.
    unsafe fn sync(&self) {
        let barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(
                vk::AccessFlags::SHADER_READ
                    | vk::AccessFlags::SHADER_WRITE
                    | vk::AccessFlags::TRANSFER_WRITE,
            );
        // SAFETY: recording into an open command buffer.
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn image_barrier(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
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
        .subresource_range(color_range().aspect_mask(aspect));
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

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

impl FrameGen {
    pub fn new(ctx: &VulkanContext) -> Result<FrameGen, PresentError> {
        let device = &ctx.device;
        // SAFETY: handles made here are owned by the generator and destroyed in Drop.
        unsafe {
            let mem = ctx
                .instance
                .get_physical_device_memory_properties(ctx.physical);
            let props = ctx.instance.get_physical_device_properties(ctx.physical);
            let families = ctx
                .instance
                .get_physical_device_queue_family_properties(ctx.physical);
            let timestamps = families
                .get(ctx.family as usize)
                .is_some_and(|f| f.timestamp_valid_bits != 0);
            let b = Kind::Buffer;
            let make = |spirv: &[u32], kinds: &[Kind]| Pass::new(device, spirv, kinds);
            let passes = Passes {
                load_color: make(shaders::FG_LOAD_COLOR, &[Kind::Sampled, b, b])?,
                load_depth: make(shaders::FG_LOAD_DEPTH, &[Kind::Sampled, b, b])?,
                load_scalar: make(shaders::FG_LOAD_SCALAR, &[Kind::Sampled, b, b])?,
                load_motion: make(shaders::FG_LOAD_MOTION, &[Kind::Sampled, b, b])?,
                key: make(shaders::FG_KEY, &[b, b, b, b])?,
                field: make(shaders::FG_FIELD, &[b, b, b, b, b])?,
                interp: make(shaders::FG_INTERP, &[b, b, b, b, b, b, b, b])?,
                store: make(shaders::FG_STORE, &[b, Kind::Storage, b])?,
            };
            let command_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(ctx.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let mut generator = FrameGen {
                ctx: ctx.clone(),
                mem,
                passes,
                command_pool,
                slots: Vec::new(),
                next: 0,
                sized: None,
                current: 0,
                have_previous: false,
                held: 0,
                period_ns: f64::from(props.limits.timestamp_period),
                timestamps,
                dlss: None,
                dlss_failure: None,
            };
            for _ in 0..SLOTS {
                let slot = generator.make_slot()?;
                generator.slots.push(slot);
            }
            Ok(generator)
        }
    }

    unsafe fn make_slot(&self) -> Result<Slot, PresentError> {
        let device = &self.ctx.device;
        // SAFETY: plain object creation; the caller pushes the slot into `slots`, which Drop tears down.
        unsafe {
            let cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.command_pool)
                    .command_buffer_count(1),
            )?[0];
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(40),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(4),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(4),
            ];
            let pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(12)
                    .pool_sizes(&sizes),
                None,
            )?;
            let size = PARAM_STRIDE * PARAM_REGIONS;
            let params = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::STORAGE_BUFFER),
                None,
            )?;
            let needs = device.get_buffer_memory_requirements(params);
            let kind = find_memory(
                &self.mem,
                needs.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or_else(|| PresentError::Vulkan("no host memory for the parameters".into()))?;
            let params_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            )?;
            device.bind_buffer_memory(params, params_memory, 0)?;
            let mapped =
                device.map_memory(params_memory, 0, size, vk::MemoryMapFlags::empty())? as usize;
            let queries = device.create_query_pool(
                &vk::QueryPoolCreateInfo::default()
                    .query_type(vk::QueryType::TIMESTAMP)
                    .query_count(4),
                None,
            )?;
            Ok(Slot {
                cmd,
                fence,
                pool,
                params,
                params_memory,
                mapped,
                views: Vec::new(),
                queries,
                submitted: false,
            })
        }
    }

    unsafe fn buffer(&self, size: u64) -> Result<Buf, PresentError> {
        let device = &self.ctx.device;
        // SAFETY: plain object creation; the caller stores the result in `Sized`, whose teardown frees it.
        unsafe {
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default().size(size.max(4)).usage(
                    vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                ),
                None,
            )?;
            let needs = device.get_buffer_memory_requirements(buffer);
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
            .ok_or_else(|| PresentError::Vulkan("no memory for a frame buffer".into()))?;
            let memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            )?;
            device.bind_buffer_memory(buffer, memory, 0)?;
            Ok(Buf { buffer, memory })
        }
    }

    unsafe fn image(&self, size: [u32; 2]) -> Result<Img, PresentError> {
        let device = &self.ctx.device;
        // SAFETY: plain object creation; the caller stores the result in `Sized`, whose teardown frees it.
        unsafe {
            let image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(HOLD_FORMAT)
                    .extent(vk::Extent3D {
                        width: size[0],
                        height: size[1],
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::STORAGE
                            | vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_SRC,
                    )
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
            .ok_or_else(|| PresentError::Vulkan("no memory for a held frame".into()))?;
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
                    .format(HOLD_FORMAT)
                    .subresource_range(color_range()),
                None,
            )?;
            Ok(Img {
                image,
                view,
                memory,
            })
        }
    }

    fn free_sized(&mut self) {
        let Some(sized) = self.sized.take() else {
            return;
        };
        let device = &self.ctx.device;
        // SAFETY: the device is idle, so nothing uses the resources any more.
        unsafe {
            let _ = device.device_wait_idle();
            for b in [
                &sized.color[0],
                &sized.color[1],
                &sized.depth[0],
                &sized.depth[1],
                &sized.motion,
                &sized.key,
                &sized.field,
                &sized.dst,
            ] {
                device.destroy_buffer(b.buffer, None);
                device.free_memory(b.memory, None);
            }
            for i in sized.real.iter().chain(sized.generated.iter()) {
                device.destroy_image_view(i.view, None);
                device.destroy_image(i.image, None);
                device.free_memory(i.memory, None);
            }
        }
    }

    fn ensure_sized(&mut self, display: [u32; 2], render: [u32; 2]) -> Result<(), PresentError> {
        if self
            .sized
            .as_ref()
            .is_some_and(|s| s.display == display && s.render == render)
        {
            return Ok(());
        }
        self.free_sized();
        self.have_previous = false;
        let texels = u64::from(display[0]) * u64::from(display[1]);
        let rtexels = u64::from(render[0]) * u64::from(render[1]);
        // SAFETY: each resource is moved into `Sized`; a failure part-way leaks nothing the driver keeps past device
        // teardown, and `free_sized` frees whatever was stored.
        let sized = unsafe {
            Sized {
                display,
                render,
                color: [self.buffer(texels * 16)?, self.buffer(texels * 16)?],
                depth: [self.buffer(rtexels * 4)?, self.buffer(rtexels * 4)?],
                motion: self.buffer(rtexels * 8)?,
                key: self.buffer(texels * 4)?,
                field: self.buffer(texels * 8)?,
                dst: self.buffer(texels * 16)?,
                real: (0..HELD)
                    .map(|_| self.image(display))
                    .collect::<Result<_, _>>()?,
                generated: (0..HELD)
                    .map(|_| self.image(display))
                    .collect::<Result<_, _>>()?,
            }
        };
        self.sized = Some(sized);
        Ok(())
    }

    /// Forgets the previous frame: the next capture generates nothing.
    pub fn forget(&mut self) {
        self.have_previous = false;
    }

    /// Makes every later DLSS request fall back, as a failed start of NGX does.
    #[cfg(test)]
    pub(super) fn refuse_dlss(&mut self, why: &str) {
        self.dlss_failure = Some(why.to_string());
    }

    /// NGX ready for these sizes, or the reason it is not (and will not be tried again).
    fn dlss_ready(&mut self, size: [u32; 2], render: [u32; 2]) -> Result<DlssState, String> {
        if let Some(why) = &self.dlss_failure {
            return Err(why.clone());
        }
        let mut state = match self.dlss.take() {
            Some(state) => state,
            None => match DlssState::open(&self.ctx, dlssg::library_dir().as_deref()) {
                Ok(state) => state,
                Err(why) => {
                    let why = format!("DLSS frame generation cannot start ({why})");
                    self.dlss_failure = Some(why.clone());
                    return Err(why);
                }
            },
        };
        match state.prepare(&self.ctx, self.command_pool, size, render, HOLD_FORMAT) {
            Ok(()) => Ok(state),
            Err(why) => {
                let why = format!("DLSS frame generation cannot be set up ({why})");
                self.dlss = Some(state);
                self.give_up_on_dlss(why.clone());
                Err(why)
            }
        }
    }

    /// Stops using NGX for the rest of the run. The GPU finishes what it was given first, so the feature can go.
    fn give_up_on_dlss(&mut self, why: String) {
        // SAFETY: waiting for the device to be idle has no preconditions.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
        }
        self.dlss = None;
        self.dlss_failure = Some(why);
    }

    fn check_inputs(&self, c: &Capture<'_>, size: [u32; 2]) -> Result<(), PresentError> {
        let no = |why: &str| Err(PresentError::Unsupported(why.to_string()));
        let render = c.inputs.render_size;
        if size[0] == 0 || size[1] == 0 || render[0] == 0 || render[1] == 0 {
            return no("frame generation: an image has no area");
        }
        for (name, image) in [
            ("colour", c.color),
            ("depth", c.depth),
            ("motion vector", c.motion),
        ] {
            if !image.usage.contains(vk::ImageUsageFlags::SAMPLED) {
                return Err(PresentError::Unsupported(format!(
                    "frame generation: the {name} image cannot be sampled"
                )));
            }
            if image.samples != vk::SampleCountFlags::TYPE_1
                || image.layout == vk::ImageLayout::UNDEFINED
            {
                return Err(PresentError::Unsupported(format!(
                    "frame generation: the {name} image is multisampled or has no defined layout"
                )));
            }
        }
        if !is_displayable(c.color.format) {
            return no("frame generation: the colour format cannot be read as floats");
        }
        for (name, image) in [("depth", c.depth), ("motion vector", c.motion)] {
            if image.extent.0 < render[0] || image.extent.1 < render[1] {
                return Err(PresentError::Unsupported(format!(
                    "frame generation: the {name} image is smaller than the render size"
                )));
            }
        }
        if depth_aspects(c.motion.format).is_some() {
            return no("frame generation: the motion vectors are in a depth format");
        }
        Ok(())
    }

    fn view(
        &self,
        slot: &mut Slot,
        image: &SourceImage,
        aspect: vk::ImageAspectFlags,
    ) -> Result<vk::ImageView, PresentError> {
        // SAFETY: the view is destroyed when the slot's fence has signalled.
        let view = unsafe {
            self.ctx.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image.image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(image.format)
                    .subresource_range(color_range().aspect_mask(aspect)),
                None,
            )?
        };
        slot.views.push(view);
        Ok(view)
    }

    /// Records and submits the capture of one real frame.
    pub fn capture(&mut self, c: &Capture<'_>) -> Result<Captured, PresentError> {
        let whole = Rect {
            left: 0,
            top: 0,
            right: c.color.extent.0 as i32,
            bottom: c.color.extent.1 as i32,
        };
        let r = c.src_rect.unwrap_or(whole);
        let r = Rect {
            left: r.left.clamp(0, whole.right),
            top: r.top.clamp(0, whole.bottom),
            right: r.right.clamp(0, whole.right),
            bottom: r.bottom.clamp(0, whole.bottom),
        };
        let r = if r.is_empty() { whole } else { r };
        let size = [r.width() as u32, r.height() as u32];
        self.check_inputs(c, size)?;
        let render = c.inputs.render_size;
        self.ensure_sized(size, render)?;
        let mut fallback = None;
        let mut dlss = None;
        if c.method == FrameGenMethod::Dlss {
            match self.dlss_ready(size, render) {
                Ok(state) => dlss = Some(state),
                Err(why) => fallback = Some(why),
            }
        }
        let mut generate = c.generate && self.have_previous;
        let index = self.next;
        let mut slot = std::mem::take(&mut self.slots[index]);
        let mut result = self.record(&mut slot, c, r, size, render, generate, dlss.as_mut());
        if let (Err(error), Some(_)) = (&result, &dlss) {
            let why = format!("DLSS frame generation failed ({error})");
            self.give_up_on_dlss(why.clone());
            dlss = None;
            fallback = Some(why);
            self.have_previous = false;
            generate = false;
            result = self.record(&mut slot, c, r, size, render, generate, None);
        }
        self.slots[index] = slot;
        self.dlss = dlss;
        let mut captured = result?;
        captured.fallback = fallback;
        self.next = (self.next + 1) % SLOTS;
        self.current ^= 1;
        self.have_previous = true;
        self.held = (self.held + 1) % HELD;
        Ok(captured)
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        slot: &mut Slot,
        c: &Capture<'_>,
        region: Rect,
        size: [u32; 2],
        render: [u32; 2],
        generate: bool,
        mut dlss: Option<&mut DlssState>,
    ) -> Result<Captured, PresentError> {
        let device = self.ctx.device.clone();
        let Some(sized) = self.sized.as_ref() else {
            return Err(PresentError::Vulkan("no frame generation buffers".into()));
        };
        let cur = self.current;
        let prev = cur ^ 1;
        let held = self.held;
        let (depth_aspect, depth_barrier_aspect) = depth_aspects(c.depth.format).unzip();
        // SAFETY: the slot's fence guards every object it owns; it is waited before any is reused. Everything recorded
        // stays alive until that fence: the buffers and images live in `sized`, which is only replaced by a capture
        // that first waits for the device to be idle, and the views are destroyed after the fence.
        unsafe {
            if slot.submitted {
                device.wait_for_fences(&[slot.fence], true, u64::MAX)?;
            }
            for v in slot.views.drain(..) {
                device.destroy_image_view(v, None);
            }
            device.reset_descriptor_pool(slot.pool, vk::DescriptorPoolResetFlags::empty())?;
            let load_depth_pass = if depth_aspect.is_some() {
                &self.passes.load_depth
            } else {
                &self.passes.load_scalar
            };

            let phase = c.phase.to_bits();
            let near_far =
                if c.inputs.camera_far.is_finite() && c.inputs.camera_far > c.inputs.camera_near {
                    c.inputs.camera_near / c.inputs.camera_far
                } else {
                    0.0
                };
            let write = |region: u64, values: &[u32]| {
                let bytes = words(values);
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (slot.mapped as *mut u8).add((region * PARAM_STRIDE) as usize),
                    bytes.len(),
                );
            };
            write(
                REGION_COLOR,
                &[size[0], size[1], region.left as u32, region.top as u32],
            );
            write(REGION_DEPTH, &[render[0], render[1]]);
            write(REGION_MOTION, &[render[0], render[1]]);
            write(
                REGION_MAIN,
                &[
                    size[0],
                    size[1],
                    render[0],
                    render[1],
                    phase,
                    u32::from(c.inputs.depth_reversed),
                    DEPTH_TOLERANCE.to_bits(),
                    near_far.to_bits(),
                    c.inputs.motion_vector_scale[0].to_bits(),
                    c.inputs.motion_vector_scale[1].to_bits(),
                    c.hud_threshold.max(0.0).to_bits(),
                ],
            );
            write(REGION_STORE, &[size[0], size[1]]);

            let color_view = self.view(slot, c.color, vk::ImageAspectFlags::COLOR)?;
            let depth_view = self.view(
                slot,
                c.depth,
                depth_aspect.unwrap_or(vk::ImageAspectFlags::COLOR),
            )?;
            let motion_view = self.view(slot, c.motion, vk::ImageAspectFlags::COLOR)?;

            let cmd = slot.cmd;
            device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            if self.timestamps {
                device.cmd_reset_query_pool(cmd, slot.queries, 0, 4);
                device.cmd_write_timestamp(
                    cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    slot.queries,
                    0,
                );
            }
            let rec = Recorder {
                device: &device,
                cmd,
                pool: slot.pool,
                params: slot.params,
            };
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                    .dst_access_mask(
                        vk::AccessFlags::SHADER_READ
                            | vk::AccessFlags::SHADER_WRITE
                            | vk::AccessFlags::TRANSFER_WRITE,
                    )],
                &[],
                &[],
            );
            let reading = (
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::AccessFlags::MEMORY_WRITE,
            );
            let compute_read = (
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::AccessFlags::SHADER_READ,
            );
            let given_back = (
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
            );
            let compute_done = (
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::AccessFlags::SHADER_READ,
            );
            let depth_layout = if depth_aspect.is_some() {
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL
            } else {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            };
            for (image, aspect, layout) in [
                (
                    c.color,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ),
                (
                    c.depth,
                    depth_barrier_aspect.unwrap_or(vk::ImageAspectFlags::COLOR),
                    depth_layout,
                ),
                (
                    c.motion,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ),
            ] {
                image_barrier(
                    &device,
                    cmd,
                    image.image,
                    aspect,
                    image.layout,
                    layout,
                    reading,
                    compute_read,
                );
            }
            let hold = |images: &[Img]| {
                image_barrier(
                    &device,
                    cmd,
                    images[held].image,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    given_back,
                    (
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::AccessFlags::SHADER_WRITE,
                    ),
                );
            };
            hold(&sized.real);
            if generate || dlss.is_some() {
                hold(&sized.generated);
            }
            if generate && dlss.is_none() {
                device.cmd_fill_buffer(cmd, sized.key.buffer, 0, vk::WHOLE_SIZE, 0);
            }
            let whole = |b: &Buf| Res::Buffer(b.buffer, 0, vk::WHOLE_SIZE);
            rec.run(
                &self.passes.load_color,
                &[
                    Res::Sampled(color_view),
                    whole(&sized.color[cur]),
                    rec.params(REGION_COLOR),
                ],
                size,
            )?;
            if dlss.is_none() {
                rec.run(
                    load_depth_pass,
                    &[
                        Res::Sampled(depth_view),
                        whole(&sized.depth[cur]),
                        rec.params(REGION_DEPTH),
                    ],
                    render,
                )?;
                rec.run(
                    &self.passes.load_motion,
                    &[
                        Res::Sampled(motion_view),
                        whole(&sized.motion),
                        rec.params(REGION_MOTION),
                    ],
                    render,
                )?;
            }
            rec.sync();
            if self.timestamps {
                device.cmd_write_timestamp(
                    cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    slot.queries,
                    1,
                );
            }
            if generate && dlss.is_none() {
                let main = rec.params(REGION_MAIN);
                rec.run(
                    &self.passes.key,
                    &[
                        whole(&sized.depth[cur]),
                        whole(&sized.motion),
                        whole(&sized.key),
                        main,
                    ],
                    size,
                )?;
                rec.sync();
                rec.run(
                    &self.passes.field,
                    &[
                        whole(&sized.depth[cur]),
                        whole(&sized.motion),
                        whole(&sized.key),
                        whole(&sized.field),
                        main,
                    ],
                    size,
                )?;
                rec.sync();
                rec.run(
                    &self.passes.interp,
                    &[
                        whole(&sized.color[cur]),
                        whole(&sized.color[prev]),
                        whole(&sized.depth[cur]),
                        whole(&sized.depth[prev]),
                        whole(&sized.key),
                        whole(&sized.field),
                        whole(&sized.dst),
                        main,
                    ],
                    size,
                )?;
                rec.sync();
            }
            if self.timestamps {
                device.cmd_write_timestamp(
                    cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    slot.queries,
                    2,
                );
            }
            rec.run(
                &self.passes.store,
                &[
                    whole(&sized.color[cur]),
                    Res::Storage(sized.real[held].view),
                    rec.params(REGION_STORE),
                ],
                size,
            )?;
            if generate && dlss.is_none() {
                rec.run(
                    &self.passes.store,
                    &[
                        whole(&sized.dst),
                        Res::Storage(sized.generated[held].view),
                        rec.params(REGION_STORE),
                    ],
                    size,
                )?;
            }
            let readable = |images: &[Img]| {
                image_barrier(
                    &device,
                    cmd,
                    images[held].image,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    (
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::AccessFlags::SHADER_WRITE,
                    ),
                    (
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::AccessFlags::MEMORY_READ,
                    ),
                );
            };
            readable(&sized.real);
            if let Some(state) = dlss.as_deref_mut() {
                let held_image = |img: &Img| dlssg::Image {
                    image: img.image,
                    view: img.view,
                    format: HOLD_FORMAT,
                    aspect: vk::ImageAspectFlags::COLOR,
                    size,
                };
                let frame = dlssg::Frame {
                    color: held_image(&sized.real[held]),
                    depth: dlssg::Image {
                        image: c.depth.image,
                        view: depth_view,
                        format: c.depth.format,
                        aspect: depth_aspect.unwrap_or(vk::ImageAspectFlags::COLOR),
                        size: [c.depth.extent.0, c.depth.extent.1],
                    },
                    motion: dlssg::Image {
                        image: c.motion.image,
                        view: motion_view,
                        format: c.motion.format,
                        aspect: vk::ImageAspectFlags::COLOR,
                        size: [c.motion.extent.0, c.motion.extent.1],
                    },
                    output: held_image(&sized.generated[held]),
                    render_size: render,
                    inputs: c.inputs,
                    reset: !generate || c.inputs.reset,
                };
                state
                    .evaluate(cmd, &frame)
                    .map_err(PresentError::Unsupported)?;
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[vk::MemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                        .dst_access_mask(
                            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                        )],
                    &[],
                    &[],
                );
            }
            if generate || dlss.is_some() {
                readable(&sized.generated);
            }
            for (image, aspect, layout) in [
                (
                    c.color,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ),
                (
                    c.depth,
                    depth_barrier_aspect.unwrap_or(vk::ImageAspectFlags::COLOR),
                    depth_layout,
                ),
                (
                    c.motion,
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ),
            ] {
                image_barrier(
                    &device,
                    cmd,
                    image.image,
                    aspect,
                    layout,
                    image.layout,
                    compute_done,
                    given_back,
                );
            }
            if self.timestamps {
                device.cmd_write_timestamp(
                    cmd,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    slot.queries,
                    3,
                );
            }
            device.end_command_buffer(cmd)?;
            device.reset_fences(&[slot.fence])?;
            let mut waits = Vec::new();
            let mut values = Vec::new();
            let mut stages = Vec::new();
            for image in [c.color, c.depth, c.motion] {
                for (s, v) in &image.waits {
                    waits.push(*s);
                    values.push(*v);
                    stages.push(vk::PipelineStageFlags::ALL_COMMANDS);
                }
            }
            let cmds = [cmd];
            let mut timeline =
                vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&values);
            let mut submit = vk::SubmitInfo::default()
                .wait_semaphores(&waits)
                .wait_dst_stage_mask(&stages)
                .command_buffers(&cmds);
            if values.iter().any(|v| *v != 0) {
                submit = submit.push_next(&mut timeline);
            }
            let queue = self.ctx.queue.lock().unwrap_or_else(|e| e.into_inner());
            device.queue_submit(*queue, &[submit], slot.fence)?;
            slot.submitted = true;
        }
        let held_image = |img: &Img| SourceImage {
            image: img.image,
            format: HOLD_FORMAT,
            extent: (size[0], size[1]),
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            usage: vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
            samples: vk::SampleCountFlags::TYPE_1,
            waits: Vec::new(),
        };
        Ok(Captured {
            real: held_image(&sized.real[held]),
            generated: generate.then(|| held_image(&sized.generated[held])),
            srgb_values: is_srgb(c.color.format),
            fallback: None,
        })
    }

    /// Waits for the newest capture and reads where its GPU time went, when the device has timestamps.
    pub fn last_gpu_times(&mut self) -> Option<GpuTimes> {
        if !self.timestamps {
            return None;
        }
        let slot = &self.slots[(self.next + SLOTS - 1) % SLOTS];
        if !slot.submitted {
            return None;
        }
        let mut ticks = [0u64; 4];
        // SAFETY: the fence belongs to a submitted slot and the query pool to this device.
        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[slot.fence], true, u64::MAX)
                .ok()?;
            self.ctx
                .device
                .get_query_pool_results(slot.queries, 0, &mut ticks, vk::QueryResultFlags::TYPE_64)
                .ok()?;
        }
        let ns = |a: u64, b: u64| {
            Duration::from_nanos((b.saturating_sub(a) as f64 * self.period_ns) as u64)
        };
        Some(GpuTimes {
            loads: ns(ticks[0], ticks[1]),
            generation: ns(ticks[1], ticks[2]),
            stores: ns(ticks[2], ticks[3]),
        })
    }
}

impl Drop for FrameGen {
    fn drop(&mut self) {
        // SAFETY: the device is idle before anything the generator owns is destroyed.
        unsafe {
            let _ = self.ctx.device.device_wait_idle();
        }
        self.free_sized();
        let device = &self.ctx.device;
        // SAFETY: idle device, nothing in flight.
        unsafe {
            for slot in self.slots.drain(..) {
                for v in slot.views {
                    device.destroy_image_view(v, None);
                }
                device.destroy_query_pool(slot.queries, None);
                device.destroy_buffer(slot.params, None);
                device.free_memory(slot.params_memory, None);
                device.destroy_descriptor_pool(slot.pool, None);
                device.destroy_fence(slot.fence, None);
            }
            for pass in self.passes.each() {
                pass.destroy(device);
            }
            device.destroy_command_pool(self.command_pool, None);
        }
    }
}
