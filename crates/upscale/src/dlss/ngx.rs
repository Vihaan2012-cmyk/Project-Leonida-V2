//! A safe layer over NGX's Vulkan entry points: loading, initialisation, the parameter map, capability and
//! optimal-settings queries, and DLSS feature creation, evaluation and release.

use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ash::vk;

use super::ffi::{self, names};
use super::modes::DlssOptions;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NgxError {
    #[error("NGX runtime: {0}")]
    Runtime(String),
    #[error("{call} failed: {name} ({result:#010x})", name = ffi::result_name(*result))]
    Call { call: &'static str, result: u32 },
    #[error("{0}")]
    Other(String),
}

fn check(call: &'static str, result: ffi::Result) -> Result<(), NgxError> {
    if ffi::failed(result) {
        Err(NgxError::Call { call, result })
    } else {
        Ok(())
    }
}

pub use library::Library;

use super::ffi::slots;

#[cfg(windows)]
mod library {
    use std::ffi::{CString, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };

    /// The loaded NGX loader library. It is never unloaded: NGX starts threads and keeps state in the library, and
    /// freeing it crashes the process.
    pub struct Library {
        module: *mut c_void,
        pub path: PathBuf,
    }

    // SAFETY: a module handle is a process-wide value; GetProcAddress and FreeLibrary may be called from any thread.
    unsafe impl Send for Library {}
    // SAFETY: see above.
    unsafe impl Sync for Library {}

    fn not_found(path: &Path, code: u32) -> String {
        format!(
            "NVIDIA's NGX runtime was not found at {} (needs an NVIDIA driver; Windows error {code})",
            path.display()
        )
    }

    fn load(name: &Path, flags: u32, path: PathBuf) -> Result<Library, u32> {
        let wide: Vec<u16> = name.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; no file handle is passed.
        let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), flags) };
        if module.is_null() {
            // SAFETY: reads the calling thread's last error, set by the failed load.
            return Err(unsafe { GetLastError() });
        }
        Ok(Library { module, path })
    }

    /// The folder the NVIDIA driver records for its NGX runtime (`NGXCore\FullPath`).
    fn registered_folder() -> Option<PathBuf> {
        let key: Vec<u16> = r"SOFTWARE\NVIDIA Corporation\Global\NGXCore"
            .encode_utf16()
            .chain([0])
            .collect();
        let value: Vec<u16> = "FullPath".encode_utf16().chain([0]).collect();
        let mut buffer = vec![0u16; 1024];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: both names are NUL-terminated and `buffer` has `bytes` bytes of room.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != 0 {
            return None;
        }
        let units = (bytes as usize / 2).min(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..units]);
        let text = text.trim_end_matches('\0').trim();
        (!text.is_empty()).then(|| PathBuf::from(text))
    }

    impl Library {
        /// Loads `nvngx.dll` from the system folder or, failing that, from the folder the NVIDIA driver registers
        /// for NGX; or the library at `explicit`.
        pub fn open(explicit: Option<&Path>) -> Result<Library, String> {
            if let Some(path) = explicit {
                return load(path, LOAD_WITH_ALTERED_SEARCH_PATH, path.to_path_buf())
                    .map_err(|code| not_found(path, code));
            }
            let system = load(
                Path::new("nvngx.dll"),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
                PathBuf::from("nvngx.dll (system folder)"),
            );
            let first = match system {
                Ok(library) => return Ok(library),
                Err(code) => code,
            };
            if let Some(path) = registered_folder().map(|dir| dir.join("nvngx.dll"))
                && let Ok(library) = load(&path, LOAD_WITH_ALTERED_SEARCH_PATH, path.clone())
            {
                return Ok(library);
            }
            Err(not_found(Path::new("nvngx.dll (system folder)"), first))
        }

        /// Resolves an exported function. `T` must be the `extern "C"` function pointer type of that export.
        pub fn symbol<T: Copy>(&self, name: &str) -> Option<T> {
            assert_eq!(size_of::<T>(), size_of::<usize>());
            let name = CString::new(name).ok()?;
            // SAFETY: `module` is a loaded library and `name` is NUL-terminated.
            let address = unsafe { GetProcAddress(self.module, name.as_ptr().cast()) }?;
            // SAFETY: the caller names the function pointer type of the export; sizes were checked above.
            Some(unsafe { std::mem::transmute_copy::<_, T>(&address) })
        }
    }
}

#[cfg(not(windows))]
mod library {
    use std::path::{Path, PathBuf};

