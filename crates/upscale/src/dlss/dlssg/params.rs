//! Building the parameter maps DLSS frame generation reads: the creation parameters, the per-frame resources and the
//! per-frame engine data, written through a [`Sink`] so the sequence can be checked without NGX.
//!
//! The sequence is the one NVIDIA's Vulkan helpers (`NGX_VK_CREATE_DLSSG`, `NGX_VK_EVALUATE_DLSSG`) perform. A map is
//! stateful, so every evaluation writes every value, and an absent optional resource is written as a null pointer
//! instead of being left from the frame before.

use std::ffi::c_void;

use ash::vk;

use super::names::{self, SubrectNames, resource_flags};
use crate::dlss::ffi::{self, ResourceVk};
use crate::dlss::ngx::ParamMap;

/// Where parameters go: an NGX parameter map, or a recording used by tests.
pub trait Sink {
    fn set_u32(&mut self, name: &str, value: u32);
    fn set_f32(&mut self, name: &str, value: f32);

    /// # Safety
    /// `value` is null or points at something that lives until the NGX call that reads it has returned.
    unsafe fn set_ptr(&mut self, name: &str, value: *mut c_void);
}

impl Sink for ParamMap {
    fn set_u32(&mut self, name: &str, value: u32) {
        ParamMap::set_u32(self, name, value);
    }

    fn set_f32(&mut self, name: &str, value: f32) {
        ParamMap::set_f32(self, name, value);
    }

    unsafe fn set_ptr(&mut self, name: &str, value: *mut c_void) {
        // SAFETY: forwarded from this function's contract.
        unsafe { ParamMap::set_ptr(self, name, value) }
    }
}

pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// A rectangle inside a resource, for the subrect parameters. All zeros means the whole resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Subrect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What the feature is created for. Fixed for the life of the instance: a new size or format needs a new feature.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateParams {
    /// The size of the colour images going in and out.
    pub size: [u32; 2],
    /// The format of the colour images (the back buffer's, and the interpolated frame's).
    pub backbuffer_format: vk::Format,
    /// The size the game renders at, which depth and motion vectors have.
    pub render_size: [u32; 2],
    pub dynamic_resolution: bool,
    /// Resources that will be given on every frame, and resources that never will be; both lists name optional ones
    /// (the required ones are always given). Telling NGX lets it keep less memory.
    pub always_provided: u32,
    pub never_provided: u32,
}

impl CreateParams {
    pub fn new(size: [u32; 2], backbuffer_format: vk::Format, render_size: [u32; 2]) -> Self {
        CreateParams {
            size,
            backbuffer_format,
            render_size,
            dynamic_resolution: false,
            always_provided: 0,
            never_provided: resource_flags::HUDLESS
                | resource_flags::UI
                | resource_flags::UI_ALPHA
                | resource_flags::BIDIRECTIONAL_DISTORTION_FIELD,
        }
    }

    /// The sizes NGX documents: 4 to 7680 pixels each way.
    pub fn check(&self) -> Result<(), String> {
        for (what, size) in [("size", self.size), ("render size", self.render_size)] {
            if size.iter().any(|&v| !(4..=7680).contains(&v)) {
                return Err(format!(
                    "frame generation {what} {}x{} is outside 4 to 7680 pixels",
                    size[0], size[1]
                ));
            }
        }
        if self.backbuffer_format == vk::Format::UNDEFINED {
            return Err("frame generation needs a colour format".into());
        }
        Ok(())
    }
}

