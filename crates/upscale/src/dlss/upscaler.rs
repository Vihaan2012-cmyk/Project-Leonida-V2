//! [`DlssUpscaler`]: DLSS super resolution behind the crate's [`Upscaler`] trait, and a direct Vulkan entry point for a
//! renderer that holds the game's images on the GPU.

use std::path::PathBuf;
use std::sync::Arc;

use ash::vk;

use super::ModeSupport;
use super::modes::{DlssMode, DlssOptions, MIN_OUTPUT_SIZE};
use super::ngx::{
    self, Api, Availability, EvalParams, Feature, Ngx, NgxError, OptimalSettings, VkInit,
};
use super::vk::{GpuImage, HostBuffer, NVIDIA_VENDOR_ID, VkContext, VkHandles, transition};
use crate::{ColorSpace, Image, PixelFormat, UpscaleError, UpscaleInputs, Upscaler, UpscalerKind};

fn unavailable(reason: impl Into<String>) -> UpscaleError {
    UpscaleError::Unavailable {
        name: "DLSS",
        reason: reason.into(),
    }
}

fn ngx_error(error: NgxError) -> UpscaleError {
    unavailable(error.to_string())
}

#[derive(Clone, PartialEq)]
struct FeatureKey {
    render: [u32; 2],
    output: [u32; 2],
    flags: i32,
    mode: DlssMode,
}

struct Resources {
    color: GpuImage,
    depth: GpuImage,
    motion: GpuImage,
    exposure: GpuImage,
    output: GpuImage,
    staging: HostBuffer,
    readback: HostBuffer,
    offsets: [u64; 4],
}

struct Active {
    key: FeatureKey,
    feature: Feature,
    resources: Option<Resources>,
}

/// DLSS super resolution. Build it with [`DlssUpscaler::new`] (own NVIDIA device) or [`DlssUpscaler::with_device`]
/// (the renderer's device).
pub struct DlssUpscaler {
    active: Option<Active>,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    ngx: Ngx,
    ctx: VkContext,
    options: DlssOptions,
    availability: Availability,
    reset_next: bool,
}

// SAFETY: every Vulkan and NGX object is used only through `&mut self`.
unsafe impl Send for DlssUpscaler {}

pub(super) struct Opened {
    pub api: Arc<Api>,
    pub ctx: VkContext,
}

/// Opens the NGX loader, asks it which extensions it needs and builds an NVIDIA device that has them.
pub(super) fn open_own_device(ngx_runtime: Option<&std::path::Path>) -> Result<Opened, String> {
    let api = Arc::new(Api::open(ngx_runtime)?);
    let extensions = api.required_extensions().map_err(|e| e.to_string())?;
    let ctx = VkContext::create_nvidia(&extensions.instance, &extensions.device)?;
    Ok(Opened { api, ctx })
}

pub(super) fn init_ngx(
    api: Arc<Api>,
    ctx: &VkContext,
    options: &DlssOptions,
    dlss_dll: Option<&std::path::Path>,
) -> Result<Ngx, String> {
    let vk_init = VkInit {
        instance: ctx.instance.handle(),
        physical: ctx.physical,
        device: ctx.device.handle(),
        get_instance_proc_addr: ctx.entry.static_fn().get_instance_proc_addr,
        get_device_proc_addr: ctx.instance.fp_v1_0().get_device_proc_addr,
    };
    let dir = dlss_dll.and_then(ngx::feature_dir);
    Ngx::init(api, &vk_init, options, dir.as_deref()).map_err(|e| e.to_string())
}

impl DlssUpscaler {
    pub(super) fn from_parts(
        api: Arc<Api>,
        ctx: VkContext,
        options: DlssOptions,
        dlss_dll: Option<PathBuf>,
    ) -> Result<DlssUpscaler, UpscaleError> {
        if ctx.vendor_id() != NVIDIA_VENDOR_ID {
            return Err(unavailable(format!(
                "{} is not an NVIDIA GPU",
                ctx.device_name
            )));
        }
        let ngx = init_ngx(api, &ctx, &options, dlss_dll.as_deref()).map_err(unavailable)?;
        let availability = ngx.availability();
        if let Some(reason) = availability.reason() {
            return Err(unavailable(reason));
        }
        let (pool, cmd, fence) = make_submission(&ctx).map_err(unavailable)?;
        Ok(DlssUpscaler {
            active: None,
            pool,
            cmd,
            fence,
            ngx,
            ctx,
            options,
            availability,
            reset_next: true,
        })
    }