    pub struct Library {
        pub path: PathBuf,
    }

    impl Library {
        pub fn open(_explicit: Option<&Path>) -> Result<Library, String> {
            Err("DLSS needs Windows and an NVIDIA driver".into())
        }

        pub fn symbol<T: Copy>(&self, _name: &str) -> Option<T> {
            None
        }
    }
}

/// The resolved NGX entry points.
pub struct Api {
    pub library: Library,
    pub required_extensions: Option<ffi::FnRequiredExtensions>,
    pub init: Option<ffi::FnInit>,
    pub init_ext: Option<ffi::FnInitExt>,
    pub init_ext2: Option<ffi::FnInitExt2>,
    pub init_project_id: Option<ffi::FnInitProjectId>,
    pub shutdown: Option<ffi::FnShutdown>,
    pub shutdown1: Option<ffi::FnShutdown1>,
    pub get_capability_parameters: ffi::FnGetParameters,
    pub allocate_parameters: Option<ffi::FnGetParameters>,
    pub destroy_parameters: Option<ffi::FnDestroyParameters>,
    pub get_scratch_buffer_size: Option<ffi::FnGetScratchBufferSize>,
    pub create_feature: ffi::FnCreateFeature,
    pub create_feature1: Option<ffi::FnCreateFeature1>,
    pub evaluate_feature: ffi::FnEvaluateFeature,
    pub release_feature: ffi::FnReleaseFeature,
    pub get_feature_requirements: Option<ffi::FnGetFeatureRequirements>,
    pub get_instance_extension_requirements: Option<ffi::FnGetInstanceExtensionRequirements>,
    pub get_device_extension_requirements: Option<ffi::FnGetDeviceExtensionRequirements>,
}

/// The exports the loader must have for DLSS to be usable (besides an Init entry point).
pub const REQUIRED_EXPORTS: &[&str] = &[
    "NVSDK_NGX_VULKAN_GetCapabilityParameters",
    "NVSDK_NGX_VULKAN_AllocateParameters",
    "NVSDK_NGX_VULKAN_DestroyParameters",
    "NVSDK_NGX_VULKAN_CreateFeature",
    "NVSDK_NGX_VULKAN_EvaluateFeature",
    "NVSDK_NGX_VULKAN_ReleaseFeature",
    "NVSDK_NGX_VULKAN_Init",
    "NVSDK_NGX_VULKAN_Shutdown",
];

impl Api {
    pub fn load(library: Library) -> Result<Api, String> {
        let mut missing: Vec<&str> = Vec::new();
        macro_rules! need {
            ($name:expr) => {{
                let found = library.symbol($name);
                if found.is_none() {
                    missing.push($name);
                }
                found
            }};
        }
        let get_capability_parameters = need!("NVSDK_NGX_VULKAN_GetCapabilityParameters");
        let create_feature = need!("NVSDK_NGX_VULKAN_CreateFeature");
        let release_feature = need!("NVSDK_NGX_VULKAN_ReleaseFeature");
        let evaluate_feature = library
            .symbol("NVSDK_NGX_VULKAN_EvaluateFeature")
            .or_else(|| library.symbol("NVSDK_NGX_VULKAN_EvaluateFeature_C"));
        if evaluate_feature.is_none() {
            missing.push("NVSDK_NGX_VULKAN_EvaluateFeature");
        }
        let init = library.symbol("NVSDK_NGX_VULKAN_Init");
        let init_ext = library.symbol("NVSDK_NGX_VULKAN_Init_Ext");
        let init_ext2 = library.symbol("NVSDK_NGX_VULKAN_Init_Ext2");
        if init.is_none() && init_ext.is_none() && init_ext2.is_none() {
            missing.push("NVSDK_NGX_VULKAN_Init");
        }
        let shutdown = library.symbol("NVSDK_NGX_VULKAN_Shutdown");
        let shutdown1 = library.symbol("NVSDK_NGX_VULKAN_Shutdown1");
        if shutdown.is_none() && shutdown1.is_none() {
            missing.push("NVSDK_NGX_VULKAN_Shutdown");
        }
        if let (
            Some(get_capability_parameters),
            Some(create_feature),
            Some(evaluate_feature),
            Some(release_feature),
        ) = (
            get_capability_parameters,
            create_feature,
            evaluate_feature,
            release_feature,
        ) && missing.is_empty()
        {
            Ok(Api {
                required_extensions: library.symbol("NVSDK_NGX_VULKAN_RequiredExtensions"),
                init,
                init_ext,
                init_ext2,
                init_project_id: library.symbol("NVSDK_NGX_VULKAN_Init_ProjectID"),
                shutdown,
                shutdown1,
                get_capability_parameters,
                allocate_parameters: library.symbol("NVSDK_NGX_VULKAN_AllocateParameters"),
                destroy_parameters: library.symbol("NVSDK_NGX_VULKAN_DestroyParameters"),
                get_scratch_buffer_size: library.symbol("NVSDK_NGX_VULKAN_GetScratchBufferSize"),
                create_feature,
                create_feature1: library.symbol("NVSDK_NGX_VULKAN_CreateFeature1"),
                evaluate_feature,
                release_feature,
                get_feature_requirements: library.symbol("NVSDK_NGX_VULKAN_GetFeatureRequirements"),
                get_instance_extension_requirements: library
                    .symbol("NVSDK_NGX_VULKAN_GetFeatureInstanceExtensionRequirements"),
                get_device_extension_requirements: library
                    .symbol("NVSDK_NGX_VULKAN_GetFeatureDeviceExtensionRequirements"),
                library,
            })
        } else {
            Err(format!(
                "{} lacks the NGX entry points {}",
                library.path.display(),
                missing.join(", ")
            ))
        }
    }

