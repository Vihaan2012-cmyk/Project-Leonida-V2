//! NVIDIA DLSS frame generation through NGX on Vulkan: a frame between two real ones, made from the game's final
//! colour, depth, motion vectors and camera.
//!
//! What it needs (NVIDIA's DLSS-FG programming guide and headers, SDK 310.9; NVIDIA's files are never bundled):
//!
//! - A GeForce RTX 40 series GPU or newer, NVIDIA driver 520 or newer, Vulkan 1.1 or newer. [`support::Capabilities`]
//!   reads the verdict from NGX (`FrameGeneration.Available`) and [`support::Capabilities::reason`] says why not.
//! - The player's `nvngx_dlssg.dll` ([`support::locate`]; the driver store has one NGX also finds on its own).
//! - Vulkan extensions: the ones `NVSDK_NGX_VULKAN_RequiredExtensions` lists plus the instance and device extensions
//!   `NVSDK_NGX_VULKAN_GetFeature{Instance,Device}ExtensionRequirements` name for the feature
//!   ([`required_extensions`]); the device must enable them when it is created.
//! - A command buffer that is recording for creation and for every evaluation; NGX changes its state, and the guide
//!   states no queue, semaphore or layout rule beyond that and "the resources are in a state suitable for reading
//!   (outputs for writing)". Here inputs are in `SHADER_READ_ONLY_OPTIMAL` and the output in `GENERAL`, and the queue
//!   the commands go to must be a graphics or compute queue of the device NGX was initialised on.
//! - NGX is not thread-safe: every call runs under the lock `dlss::lock::ngx_lock` that super resolution uses too.
//!
//! Per real frame ([`session::Session::evaluate`]): the final colour of the newest frame (the feature keeps the one
//! before itself), depth and motion vectors as they were given to the upscaler, the camera and a reset flag; the output
//! is the interpolated frame, same size and format as the colour. HUD-less colour and UI are optional and improve the
//! picture where the HUD moves differently from the scene; without them the HUD is part of the colour. [`pacing`] says
//! how many frames come out per real frame and when they are shown.

pub mod names;
pub mod pacing;
pub mod params;
pub mod session;
pub mod support;

#[cfg(test)]
mod tests;

use ash::vk;

use super::DlssOptions;
use super::ngx::{Api, VkInit};
use super::vk::VkContext;
use crate::dlss::ngx::NgxError;

pub use params::{CreateParams, EvalOptions, Resources};
pub use session::Session;
pub use support::{Capabilities, Discovery};

/// The Vulkan extensions a device needs for frame generation: what NGX names for itself plus what the feature adds.
/// `physical` is the GPU the device will be made on.
pub fn required_extensions(
    api: &Api,
    options: &DlssOptions,
    instance: Option<(vk::Instance, vk::PhysicalDevice)>,
) -> Result<super::RequiredExtensions, NgxError> {
    let base = api.required_extensions()?;
    let discovery = Discovery::new(options);
    let feature_instance = api.frame_generation_instance_extensions(&discovery)?;
    let feature_device = match instance {
        Some((instance, physical)) => {
            api.frame_generation_device_extensions(&discovery, instance, physical)?
        }
        None => Vec::new(),
    };
    Ok(super::RequiredExtensions {
        instance: support::union(&base.instance, &feature_instance),
        device: support::union(&base.device, &feature_device),
    })
}

/// An NVIDIA Vulkan device with every extension NGX and the feature name enabled, for a caller that has no renderer
/// of its own (tests, the capability report).
pub fn open_device(api: &Api, options: &DlssOptions) -> Result<VkContext, String> {
    let first = required_extensions(api, options, None).map_err(|e| e.to_string())?;
    let probe = VkContext::create_nvidia(&first.instance, &[])?;
    let wanted = required_extensions(
        api,
        options,
        Some((probe.instance.handle(), probe.physical)),
    )
    .map_err(|e| e.to_string());
    drop(probe);
    let wanted = wanted?;
    VkContext::create_nvidia(&wanted.instance, &wanted.device)
}

/// The handles NGX is initialised with, taken from a context.
pub fn vk_init(ctx: &VkContext) -> VkInit {
    VkInit {
        instance: ctx.instance.handle(),
        physical: ctx.physical,
        device: ctx.device.handle(),
        get_instance_proc_addr: ctx.entry.static_fn().get_instance_proc_addr,
        get_device_proc_addr: ctx.instance.fp_v1_0().get_device_proc_addr,
    }
}

/// What frame generation can do on this machine, for a message to the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub available: bool,
    /// One sentence: where it runs, or why it cannot.
    pub summary: String,
    /// The folder holding `nvngx_dlssg.dll` when one was found.
    pub library_dir: Option<std::path::PathBuf>,
}

/// Opens an NVIDIA device, initialises NGX on it and asks whether frame generation is available. Needs no renderer and
/// takes a second or two. `dlss_dll` is the player's `nvngx_dlss.dll` setting, whose folder is searched for
/// `nvngx_dlssg.dll` too.
pub fn report(dlss_dll: Option<&std::path::Path>) -> Report {
    let located = support::locate(dlss_dll);
    let library_dir = located
        .as_ref()
        .and_then(|l| l.path.parent().map(std::path::Path::to_path_buf));
    let options = DlssOptions::default();
    let outcome = (|| -> Result<String, String> {
        let api = std::sync::Arc::new(Api::open(None)?);
        let ctx = open_device(&api, &options)?;
        let session = Session::new(api, &vk_init(&ctx), &options, library_dir.as_deref())
            .map_err(|e| e.to_string())?;
        let multiplier = session.capabilities().max_multiplier();
        Ok(format!(
            "available on {} (up to {multiplier}x, nvngx_dlssg.dll from {})",
            ctx.device_name,
            located
                .as_ref()
                .map_or("NVIDIA's driver".to_string(), |l| format!(
                    "{}: {}",
                    support::describe(l.source),
                    l.path.display()
                ))
        ))
    })();
    match outcome {
        Ok(summary) => Report {
            available: true,
            summary,
            library_dir,
        },
        Err(reason) => Report {
            available: false,
            summary: reason,
            library_dir,
        },
    }
}
