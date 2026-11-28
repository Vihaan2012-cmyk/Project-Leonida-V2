//! Our own Rust declarations of the NGX interface for Vulkan: the exported function signatures, the enum values and the
//! C struct layouts (x64 Windows) that NVIDIA publishes in its DLSS SDK headers. Nothing of NVIDIA's text is copied.

use std::ffi::{c_char, c_void};

use ash::vk;

pub type Result = u32;
pub const RESULT_SUCCESS: Result = 0x1;
pub const RESULT_FAIL: Result = 0xBAD0_0000;
pub const RESULT_FAIL_FEATURE_NOT_SUPPORTED: Result = 0xBAD0_0001;
pub const RESULT_FAIL_PLATFORM_ERROR: Result = 0xBAD0_0002;
pub const RESULT_FAIL_FEATURE_ALREADY_EXISTS: Result = 0xBAD0_0003;
pub const RESULT_FAIL_FEATURE_NOT_FOUND: Result = 0xBAD0_0004;
pub const RESULT_FAIL_INVALID_PARAMETER: Result = 0xBAD0_0005;
pub const RESULT_FAIL_SCRATCH_BUFFER_TOO_SMALL: Result = 0xBAD0_0006;
pub const RESULT_FAIL_NOT_INITIALIZED: Result = 0xBAD0_0007;
pub const RESULT_FAIL_UNSUPPORTED_INPUT_FORMAT: Result = 0xBAD0_0008;
pub const RESULT_FAIL_RW_FLAG_MISSING: Result = 0xBAD0_0009;
pub const RESULT_FAIL_MISSING_INPUT: Result = 0xBAD0_000A;
pub const RESULT_FAIL_UNABLE_TO_INITIALIZE_FEATURE: Result = 0xBAD0_000B;
pub const RESULT_FAIL_OUT_OF_DATE: Result = 0xBAD0_000C;
pub const RESULT_FAIL_OUT_OF_GPU_MEMORY: Result = 0xBAD0_000D;
pub const RESULT_FAIL_UNSUPPORTED_FORMAT: Result = 0xBAD0_000E;
pub const RESULT_FAIL_UNABLE_TO_WRITE_TO_APP_DATA_PATH: Result = 0xBAD0_000F;
pub const RESULT_FAIL_UNSUPPORTED_PARAMETER: Result = 0xBAD0_0010;
pub const RESULT_FAIL_DENIED: Result = 0xBAD0_0011;
pub const RESULT_FAIL_NOT_IMPLEMENTED: Result = 0xBAD0_0012;

/// NGX tests success by the failure base in the top twelve bits, not by equality with `Success`.
pub fn failed(result: Result) -> bool {
    result & 0xFFF0_0000 == RESULT_FAIL
}

pub fn result_name(result: Result) -> &'static str {
    match result {
        RESULT_SUCCESS => "Success",
        RESULT_FAIL => "Fail",
        RESULT_FAIL_FEATURE_NOT_SUPPORTED => "FeatureNotSupported",
        RESULT_FAIL_PLATFORM_ERROR => "PlatformError",
        RESULT_FAIL_FEATURE_ALREADY_EXISTS => "FeatureAlreadyExists",
        RESULT_FAIL_FEATURE_NOT_FOUND => "FeatureNotFound",
        RESULT_FAIL_INVALID_PARAMETER => "InvalidParameter",
        RESULT_FAIL_SCRATCH_BUFFER_TOO_SMALL => "ScratchBufferTooSmall",
        RESULT_FAIL_NOT_INITIALIZED => "NotInitialized",
        RESULT_FAIL_UNSUPPORTED_INPUT_FORMAT => "UnsupportedInputFormat",
        RESULT_FAIL_RW_FLAG_MISSING => "RWFlagMissing",
        RESULT_FAIL_MISSING_INPUT => "MissingInput",
        RESULT_FAIL_UNABLE_TO_INITIALIZE_FEATURE => "UnableToInitializeFeature",
        RESULT_FAIL_OUT_OF_DATE => "OutOfDate",
        RESULT_FAIL_OUT_OF_GPU_MEMORY => "OutOfGPUMemory",
        RESULT_FAIL_UNSUPPORTED_FORMAT => "UnsupportedFormat",
        RESULT_FAIL_UNABLE_TO_WRITE_TO_APP_DATA_PATH => "UnableToWriteToAppDataPath",
        RESULT_FAIL_UNSUPPORTED_PARAMETER => "UnsupportedParameter",
        RESULT_FAIL_DENIED => "Denied",
        RESULT_FAIL_NOT_IMPLEMENTED => "NotImplemented",
        _ => "unknown",
    }
}