    pub fn open(explicit: Option<&Path>) -> Result<Api, String> {
        Api::load(Library::open(explicit)?)
    }
}

/// A raw NGX parameter object, used through its virtual table.
#[derive(Debug)]
pub struct ParamMap {
    ptr: *mut ffi::Parameter,
}

impl ParamMap {
    /// # Safety
    /// `ptr` must point at a live object with the virtual table layout [`ffi::slots`] documents, for as long as
    /// the returned value is used.
    pub unsafe fn from_raw(ptr: *mut ffi::Parameter) -> ParamMap {
        ParamMap { ptr }
    }

    pub fn as_ptr(&self) -> *mut ffi::Parameter {
        self.ptr
    }

    /// Slot `index` of the parameter object's virtual table, as a function pointer type `F`.
    fn slot<F: Copy>(&self, index: usize) -> F {
        assert_eq!(size_of::<F>(), size_of::<usize>());
        // SAFETY: `ptr` is a live NGX parameter object, a C++ object whose first word points at its table of virtual
        // functions; the table has the 17 slots `slots` documents, so `index` is inside it.
        unsafe {
            let table = *(self.ptr as *const *const usize);
            std::mem::transmute_copy::<usize, F>(&*table.add(index))
        }
    }

    pub fn set_u32(&mut self, name: &str, value: u32) {
        let name = cname(name);
        let set: ffi::FnSetU32 = self.slot(slots::SET_U32);
        // SAFETY: `ptr` is a live parameter map and `name` is NUL-terminated.
        unsafe { set(self.ptr, name.as_ptr(), value) }
    }

    pub fn set_i32(&mut self, name: &str, value: i32) {
        let name = cname(name);
        let set: ffi::FnSetI32 = self.slot(slots::SET_I32);
        // SAFETY: as in `set_u32`.
        unsafe { set(self.ptr, name.as_ptr(), value) }
    }

    pub fn set_f32(&mut self, name: &str, value: f32) {
        let name = cname(name);
        let set: ffi::FnSetF32 = self.slot(slots::SET_F32);
        // SAFETY: as in `set_u32`.
        unsafe { set(self.ptr, name.as_ptr(), value) }
    }

    pub fn set_u64(&mut self, name: &str, value: u64) {
        let name = cname(name);
        let set: ffi::FnSetU64 = self.slot(slots::SET_U64);
        // SAFETY: as in `set_u32`.
        unsafe { set(self.ptr, name.as_ptr(), value) }
    }

    /// Stores a pointer the map keeps until the call that reads it ends: the caller keeps the target alive until then.
    ///
    /// # Safety
    /// `value` must stay valid until the NGX call that reads it has returned.
    pub unsafe fn set_ptr(&mut self, name: &str, value: *mut c_void) {
        let name = cname(name);
        let set: ffi::FnSetPtr = self.slot(slots::SET_PTR);
        // SAFETY: as in `set_u32`; the pointer is only dereferenced by NGX during the evaluate call.
        unsafe { set(self.ptr, name.as_ptr(), value) }
    }

    pub fn get_u32(&self, name: &str) -> Option<u32> {
        let name = cname(name);
        let get: ffi::FnGetU32 = self.slot(slots::GET_U32);
        let mut value = 0u32;
        // SAFETY: as in `set_u32`; `value` is a valid out-pointer.
        let result = unsafe { get(self.ptr, name.as_ptr(), &mut value) };
        (!ffi::failed(result)).then_some(value)
    }