/// Writes the creation parameters.
pub fn apply_create(sink: &mut dyn Sink, create: &CreateParams) {
    sink.set_u32(ffi::names::CREATION_NODE_MASK, 1);
    sink.set_u32(ffi::names::VISIBILITY_NODE_MASK, 1);
    sink.set_u32(ffi::names::WIDTH, create.size[0]);
    sink.set_u32(ffi::names::HEIGHT, create.size[1]);
    sink.set_u32(names::WIDTH, create.size[0]);
    sink.set_u32(names::HEIGHT, create.size[1]);
    sink.set_u32(
        names::BACKBUFFER_FORMAT,
        create.backbuffer_format.as_raw() as u32,
    );
    sink.set_u32(names::INTERNAL_WIDTH, create.render_size[0]);
    sink.set_u32(names::INTERNAL_HEIGHT, create.render_size[1]);
    sink.set_u32(
        names::DYNAMIC_RESOLUTION,
        u32::from(create.dynamic_resolution),
    );
    sink.set_u32(names::ALWAYS_PROVIDED_FLAGS, create.always_provided);
    sink.set_u32(names::NEVER_PROVIDED_FLAGS, create.never_provided);
    sink.set_u32(names::ASYNC_CREATE_ENABLED, 0);
    sink.set_u32(names::UI_RECOMPOSITION_ENABLED, 0);
}

/// The images of one evaluation. Every resource is described by an [`ffi::ResourceVk`] the caller keeps alive until
/// the evaluation has returned.
#[derive(Default, Clone, Copy)]
pub struct Resources<'a> {
    /// The final colour of the newest real frame, readable by a shader.
    pub backbuffer: Option<&'a ResourceVk>,
    pub depth: Option<&'a ResourceVk>,
    pub motion_vectors: Option<&'a ResourceVk>,
    /// The scene before any HUD, identical to the back buffer where there is no HUD.
    pub hudless: Option<&'a ResourceVk>,
    /// Premultiplied UI colour and alpha.
    pub ui: Option<&'a ResourceVk>,
    pub ui_alpha: Option<&'a ResourceVk>,
    pub distortion_field: Option<&'a ResourceVk>,
    /// Where the generated frame is written (storage image, same size and format as the back buffer).
    pub output_interpolated: Option<&'a ResourceVk>,
    /// Where NGX copies the real frame to, when the caller does not keep it itself.
    pub output_real: Option<&'a ResourceVk>,
    /// A 4-byte buffer NGX sets to true when the generated frame should not be shown.
    pub output_disable_interpolation: Option<&'a ResourceVk>,
}

impl Resources<'_> {
    /// The resource NGX cannot run without that is missing, if any.
    pub fn missing_required(&self) -> Option<&'static str> {
        [
            ("backbuffer", self.backbuffer.is_none()),
            ("depth", self.depth.is_none()),
            ("motion vectors", self.motion_vectors.is_none()),
            ("interpolated output", self.output_interpolated.is_none()),
        ]
        .into_iter()
        .find_map(|(name, missing)| missing.then_some(name))
    }
}

/// The engine data of one evaluation (`NVSDK_NGX_DLSSG_Opt_Eval_Params`). Matrices are row-major and hold no
/// temporal anti-aliasing jitter.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalOptions {
    /// Frames generated between the two real ones (1 for 2x), and the index of the one this call makes (from 1).
    pub multi_frame_count: u32,
    pub multi_frame_index: u32,
    pub camera_view_to_clip: Mat4,
    pub clip_to_camera_view: Mat4,
    pub clip_to_lens_clip: Mat4,
    pub clip_to_prev_clip: Mat4,
    pub prev_clip_to_clip: Mat4,
    /// Clip-space jitter of this frame.
    pub jitter_offset: [f32; 2],
    /// Factors that bring the motion vectors to pixels of the motion vector image.
    pub mvec_scale: [f32; 2],
    pub camera_pinhole_offset: [f32; 2],
    pub camera_pos: [f32; 3],
    pub camera_up: [f32; 3],
    pub camera_right: [f32; 3],
    pub camera_fwd: [f32; 3],
    pub camera_near: f32,
    pub camera_far: f32,
    /// Vertical field of view in radians.
    pub camera_fov: f32,
    pub camera_aspect_ratio: f32,
    /// The colour images are display-ready HDR.
    pub hdr: bool,
    pub depth_inverted: bool,
    /// The motion vectors include the camera's motion (the game's usual layout).
    pub camera_motion_included: bool,
    /// The frame has nothing to do with the one before: the generated frame is then a copy of the new one.
    pub reset: bool,
    pub automode_override_reset: bool,
    /// The game is showing a menu or a video: no scene is rendered.
    pub not_rendering_game_frames: bool,
    pub ortho_projection: bool,
    pub mvec_invalid_value: f32,
    pub mvec_dilated: bool,
    pub menu_detection: bool,
    pub min_relative_linear_depth_object_separation: f32,
    pub backbuffer_subrect: Subrect,
    pub mvecs_subrect: Subrect,
    pub depth_subrect: Subrect,
    pub hudless_subrect: Subrect,
    pub ui_subrect: Subrect,
    pub ui_alpha_subrect: Subrect,
    pub output_interpolated_subrect: Subrect,
    pub output_real_subrect: Subrect,
}

