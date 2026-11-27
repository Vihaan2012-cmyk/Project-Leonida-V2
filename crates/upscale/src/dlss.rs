//! Optional NVIDIA DLSS through NVIDIA's NGX runtime on the player's own machine. Nothing of NVIDIA's is linked,
//! bundled or shipped.
//!
//! The player supplies NVIDIA's files: the driver installs `nvngx.dll` (the NGX loader) in the system folder, and the
//! player points the configuration at a DLSS library (`nvngx_dlss.dll`) they obtained from NVIDIA. DLSS only works on
//! NVIDIA GPUs with tensor cores (GeForce RTX); on anything else, or without those files, it reports unavailable and
//! the caller falls back to [`crate::fsr1`] or [`crate::temporal`].
//!
//! What is done: [`probe`] loads the driver's NGX loader (from the system folder only, unless the configuration names
//! another path), resolves the public `NVSDK_NGX_VULKAN_*` entry points by name and checks the configured DLSS file
//! is a Windows library. The DLSS library itself is never loaded by us: NGX loads it from the path the application
//! hands it.
//!
//! What is not done: evaluating DLSS. That needs the renderer's `VkInstance`/`VkDevice` (not visible through
//! [`crate::Compute`]) and NGX's `NVSDK_NGX_Parameter`, which NVIDIA's header declares as a C++ abstract class with
//! overloaded virtual `Set`/`Get` methods; its virtual-table order is not stated by NVIDIA's public documentation, so
//! calling it would be a guess. [`DlssUpscaler::new`] therefore returns [`UpscaleError::Unimplemented`] when the
//! runtime is present, and [`UpscaleError::Unavailable`] when it is not. DLSS frame generation goes through
//! Streamline's swap-chain proxy and is likewise not offered.

use std::path::{Path, PathBuf};

use crate::{Image, UpscaleError, UpscaleInputs, Upscaler, UpscalerKind};

/// The NGX entry points whose presence [`probe`] checks (names from NVIDIA's public `nvsdk_ngx_vk.h`).
pub const ENTRY_POINTS: &[&str] = &[
    "NVSDK_NGX_VULKAN_Init",
    "NVSDK_NGX_VULKAN_Shutdown",
    "NVSDK_NGX_VULKAN_GetCapabilityParameters",
    "NVSDK_NGX_VULKAN_CreateFeature",
    "NVSDK_NGX_VULKAN_EvaluateFeature",
    "NVSDK_NGX_VULKAN_ReleaseFeature",
];

#[derive(Debug, Clone, Default)]
pub struct DlssConfig {
    /// The DLSS library the player points to (`nvngx_dlss.dll`).
    pub dlss_dll: Option<PathBuf>,
    /// Where to find the NGX loader when it is not in the system folder.
    pub ngx_runtime: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlssStatus {
    /// The loader and its entry points are present and a DLSS file is configured.
    Present {
        ngx: PathBuf,
        dlss: PathBuf,
    },
    Unavailable(String),
}

/// Looks for NVIDIA's NGX runtime and the configured DLSS file. Loads the NGX loader and unloads it again.
pub fn probe(config: &DlssConfig) -> DlssStatus {
    let Some(dlss) = &config.dlss_dll else {
        return DlssStatus::Unavailable("no DLSS library is configured".into());
    };
    if let Err(reason) = check_dlss_file(dlss) {
        return DlssStatus::Unavailable(reason);
    }
    match loader::open(config.ngx_runtime.as_deref()) {
        Ok(ngx) => DlssStatus::Present {
            ngx: ngx.path.clone(),
            dlss: dlss.clone(),
        },
        Err(reason) => DlssStatus::Unavailable(reason),
    }
}

fn check_dlss_file(path: &Path) -> Result<(), String> {
    let mut head = [0u8; 2];
    let read = std::fs::File::open(path).and_then(|mut file| {
        std::io::Read::read_exact(&mut file, &mut head)?;
        Ok(())
    });
    match read {
        Err(error) => Err(format!("{}: {error}", path.display())),
        Ok(()) if &head != b"MZ" => Err(format!("{} is not a Windows library", path.display())),
        Ok(()) => Ok(()),
    }
}

/// DLSS super resolution. It cannot be created yet, see the module documentation.
pub struct DlssUpscaler {
    _private: (),
}

impl DlssUpscaler {
    pub fn new(config: &DlssConfig) -> Result<DlssUpscaler, UpscaleError> {
        match probe(config) {
            DlssStatus::Unavailable(reason) => Err(UpscaleError::Unavailable {
                name: "DLSS",
                reason,
            }),
            DlssStatus::Present { .. } => Err(UpscaleError::Unimplemented {
                name: "DLSS",
                reason: "NGX evaluation needs the renderer's Vulkan device and NVIDIA's \
                         NVSDK_NGX_Parameter C++ interface, whose layout public documentation does not give"
                    .into(),
            }),
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

    fn upscale(&mut self, _inputs: &UpscaleInputs<'_>) -> Result<Image, UpscaleError> {
        Err(UpscaleError::Unimplemented {
            name: "DLSS",
            reason: "see DlssUpscaler::new".into(),
        })
    }

    fn reset(&mut self) {}
}

#[cfg(windows)]
mod loader {
    use std::ffi::{CString, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
    };

    pub struct Ngx {
        module: *mut c_void,
        pub path: PathBuf,
    }

    impl Drop for Ngx {
        fn drop(&mut self) {
            // SAFETY: `module` came from a successful LoadLibraryExW and is released once.
            unsafe { FreeLibrary(self.module) };
        }
    }

    pub fn open(explicit: Option<&Path>) -> Result<Ngx, String> {
        let (name, flags, path) = match explicit {
            Some(path) => (
                path.to_path_buf(),
                LOAD_WITH_ALTERED_SEARCH_PATH,
                path.to_path_buf(),
            ),
            None => (
                PathBuf::from("nvngx.dll"),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
                PathBuf::from("nvngx.dll (system folder)"),
            ),
        };
        let wide: Vec<u16> = name.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; no file handle is passed.
        let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), flags) };
        if module.is_null() {
            return Err(format!(
                "NVIDIA's NGX runtime was not found at {} (needs an NVIDIA driver)",
                path.display()
            ));
        }
        let ngx = Ngx { module, path };
        let missing: Vec<&str> = super::ENTRY_POINTS
            .iter()
            .copied()
            .filter(|name| {
                let name = CString::new(*name).expect("entry point names have no NUL");
                // SAFETY: `module` is a loaded library and `name` is NUL-terminated.
                unsafe { GetProcAddress(ngx.module, name.as_ptr().cast()) }.is_none()
            })
            .collect();
        if missing.is_empty() {
            Ok(ngx)
        } else {
            Err(format!(
                "{} lacks the NGX entry points {}",
                ngx.path.display(),
                missing.join(", ")
            ))
        }
    }
}

#[cfg(not(windows))]
mod loader {
    use std::path::{Path, PathBuf};

    pub struct Ngx {
        pub path: PathBuf,
    }

    pub fn open(_explicit: Option<&Path>) -> Result<Ngx, String> {
        Err("DLSS needs Windows and an NVIDIA driver".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_configured_library_dlss_is_unavailable() {
        let status = probe(&DlssConfig::default());
        assert!(matches!(status, DlssStatus::Unavailable(r) if r.contains("no DLSS library")));
        assert!(matches!(
            DlssUpscaler::new(&DlssConfig::default()),
            Err(UpscaleError::Unavailable { name: "DLSS", .. })
        ));
    }
}