    pub fn get_i32(&self, name: &str) -> Option<i32> {
        let name = cname(name);
        let get: ffi::FnGetI32 = self.slot(slots::GET_I32);
        let mut value = 0i32;
        // SAFETY: as in `get_u32`.
        let result = unsafe { get(self.ptr, name.as_ptr(), &mut value) };
        (!ffi::failed(result)).then_some(value)
    }

    pub fn get_f32(&self, name: &str) -> Option<f32> {
        let name = cname(name);
        let get: ffi::FnGetF32 = self.slot(slots::GET_F32);
        let mut value = 0f32;
        // SAFETY: as in `get_u32`.
        let result = unsafe { get(self.ptr, name.as_ptr(), &mut value) };
        (!ffi::failed(result)).then_some(value)
    }

    pub fn get_ptr(&self, name: &str) -> Option<*mut c_void> {
        let name = cname(name);
        let get: ffi::FnGetPtr = self.slot(slots::GET_PTR);
        let mut value = std::ptr::null_mut();
        // SAFETY: as in `get_u32`.
        let result = unsafe { get(self.ptr, name.as_ptr(), &mut value) };
        (!ffi::failed(result) && !value.is_null()).then_some(value)
    }
}

impl std::ops::Deref for Parameters {
    type Target = ParamMap;
    fn deref(&self) -> &ParamMap {
        &self.map
    }
}

impl std::ops::DerefMut for Parameters {
    fn deref_mut(&mut self) -> &mut ParamMap {
        &mut self.map
    }
}

/// An NGX parameter map, destroyed on drop.
pub struct Parameters {
    api: Arc<Api>,
    map: ParamMap,
}

fn cname(name: &str) -> CString {
    CString::new(name).expect("parameter names have no NUL")
}

impl Parameters {
    fn from_call(
        api: &Arc<Api>,
        call: &'static str,
        function: ffi::FnGetParameters,
    ) -> Result<Parameters, NgxError> {
        let mut ptr = std::ptr::null_mut();
        // SAFETY: `ptr` is a valid out-pointer; NGX fills it with a parameter map it owns until DestroyParameters.
        check(call, unsafe { function(&mut ptr) })?;
        if ptr.is_null() {
            return Err(NgxError::Other(format!("{call} returned no parameter map")));
        }
        Ok(Parameters {
            api: api.clone(),
            // SAFETY: NGX returned a live parameter object, valid until DestroyParameters.
            map: unsafe { ParamMap::from_raw(ptr) },
        })
    }

    pub fn allocate(api: &Arc<Api>) -> Result<Parameters, NgxError> {
        let function = api.allocate_parameters.ok_or_else(|| {
            NgxError::Runtime("NVSDK_NGX_VULKAN_AllocateParameters is not exported".into())
        })?;
        Parameters::from_call(api, "NVSDK_NGX_VULKAN_AllocateParameters", function)
    }

    pub fn capabilities(api: &Arc<Api>) -> Result<Parameters, NgxError> {
        Parameters::from_call(
            api,
            "NVSDK_NGX_VULKAN_GetCapabilityParameters",
            api.get_capability_parameters,
        )
    }
}

impl Drop for Parameters {
    fn drop(&mut self) {
        if let Some(destroy) = self.api.destroy_parameters {
            // SAFETY: `ptr` came from AllocateParameters or GetCapabilityParameters and is destroyed once.
            unsafe { destroy(self.map.ptr) };
        }
    }
}

/// Whether DLSS can run here, from the capability parameter map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    pub available: bool,
    pub needs_updated_driver: bool,
    pub min_driver: Option<(u32, u32)>,
    pub feature_init_result: Option<u32>,
}

impl Availability {
    /// A sentence saying why DLSS is not available, when it is not.
    pub fn reason(&self) -> Option<String> {
        if self.available {
            return None;
        }
        if self.needs_updated_driver {
            return Some(match self.min_driver {
                Some((major, minor)) => {
                    format!(
                        "the NVIDIA driver is too old for DLSS (needs {major}.{minor} or newer)"
                    )
                }
                None => "the NVIDIA driver is too old for DLSS".into(),
            });
        }
        Some(match self.feature_init_result {
            Some(result) if ffi::failed(result) => format!(
                "NGX could not initialise DLSS: {} ({result:#010x})",
                ffi::result_name(result)
            ),
            _ => "NGX reports DLSS as unavailable on this GPU".into(),
        })
    }
}