/// `NVSDK_NGX_Version_API`, the SDK version the application states at initialisation.
pub const VERSION_API: u32 = 0x15;

pub type Feature = i32;
pub const FEATURE_SUPER_SAMPLING: Feature = 1;

pub type PerfQuality = i32;
pub const PERF_QUALITY_MAX_PERF: PerfQuality = 0;
pub const PERF_QUALITY_BALANCED: PerfQuality = 1;
pub const PERF_QUALITY_MAX_QUALITY: PerfQuality = 2;
pub const PERF_QUALITY_ULTRA_PERFORMANCE: PerfQuality = 3;
pub const PERF_QUALITY_ULTRA_QUALITY: PerfQuality = 4;
pub const PERF_QUALITY_DLAA: PerfQuality = 5;

pub const DLSS_FLAG_IS_HDR: i32 = 1 << 0;
pub const DLSS_FLAG_MV_LOW_RES: i32 = 1 << 1;
pub const DLSS_FLAG_MV_JITTERED: i32 = 1 << 2;
pub const DLSS_FLAG_DEPTH_INVERTED: i32 = 1 << 3;
pub const DLSS_FLAG_AUTO_EXPOSURE: i32 = 1 << 6;
pub const DLSS_FLAG_ALPHA_UPSCALING: i32 = 1 << 7;

pub type FeatureSupportResult = u32;
pub const FEATURE_SUPPORTED: FeatureSupportResult = 0;
pub const FEATURE_CHECK_NOT_PRESENT: FeatureSupportResult = 1;
pub const FEATURE_DRIVER_VERSION_UNSUPPORTED: FeatureSupportResult = 2;
pub const FEATURE_ADAPTER_UNSUPPORTED: FeatureSupportResult = 4;
pub const FEATURE_OS_VERSION_BELOW_MINIMUM: FeatureSupportResult = 8;
pub const FEATURE_NOT_IMPLEMENTED: FeatureSupportResult = 16;

pub type EngineType = i32;
pub const ENGINE_TYPE_CUSTOM: EngineType = 0;

pub type IdentifierType = i32;
pub const IDENTIFIER_APPLICATION_ID: IdentifierType = 0;
pub const IDENTIFIER_PROJECT_ID: IdentifierType = 1;

pub type LoggingLevel = i32;
pub const LOGGING_OFF: LoggingLevel = 0;
pub const LOGGING_ON: LoggingLevel = 1;
pub const LOGGING_VERBOSE: LoggingLevel = 2;

/// `NVSDK_NGX_Handle`: opaque.
#[repr(C)]
pub struct Handle {
    _private: [u8; 0],
}

/// `NVSDK_NGX_Parameter`: a C++ interface; the exported `NVSDK_NGX_Parameter_*` C functions are the way to use it.
#[repr(C)]
pub struct Parameter {
    _private: [u8; 0],
}

