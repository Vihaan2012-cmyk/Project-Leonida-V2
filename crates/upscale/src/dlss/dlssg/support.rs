//! What NGX says about frame generation on this machine: the capability map, the Vulkan extensions the feature wants
//! beyond the ones NGX itself names, and where NVIDIA's `nvngx_dlssg.dll` is.

use std::ffi::{CString, c_char};
use std::path::{Path, PathBuf};

use ash::vk;

use super::names;
use crate::dlss::ffi;
use crate::dlss::locate::Source;
use crate::dlss::ngx::{Api, NgxError, ParamMap};

/// The feature library NGX loads for frame generation.
pub const LIBRARY_NAME: &str = "nvngx_dlssg.dll";

/// Whether frame generation can run here, from the capability parameter map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub available: bool,
    pub needs_updated_driver: bool,
    pub min_driver: Option<(u32, u32)>,
    pub feature_init_result: Option<u32>,
    /// The most frames NGX can generate between two real ones: 1 means 2x only, 3 means up to 4x.
    pub multi_frame_count_max: u32,
}

impl Capabilities {
    pub fn read(map: &ParamMap) -> Capabilities {
        Capabilities {
            available: map.get_i32(names::AVAILABLE).is_some_and(|v| v != 0),
            needs_updated_driver: map
                .get_i32(names::NEEDS_UPDATED_DRIVER)
                .is_some_and(|v| v != 0),
            min_driver: map
                .get_u32(names::MIN_DRIVER_MAJOR)
                .zip(map.get_u32(names::MIN_DRIVER_MINOR))
                .filter(|(major, _)| *major != 0),
            feature_init_result: map.get_i32(names::FEATURE_INIT_RESULT).map(|v| v as u32),
            multi_frame_count_max: map
                .get_u32(names::MULTI_FRAME_COUNT_MAX)
                .unwrap_or(1)
                .max(1),
        }
    }

    /// The largest multiplier NGX offers (2 when only one frame is generated between real ones).
    pub fn max_multiplier(&self) -> u32 {
        self.multi_frame_count_max + 1
    }

    /// A sentence saying why frame generation is not available, when it is not.
    pub fn reason(&self) -> Option<String> {
        if self.available {
            return None;
        }
        if self.needs_updated_driver {
            return Some(match self.min_driver {
                Some((major, minor)) => format!(
                    "the NVIDIA driver is too old for DLSS frame generation (needs {major}.{minor} or newer)"
                ),
                None => "the NVIDIA driver is too old for DLSS frame generation".into(),
            });
        }
        Some(match self.feature_init_result {
            Some(result) if ffi::failed(result) => format!(
                "NGX could not initialise DLSS frame generation: {} ({result:#010x}); it needs a GeForce RTX 40 series GPU or newer",
                ffi::result_name(result)
            ),
            _ => "NGX reports DLSS frame generation as unavailable on this GPU (it needs a GeForce RTX 40 series GPU or newer)".into(),
        })
    }
}

/// The Vulkan extensions frame generation wants enabled in addition to what [`Api::required_extensions`] lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureExtensions {
    pub instance: Vec<String>,
    pub device: Vec<String>,
}

/// The identity NGX is told about when it is asked what a feature needs: a project id of our own.
pub struct Discovery {
    project: CString,
    engine: CString,
    data_path: Vec<u16>,
}

impl Discovery {
    pub fn new(options: &crate::dlss::DlssOptions) -> Discovery {
        Discovery {
            project: CString::new(options.project_id.as_str()).unwrap_or_default(),
            engine: CString::new(options.engine_version.as_str()).unwrap_or_default(),
            data_path: std::env::temp_dir()
                .to_string_lossy()
                .replace('/', "\\")
                .encode_utf16()
                .chain([0])
                .collect(),
        }
    }

    fn info(&self) -> ffi::FeatureDiscoveryInfo {
        ffi::FeatureDiscoveryInfo {
            sdk_version: ffi::VERSION_API,
            feature_id: names::FEATURE_FRAME_GENERATION,
            identifier: ffi::ApplicationIdentifier {
                identifier_type: ffi::IDENTIFIER_PROJECT_ID,
                v: ffi::IdentifierValue {
                    project_desc: ffi::ProjectIdDescription {
                        project_id: self.project.as_ptr(),
                        engine_type: ffi::ENGINE_TYPE_CUSTOM,
                        engine_version: self.engine.as_ptr(),
                    },
                },
            },
            application_data_path: self.data_path.as_ptr(),
            feature_info: std::ptr::null(),
        }
    }
}