impl Default for EvalOptions {
    fn default() -> Self {
        EvalOptions {
            multi_frame_count: 1,
            multi_frame_index: 1,
            camera_view_to_clip: IDENTITY,
            clip_to_camera_view: IDENTITY,
            clip_to_lens_clip: IDENTITY,
            clip_to_prev_clip: IDENTITY,
            prev_clip_to_clip: IDENTITY,
            jitter_offset: [0.0; 2],
            mvec_scale: [1.0; 2],
            camera_pinhole_offset: [0.0; 2],
            camera_pos: [0.0; 3],
            camera_up: [0.0, 1.0, 0.0],
            camera_right: [1.0, 0.0, 0.0],
            camera_fwd: [0.0, 0.0, 1.0],
            camera_near: 0.1,
            camera_far: 1000.0,
            camera_fov: std::f32::consts::FRAC_PI_3,
            camera_aspect_ratio: 16.0 / 9.0,
            hdr: false,
            depth_inverted: false,
            camera_motion_included: true,
            reset: false,
            automode_override_reset: false,
            not_rendering_game_frames: false,
            ortho_projection: false,
            mvec_invalid_value: 0.0,
            mvec_dilated: false,
            menu_detection: false,
            min_relative_linear_depth_object_separation: 40.0,
            backbuffer_subrect: Subrect::default(),
            mvecs_subrect: Subrect::default(),
            depth_subrect: Subrect::default(),
            hudless_subrect: Subrect::default(),
            ui_subrect: Subrect::default(),
            ui_alpha_subrect: Subrect::default(),
            output_interpolated_subrect: Subrect::default(),
            output_real_subrect: Subrect::default(),
        }
    }
}

impl EvalOptions {
    pub fn check(&self) -> Result<(), String> {
        if self.multi_frame_count == 0
            || self.multi_frame_index == 0
            || self.multi_frame_index > self.multi_frame_count
        {
            return Err(format!(
                "frame {} of {} generated frames does not exist",
                self.multi_frame_index, self.multi_frame_count
            ));
        }
        if !self.mvec_scale.iter().all(|s| s.is_finite() && *s != 0.0) {
            return Err("the motion vector scale must be finite and not zero".into());
        }
        Ok(())
    }
}

/// Writes the resource pointers.
///
/// # Safety
/// Every resource in `resources` stays alive and unmoved until the evaluation using the map has returned.
pub unsafe fn apply_resources(sink: &mut dyn Sink, resources: &Resources<'_>) {
    let pointer = |r: Option<&ResourceVk>| {
        r.map_or(std::ptr::null_mut(), |r| {
            std::ptr::from_ref(r).cast_mut().cast::<c_void>()
        })
    };
    // SAFETY: forwarded from this function's contract; null clears a resource left from an earlier frame.
    unsafe {
        sink.set_ptr(names::BACKBUFFER, pointer(resources.backbuffer));
        sink.set_ptr(names::MVECS, pointer(resources.motion_vectors));
        sink.set_ptr(names::DEPTH, pointer(resources.depth));
        sink.set_ptr(names::HUDLESS, pointer(resources.hudless));
        sink.set_ptr(names::UI, pointer(resources.ui));
        sink.set_ptr(names::UI_ALPHA, pointer(resources.ui_alpha));
        sink.set_ptr(
            names::BIDIRECTIONAL_DISTORTION_FIELD,
            pointer(resources.distortion_field),
        );
        sink.set_ptr(
            names::OUTPUT_INTERPOLATED,
            pointer(resources.output_interpolated),
        );
        sink.set_ptr(names::OUTPUT_REAL, pointer(resources.output_real));
        sink.set_ptr(
            names::OUTPUT_DISABLE_INTERPOLATION,
            pointer(resources.output_disable_interpolation),
        );
    }
}