/// The result of NGX's optimal-settings query for one mode and output size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptimalSettings {
    pub render_size: [u32; 2],
    pub min_render_size: [u32; 2],
    pub max_render_size: [u32; 2],
    pub sharpness: f32,
}

pub struct ExtensionLists {
    pub instance: Vec<String>,
    pub device: Vec<String>,
}

impl Api {
    /// Names of the extensions NGX wants enabled, copied out of NGX's own tables.
    pub fn required_extensions(&self) -> Result<ExtensionLists, NgxError> {
        let function = self.required_extensions.ok_or_else(|| {
            NgxError::Runtime("NVSDK_NGX_VULKAN_RequiredExtensions is not exported".into())
        })?;
        let (mut ic, mut dc) = (0u32, 0u32);
        let (mut ip, mut dp): (*const *const c_char, *const *const c_char) =
            (std::ptr::null(), std::ptr::null());
        // SAFETY: all four out-pointers are valid; NGX returns arrays of NUL-terminated names it owns.
        check("NVSDK_NGX_VULKAN_RequiredExtensions", unsafe {
            function(&mut ic, &mut ip, &mut dc, &mut dp)
        })?;
        let read = |count: u32, list: *const *const c_char| -> Vec<String> {
            if list.is_null() {
                return Vec::new();
            }
            (0..count as usize)
                .filter_map(|i| {
                    // SAFETY: NGX reported `count` entries, each a NUL-terminated string.
                    let entry = unsafe { *list.add(i) };
                    (!entry.is_null()).then(|| {
                        // SAFETY: see above.
                        unsafe { std::ffi::CStr::from_ptr(entry) }
                            .to_string_lossy()
                            .into_owned()
                    })
                })
                .collect()
        };
        Ok(ExtensionLists {
            instance: read(ic, ip),
            device: read(dc, dp),
        })
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.to_string_lossy()
        .replace('/', "\\")
        .encode_utf16()
        .chain([0])
        .collect()
}

static LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

unsafe extern "C" fn collect_log(
    message: *const c_char,
    _level: ffi::LoggingLevel,
    _feature: ffi::Feature,
) {
    if message.is_null() {
        return;
    }
    // SAFETY: NGX passes a NUL-terminated string valid for the call.
    let line = unsafe { std::ffi::CStr::from_ptr(message) }
        .to_string_lossy()
        .trim_end()
        .to_string();
    if let Ok(mut log) = LOG.lock()
        && log.len() < 4096
    {
        log.push(line);
    }
}

/// The NGX log lines collected since the last call (see [`DlssOptions::log_level`]).
pub fn take_log() -> Vec<String> {
    LOG.lock()
        .map(|mut log| std::mem::take(&mut *log))
        .unwrap_or_default()
}

/// An initialised NGX instance for one Vulkan device.
pub struct Ngx {
    api: Arc<Api>,
    device: vk::Device,
    capabilities: std::mem::ManuallyDrop<Parameters>,
    _wide_paths: Vec<Vec<u16>>,
    _path_array: Vec<*const u16>,
    _feature_info: Box<ffi::FeatureCommonInfo>,
}

// SAFETY: NGX documents the entry points as callable from any thread given external synchronisation per device; the
// raw pointers kept here only keep path buffers alive.
unsafe impl Send for Ngx {}

pub struct VkInit {
    pub instance: vk::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: vk::Device,
    pub get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pub get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
}

impl Ngx {
    /// Initialises NGX for a device. `feature_dir` is the folder holding `nvngx_dlss.dll`, searched in addition to
    /// NGX's own locations.
    pub fn init(
        api: Arc<Api>,
        vk: &VkInit,
        options: &DlssOptions,
        feature_dir: Option<&Path>,
    ) -> Result<Ngx, NgxError> {
        let data_path = options
            .app_data_path
            .clone()
            .unwrap_or_else(std::env::temp_dir);
        let data_path_wide = wide(&data_path);
        let mut wide_paths = vec![data_path_wide];
        let mut path_array: Vec<*const u16> = Vec::new();
        if let Some(dir) = feature_dir {
            wide_paths.push(wide(dir));
            path_array.push(wide_paths[1].as_ptr());
        }
        let feature_info = Box::new(ffi::FeatureCommonInfo {
            path_list_info: ffi::PathListInfo {
                path: if path_array.is_empty() {
                    std::ptr::null()
                } else {
                    path_array.as_ptr()
                },
                length: path_array.len() as u32,
            },
            internal_data: std::ptr::null_mut(),
            logging_info: ffi::LoggingInfo {
                logging_callback: (options.log_level != ffi::LOGGING_OFF)
                    .then_some(collect_log as ffi::AppLogCallback),
                minimum_logging_level: options.log_level,
                disable_other_logging_sinks: false,
            },
        });
        let gipa = Some(vk.get_instance_proc_addr);
        let gdpa = Some(vk.get_device_proc_addr);
        let info: *const ffi::FeatureCommonInfo = &*feature_info;
        let app_id = options.application_id;
        // SAFETY: every pointer argument refers to a buffer that lives in `wide_paths` or `feature_info`, both alive
        // for the call; the Vulkan handles belong to the caller's live device.
        let result = unsafe {
            match (api.init_ext2, api.init_ext, api.init) {
                _ if options.use_project_id && api.init_project_id.is_some() => {
                    let project = cname(&options.project_id);
                    let engine = cname(&options.engine_version);
                    (
                        "NVSDK_NGX_VULKAN_Init_ProjectID",
                        api.init_project_id.unwrap()(
                            project.as_ptr(),
                            ffi::ENGINE_TYPE_CUSTOM,
                            engine.as_ptr(),
                            wide_paths[0].as_ptr(),
                            vk.instance,
                            vk.physical,
                            vk.device,
                            ffi::VERSION_API,
                            info,
                        ),
                    )
                }
                (Some(init), _, _) => (
                    "NVSDK_NGX_VULKAN_Init_Ext2",
                    init(
                        app_id,
                        wide_paths[0].as_ptr(),
                        vk.instance,
                        vk.physical,
                        vk.device,
                        gipa,
                        gdpa,
                        ffi::VERSION_API,
                        info,
                    ),
                ),
                (None, Some(init), _) => (
                    "NVSDK_NGX_VULKAN_Init_Ext",
                    init(
                        app_id,
                        wide_paths[0].as_ptr(),
                        vk.instance,
                        vk.physical,
                        vk.device,
                        ffi::VERSION_API,
                        info,
                    ),
                ),
                (None, None, Some(init)) => {
                    if !path_array.is_empty() {
                        return Err(NgxError::Call {
                            call: "NVSDK_NGX_VULKAN_Init (the driver has no Init_Ext for search paths)",
                            result: ffi::RESULT_FAIL_OUT_OF_DATE,
                        });
                    }
                    (
                        "NVSDK_NGX_VULKAN_Init",
                        init(
                            app_id,
                            wide_paths[0].as_ptr(),
                            vk.instance,
                            vk.physical,
                            vk.device,
                            ffi::VERSION_API,
                        ),
                    )
                }
                _ => {
                    return Err(NgxError::Runtime(
                        "NGX has no usable Init entry point".into(),
                    ));
                }
            }
        };
        check(result.0, result.1)?;
        let capabilities = match Parameters::capabilities(&api) {
            Ok(capabilities) => capabilities,
            Err(error) => {
                shutdown(&api, vk.device);
                return Err(error);
            }
        };
        Ok(Ngx {
            api,
            device: vk.device,
            capabilities: std::mem::ManuallyDrop::new(capabilities),
            _wide_paths: wide_paths,
            _path_array: path_array,
            _feature_info: feature_info,
        })
    }