    /// Creates DLSS on its own Vulkan device (an NVIDIA GPU with the extensions NGX names).
    pub fn new(config: &super::DlssConfig) -> Result<DlssUpscaler, UpscaleError> {
        Self::with_options(config, DlssOptions::default())
    }

    pub fn with_options(
        config: &super::DlssConfig,
        options: DlssOptions,
    ) -> Result<DlssUpscaler, UpscaleError> {
        if let super::DlssStatus::Unavailable(reason) = super::probe(config) {
            return Err(unavailable(reason));
        }
        let Opened { api, ctx } =
            open_own_device(config.ngx_runtime.as_deref()).map_err(unavailable)?;
        Self::from_parts(api, ctx, options, config.dlss_dll.clone())
    }

    /// Creates DLSS on the renderer's device. The device must have been created with the extensions
    /// [`super::required_extensions`] lists.
    ///
    /// # Safety
    /// The handles must be live and belong together; the queue must not be used by another thread during calls.
    pub unsafe fn with_device(
        config: &super::DlssConfig,
        options: DlssOptions,
        handles: VkHandles,
    ) -> Result<DlssUpscaler, UpscaleError> {
        if let super::DlssStatus::Unavailable(reason) = super::probe(config) {
            return Err(unavailable(reason));
        }
        let api = Arc::new(Api::open(config.ngx_runtime.as_deref()).map_err(unavailable)?);
        // SAFETY: forwarded from this function's contract.
        let ctx = unsafe { VkContext::borrow(handles) }.map_err(unavailable)?;
        Self::from_parts(api, ctx, options, config.dlss_dll.clone())
    }

    pub fn device_name(&self) -> &str {
        &self.ctx.device_name
    }

    pub fn availability(&self) -> &Availability {
        &self.availability
    }

    pub fn mode(&self) -> DlssMode {
        self.options.mode
    }

    pub fn set_mode(&mut self, mode: DlssMode) {
        self.options.mode = mode;
    }

    /// NGX's recommended render size for `output` in `mode`; `render_size` is zero when NGX does not offer the mode.
    pub fn optimal_settings(
        &mut self,
        output: [u32; 2],
        mode: DlssMode,
    ) -> Result<OptimalSettings, UpscaleError> {
        self.ngx.optimal_settings(output, mode).map_err(ngx_error)
    }

    /// Which modes NGX offers for an output size, with their render sizes.
    pub fn mode_support(&mut self, output: [u32; 2]) -> Vec<ModeSupport> {
        DlssMode::ALL
            .iter()
            .map(|&mode| ModeSupport {
                mode,
                settings: self.optimal_settings(output, mode).ok(),
            })
            .collect()
    }

    /// Creates (or keeps) the DLSS feature for these sizes, submitting NGX's setup commands and waiting for them.
    /// Returns whether the feature is new, in which case the first evaluation resets history.
    pub fn prepare_feature(
        &mut self,
        render: [u32; 2],
        output: [u32; 2],
        flags: i32,
    ) -> Result<bool, UpscaleError> {
        if output[0] < MIN_OUTPUT_SIZE[0] || output[1] < MIN_OUTPUT_SIZE[1] {
            return Err(UpscaleError::BadInput {
                what: "sizes",
                reason: format!(
                    "DLSS needs an output of at least {}x{}, got {}x{}",
                    MIN_OUTPUT_SIZE[0], MIN_OUTPUT_SIZE[1], output[0], output[1]
                ),
            });
        }
        let key = FeatureKey {
            render,
            output,
            flags,
            mode: self.options.mode,
        };
        if self.active.as_ref().is_some_and(|a| a.key == key) {
            return Ok(false);
        }
        self.drop_active();
        let (cmd, fence) = (self.cmd, self.fence);
        let ngx = &mut self.ngx;
        let feature = record_submit(&self.ctx, cmd, fence, |_| {
            ngx.create_feature(cmd, render, output, key.mode, flags)
                .map_err(ngx_error)
        })?;
        self.active = Some(Active {
            key,
            feature,
            resources: None,
        });
        Ok(true)
    }

