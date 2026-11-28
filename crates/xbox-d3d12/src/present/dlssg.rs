//! NVIDIA DLSS frame generation as the generator behind [`super::generate::FrameGen`]: the same captured frames and
//! the same pacing, with NGX making the frame between two real ones instead of the passes of `upscale::framegen`.
//!
//! What the presenter's device gives NGX per real frame: the title's final colour, held as a half-float image of our own
//! (the same conversion the own generator makes, so an sRGB back buffer arrives linear), the title's depth and motion
//! vector images as the upscaler dispatch described them, and a half-float image to write the generated frame to. The
//! HUD is part of the colour: the title composes it into the image it presents and the HUD-less scene is not kept.

use std::path::Path;
use std::sync::Arc;

use ash::vk;
use upscale::dlss::DlssOptions;
use upscale::dlss::dlssg::params::{self, EvalOptions, Resources};
use upscale::dlss::dlssg::{CreateParams, Session};
use upscale::dlss::ffi::ResourceVk;
use upscale::dlss::ngx::{Api, VkInit};

use super::vulkan::VulkanContext;

/// Vertical field of view used when the game's dispatch gave none (60 degrees).
const DEFAULT_FOV_Y: f32 = std::f32::consts::FRAC_PI_3;

/// NGX initialised on the presenter's device, and the feature made for the current sizes.
pub(super) struct DlssState {
    session: Session,
}

/// One image NGX is handed: where it is and how it is described.
#[derive(Clone, Copy)]
pub(super) struct Image {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub format: vk::Format,
    pub aspect: vk::ImageAspectFlags,
    pub size: [u32; 2],
}

impl Image {
    fn resource(&self, read_write: bool) -> ResourceVk {
        ResourceVk::image_view(
            self.view,
            self.image,
            vk::ImageSubresourceRange {
                aspect_mask: self.aspect,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            self.format,
            self.size[0],
            self.size[1],
            read_write,
        )
    }
}

/// The images and the frame's facts for one evaluation.
pub(super) struct Frame<'a> {
    pub color: Image,
    pub depth: Image,
    pub motion: Image,
    pub output: Image,
    pub render_size: [u32; 2],
    pub inputs: &'a upscale::framegen::FrameInputs,
    pub reset: bool,
}

impl DlssState {
    /// Initialises NGX on the presenter's device. `library_dir` holds `nvngx_dlssg.dll` when the player gave one.
    pub(super) fn open(
        ctx: &VulkanContext,
        library_dir: Option<&Path>,
    ) -> Result<DlssState, String> {
        let api = Arc::new(Api::open(None)?);
        let init = VkInit {
            instance: ctx.instance.handle(),
            physical: ctx.physical,
            device: ctx.device.handle(),
            get_instance_proc_addr: ctx.entry.static_fn().get_instance_proc_addr,
            get_device_proc_addr: ctx.instance.fp_v1_0().get_device_proc_addr,
        };
        let session = Session::new(api, &init, &DlssOptions::default(), library_dir)
            .map_err(|e| e.to_string())?;
        Ok(DlssState { session })
    }

    /// Makes the feature for these sizes unless it exists, submitting NGX's setup commands and waiting for them. The
    /// device is idle when the sizes changed (the generator waits before it frees its images).
    pub(super) fn prepare(
        &mut self,
        ctx: &VulkanContext,
        pool: vk::CommandPool,
        size: [u32; 2],
        render: [u32; 2],
        format: vk::Format,
    ) -> Result<(), String> {
        let create = CreateParams::new(size, format, render);
        if self.session.created_for() == Some(&create) {
            return Ok(());
        }
        let device = &ctx.device;
        // SAFETY: a one-time command buffer from the generator's pool, submitted under the queue lock and waited on
        // before it is freed; NGX records into it only while it is recording.
        unsafe {
            let cmd = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .command_buffer_count(1),
                )
                .map_err(|e| e.to_string())?[0];
            let fence = device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|e| e.to_string())?;
            let outcome = (|| -> Result<(), String> {
                device
                    .begin_command_buffer(
                        cmd,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                    )
                    .map_err(|e| e.to_string())?;
                let made = self.session.prepare(cmd, &create);
                device.end_command_buffer(cmd).map_err(|e| e.to_string())?;
                made.map_err(|e| e.to_string())?;
                let cmds = [cmd];
                let queue = ctx.queue.lock().unwrap_or_else(|e| e.into_inner());
                device
                    .queue_submit(
                        *queue,
                        &[vk::SubmitInfo::default().command_buffers(&cmds)],
                        fence,
                    )
                    .map_err(|e| e.to_string())?;
                drop(queue);
                device
                    .wait_for_fences(&[fence], true, u64::MAX)
                    .map_err(|e| e.to_string())
            })();
            device.destroy_fence(fence, None);
            device.free_command_buffers(pool, &[cmd]);
            outcome
        }
    }

    /// Records the evaluation into `cmd`. The colour and depth/motion images must be readable by a shader and the
    /// output in `GENERAL`.
    pub(super) fn evaluate(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: &Frame<'_>,
    ) -> Result<(), String> {
        let (color, depth, motion, output) = (
            frame.color.resource(false),
            frame.depth.resource(false),
            frame.motion.resource(false),
            frame.output.resource(true),
        );
        let resources = Resources {
            backbuffer: Some(&color),
            depth: Some(&depth),
            motion_vectors: Some(&motion),
            output_interpolated: Some(&output),
            ..Resources::default()
        };
        self.session
            .evaluate(cmd, &resources, &options(frame))
            .map_err(|e| e.to_string())
    }
}

/// The engine data of a frame from what the upscaler dispatch told us.
fn options(frame: &Frame<'_>) -> EvalOptions {
    let inputs = frame.inputs;
    let fov = if inputs.camera_fov_y.is_finite() && inputs.camera_fov_y > 0.0 {
        inputs.camera_fov_y
    } else {
        DEFAULT_FOV_Y
    };
    let near = if inputs.camera_near > 0.0 {
        inputs.camera_near
    } else {
        0.1
    };
    let far = if inputs.camera_far > near {
        inputs.camera_far
    } else {
        f32::INFINITY
    };
    let render = frame.render_size;
    let mut options =
        params::options_from_projection(frame.color.size, fov, near, far, inputs.depth_reversed);
    options.reset = frame.reset;
    options.mvec_scale = inputs.motion_vector_scale;
    options.jitter_offset = [
        2.0 * inputs.jitter[0] / render[0].max(1) as f32,
        2.0 * inputs.jitter[1] / render[1].max(1) as f32,
    ];
    let subrect = |image: &Image| {
        if image.size == render {
            params::Subrect::default()
        } else {
            params::Subrect {
                x: 0,
                y: 0,
                width: render[0],
                height: render[1],
            }
        }
    };
    options.depth_subrect = subrect(&frame.depth);
    options.mvecs_subrect = subrect(&frame.motion);
    options
}

static LIBRARY_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// The folder holding the player's `nvngx_dlssg.dll`; `None` leaves it to the search in
/// `upscale::dlss::dlssg::support::locate`.
pub fn set_library_dir(dir: Option<std::path::PathBuf>) {
    *LIBRARY_DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// Where NGX is told to look for `nvngx_dlssg.dll`.
pub(super) fn library_dir() -> Option<std::path::PathBuf> {
    let set = LIBRARY_DIR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    set.or_else(|| {
        upscale::dlss::dlssg::support::locate(None)
            .and_then(|found| found.path.parent().map(Path::to_path_buf))
    })
}
