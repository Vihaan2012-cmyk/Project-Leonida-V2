//! The parameter names, feature id and flag values of NVIDIA's DLSS frame generation feature, as the keys of the NGX
//! parameter map (our own declarations, from NVIDIA's public SDK headers).

/// `NVSDK_NGX_Feature_FrameGeneration`.
pub const FEATURE_FRAME_GENERATION: i32 = 11;

pub const AVAILABLE: &str = "FrameGeneration.Available";
pub const FEATURE_INIT_RESULT: &str = "FrameGeneration.FeatureInitResult";
pub const MIN_DRIVER_MAJOR: &str = "FrameGeneration.MinDriverVersionMajor";
pub const MIN_DRIVER_MINOR: &str = "FrameGeneration.MinDriverVersionMinor";
pub const NEEDS_UPDATED_DRIVER: &str = "FrameGeneration.NeedsUpdatedDriver";
/// Capability map: the most frames that can be generated between two real ones (3 for 4x); absent or 1 when only 2x.
pub const MULTI_FRAME_COUNT_MAX: &str = "DLSSG.MultiFrameCountMax";

pub const BACKBUFFER_FORMAT: &str = "DLSSG.BackbufferFormat";
pub const WIDTH: &str = "DLSSG.Width";
pub const HEIGHT: &str = "DLSSG.Height";
pub const INTERNAL_WIDTH: &str = "DLSSG.InternalWidth";
pub const INTERNAL_HEIGHT: &str = "DLSSG.InternalHeight";
pub const DYNAMIC_RESOLUTION: &str = "DLSSG.DynamicResolution";
pub const ALWAYS_PROVIDED_FLAGS: &str = "DLSSG.ResourceAlwaysProvidedFlags";
pub const NEVER_PROVIDED_FLAGS: &str = "DLSSG.ResourceNeverProvidedFlags";
pub const ASYNC_CREATE_ENABLED: &str = "DLSSG.AsyncCreateEnabled";
pub const UI_RECOMPOSITION_ENABLED: &str = "DLSSG.UserInterfaceRecompositionEnabled";

pub const BACKBUFFER: &str = "DLSSG.Backbuffer";
pub const MVECS: &str = "DLSSG.MVecs";
pub const DEPTH: &str = "DLSSG.Depth";
pub const HUDLESS: &str = "DLSSG.HUDLess";
pub const UI: &str = "DLSSG.UI";
pub const UI_ALPHA: &str = "DLSSG.UIAlpha";
pub const BIDIRECTIONAL_DISTORTION_FIELD: &str = "DLSSG.BidirectionalDistortionField";
pub const OUTPUT_INTERPOLATED: &str = "DLSSG.OutputInterpolated";
pub const OUTPUT_REAL: &str = "DLSSG.OutputReal";
pub const OUTPUT_DISABLE_INTERPOLATION: &str = "DLSSG.OutputDisableInterpolation";

pub const MULTI_FRAME_COUNT: &str = "DLSSG.MultiFrameCount";
pub const MULTI_FRAME_INDEX: &str = "DLSSG.MultiFrameIndex";
pub const BACKBUFFER_FRAME_ID: &str = "DLSSG.BackbufferFrameID";

pub const CAMERA_VIEW_TO_CLIP: &str = "DLSSG.CameraViewToClip";
pub const CLIP_TO_CAMERA_VIEW: &str = "DLSSG.ClipToCameraView";
pub const CLIP_TO_LENS_CLIP: &str = "DLSSG.ClipToLensClip";
pub const CLIP_TO_PREV_CLIP: &str = "DLSSG.ClipToPrevClip";
pub const PREV_CLIP_TO_CLIP: &str = "DLSSG.PrevClipToClip";

pub const JITTER_OFFSET_X: &str = "DLSSG.JitterOffsetX";
pub const JITTER_OFFSET_Y: &str = "DLSSG.JitterOffsetY";
pub const MVEC_SCALE_X: &str = "DLSSG.MvecScaleX";
pub const MVEC_SCALE_Y: &str = "DLSSG.MvecScaleY";
pub const CAMERA_PINHOLE_OFFSET_X: &str = "DLSSG.CameraPinholeOffsetX";
pub const CAMERA_PINHOLE_OFFSET_Y: &str = "DLSSG.CameraPinholeOffsetY";
pub const CAMERA_POS: [&str; 3] = ["DLSSG.CameraPosX", "DLSSG.CameraPosY", "DLSSG.CameraPosZ"];
pub const CAMERA_UP: [&str; 3] = ["DLSSG.CameraUpX", "DLSSG.CameraUpY", "DLSSG.CameraUpZ"];
pub const CAMERA_RIGHT: [&str; 3] = [
    "DLSSG.CameraRightX",
    "DLSSG.CameraRightY",
    "DLSSG.CameraRightZ",
];
pub const CAMERA_FWD: [&str; 3] = ["DLSSG.CameraFwdX", "DLSSG.CameraFwdY", "DLSSG.CameraFwdZ"];
pub const CAMERA_NEAR: &str = "DLSSG.CameraNear";
pub const CAMERA_FAR: &str = "DLSSG.CameraFar";
pub const CAMERA_FOV: &str = "DLSSG.CameraFOV";
pub const CAMERA_ASPECT_RATIO: &str = "DLSSG.CameraAspectRatio";