    fn drop_active(&mut self) {
        if let Some(active) = self.active.take() {
            // SAFETY: waits for the queue so no command buffer still uses the objects released below.
            unsafe { self.ctx.device.queue_wait_idle(self.ctx.queue).ok() };
            if let Some(resources) = active.resources {
                self.destroy_resources(resources);
            }
            drop(active.feature);
        }
    }

    fn destroy_resources(&self, r: Resources) {
        self.ctx.destroy_image(r.color);
        self.ctx.destroy_image(r.depth);
        self.ctx.destroy_image(r.motion);
        self.ctx.destroy_image(r.exposure);
        self.ctx.destroy_image(r.output);
        self.ctx.destroy_buffer(r.staging);
        self.ctx.destroy_buffer(r.readback);
    }

    fn resources_for(&mut self, render: [u32; 2], output: [u32; 2]) -> Result<(), UpscaleError> {
        let key_render = self.active.as_ref().map(|a| a.key.render).unwrap_or(render);
        if self.active.as_ref().is_some_and(|a| a.resources.is_some()) {
            return Ok(());
        }
        let ctx = &self.ctx;
        let sampled = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST;
        let err = |e: String| unavailable(e);
        let color = ctx
            .create_image(vk::Format::R32G32B32A32_SFLOAT, key_render, sampled)
            .map_err(err)?;
        let depth = ctx
            .create_image(vk::Format::R32_SFLOAT, key_render, sampled)
            .map_err(err)?;
        let motion = ctx
            .create_image(vk::Format::R32G32_SFLOAT, key_render, sampled)
            .map_err(err)?;
        let exposure = ctx
            .create_image(vk::Format::R32_SFLOAT, [1, 1], sampled)
            .map_err(err)?;
        let output_image = ctx
            .create_image(
                vk::Format::R16G16B16A16_SFLOAT,
                output,
                vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )
            .map_err(err)?;
        let texels = key_render[0] as u64 * key_render[1] as u64;
        let offsets = [0, texels * 16, texels * 16 + texels * 4, texels * 28];
        let staging = ctx
            .create_buffer(texels * 28 + 16, vk::BufferUsageFlags::TRANSFER_SRC)
            .map_err(err)?;
        let readback = ctx
            .create_buffer(
                output[0] as u64 * output[1] as u64 * 8,
                vk::BufferUsageFlags::TRANSFER_DST,
            )
            .map_err(err)?;
        if let Some(active) = self.active.as_mut() {
            active.resources = Some(Resources {
                color,
                depth,
                motion,
                exposure,
                output: output_image,
                staging,
                readback,
                offsets,
            });
        }
        Ok(())
    }

    /// Evaluates DLSS into `cmd` for images the caller owns. The images must be in the layouts NGX documents: the
    /// inputs readable by a compute shader (`SHADER_READ_ONLY_OPTIMAL`, `SAMPLED` usage), the output `GENERAL` with
    /// `STORAGE` usage. NGX changes the command buffer's state, so the caller restores its own afterwards.
    pub fn evaluate_vk(
        &mut self,
        cmd: vk::CommandBuffer,
        eval: &EvalParams<'_>,
    ) -> Result<(), UpscaleError> {
        let reset = std::mem::take(&mut self.reset_next);
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| unavailable("prepare_feature was not called"))?;
        let mut eval = EvalParams { ..*eval };
        eval.reset |= reset;
        active.feature.evaluate(cmd, &eval).map_err(ngx_error)
    }

    fn flags_for(&self, inputs: &UpscaleInputs<'_>) -> i32 {
        let mut options = self.options.clone();
        options.hdr = inputs.color_space == ColorSpace::Linear;
        options.depth_inverted = inputs.depth_reversed;
        options.feature_flags()
    }
}