fn subrect(sink: &mut dyn Sink, names: SubrectNames, rect: Subrect) {
    let [x, y, width, height] = names.0;
    sink.set_u32(x, rect.x);
    sink.set_u32(y, rect.y);
    sink.set_u32(width, rect.width);
    sink.set_u32(height, rect.height);
}

fn vector(sink: &mut dyn Sink, names: [&str; 3], value: [f32; 3]) {
    for (name, v) in names.into_iter().zip(value) {
        sink.set_f32(name, v);
    }
}

/// Writes the engine data. The matrix pointers refer to `options`.
///
/// # Safety
/// `options` stays alive and unmoved until the evaluation using the map has returned.
pub unsafe fn apply_options(sink: &mut dyn Sink, options: &EvalOptions) {
    sink.set_u32(names::MULTI_FRAME_COUNT, options.multi_frame_count);
    sink.set_u32(names::MULTI_FRAME_INDEX, options.multi_frame_index);
    let matrix = |m: &Mat4| std::ptr::from_ref(m).cast_mut().cast::<c_void>();
    // SAFETY: forwarded from this function's contract.
    unsafe {
        sink.set_ptr(
            names::CAMERA_VIEW_TO_CLIP,
            matrix(&options.camera_view_to_clip),
        );
        sink.set_ptr(
            names::CLIP_TO_CAMERA_VIEW,
            matrix(&options.clip_to_camera_view),
        );
        sink.set_ptr(names::CLIP_TO_LENS_CLIP, matrix(&options.clip_to_lens_clip));
        sink.set_ptr(names::CLIP_TO_PREV_CLIP, matrix(&options.clip_to_prev_clip));
        sink.set_ptr(names::PREV_CLIP_TO_CLIP, matrix(&options.prev_clip_to_clip));
    }
    sink.set_f32(names::JITTER_OFFSET_X, options.jitter_offset[0]);
    sink.set_f32(names::JITTER_OFFSET_Y, options.jitter_offset[1]);
    sink.set_f32(names::MVEC_SCALE_X, options.mvec_scale[0]);
    sink.set_f32(names::MVEC_SCALE_Y, options.mvec_scale[1]);
    sink.set_f32(
        names::CAMERA_PINHOLE_OFFSET_X,
        options.camera_pinhole_offset[0],
    );
    sink.set_f32(
        names::CAMERA_PINHOLE_OFFSET_Y,
        options.camera_pinhole_offset[1],
    );
    vector(sink, names::CAMERA_POS, options.camera_pos);
    vector(sink, names::CAMERA_UP, options.camera_up);
    vector(sink, names::CAMERA_RIGHT, options.camera_right);
    vector(sink, names::CAMERA_FWD, options.camera_fwd);
    sink.set_f32(names::CAMERA_NEAR, options.camera_near);
    sink.set_f32(names::CAMERA_FAR, options.camera_far);
    sink.set_f32(names::CAMERA_FOV, options.camera_fov);
    sink.set_f32(names::CAMERA_ASPECT_RATIO, options.camera_aspect_ratio);
    sink.set_u32(names::COLOR_BUFFERS_HDR, u32::from(options.hdr));
    sink.set_u32(names::DEPTH_INVERTED, u32::from(options.depth_inverted));
    sink.set_u32(
        names::CAMERA_MOTION_INCLUDED,
        u32::from(options.camera_motion_included),
    );
    sink.set_u32(names::RESET, u32::from(options.reset));
    sink.set_u32(
        names::AUTOMODE_OVERRIDE_RESET,
        u32::from(options.automode_override_reset),
    );
    sink.set_u32(
        names::NOT_RENDERING_GAME_FRAMES,
        u32::from(options.not_rendering_game_frames),
    );
    sink.set_u32(names::ORTHO_PROJECTION, u32::from(options.ortho_projection));
    sink.set_f32(names::MVEC_INVALID_VALUE, options.mvec_invalid_value);
    sink.set_u32(names::MVEC_DILATED, u32::from(options.mvec_dilated));
    sink.set_u32(
        names::MENU_DETECTION_ENABLED,
        u32::from(options.menu_detection),
    );
    sink.set_f32(
        names::MIN_RELATIVE_LINEAR_DEPTH_OBJECT_SEPARATION,
        options.min_relative_linear_depth_object_separation,
    );
    subrect(sink, names::SUBRECT_MVECS, options.mvecs_subrect);
    subrect(sink, names::SUBRECT_DEPTH, options.depth_subrect);
    subrect(sink, names::SUBRECT_HUDLESS, options.hudless_subrect);
    subrect(sink, names::SUBRECT_UI, options.ui_subrect);
    subrect(sink, names::SUBRECT_UI_ALPHA, options.ui_alpha_subrect);
    subrect(sink, names::SUBRECT_BACKBUFFER, options.backbuffer_subrect);
    subrect(
        sink,
        names::SUBRECT_OUTPUT_INTERPOLATED,
        options.output_interpolated_subrect,
    );
    subrect(
        sink,
        names::SUBRECT_OUTPUT_REAL,
        options.output_real_subrect,
    );
}

