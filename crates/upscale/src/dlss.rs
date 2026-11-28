//! Optional NVIDIA DLSS super resolution through NGX, the runtime that comes with NVIDIA's driver. Nothing of
//! NVIDIA's is linked, bundled or shipped.
//!
//! The player supplies NVIDIA's files: the driver installs the NGX loader (`nvngx.dll`), and the player puts a DLSS
//! library (`nvngx_dlss.dll`) they obtained from NVIDIA where [`locate::locate`] looks (the `dlss_path` setting,
//! NVIDIA's NGX folders, next to the executable). DLSS only works on NVIDIA GPUs with
//! tensor cores (GeForce RTX); anywhere else, or when NGX fails, [`create_with_fallback`] hands back FSR 1 or the
//! temporal upscaler together with a message saying why.
//!
//! What runs:
//! - [`required_extensions`]: the instance and device extensions NGX names; the renderer's device must enable them.
//! - [`DlssUpscaler`]: NGX initialisation, the DLSS availability query, the optimal-settings query
//!   ([`DlssUpscaler::optimal_settings`], so the game's render size can follow the quality mode), feature creation,
//!   evaluation and release, behind the crate's [`Upscaler`] trait (CPU images in and out) and through
//!   [`DlssUpscaler::evaluate_vk`] for images the renderer keeps on the GPU.
//! - [`DlssMode`]: the game's quality modes mapped to DLSS modes and DLAA.
//!
//! NGX's parameter map is a C++ interface (`NVSDK_NGX_Parameter`); the driver exports no C wrappers for it, so the
//! `Set` and `Get` overloads are called through the virtual table in the order NVIDIA's public header declares them
//! (see [`ffi::slots`]). The declarations in [`ffi`] are our own, written from NVIDIA's public SDK headers
//! (<https://github.com/NVIDIA/DLSS>, NVIDIA RTX SDK licence): no NVIDIA file is copied or distributed.

pub mod fallback;
pub mod ffi;
pub mod locate;
pub mod modes;
pub mod ngx;
mod upscaler;
pub mod vk;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

pub use fallback::{Selection, create_with_fallback, select_upscaler};
pub use modes::{DlssMode, DlssOptions};
pub use ngx::{Availability, OptimalSettings};
pub use upscaler::DlssUpscaler;

/// The NGX entry points whose presence [`probe`] checks (names from NVIDIA's public `nvsdk_ngx_vk.h`).
pub use ngx::REQUIRED_EXPORTS as ENTRY_POINTS;

/// The file name NGX looks for in the folders it is given.
pub const DLSS_LIBRARY_NAME: &str = "nvngx_dlss.dll";

#[derive(Debug, Clone, Default)]
pub struct DlssConfig {
    /// The DLSS library the player points to (`nvngx_dlss.dll`); when `None` the places [`locate::locate`] names are
    /// searched.
    pub dlss_dll: Option<PathBuf>,
    /// Where to find the NGX loader when it is not in the system folder.
    pub ngx_runtime: Option<PathBuf>,
    /// Replaces the default search places (NGX's folders, the executable's folder) when `Some`.
    pub search_folders: Option<Vec<PathBuf>>,
}