pub type AppLogCallback = unsafe extern "C" fn(*const c_char, LoggingLevel, Feature);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PathListInfo {
    pub path: *const *const u16,
    pub length: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoggingInfo {
    pub logging_callback: Option<AppLogCallback>,
    pub minimum_logging_level: LoggingLevel,
    pub disable_other_logging_sinks: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FeatureCommonInfo {
    pub path_list_info: PathListInfo,
    pub internal_data: *mut c_void,
    pub logging_info: LoggingInfo,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProjectIdDescription {
    pub project_id: *const c_char,
    pub engine_type: EngineType,
    pub engine_version: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union IdentifierValue {
    pub project_desc: ProjectIdDescription,
    pub application_id: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ApplicationIdentifier {
    pub identifier_type: IdentifierType,
    pub v: IdentifierValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FeatureDiscoveryInfo {
    pub sdk_version: u32,
    pub feature_id: Feature,
    pub identifier: ApplicationIdentifier,
    pub application_data_path: *const u16,
    pub feature_info: *const FeatureCommonInfo,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FeatureRequirement {
    pub feature_supported: FeatureSupportResult,
    pub min_hw_architecture: u32,
    pub min_os_version: [c_char; 255],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

pub type ResourceVkType = i32;
pub const RESOURCE_VK_TYPE_IMAGEVIEW: ResourceVkType = 0;
pub const RESOURCE_VK_TYPE_BUFFER: ResourceVkType = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ImageViewInfoVk {
    pub image_view: vk::ImageView,
    pub image: vk::Image,
    pub subresource_range: vk::ImageSubresourceRange,
    pub format: vk::Format,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferInfoVk {
    pub buffer: vk::Buffer,
    pub size_in_bytes: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union ResourceVkUnion {
    pub image_view_info: ImageViewInfoVk,
    pub buffer_info: BufferInfoVk,
}

/// `NVSDK_NGX_Resource_VK`: how an image view is handed to NGX.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResourceVk {
    pub resource: ResourceVkUnion,
    pub kind: ResourceVkType,
    pub read_write: bool,
}

impl ResourceVk {
    pub fn image_view(
        view: vk::ImageView,
        image: vk::Image,
        range: vk::ImageSubresourceRange,
        format: vk::Format,
        width: u32,
        height: u32,
        read_write: bool,
    ) -> ResourceVk {
        ResourceVk {
            resource: ResourceVkUnion {
                image_view_info: ImageViewInfoVk {
                    image_view: view,
                    image,
                    subresource_range: range,
                    format,
                    width,
                    height,
                },
            },
            kind: RESOURCE_VK_TYPE_IMAGEVIEW,
            read_write,
        }
    }
}

/// The parameter names (the keys of the NGX parameter map) the DLSS helpers use.
pub mod names {
    pub const WIDTH: &str = "Width";
    pub const HEIGHT: &str = "Height";
    pub const OUT_WIDTH: &str = "OutWidth";
    pub const OUT_HEIGHT: &str = "OutHeight";
    pub const PERF_QUALITY_VALUE: &str = "PerfQualityValue";
    pub const RTX_VALUE: &str = "RTXValue";
    pub const SHARPNESS: &str = "Sharpness";
    pub const CREATION_NODE_MASK: &str = "CreationNodeMask";
    pub const VISIBILITY_NODE_MASK: &str = "VisibilityNodeMask";
    pub const DLSS_FEATURE_CREATE_FLAGS: &str = "DLSS.Feature.Create.Flags";
    pub const DLSS_ENABLE_OUTPUT_SUBRECTS: &str = "DLSS.Enable.Output.Subrects";
    pub const DYNAMIC_MAX_RENDER_WIDTH: &str = "DLSS.Get.Dynamic.Max.Render.Width";
    pub const DYNAMIC_MAX_RENDER_HEIGHT: &str = "DLSS.Get.Dynamic.Max.Render.Height";
    pub const DYNAMIC_MIN_RENDER_WIDTH: &str = "DLSS.Get.Dynamic.Min.Render.Width";
    pub const DYNAMIC_MIN_RENDER_HEIGHT: &str = "DLSS.Get.Dynamic.Min.Render.Height";
    pub const OPTIMAL_SETTINGS_CALLBACK: &str = "DLSSOptimalSettingsCallback";
    pub const SUPER_SAMPLING_AVAILABLE: &str = "SuperSampling.Available";
    pub const SUPER_SAMPLING_NEEDS_UPDATED_DRIVER: &str = "SuperSampling.NeedsUpdatedDriver";
    pub const SUPER_SAMPLING_MIN_DRIVER_MAJOR: &str = "SuperSampling.MinDriverVersionMajor";
    pub const SUPER_SAMPLING_MIN_DRIVER_MINOR: &str = "SuperSampling.MinDriverVersionMinor";
    pub const SUPER_SAMPLING_FEATURE_INIT_RESULT: &str = "SuperSampling.FeatureInitResult";
    pub const COLOR: &str = "Color";
    pub const OUTPUT: &str = "Output";
    pub const DEPTH: &str = "Depth";
    pub const MOTION_VECTORS: &str = "MotionVectors";
    pub const JITTER_OFFSET_X: &str = "Jitter.Offset.X";
    pub const JITTER_OFFSET_Y: &str = "Jitter.Offset.Y";
    pub const RESET: &str = "Reset";
    pub const MV_SCALE_X: &str = "MV.Scale.X";
    pub const MV_SCALE_Y: &str = "MV.Scale.Y";
    pub const EXPOSURE_TEXTURE: &str = "ExposureTexture";
    pub const TRANSPARENCY_MASK: &str = "TransparencyMask";
    pub const PRE_EXPOSURE: &str = "DLSS.Pre.Exposure";
    pub const EXPOSURE_SCALE: &str = "DLSS.Exposure.Scale";
    pub const RENDER_SUBRECT_WIDTH: &str = "DLSS.Render.Subrect.Dimensions.Width";
    pub const RENDER_SUBRECT_HEIGHT: &str = "DLSS.Render.Subrect.Dimensions.Height";
    pub const COLOR_SUBRECT_BASE_X: &str = "DLSS.Input.Color.Subrect.Base.X";
    pub const COLOR_SUBRECT_BASE_Y: &str = "DLSS.Input.Color.Subrect.Base.Y";
    pub const DEPTH_SUBRECT_BASE_X: &str = "DLSS.Input.Depth.Subrect.Base.X";
    pub const DEPTH_SUBRECT_BASE_Y: &str = "DLSS.Input.Depth.Subrect.Base.Y";
    pub const MV_SUBRECT_BASE_X: &str = "DLSS.Input.MV.Subrect.Base.X";
    pub const MV_SUBRECT_BASE_Y: &str = "DLSS.Input.MV.Subrect.Base.Y";
    pub const OUTPUT_SUBRECT_BASE_X: &str = "DLSS.Output.Subrect.Base.X";
    pub const OUTPUT_SUBRECT_BASE_Y: &str = "DLSS.Output.Subrect.Base.Y";
    pub const FRAME_TIME_DELTA_MS: &str = "FrameTimeDeltaInMsec";
}

pub type ProgressCallbackC = unsafe extern "C" fn(f32, *mut bool);

pub type FnRequiredExtensions = unsafe extern "C" fn(
    *mut u32,
    *mut *const *const c_char,
    *mut u32,
    *mut *const *const c_char,
) -> Result;
/// The Vulkan `Init` of the driver's NGX core (`nvngx.dll`). It is not the SDK's wrapper signature: that wrapper (a
/// static library) forwards to these, and the version number is the sixth argument.
pub type FnInit = unsafe extern "C" fn(
    u64,
    *const u16,
    vk::Instance,
    vk::PhysicalDevice,
    vk::Device,
    u32,
) -> Result;
/// Core `Init_Ext`: as [`FnInit`] plus the common feature info (search paths, logging) after the version.
pub type FnInitExt = unsafe extern "C" fn(
    u64,
    *const u16,
    vk::Instance,
    vk::PhysicalDevice,
    vk::Device,
    u32,
    *const FeatureCommonInfo,
) -> Result;
/// Core `Init_Ext2`, present in newer drivers: also takes the loader's function-pointer getters.
pub type FnInitExt2 = unsafe extern "C" fn(
    u64,
    *const u16,
    vk::Instance,
    vk::PhysicalDevice,
    vk::Device,
    Option<vk::PFN_vkGetInstanceProcAddr>,
    Option<vk::PFN_vkGetDeviceProcAddr>,
    u32,
    *const FeatureCommonInfo,
) -> Result;
/// Core `Init_ProjectID` (candidate layout: the `Init_Ext` arguments with the project description in place of the
/// application id).
pub type FnInitProjectId = unsafe extern "C" fn(
    *const c_char,
    EngineType,
    *const c_char,
    *const u16,
    vk::Instance,
    vk::PhysicalDevice,
    vk::Device,
    u32,
    *const FeatureCommonInfo,
) -> Result;
pub type FnShutdown = unsafe extern "C" fn() -> Result;
pub type FnShutdown1 = unsafe extern "C" fn(vk::Device) -> Result;
pub type FnGetParameters = unsafe extern "C" fn(*mut *mut Parameter) -> Result;
pub type FnDestroyParameters = unsafe extern "C" fn(*mut Parameter) -> Result;
pub type FnGetScratchBufferSize =
    unsafe extern "C" fn(Feature, *const Parameter, *mut usize) -> Result;
pub type FnCreateFeature =
    unsafe extern "C" fn(vk::CommandBuffer, Feature, *mut Parameter, *mut *mut Handle) -> Result;
pub type FnCreateFeature1 = unsafe extern "C" fn(
    vk::Device,
    vk::CommandBuffer,
    Feature,
    *mut Parameter,
    *mut *mut Handle,
) -> Result;
pub type FnEvaluateFeature = unsafe extern "C" fn(
    vk::CommandBuffer,
    *const Handle,
    *const Parameter,
    Option<ProgressCallbackC>,
) -> Result;
pub type FnReleaseFeature = unsafe extern "C" fn(*mut Handle) -> Result;
pub type FnGetFeatureRequirements = unsafe extern "C" fn(
    vk::Instance,
    vk::PhysicalDevice,
    *const FeatureDiscoveryInfo,
    *mut FeatureRequirement,
) -> Result;
pub type FnGetInstanceExtensionRequirements = unsafe extern "C" fn(
    *const FeatureDiscoveryInfo,
    *mut u32,
    *mut *mut vk::ExtensionProperties,
) -> Result;
pub type FnGetDeviceExtensionRequirements = unsafe extern "C" fn(
    vk::Instance,
    vk::PhysicalDevice,
    *const FeatureDiscoveryInfo,
    *mut u32,
    *mut *mut vk::ExtensionProperties,
) -> Result;

/// Slots of the `NVSDK_NGX_Parameter` virtual table, in the order the SDK header declares its pure virtual `Set`
/// overloads (unsigned long long, float, double, unsigned int, int, ID3D11Resource*, ID3D12Resource*, void*), then the
/// `Get` overloads in the same order, then `Reset`. The Windows driver is built with MSVC, which lays virtual
/// functions out in declaration order.
pub mod slots {
    pub const SET_U64: usize = 7;
    pub const SET_F32: usize = 6;
    pub const SET_U32: usize = 4;
    pub const SET_I32: usize = 3;
    pub const SET_PTR: usize = 0;
    pub const GET_U64: usize = 15;
    pub const GET_F32: usize = 14;
    pub const GET_U32: usize = 12;
    pub const GET_I32: usize = 11;
    pub const GET_PTR: usize = 8;
    pub const RESET: usize = 16;
}

pub type FnSetU64 = unsafe extern "C" fn(*mut Parameter, *const c_char, u64);
pub type FnSetF32 = unsafe extern "C" fn(*mut Parameter, *const c_char, f32);
pub type FnSetU32 = unsafe extern "C" fn(*mut Parameter, *const c_char, u32);
pub type FnSetI32 = unsafe extern "C" fn(*mut Parameter, *const c_char, i32);
pub type FnSetPtr = unsafe extern "C" fn(*mut Parameter, *const c_char, *mut c_void);
pub type FnGetU64 = unsafe extern "C" fn(*mut Parameter, *const c_char, *mut u64) -> Result;
pub type FnGetF32 = unsafe extern "C" fn(*mut Parameter, *const c_char, *mut f32) -> Result;
pub type FnGetU32 = unsafe extern "C" fn(*mut Parameter, *const c_char, *mut u32) -> Result;
pub type FnGetI32 = unsafe extern "C" fn(*mut Parameter, *const c_char, *mut i32) -> Result;
pub type FnGetPtr = unsafe extern "C" fn(*mut Parameter, *const c_char, *mut *mut c_void) -> Result;

/// The callback behind `DLSSOptimalSettingsCallback`: reads `Width`, `Height`, `PerfQualityValue`, writes the sizes.
pub type FnOptimalSettings = unsafe extern "C" fn(*mut Parameter) -> Result;

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::*;

    #[test]
    fn struct_layouts_match_the_headers_on_x64() {
        assert_eq!(size_of::<PathListInfo>(), 16);
        assert_eq!(offset_of!(PathListInfo, length), 8);
        assert_eq!(size_of::<LoggingInfo>(), 16);
        assert_eq!(offset_of!(LoggingInfo, minimum_logging_level), 8);
        assert_eq!(offset_of!(LoggingInfo, disable_other_logging_sinks), 12);
        assert_eq!(size_of::<FeatureCommonInfo>(), 40);
        assert_eq!(offset_of!(FeatureCommonInfo, internal_data), 16);
        assert_eq!(offset_of!(FeatureCommonInfo, logging_info), 24);
        assert_eq!(size_of::<ProjectIdDescription>(), 24);
        assert_eq!(offset_of!(ProjectIdDescription, engine_type), 8);
        assert_eq!(offset_of!(ProjectIdDescription, engine_version), 16);
        assert_eq!(size_of::<ApplicationIdentifier>(), 32);
        assert_eq!(offset_of!(ApplicationIdentifier, v), 8);
        assert_eq!(size_of::<FeatureDiscoveryInfo>(), 56);
        assert_eq!(offset_of!(FeatureDiscoveryInfo, feature_id), 4);
        assert_eq!(offset_of!(FeatureDiscoveryInfo, identifier), 8);
        assert_eq!(offset_of!(FeatureDiscoveryInfo, application_data_path), 40);
        assert_eq!(offset_of!(FeatureDiscoveryInfo, feature_info), 48);
        assert_eq!(size_of::<FeatureRequirement>(), 264);
        assert_eq!(offset_of!(FeatureRequirement, min_hw_architecture), 4);
        assert_eq!(offset_of!(FeatureRequirement, min_os_version), 8);
        assert_eq!(size_of::<Dimensions>(), 8);
    }

    #[test]
    fn vulkan_resource_wrapper_layout_matches_the_header_on_x64() {
        assert_eq!(size_of::<ImageViewInfoVk>(), 48);
        assert_eq!(offset_of!(ImageViewInfoVk, image), 8);
        assert_eq!(offset_of!(ImageViewInfoVk, subresource_range), 16);
        assert_eq!(offset_of!(ImageViewInfoVk, format), 36);
        assert_eq!(offset_of!(ImageViewInfoVk, width), 40);
        assert_eq!(offset_of!(ImageViewInfoVk, height), 44);
        assert_eq!(size_of::<BufferInfoVk>(), 16);
        assert_eq!(size_of::<ResourceVkUnion>(), 48);
        assert_eq!(offset_of!(ResourceVk, kind), 48);
        assert_eq!(offset_of!(ResourceVk, read_write), 52);
        assert_eq!(size_of::<ResourceVk>(), 56);
    }

    #[test]
    fn parameter_table_slots_follow_the_msvc_overload_order() {
        assert_eq!(slots::SET_U64 - slots::SET_PTR, 7);
        assert_eq!(slots::GET_U64 - slots::GET_PTR, 7);
        assert_eq!(slots::GET_PTR, slots::SET_U64 + 1);
        assert_eq!(slots::SET_U32 - slots::SET_I32, 1);
        assert_eq!(slots::GET_U32 - slots::GET_I32, 1);
        assert_eq!(slots::RESET, 16);
    }

    #[test]
    fn result_codes_use_the_failure_mask() {
        assert!(!failed(RESULT_SUCCESS));
        assert!(failed(RESULT_FAIL));
        assert!(failed(RESULT_FAIL_NOT_IMPLEMENTED));
        assert!(failed(RESULT_FAIL_OUT_OF_DATE));
        assert!(!failed(0x2));
        assert_eq!(result_name(RESULT_FAIL_OUT_OF_DATE), "OutOfDate");
        assert_eq!(result_name(0x1234), "unknown");
    }

    #[test]
    fn enum_and_flag_values_match_the_headers() {
        assert_eq!(VERSION_API, 21);
        assert_eq!(FEATURE_SUPER_SAMPLING, 1);
        assert_eq!(
            [
                PERF_QUALITY_MAX_PERF,
                PERF_QUALITY_BALANCED,
                PERF_QUALITY_MAX_QUALITY,
                PERF_QUALITY_ULTRA_PERFORMANCE,
                PERF_QUALITY_ULTRA_QUALITY,
                PERF_QUALITY_DLAA
            ],
            [0, 1, 2, 3, 4, 5]
        );
        assert_eq!(DLSS_FLAG_IS_HDR, 1);
        assert_eq!(DLSS_FLAG_MV_LOW_RES, 2);
        assert_eq!(DLSS_FLAG_MV_JITTERED, 4);
        assert_eq!(DLSS_FLAG_DEPTH_INVERTED, 8);
        assert_eq!(DLSS_FLAG_AUTO_EXPOSURE, 0x40);
    }
}