/// A perspective projection from camera view space (looking down +z) to Vulkan clip space (depth 0 to 1, y down), as a
/// row-major matrix applied to row vectors. An infinite `far` gives the infinite-far projection. With `reversed` the
/// near plane maps to 1 and the far plane to 0. Which handedness and vector convention NGX expects is not stated in
/// the guide; this is the layout its parameter documentation implies ("row-major, post-multiplication").
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32, reversed: bool) -> Mat4 {
    let tan = (fov_y * 0.5).tan();
    let (sy, sx) = (1.0 / tan, 1.0 / (tan * aspect));
    let (a, b) = match (far.is_finite(), reversed) {
        (true, false) => (far / (far - near), -near * far / (far - near)),
        (true, true) => (near / (near - far), -near * far / (near - far)),
        (false, false) => (1.0, -near),
        (false, true) => (0.0, near),
    };
    [
        [sx, 0.0, 0.0, 0.0],
        [0.0, sy, 0.0, 0.0],
        [0.0, 0.0, a, 1.0],
        [0.0, 0.0, b, 0.0],
    ]
}

/// The inverse of a matrix, `None` when it is singular.
pub fn invert(m: &Mat4) -> Option<Mat4> {
    let mut a = [[0f64; 8]; 4];
    for (i, row) in a.iter_mut().enumerate() {
        for j in 0..4 {
            row[j] = f64::from(m[i][j]);
        }
        row[4 + i] = 1.0;
    }
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let scale = a[col][col];
        for v in &mut a[col] {
            *v /= scale;
        }
        for row in 0..4 {
            if row != col {
                let factor = a[row][col];
                if factor != 0.0 {
                    let pivot_row = a[col];
                    for (v, p) in a[row].iter_mut().zip(pivot_row) {
                        *v -= factor * p;
                    }
                }
            }
        }
    }
    let mut out = IDENTITY;
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][4 + j] as f32;
        }
    }
    Some(out)
}

/// The engine data for a game that tells us a field of view and the clip distances but not its camera matrices: the
/// projection pair from them, no jitter in the matrices, and identity for the camera's own motion because the game's
/// motion vectors already include it.
pub fn options_from_projection(
    size: [u32; 2],
    fov_y: f32,
    near: f32,
    far: f32,
    depth_inverted: bool,
) -> EvalOptions {
    let aspect = size[0] as f32 / size[1].max(1) as f32;
    let projection = perspective(fov_y, aspect, near, far, depth_inverted);
    EvalOptions {
        camera_view_to_clip: projection,
        clip_to_camera_view: invert(&projection).unwrap_or(IDENTITY),
        camera_near: near,
        camera_far: if far.is_finite() { far } else { 1.0e6 },
        camera_fov: fov_y,
        camera_aspect_ratio: aspect,
        depth_inverted,
        ..EvalOptions::default()
    }
}