impl DlssConfig {
    /// The DLSS library this configuration finds, or why there is none.
    pub fn locate(&self) -> Result<locate::Located, locate::NotFound> {
        locate::locate(self.dlss_dll.as_deref(), self.search_folders.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlssStatus {
    /// The loader and its entry points are present and a DLSS file was found.
    Present {
        ngx: PathBuf,
        dlss: PathBuf,
        source: locate::Source,
    },
    Unavailable(String),
}

/// Looks for NVIDIA's NGX runtime and the DLSS file. Loads the NGX loader and unloads it again.
pub fn probe(config: &DlssConfig) -> DlssStatus {
    let located = match config.locate() {
        Ok(located) => located,
        Err(missing) => return DlssStatus::Unavailable(missing.to_string()),
    };
    match ngx::Api::open(config.ngx_runtime.as_deref()) {
        Ok(api) => DlssStatus::Present {
            ngx: api.library.path.clone(),
            dlss: located.path,
            source: located.source,
        },
        Err(reason) => DlssStatus::Unavailable(reason),
    }
}

/// The Vulkan extensions NGX needs enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequiredExtensions {
    pub instance: Vec<String>,
    pub device: Vec<String>,
}

/// Asks the NGX loader which Vulkan extensions it needs, using the loader in the system folder. Empty when NGX is not
/// present (no NVIDIA driver): the device then needs nothing for DLSS, which will not be offered anyway.
pub fn required_extensions() -> RequiredExtensions {
    required_extensions_with(&DlssConfig::default()).unwrap_or_default()
}

/// As [`required_extensions`], for a configured loader location, and with the reason when NGX cannot be asked.
pub fn required_extensions_with(config: &DlssConfig) -> Result<RequiredExtensions, String> {
    let api = ngx::Api::open(config.ngx_runtime.as_deref())?;
    let lists = api.required_extensions().map_err(|e| e.to_string())?;
    Ok(RequiredExtensions {
        instance: lists.instance,
        device: lists.device,
    })
}

/// One DLSS mode and what NGX says about it for an output size; `settings` is `None` when NGX could not be asked.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeSupport {
    pub mode: DlssMode,
    pub settings: Option<OptimalSettings>,
}

impl ModeSupport {
    /// Whether NGX offers the mode (it reports a zero render size for modes it does not).
    pub fn offered(&self) -> bool {
        self.settings
            .is_some_and(|s| s.render_size[0] > 0 && s.render_size[1] > 0)
    }
}

/// What NGX says about DLSS on this machine, without the game's renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct DlssCapabilities {
    pub device_name: String,
    pub availability: Availability,
    pub modes: Vec<ModeSupport>,
}

/// Creates an NVIDIA Vulkan device with NGX's extensions, initialises NGX on it and asks whether DLSS is available and
/// which render size each mode gives for `output`. Needs no DLSS library path: NGX also looks in its own locations.
pub fn query_capabilities(
    config: &DlssConfig,
    output: [u32; 2],
) -> Result<DlssCapabilities, String> {
    let options = DlssOptions::default();
    let located = config.locate().ok();
    let opened = upscaler::open_own_device(config.ngx_runtime.as_deref())?;
    let mut ngx = upscaler::init_ngx(
        opened.api,
        &opened.ctx,
        &options,
        located.as_ref().map(|l| l.path.as_path()),
    )?;
    let availability = ngx.availability();
    let modes = DlssMode::ALL
        .iter()
        .map(|&mode| ModeSupport {
            mode,
            settings: ngx.optimal_settings(output, mode).ok(),
        })
        .collect();
    Ok(DlssCapabilities {
        device_name: opened.ctx.device_name.clone(),
        availability,
        modes,
    })
}

/// Whether DLSS can run on this machine and, when it cannot, the reason in a sentence for the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlssReport {
    pub available: bool,
    pub summary: String,
}

/// Looks for the DLSS library and NGX, opens an NVIDIA device and asks NGX whether DLSS is available. Needs no
/// renderer; takes a second or two because it creates a Vulkan device.
pub fn report(config: &DlssConfig) -> DlssReport {
    let unavailable = |reason: String| DlssReport {
        available: false,
        summary: format!("unavailable: {reason}"),
    };
    let located = match config.locate() {
        Ok(located) => located,
        Err(missing) => return unavailable(missing.to_string()),
    };
    if let Err(reason) = ngx::Api::open(config.ngx_runtime.as_deref()) {
        return unavailable(reason);
    }
    match query_capabilities(config, [1920, 1080]) {
        Err(reason) => unavailable(reason),
        Ok(capabilities) => match capabilities.availability.reason() {
            Some(reason) => unavailable(format!("{reason} ({})", capabilities.device_name)),
            None => DlssReport {
                available: true,
                summary: format!(
                    "available on {} (nvngx_dlss.dll from {}: {})",
                    capabilities.device_name,
                    located.source.describe(),
                    located.path.display()
                ),
            },
        },
    }
}