    pub fn api(&self) -> &Arc<Api> {
        &self.api
    }

    pub fn availability(&self) -> Availability {
        let caps = &self.capabilities;
        Availability {
            available: caps
                .get_i32(names::SUPER_SAMPLING_AVAILABLE)
                .is_some_and(|v| v != 0),
            needs_updated_driver: caps
                .get_i32(names::SUPER_SAMPLING_NEEDS_UPDATED_DRIVER)
                .is_some_and(|v| v != 0),
            min_driver: caps
                .get_u32(names::SUPER_SAMPLING_MIN_DRIVER_MAJOR)
                .zip(caps.get_u32(names::SUPER_SAMPLING_MIN_DRIVER_MINOR))
                .filter(|(major, _)| *major != 0),
            feature_init_result: caps
                .get_i32(names::SUPER_SAMPLING_FEATURE_INIT_RESULT)
                .map(|v| v as u32),
        }
    }

    /// NGX's recommended render size for an output size and mode; `render_size` is `[0, 0]` when NGX does not offer
    /// the mode.
    pub fn optimal_settings(
        &mut self,
        output: [u32; 2],
        mode: super::modes::DlssMode,
    ) -> Result<OptimalSettings, NgxError> {
        let callback = self
            .capabilities
            .get_ptr(names::OPTIMAL_SETTINGS_CALLBACK)
            .ok_or(NgxError::Call {
                call: "DLSSOptimalSettingsCallback",
                result: ffi::RESULT_FAIL_OUT_OF_DATE,
            })?;
        // SAFETY: NGX stores a function with the optimal-settings callback signature under this name.
        let callback: ffi::FnOptimalSettings = unsafe { std::mem::transmute(callback) };
        let caps = &mut self.capabilities;
        caps.set_u32(names::WIDTH, output[0]);
        caps.set_u32(names::HEIGHT, output[1]);
        caps.set_i32(names::PERF_QUALITY_VALUE, mode.perf_quality());
        caps.set_i32(names::RTX_VALUE, 0);
        // SAFETY: `ptr` is the live capability map the callback belongs to.
        check("DLSSOptimalSettingsCallback", unsafe {
            callback(caps.as_ptr())
        })?;
        let render = [
            caps.get_u32(names::OUT_WIDTH).unwrap_or(0),
            caps.get_u32(names::OUT_HEIGHT).unwrap_or(0),
        ];
        let pair = |w: &str, h: &str, default: [u32; 2]| {
            [
                caps.get_u32(w).unwrap_or(default[0]),
                caps.get_u32(h).unwrap_or(default[1]),
            ]
        };
        Ok(OptimalSettings {
            render_size: render,
            min_render_size: pair(
                names::DYNAMIC_MIN_RENDER_WIDTH,
                names::DYNAMIC_MIN_RENDER_HEIGHT,
                render,
            ),
            max_render_size: pair(
                names::DYNAMIC_MAX_RENDER_WIDTH,
                names::DYNAMIC_MAX_RENDER_HEIGHT,
                render,
            ),
            sharpness: caps.get_f32(names::SHARPNESS).unwrap_or(0.0),
        })
    }