fn copy_names(count: u32, list: *mut vk::ExtensionProperties) -> Vec<String> {
    if list.is_null() {
        return Vec::new();
    }
    (0..count as usize)
        .map(|i| {
            // SAFETY: NGX reported `count` entries, each a `VkExtensionProperties` whose name is NUL-terminated.
            let properties = unsafe { &*list.add(i) };
            // SAFETY: Vulkan name arrays are NUL-terminated.
            unsafe { std::ffi::CStr::from_ptr(properties.extension_name.as_ptr().cast::<c_char>()) }
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

impl Api {
    /// The instance extensions frame generation needs beyond `RequiredExtensions`. Asks NGX before it is initialised.
    /// Empty when the driver exports no such query: NVIDIA's driver library (616.64) exports only
    /// `NVSDK_NGX_VULKAN_RequiredExtensions`, whose list was enough to create and run frame generation on an RTX 4060.
    pub fn frame_generation_instance_extensions(
        &self,
        discovery: &Discovery,
    ) -> Result<Vec<String>, NgxError> {
        let Some(function) = self.get_instance_extension_requirements else {
            return Ok(Vec::new());
        };
        let info = discovery.info();
        let (mut count, mut list) = (0u32, std::ptr::null_mut());
        let _turn = super::turn::ngx_turn();
        // SAFETY: `info` and its strings live for the call; the out-pointers are valid; NGX owns the array it returns.
        let result = unsafe { function(&info, &mut count, &mut list) };
        if ffi::failed(result) {
            return Err(NgxError::Call {
                call: "NVSDK_NGX_VULKAN_GetFeatureInstanceExtensionRequirements",
                result,
            });
        }
        Ok(copy_names(count, list))
    }

    /// The device extensions frame generation needs on `physical`.
    pub fn frame_generation_device_extensions(
        &self,
        discovery: &Discovery,
        instance: vk::Instance,
        physical: vk::PhysicalDevice,
    ) -> Result<Vec<String>, NgxError> {
        let Some(function) = self.get_device_extension_requirements else {
            return Ok(Vec::new());
        };
        let info = discovery.info();
        let (mut count, mut list) = (0u32, std::ptr::null_mut());
        let _turn = super::turn::ngx_turn();
        // SAFETY: as above; the handles belong to the caller's live instance.
        let result = unsafe { function(instance, physical, &info, &mut count, &mut list) };
        if ffi::failed(result) {
            return Err(NgxError::Call {
                call: "NVSDK_NGX_VULKAN_GetFeatureDeviceExtensionRequirements",
                result,
            });
        }
        Ok(copy_names(count, list))
    }

    /// Whether the driver supports frame generation on `physical`, asked before NGX is initialised.
    pub fn frame_generation_requirement(
        &self,
        discovery: &Discovery,
        instance: vk::Instance,
        physical: vk::PhysicalDevice,
    ) -> Result<ffi::FeatureRequirement, NgxError> {
        let function = self.get_feature_requirements.ok_or_else(|| {
            NgxError::Runtime("NVSDK_NGX_VULKAN_GetFeatureRequirements is not exported".into())
        })?;
        let info = discovery.info();
        let mut requirement = ffi::FeatureRequirement {
            feature_supported: ffi::FEATURE_CHECK_NOT_PRESENT,
            min_hw_architecture: 0,
            min_os_version: [0; 255],
        };
        let _turn = super::turn::ngx_turn();
        // SAFETY: as above; `requirement` is a valid out-structure.
        let result = unsafe { function(instance, physical, &info, &mut requirement) };
        if ffi::failed(result) {
            return Err(NgxError::Call {
                call: "NVSDK_NGX_VULKAN_GetFeatureRequirements",
                result,
            });
        }
        Ok(requirement)
    }
}

/// `a` followed by the entries of `b` it does not have.
pub fn union(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = a.to_vec();
    for name in b {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

fn check_library(path: &Path) -> bool {
    let named = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(LIBRARY_NAME));
    let mut head = [0u8; 2];
    named
        && std::fs::File::open(path)
            .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head))
            .is_ok()
        && &head == b"MZ"
}

/// Where [`LIBRARY_NAME`] was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub path: PathBuf,
    pub source: Source,
}

/// Where [`locate`] looked, in words (the environment variable is `LEONIDA_DLSSG_DLL` here).
pub fn describe(source: Source) -> &'static str {
    match source {
        Source::Environment => "the LEONIDA_DLSSG_DLL variable",
        Source::Configured => "the folder of the dlss_path setting",
        other => other.describe(),
    }
}

/// Looks for `nvngx_dlssg.dll`: the `LEONIDA_DLSSG_DLL` variable (a file), next to the player's `nvngx_dlss.dll`
/// (`dlss_path`), NVIDIA's NGX folders, then the executable's folder. `None` is not an error: NGX also looks in the
/// driver's own folders, so the caller may still try.
pub fn locate(dlss_dll: Option<&Path>) -> Option<Located> {
    let env = std::env::var_os("LEONIDA_DLSSG_DLL")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    locate_with(env, dlss_dll)
}

/// As [`locate`], with the `LEONIDA_DLSSG_DLL` value given.
pub fn locate_with(env: Option<PathBuf>, dlss_dll: Option<&Path>) -> Option<Located> {
    if let Some(file) = env {
        return check_library(&file).then_some(Located {
            path: file,
            source: Source::Environment,
        });
    }
    if let Some(dlss) = dlss_dll {
        let dir = if dlss.is_dir() {
            Some(dlss)
        } else {
            dlss.parent()
        };
        if let Some(file) = dir
            .map(|d| d.join(LIBRARY_NAME))
            .filter(|f| check_library(f))
        {
            return Some(Located {
                path: file,
                source: Source::Configured,
            });
        }
    }
    let mut places: Vec<(PathBuf, Source, u32)> = Vec::new();
    if let Some(base) = std::env::var_os("ProgramData") {
        places.push((
            PathBuf::from(base)
                .join("NVIDIA")
                .join("NGX")
                .join("models"),
            Source::NgxModels,
            4,
        ));
    }
    if let Some(dir) = crate::dlss::ngx::registered_ngx_folder() {
        places.push((dir, Source::DriverStore, 0));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        places.push((dir, Source::ExecutableFolder, 0));
    }
    places
        .into_iter()
        .find_map(|(dir, source, depth)| find_in(&dir, depth).map(|path| Located { path, source }))
}

/// [`LIBRARY_NAME`] in `dir` or, for at most `depth` levels, below it.
pub fn find_in(dir: &Path, depth: u32) -> Option<PathBuf> {
    let direct = dir.join(LIBRARY_NAME);
    if check_library(&direct) {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect();
    children.sort();
    children
        .into_iter()
        .find_map(|child| find_in(&child, depth - 1))
}