/// Records one command buffer with `body`, submits it and waits for it.
fn record_submit<T>(
    ctx: &VkContext,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    body: impl FnOnce(&ash::Device) -> Result<T, UpscaleError>,
) -> Result<T, UpscaleError> {
    let device = &ctx.device;
    let vk_error = |e: vk::Result| unavailable(format!("Vulkan: {e}"));
    // SAFETY: the command buffer, fence and queue belong to this device and are used from one thread at a time; the
    // buffer is ended, submitted once and waited on.
    unsafe {
        device
            .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
            .map_err(vk_error)?;
        device
            .begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(vk_error)?;
        let value = body(device);
        let ended = device.end_command_buffer(cmd);
        let value = value?;
        ended.map_err(vk_error)?;
        let commands = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&commands);
        device
            .reset_fences(&[fence])
            .and_then(|()| device.queue_submit(ctx.queue, &[submit], fence))
            .and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX))
            .map_err(vk_error)?;
        Ok(value)
    }
}

fn make_submission(
    ctx: &VkContext,
) -> Result<(vk::CommandPool, vk::CommandBuffer, vk::Fence), String> {
    // SAFETY: plain creation calls on the live device; the pool is destroyed on error.
    unsafe {
        let pool = ctx
            .device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(ctx.queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(|e| format!("Vulkan: {e}"))?;
        let cmd = ctx.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .command_buffer_count(1),
        );
        let fence = ctx
            .device
            .create_fence(&vk::FenceCreateInfo::default(), None);
        match (cmd, fence) {
            (Ok(cmd), Ok(fence)) => Ok((pool, cmd[0], fence)),
            (cmd, fence) => {
                if let Ok(fence) = fence {
                    ctx.device.destroy_fence(fence, None);
                }
                drop(cmd);
                ctx.device.destroy_command_pool(pool, None);
                Err("Vulkan: could not create a command buffer".into())
            }
        }
    }
}

impl Drop for DlssUpscaler {
    fn drop(&mut self) {
        // SAFETY: the queue is drained first so nothing still uses the objects destroyed here.
        unsafe { self.ctx.device.queue_wait_idle(self.ctx.queue).ok() };
        self.drop_active();
        // SAFETY: as above; the fence and pool are destroyed once (the command buffer goes with the pool).
        unsafe {
            self.ctx.device.destroy_fence(self.fence, None);
            self.ctx.device.destroy_command_pool(self.pool, None);
        }
    }
}

