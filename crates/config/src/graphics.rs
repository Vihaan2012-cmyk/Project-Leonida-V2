//! `graphics.toml`: resolution, upscaling, frame rate, window, filtering and field of view.

use serde::{Deserialize, Serialize};

use crate::{Problems, Section};

pub use crate::sampler::adjust_sampler;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Upscaler {
    #[default]
    Off,
    /// Spatial upscaling of the single frame.
    Fsr1,
    /// Upscaling from several frames.
    Temporal,
    Dlss,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameRateMode {
    /// The console's pacing: 30 or 60 frames a second as the game asks.
    #[default]
    Console,
    /// At most `frame_rate_cap` frames a second.
    Cap,
    Uncapped,
    /// One frame per screen refresh.
    Vsync,
}

/// `frame_rate_mode` and `frame_rate_cap` together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRate {
    Console,
    Cap(u32),
    Uncapped,
    Vsync,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    #[default]
    Windowed,
    /// A window without a border that covers the screen.
    Borderless,
    Fullscreen,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AspectMode {
    /// The game's 16:9 picture, with bars where the window is wider or taller.
    #[default]
    Letterbox,
    /// The picture takes the window's shape, showing more of the world sideways or vertically.
    Native,
    /// The 16:9 picture stretched over the window.
    Stretch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineNotReady {
    /// The draw waits for its pipeline to finish building: nothing is missing, the frame takes as long as the build.
    #[default]
    Wait,
    /// The draw is left out until the pipeline is built: no stall, but objects are missing for a frame or more.
    Skip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Graphics {
    pub resolution_scale: f32,
    pub upscaler: Upscaler,
    pub frame_generation: bool,
    pub frame_rate_mode: FrameRateMode,
    pub frame_rate_cap: u32,
    pub window_mode: WindowMode,
    pub window_width: u32,
    pub window_height: u32,
    pub hdr: bool,
    pub anisotropic_filtering: u32,
    pub texture_lod_bias: f32,
    pub aspect_mode: AspectMode,
    pub fov_offset: f32,
    pub fov_scale: f32,
    pub pipeline_not_ready: PipelineNotReady,
    /// With a frame rate other than the console's, also patch the game logic that counts frames or floors its
    /// step at 1/60 s, so it keeps its 60 fps speed (`enhancements::frame_timers`).
    pub fix_frame_timers: bool,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            resolution_scale: 1.0,
            upscaler: Upscaler::Off,
            frame_generation: false,
            frame_rate_mode: FrameRateMode::Console,
            frame_rate_cap: 60,
            window_mode: WindowMode::Windowed,
            window_width: 1920,
            window_height: 1080,
            hdr: false,
            anisotropic_filtering: 16,
            texture_lod_bias: 0.0,
            aspect_mode: AspectMode::Letterbox,
            fov_offset: 0.0,
            fov_scale: 1.0,
            pipeline_not_ready: PipelineNotReady::Wait,
            fix_frame_timers: true,
        }
    }
}

impl Graphics {
    pub fn frame_rate(&self) -> FrameRate {
        match self.frame_rate_mode {
            FrameRateMode::Console => FrameRate::Console,
            FrameRateMode::Cap => FrameRate::Cap(self.frame_rate_cap),
            FrameRateMode::Uncapped => FrameRate::Uncapped,
            FrameRateMode::Vsync => FrameRate::Vsync,
        }
    }

    /// The texture LOD bias to add to every sampler: `texture_lod_bias`, plus `log2(resolution_scale)` while an
    /// upscaler renders below the output size, so textures keep the sharpness of the output resolution.
    pub fn effective_lod_bias(&self) -> f32 {
        let compensation = if self.upscaler != Upscaler::Off && self.resolution_scale < 1.0 {
            self.resolution_scale.log2()
        } else {
            0.0
        };
        self.texture_lod_bias + compensation
    }
}

impl Section for Graphics {
    const FILE: &'static str = "graphics.toml";
    fn default_text() -> String {
        "\
# Leonida graphics settings. Delete this file (or run `leonida config reset`) to get it back with the defaults.

# How big the game renders, as a share of the window: 1.0 is the window's size, 0.5 half of it in each direction,
# 2.0 twice (supersampling). 0.25 to 2.0. Below 1.0 an upscaler brings the picture back to the window's size.
resolution_scale = 1.0

# What brings a smaller picture up to the window's size: \"off\", \"fsr1\" (spatial), \"temporal\" (from several
# frames) or \"dlss\" (NVIDIA GPUs only).
upscaler = \"off\"

# Makes extra frames between the game's own to look smoother. Adds a little delay.
frame_generation = false

# \"console\" keeps the console's 30 or 60 frames a second; \"cap\" stops at frame_rate_cap; \"uncapped\" runs as
# fast as it can; \"vsync\" shows one frame per screen refresh.
frame_rate_mode = \"console\"
# Used only when frame_rate_mode is \"cap\". 15 to 1000.
frame_rate_cap = 60

# \"windowed\", \"borderless\" (no border, covers the screen) or \"fullscreen\".
window_mode = \"windowed\"
# The window's size in pixels when windowed. 640 to 16384.
window_width = 1920
window_height = 1080

# High dynamic range output. Needs a screen that supports it.
hdr = false

# Sharpness of textures seen at a slant: 1 (off), 2, 4, 8 or 16. The game's own level is never lowered.
anisotropic_filtering = 16

# Added to the level of detail every texture is sampled at. Negative is sharper, positive blurrier. -4.0 to 4.0.
# With an upscaler and a resolution_scale below 1.0, Leonida already adds the right amount on top of this.
texture_lod_bias = 0.0

# What happens when the window is not 16:9: \"letterbox\" (bars), \"native\" (the picture fills the window) or
# \"stretch\" (the 16:9 picture is stretched).
aspect_mode = \"letterbox\"

# Degrees added to the game's field of view. -30.0 to 30.0.
fov_offset = 0.0

# Factor on the game's vertical field of view, applied to the game's code: 1.0 leaves it alone, 1.2 shows more of
# the world. 0.5 to 1.8.
fov_scale = 1.0

# The game's shaders are turned into GPU pipelines on background threads. When a draw needs one that is not
# finished: \"wait\" holds the frame until it is (nothing is missing, but the game may stutter), \"skip\" leaves
# the draw out until it is (no stutter, but objects can be missing for a moment).
pipeline_not_ready = \"wait\"

# Above 60 fps, some of the game's timers count frames and some steps stop at 1/60 s, so they would run fast.
# true patches them to keep their 60 fps speed; false leaves them as the console runs them.
fix_frame_timers = true
"
        .to_string()
    }

    fn check(&self, problems: &mut Problems) {
        problems.range_f32("resolution_scale", self.resolution_scale, 0.25, 2.0);
        problems.range_u32("frame_rate_cap", self.frame_rate_cap, 15, 1000);
        problems.range_u32("window_width", self.window_width, 640, 16384);
        problems.range_u32("window_height", self.window_height, 640, 16384);
        if ![1, 2, 4, 8, 16].contains(&self.anisotropic_filtering) {
            problems.add(
                "anisotropic_filtering",
                format!(
                    "must be 1, 2, 4, 8 or 16, got {}",
                    self.anisotropic_filtering
                ),
            );
        }
        problems.range_f32("texture_lod_bias", self.texture_lod_bias, -4.0, 4.0);
        problems.range_f32("fov_offset", self.fov_offset, -30.0, 30.0);
        problems.range_f32("fov_scale", self.fov_scale, 0.5, 1.8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_parses() {
        let text = "\
upscaler = \"dlss\"\nframe_generation = true\nframe_rate_mode = \"uncapped\"\nwindow_mode = \"fullscreen\"\n\
hdr = true\naspect_mode = \"native\"\nfov_offset = -5\ntexture_lod_bias = -1\n";
        let graphics = Graphics::parse(text).unwrap();
        assert_eq!(graphics.upscaler, Upscaler::Dlss);
        assert_eq!(graphics.frame_rate(), FrameRate::Uncapped);
        assert_eq!(graphics.window_mode, WindowMode::Fullscreen);
        assert_eq!(graphics.aspect_mode, AspectMode::Native);
        assert_eq!(graphics.fov_offset, -5.0);
        assert_eq!(graphics.texture_lod_bias, -1.0);
        assert!(graphics.hdr && graphics.frame_generation);
        assert_eq!(
            Graphics::parse("frame_rate_mode = \"vsync\"")
                .unwrap()
                .frame_rate(),
            FrameRate::Vsync
        );
    }

    #[test]
    fn out_of_range_values_are_reported_together() {
        let text = "\
resolution_scale = 0.1\nframe_rate_cap = 5\nwindow_width = 100\ntexture_lod_bias = 9.5\nfov_offset = 40\n\
anisotropic_filtering = 0\n";
        let error = Graphics::parse(text).unwrap_err().to_string();
        assert!(
            error.starts_with("graphics.toml has 6 problem(s):"),
            "{error}"
        );
        assert!(
            error.contains("frame_rate_cap: must be between 15 and 1000, got 5"),
            "{error}"
        );
        assert!(
            error.contains("window_width: must be between 640 and 16384, got 100"),
            "{error}"
        );
        assert!(
            error.contains("texture_lod_bias: must be between -4 and 4, got 9.5"),
            "{error}"
        );
        assert!(
            error.contains("fov_offset: must be between -30 and 30, got 40"),
            "{error}"
        );
    }

    #[test]
    fn a_draw_waits_for_an_unbuilt_pipeline_unless_told_to_skip() {
        assert_eq!(
            Graphics::default().pipeline_not_ready,
            PipelineNotReady::Wait
        );
        assert_eq!(
            Graphics::parse("pipeline_not_ready = \"skip\"")
                .unwrap()
                .pipeline_not_ready,
            PipelineNotReady::Skip
        );
        assert!(Graphics::parse("pipeline_not_ready = \"later\"").is_err());
    }

    #[test]
    fn fov_scale_defaults_to_one_and_is_range_checked() {
        assert_eq!(Graphics::default().fov_scale, 1.0);
        assert_eq!(Graphics::parse("fov_scale = 1.25").unwrap().fov_scale, 1.25);
        let error = Graphics::parse("fov_scale = 2.5").unwrap_err().to_string();
        assert!(
            error.contains("fov_scale: must be between 0.5 and 1.8, got 2.5"),
            "{error}"
        );
    }

    #[test]
    fn nan_is_not_in_range() {
        let graphics = Graphics {
            resolution_scale: f32::NAN,
            ..Graphics::default()
        };
        assert!(graphics.validate().is_err());
    }

    #[test]
    fn upscaling_adds_mip_bias_only_below_native() {
        let mut graphics = Graphics {
            texture_lod_bias: -0.5,
            ..Graphics::default()
        };
        assert_eq!(graphics.effective_lod_bias(), -0.5);
        graphics.resolution_scale = 0.5;
        assert_eq!(graphics.effective_lod_bias(), -0.5);
        graphics.upscaler = Upscaler::Fsr1;
        assert_eq!(graphics.effective_lod_bias(), -1.5);
        graphics.resolution_scale = 1.5;
        assert_eq!(graphics.effective_lod_bias(), -0.5);
    }
}