    /// Creates a DLSS feature, recording NGX's setup commands into `cmd`. The command buffer must be submitted and
    /// finished before the feature is evaluated.
    pub fn create_feature(
        &mut self,
        cmd: vk::CommandBuffer,
        render: [u32; 2],
        output: [u32; 2],
        mode: super::modes::DlssMode,
        flags: i32,
    ) -> Result<Feature, NgxError> {
        let mut params = Parameters::allocate(&self.api)?;
        params.set_u32(names::CREATION_NODE_MASK, 1);
        params.set_u32(names::VISIBILITY_NODE_MASK, 1);
        params.set_u32(names::WIDTH, render[0]);
        params.set_u32(names::HEIGHT, render[1]);
        params.set_u32(names::OUT_WIDTH, output[0]);
        params.set_u32(names::OUT_HEIGHT, output[1]);
        params.set_i32(names::PERF_QUALITY_VALUE, mode.perf_quality());
        params.set_i32(names::DLSS_FEATURE_CREATE_FLAGS, flags);
        params.set_i32(names::DLSS_ENABLE_OUTPUT_SUBRECTS, 0);
        let mut handle = std::ptr::null_mut();
        // SAFETY: `cmd` is a command buffer in the recording state, `params` is a live map and `handle` an out-pointer.
        let result = unsafe {
            match self.api.create_feature1 {
                Some(create) => (
                    "NVSDK_NGX_VULKAN_CreateFeature1",
                    create(
                        self.device,
                        cmd,
                        ffi::FEATURE_SUPER_SAMPLING,
                        params.as_ptr(),
                        &mut handle,
                    ),
                ),
                None => (
                    "NVSDK_NGX_VULKAN_CreateFeature",
                    (self.api.create_feature)(
                        cmd,
                        ffi::FEATURE_SUPER_SAMPLING,
                        params.as_ptr(),
                        &mut handle,
                    ),
                ),
            }
        };
        check(result.0, result.1)?;
        Ok(Feature {
            api: self.api.clone(),
            handle,
            params,
        })
    }
}

/// Shuts NGX down. The driver's `Shutdown1(device)` crashes where the argument-less `Shutdown` works, so the latter is
/// preferred.
fn shutdown(api: &Api, device: vk::Device) {
    // SAFETY: NGX was initialised on `device` and is shut down once per initialisation.
    unsafe {
        if let Some(shutdown) = api.shutdown {
            shutdown();
        } else if let Some(shutdown1) = api.shutdown1 {
            shutdown1(device);
        }
    }
}

impl Drop for Ngx {
    fn drop(&mut self) {
        // SAFETY: dropped once, here, and before NGX shuts down: the map belongs to the NGX instance.
        unsafe { std::mem::ManuallyDrop::drop(&mut self.capabilities) };
        shutdown(&self.api, self.device);
    }
}

/// A DLSS feature instance. Release it before the NGX instance is dropped and after its command buffers finished.
pub struct Feature {
    api: Arc<Api>,
    handle: *mut ffi::Handle,
    params: Parameters,
}

// SAFETY: see `Ngx`.
unsafe impl Send for Feature {}

/// One DLSS evaluation. The `ResourceVk` values must stay alive during the call.
pub struct EvalParams<'a> {
    pub color: &'a ffi::ResourceVk,
    pub depth: &'a ffi::ResourceVk,
    pub motion_vectors: &'a ffi::ResourceVk,
    pub output: &'a ffi::ResourceVk,
    pub exposure: Option<&'a ffi::ResourceVk>,
    pub transparency_mask: Option<&'a ffi::ResourceVk>,
    pub render_size: [u32; 2],
    pub jitter: [f32; 2],
    pub motion_vector_scale: [f32; 2],
    pub reset: bool,
    pub pre_exposure: f32,
    pub exposure_scale: f32,
    pub sharpness: f32,
    pub frame_time_ms: f32,
}