impl Upscaler for DlssUpscaler {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Dlss
    }

    fn name(&self) -> &'static str {
        "NVIDIA DLSS"
    }

    fn uses_jitter(&self) -> bool {
        true
    }

    fn upscale(&mut self, inputs: &UpscaleInputs<'_>) -> Result<Image, UpscaleError> {
        inputs.validate()?;
        let depth = inputs.depth.ok_or(UpscaleError::BadInput {
            what: "depth",
            reason: "DLSS needs a depth buffer".into(),
        })?;
        let motion = inputs.motion.ok_or(UpscaleError::BadInput {
            what: "motion vectors",
            reason: "DLSS needs motion vectors".into(),
        })?;
        let [rw, rh] = inputs.render_size;
        let max = self.options.max_render_size.unwrap_or(inputs.render_size);
        let feature_render = [max[0].max(rw), max[1].max(rh)];
        let new_feature =
            self.prepare_feature(feature_render, inputs.output_size, self.flags_for(inputs))?;
        self.resources_for(feature_render, inputs.output_size)?;

        let color = inputs.color.rgba_f32(rw, rh, "colour")?;
        let depth = depth.channel_f32(0, rw, rh, "depth")?;
        let motion = motion.rg_f32(rw, rh, "motion vectors")?;
        let reset = inputs.reset || new_feature || std::mem::take(&mut self.reset_next);

        let (cmd, fence) = (self.cmd, self.fence);
        let auto_exposure = self.options.auto_exposure;
        let Self { active, ctx, .. } = self;
        let active = active.as_mut().expect("prepare_feature set it");
        let Active {
            feature, resources, ..
        } = active;
        let res = resources.as_ref().expect("resources_for set them");
        // SAFETY: the buffers were allocated with room for the render size; nothing else touches them.
        unsafe {
            let base = res.staging.mapped();
            copy_f32(base.add(res.offsets[0] as usize), &color);
            copy_f32(base.add(res.offsets[1] as usize), &depth);
            copy_f32(base.add(res.offsets[2] as usize), &motion);
            copy_f32(base.add(res.offsets[3] as usize), &[1.0]);
        }
        let color_res = res.color.resource(false);
        let depth_res = res.depth.resource(false);
        let motion_res = res.motion.resource(false);
        let exposure_res = res.exposure.resource(false);
        let output_res = res.output.resource(true);
        let eval = EvalParams {
            color: &color_res,
            depth: &depth_res,
            motion_vectors: &motion_res,
            output: &output_res,
            exposure: (!auto_exposure).then_some(&exposure_res),
            transparency_mask: None,
            render_size: [rw, rh],
            jitter: inputs.jitter,
            motion_vector_scale: inputs.motion_vector_scale,
            reset,
            pre_exposure: inputs.exposure,
            exposure_scale: 1.0,
            sharpness: 0.0,
            frame_time_ms: inputs.frame_time_ms,
        };
        let [ow, oh] = inputs.output_size;
        record_submit(ctx, cmd, fence, |device| {
            let to_dst = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
            let read = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
            let undefined = vk::ImageLayout::UNDEFINED;
            let uploads = [
                (&res.color, res.offsets[0], [rw, rh]),
                (&res.depth, res.offsets[1], [rw, rh]),
                (&res.motion, res.offsets[2], [rw, rh]),
                (&res.exposure, res.offsets[3], [1, 1]),
            ];
            for (image, offset, size) in uploads {
                transition(device, cmd, image, undefined, to_dst);
                let region = vk::BufferImageCopy::default()
                    .buffer_offset(offset)
                    .image_subresource(color_layers())
                    .image_extent(vk::Extent3D {
                        width: size[0],
                        height: size[1],
                        depth: 1,
                    });
                // SAFETY: `cmd` is recording; buffer and image belong to this device.
                unsafe {
                    device.cmd_copy_buffer_to_image(
                        cmd,
                        res.staging.buffer,
                        image.image,
                        to_dst,
                        &[region],
                    );
                }
                transition(device, cmd, image, to_dst, read);
            }
            transition(
                device,
                cmd,
                &res.output,
                undefined,
                vk::ImageLayout::GENERAL,
            );
            feature.evaluate(cmd, &eval).map_err(ngx_error)?;
            transition(
                device,
                cmd,
                &res.output,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(color_layers())
                .image_extent(vk::Extent3D {
                    width: ow,
                    height: oh,
                    depth: 1,
                });
            // SAFETY: as above.
            unsafe {
                device.cmd_copy_image_to_buffer(
                    cmd,
                    res.output.image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    res.readback.buffer,
                    &[region],
                );
                let barrier = vk::BufferMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(res.readback.buffer)
                    .size(vk::WHOLE_SIZE);
                device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[barrier],
                    &[],
                );
            }
            Ok(())
        })?;
        // SAFETY: the readback buffer is host-visible and coherent, was written by the finished submission and holds
        // at least this many bytes.
        let bytes = unsafe {
            std::slice::from_raw_parts(res.readback.mapped(), ow as usize * oh as usize * 8)
        };
        if inputs.output_format == PixelFormat::Rgba16Float {
            return Ok(Image {
                width: ow,
                height: oh,
                format: PixelFormat::Rgba16Float,
                data: bytes.to_vec(),
            });
        }
        let rgba = crate::ImageRef::new(ow, oh, PixelFormat::Rgba16Float, bytes).rgba_f32(
            ow,
            oh,
            "DLSS output",
        )?;
        Image::from_rgba_f32(ow, oh, inputs.output_format, &rgba)
    }

    fn reset(&mut self) {
        self.reset_next = true;
    }
}

/// Copies floats into mapped memory.
///
/// # Safety
/// `dst` must have room for `values.len()` floats.
unsafe fn copy_f32(dst: *mut u8, values: &[f32]) {
    // SAFETY: the caller guarantees room; the source and destination cannot overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(
            values.as_ptr().cast::<u8>(),
            dst,
            std::mem::size_of_val(values),
        );
    }
}

fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    }
}
