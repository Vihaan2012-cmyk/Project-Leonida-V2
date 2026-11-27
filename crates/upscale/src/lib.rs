//! Upscaling and frame generation fed by the game's own inputs: colour, depth, motion vectors and jitter.
//!
//! The game already drives AMD's upscaler through the console's FidelityFX loader library; the code that implements
//! those entry points builds an [`UpscaleInputs`] from what the game hands over and calls an [`Upscaler`].
//!
//! - [`fsr1`]: AMD FidelityFX Super Resolution 1 (EASU + RCAS), MIT licensed, ported to WGSL.
//! - [`temporal`]: our own temporal upscaler (jitter-aware accumulation with history rectification), because FSR 2
//!   and 3 are C++ SDKs.
//! - [`dlss`]: optional NVIDIA DLSS through the player's own NGX runtime; never shipped, only probed at run time.
//! - [`framegen`]: interpolated frames from motion vectors and depth, with a pacing helper for the presenter.
//!
//! All shaders are WGSL compiled to SPIR-V by `naga` in `build.rs`; [`shaders`] exposes them so a renderer with
//! persistent resources can build its own pipelines.
#![allow(unsafe_code)]

pub mod compute;
pub mod dlss;
pub mod framegen;
pub mod fsr1;
pub mod image;
pub mod temporal;

pub use compute::{Compute, SharedCompute};
pub use image::{Image, ImageRef, PixelFormat};

/// The compiled passes. Each takes its storage buffers at set 0, bindings 0.. in the order the Rust module documents,
/// with the parameter words last, and runs 8x8 workgroups over the output.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shaders.rs"));
}