impl Feature {
    pub fn evaluate(
        &mut self,
        cmd: vk::CommandBuffer,
        eval: &EvalParams<'_>,
    ) -> Result<(), NgxError> {
        let p = &mut self.params;
        let ptr = |r: &ffi::ResourceVk| r as *const ffi::ResourceVk as *mut c_void;
        // SAFETY: the resources are borrowed for the whole evaluate call below.
        unsafe {
            p.set_ptr(names::COLOR, ptr(eval.color));
            p.set_ptr(names::OUTPUT, ptr(eval.output));
            p.set_ptr(names::DEPTH, ptr(eval.depth));
            p.set_ptr(names::MOTION_VECTORS, ptr(eval.motion_vectors));
            if let Some(exposure) = eval.exposure {
                p.set_ptr(names::EXPOSURE_TEXTURE, ptr(exposure));
            }
            if let Some(mask) = eval.transparency_mask {
                p.set_ptr(names::TRANSPARENCY_MASK, ptr(mask));
            }
        }
        p.set_f32(names::JITTER_OFFSET_X, eval.jitter[0]);
        p.set_f32(names::JITTER_OFFSET_Y, eval.jitter[1]);
        p.set_f32(names::SHARPNESS, eval.sharpness);
        p.set_i32(names::RESET, eval.reset as i32);
        p.set_f32(names::MV_SCALE_X, eval.motion_vector_scale[0]);
        p.set_f32(names::MV_SCALE_Y, eval.motion_vector_scale[1]);
        p.set_f32(names::PRE_EXPOSURE, eval.pre_exposure);
        p.set_f32(names::EXPOSURE_SCALE, eval.exposure_scale);
        p.set_f32(names::FRAME_TIME_DELTA_MS, eval.frame_time_ms);
        p.set_u32(names::RENDER_SUBRECT_WIDTH, eval.render_size[0]);
        p.set_u32(names::RENDER_SUBRECT_HEIGHT, eval.render_size[1]);
        for name in [
            names::COLOR_SUBRECT_BASE_X,
            names::COLOR_SUBRECT_BASE_Y,
            names::DEPTH_SUBRECT_BASE_X,
            names::DEPTH_SUBRECT_BASE_Y,
            names::MV_SUBRECT_BASE_X,
            names::MV_SUBRECT_BASE_Y,
            names::OUTPUT_SUBRECT_BASE_X,
            names::OUTPUT_SUBRECT_BASE_Y,
        ] {
            p.set_u32(name, 0);
        }
        // SAFETY: `cmd` is recording, the handle belongs to this feature and the parameter map points at resources
        // that the caller keeps alive for the call.
        check("NVSDK_NGX_VULKAN_EvaluateFeature", unsafe {
            (self.api.evaluate_feature)(cmd, self.handle, p.as_ptr(), None)
        })
    }
}

impl Drop for Feature {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFeature and is released once.
        unsafe { (self.api.release_feature)(self.handle) };
    }
}

/// Feature search directory for a configured DLSS library path.
pub fn feature_dir(dlss_dll: &Path) -> Option<PathBuf> {
    dlss_dll.parent().map(Path::to_path_buf)
}