pub const COLOR_BUFFERS_HDR: &str = "DLSSG.ColorBuffersHDR";
pub const DEPTH_INVERTED: &str = "DLSSG.DepthInverted";
pub const CAMERA_MOTION_INCLUDED: &str = "DLSSG.CameraMotionIncluded";
pub const RESET: &str = "DLSSG.Reset";
pub const AUTOMODE_OVERRIDE_RESET: &str = "DLSSG.AutomodeOverrideReset";
pub const NOT_RENDERING_GAME_FRAMES: &str = "DLSSG.NotRenderingGameFrames";
pub const ORTHO_PROJECTION: &str = "DLSSG.OrthoProjection";
pub const MVEC_INVALID_VALUE: &str = "DLSSG.MvecInvalidValue";
pub const MVEC_DILATED: &str = "DLSSG.MvecDilated";
pub const MENU_DETECTION_ENABLED: &str = "DLSSG.MenuDetectionEnabled";
pub const MIN_RELATIVE_LINEAR_DEPTH_OBJECT_SEPARATION: &str =
    "DLSSG.MinRelativeLinearDepthObjectSeparation";

/// The names of the four values of one subrect: base x, base y, width, height.
#[derive(Debug, Clone, Copy)]
pub struct SubrectNames(pub [&'static str; 4]);

pub const SUBRECT_BACKBUFFER: SubrectNames = SubrectNames([
    "DLSSG.InputBackbufferSubrectBaseX",
    "DLSSG.InputBackbufferSubrectBaseY",
    "DLSSG.InputBackbufferSubrectWidth",
    "DLSSG.InputBackbufferSubrectHeight",
]);
pub const SUBRECT_MVECS: SubrectNames = SubrectNames([
    "DLSSG.MVecsSubrectBaseX",
    "DLSSG.MVecsSubrectBaseY",
    "DLSSG.MVecsSubrectWidth",
    "DLSSG.MVecsSubrectHeight",
]);
pub const SUBRECT_DEPTH: SubrectNames = SubrectNames([
    "DLSSG.DepthSubrectBaseX",
    "DLSSG.DepthSubrectBaseY",
    "DLSSG.DepthSubrectWidth",
    "DLSSG.DepthSubrectHeight",
]);
pub const SUBRECT_HUDLESS: SubrectNames = SubrectNames([
    "DLSSG.HUDLessSubrectBaseX",
    "DLSSG.HUDLessSubrectBaseY",
    "DLSSG.HUDLessSubrectWidth",
    "DLSSG.HUDLessSubrectHeight",
]);
pub const SUBRECT_UI: SubrectNames = SubrectNames([
    "DLSSG.UISubrectBaseX",
    "DLSSG.UISubrectBaseY",
    "DLSSG.UISubrectWidth",
    "DLSSG.UISubrectHeight",
]);
pub const SUBRECT_UI_ALPHA: SubrectNames = SubrectNames([
    "DLSSG.UIAlphaSubrectBaseX",
    "DLSSG.UIAlphaSubrectBaseY",
    "DLSSG.UIAlphaSubrectWidth",
    "DLSSG.UIAlphaSubrectHeight",
]);
pub const SUBRECT_OUTPUT_INTERPOLATED: SubrectNames = SubrectNames([
    "DLSSG.OutputInterpolatedSubrectBaseX",
    "DLSSG.OutputInterpolatedSubrectBaseY",
    "DLSSG.OutputInterpolatedSubrectWidth",
    "DLSSG.OutputInterpolatedSubrectHeight",
]);
pub const SUBRECT_OUTPUT_REAL: SubrectNames = SubrectNames([
    "DLSSG.OutputRealSubrectBaseX",
    "DLSSG.OutputRealSubrectBaseY",
    "DLSSG.OutputRealSubrectWidth",
    "DLSSG.OutputRealSubrectHeight",
]);

/// `NVSDK_NGX_DLSSG_ResourceFlags`: which resources the application always or never provides.
pub mod resource_flags {
    pub const BACKBUFFER: u32 = 1 << 0;
    pub const MVECS: u32 = 1 << 1;
    pub const DEPTH: u32 = 1 << 2;
    pub const HUDLESS: u32 = 1 << 3;
    pub const UI: u32 = 1 << 4;
    pub const BIDIRECTIONAL_DISTORTION_FIELD: u32 = 1 << 5;
    pub const UI_ALPHA: u32 = 1 << 7;
    pub const OUTPUT_INTERPOLATED: u32 = 1 << 24;
    pub const OUTPUT_REAL: u32 = 1 << 25;
    pub const OUTPUT_DISABLE_INTERPOLATION: u32 = 1 << 26;
}