#[derive(Debug, thiserror::Error)]
pub enum UpscaleError {
    #[error("GPU error: {0}")]
    Gpu(#[from] gpu::GpuError),
    #[error("{what}: {reason}")]
    BadInput { what: &'static str, reason: String },
    #[error("{format:?} cannot be used for {what}")]
    UnsupportedFormat {
        format: PixelFormat,
        what: &'static str,
    },
    #[error("{name} is not available: {reason}")]
    Unavailable { name: &'static str, reason: String },
    #[error("{name} is not implemented: {reason}")]
    Unimplemented { name: &'static str, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpscalerKind {
    Fsr1,
    Temporal,
    Dlss,
}

/// How the colour values are encoded. FSR 1 expects perceptual values; linear input is converted internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    /// Display-referred values already in a perceptual (gamma) encoding.
    Perceptual,
    /// Linear light, possibly above 1.0.
    Linear,
}

/// Everything one upscale call needs. Images may be larger than the sizes given (a dynamic-resolution viewport in the
/// top-left corner); only the top-left `render_size` texels are read.
#[derive(Debug, Clone, Copy)]
pub struct UpscaleInputs<'a> {
    pub color: ImageRef<'a>,
    /// One value per texel (the first channel), same viewport as the colour.
    pub depth: Option<ImageRef<'a>>,
    /// Two channels per texel: the vector from this frame's position to the previous frame's, times
    /// `motion_vector_scale`, in render-resolution pixels (+x right, +y down).
    pub motion: Option<ImageRef<'a>>,
    /// Optional 0..1 mask of texels whose history must not be trusted (particles, reflections).
    pub reactive: Option<ImageRef<'a>>,
    pub render_size: [u32; 2],
    pub output_size: [u32; 2],
    pub output_format: PixelFormat,
    pub color_space: ColorSpace,
    /// The sub-pixel offset the camera was jittered by this frame, in render-resolution pixels (+x right, +y down):
    /// the sample in texel `i` sits at `i + 0.5 - jitter` of the unjittered image.
    pub jitter: [f32; 2],
    pub motion_vector_scale: [f32; 2],
    /// Larger depth means nearer (reversed Z).
    pub depth_reversed: bool,
    pub camera_near: f32,
    pub camera_far: f32,
    pub camera_fov_y: f32,
    /// How much brighter `color` is than scene radiance (the game's pre-exposure); history is kept in radiance.
    pub exposure: f32,
    /// 0 turns RCAS off; 1 is the sharpest. Used by every upscaler that sharpens.
    pub sharpness: f32,
    /// Drop all history (camera cut, resolution change).
    pub reset: bool,
    pub frame_time_ms: f32,
}

impl<'a> UpscaleInputs<'a> {
    pub fn new(color: ImageRef<'a>, render_size: [u32; 2], output_size: [u32; 2]) -> Self {
        UpscaleInputs {
            color,
            depth: None,
            motion: None,
            reactive: None,
            render_size,
            output_size,
            output_format: PixelFormat::Rgba16Float,
            color_space: ColorSpace::Linear,
            jitter: [0.0, 0.0],
            motion_vector_scale: [1.0, 1.0],
            depth_reversed: true,
            camera_near: 0.1,
            camera_far: f32::INFINITY,
            camera_fov_y: 1.0,
            exposure: 1.0,
            sharpness: 0.8,
            reset: false,
            frame_time_ms: 16.6,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), UpscaleError> {
        let [rw, rh] = self.render_size;
        let [ow, oh] = self.output_size;
        if rw == 0 || rh == 0 || ow == 0 || oh == 0 {
            return Err(UpscaleError::BadInput {
                what: "sizes",
                reason: "a size is zero".into(),
            });
        }
        if !self.output_format.is_color() {
            return Err(UpscaleError::UnsupportedFormat {
                format: self.output_format,
                what: "the upscaled output",
            });
        }
        if !(self.exposure > 0.0) {
            return Err(UpscaleError::BadInput {
                what: "exposure",
                reason: "must be positive".into(),
            });
        }
        Ok(())
    }
}

pub trait Upscaler {
    fn kind(&self) -> UpscalerKind;
    fn name(&self) -> &'static str;
    /// Whether the result improves with camera jitter (and so needs motion vectors).
    fn uses_jitter(&self) -> bool;
    fn jitter_phase_count(&self, render_width: u32, output_width: u32) -> u32 {
        jitter_phase_count(render_width, output_width)
    }
    fn upscale(&mut self, inputs: &UpscaleInputs<'_>) -> Result<Image, UpscaleError>;
    /// Forget all history.
    fn reset(&mut self);
}

/// Inputs of one frame-generation call: the real frame that just finished, at display resolution.
#[derive(Debug, Clone, Copy)]
pub struct FrameGenInputs<'a> {
    pub color: ImageRef<'a>,
    /// Depth and motion vectors are at `render_size`; colour is at its own size.
    pub depth: ImageRef<'a>,
    pub motion: ImageRef<'a>,
    pub render_size: [u32; 2],
    pub output_format: PixelFormat,
    pub motion_vector_scale: [f32; 2],
    pub depth_reversed: bool,
    pub camera_near: f32,
    pub camera_far: f32,
    /// Where between the previous real frame (0) and this one (1) the generated frame sits.
    pub phase: f32,
    /// Relative depth difference up to which two surfaces count as the same.
    pub depth_tolerance: f32,
    pub reset: bool,
}

impl<'a> FrameGenInputs<'a> {
    pub fn new(
        color: ImageRef<'a>,
        depth: ImageRef<'a>,
        motion: ImageRef<'a>,
        render_size: [u32; 2],
    ) -> Self {
        FrameGenInputs {
            color,
            depth,
            motion,
            render_size,
            output_format: PixelFormat::Rgba16Float,
            motion_vector_scale: [1.0, 1.0],
            depth_reversed: true,
            camera_near: 0.1,
            camera_far: f32::INFINITY,
            phase: 0.5,
            depth_tolerance: 0.03,
            reset: false,
        }
    }
}

pub trait FrameGenerator {
    fn name(&self) -> &'static str;
    /// Takes the newest real frame and returns the frame between the previous real frame and it, or `None` when
    /// there is no previous frame (first call, after a reset or a size change). The caller presents the generated
    /// frame half a frame time before this real one, see [`framegen::Pacer`].
    fn generate(&mut self, inputs: &FrameGenInputs<'_>) -> Result<Option<Image>, UpscaleError>;
    fn reset(&mut self);
}

/// Render-resolution divisors of the usual quality presets (the values AMD's upscalers use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityMode {
    Native,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}

impl QualityMode {
    pub fn ratio(self) -> f32 {
        match self {
            QualityMode::Native => 1.0,
            QualityMode::Quality => 1.5,
            QualityMode::Balanced => 1.7,
            QualityMode::Performance => 2.0,
            QualityMode::UltraPerformance => 3.0,
        }
    }

    pub fn render_size(self, output: [u32; 2]) -> [u32; 2] {
        let r = self.ratio();
        [
            ((output[0] as f32 / r) as u32).max(1),
            ((output[1] as f32 / r) as u32).max(1),
        ]
    }
}

/// Jitter positions per cycle: 8 at native resolution, growing with the square of the upscale ratio.
pub fn jitter_phase_count(render_width: u32, output_width: u32) -> u32 {
    let ratio = output_width as f32 / render_width.max(1) as f32;
    (8.0 * ratio * ratio).round().max(1.0) as u32
}

/// The Halton (2, 3) jitter for frame `index`, in -0.5..0.5 render pixels.
pub fn jitter_offset(index: u32, phase_count: u32) -> [f32; 2] {
    let i = index % phase_count.max(1) + 1;
    [halton(i, 2) - 0.5, halton(i, 3) - 0.5]
}

fn halton(mut index: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0, 0.0);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

/// The texture LOD bias that keeps texture sharpness at the output resolution while rendering smaller.
pub fn texture_lod_bias(render_width: u32, output_width: u32) -> f32 {
    (render_width as f32 / output_width as f32).log2() - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_jitter_follow_the_amd_conventions() {
        assert_eq!(
            QualityMode::Performance.render_size([3840, 2160]),
            [1920, 1080]
        );
        assert_eq!(QualityMode::Quality.render_size([1920, 1080]), [1280, 720]);
        assert_eq!(jitter_phase_count(1920, 1920), 8);
        assert_eq!(jitter_phase_count(1280, 1920), 18);
        let offsets: Vec<_> = (0..8).map(|i| jitter_offset(i, 8)).collect();
        assert_eq!(offsets[0], [0.0, 1.0 / 3.0 - 0.5]);
        assert!(offsets.iter().flatten().all(|v| (-0.5..0.5).contains(v)));
        assert_eq!(jitter_offset(8, 8), offsets[0]);
        assert!((texture_lod_bias(960, 1920) - -2.0).abs() < 1e-6);
    }
}
