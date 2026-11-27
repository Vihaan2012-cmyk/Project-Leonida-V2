//! Our own temporal upscaler. Every output texel reconstructs the jittered render samples around it with a Gaussian
//! kernel (a narrower one once there is history, since the accumulation does the reconstruction), reprojects last frame's output with the game's motion vectors (Catmull-Rom), clips that history to the
//! mean and spread of the current neighbourhood, and blends with a weight that falls as the history gets longer
//! (up to `history_frames`). `reset`, an out-of-screen reprojection, a large clip or the reactive mask restart the
//! accumulation. RCAS (from [`crate::fsr1`]) sharpens the result.
//!
//! Pass layout (`shaders/temporal.wgsl`): `[colour rgba, depth f32, motion vec2, reactive f32, history rgba,
//! destination rgba, new history rgba, params]` with params `[render w, render h, out w, out h, jitter x, jitter y,
//! exposure, motion scale x, motion scale y, flags (1 reset, 2 depth, 4 motion, 8 reactive, 16 reversed depth),
//! history frames]`, the floats as bits. History holds compressed radiance in rgb and the number of frames
//! accumulated in alpha, at output resolution.

use crate::compute::{SharedCompute, floats, groups_for, words};
use crate::fsr1::rcas_lobe;
use crate::{ColorSpace, Image, UpscaleError, UpscaleInputs, Upscaler, UpscalerKind, shaders};

#[derive(Debug, Clone, Copy)]
pub struct TemporalConfig {
    /// Frames of history at which the blend weight stops falling.
    pub history_frames: f32,
}

impl Default for TemporalConfig {
    fn default() -> Self {
        TemporalConfig {
            history_frames: 16.0,
        }
    }
}

pub struct TemporalUpscaler {
    compute: SharedCompute,
    config: TemporalConfig,
    history: Vec<u32>,
    history_size: [u32; 2],
}

impl TemporalUpscaler {
    pub fn new(compute: SharedCompute, config: TemporalConfig) -> Self {
        TemporalUpscaler {
            compute,
            config,
            history: Vec::new(),
            history_size: [0, 0],
        }
    }
}

impl Upscaler for TemporalUpscaler {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Temporal
    }

    fn name(&self) -> &'static str {
        "Leonida temporal upscaler"
    }

    fn uses_jitter(&self) -> bool {
        true
    }

    fn upscale(&mut self, inputs: &UpscaleInputs<'_>) -> Result<Image, UpscaleError> {
        inputs.validate()?;
        let [rw, rh] = inputs.render_size;
        let [ow, oh] = inputs.output_size;
        let linear = inputs.color_space == ColorSpace::Linear;
        let texels = (ow * oh) as usize;
        let mut reset = inputs.reset;
        if self.history_size != [ow, oh] || self.history.len() != texels * 4 {
            reset = true;
            self.history = vec![0; texels * 4];
            self.history_size = [ow, oh];
        }

        let mut flags = reset as u32;
        let mut color = words(&inputs.color.rgba_f32(rw, rh, "colour")?);
        let mut depth = vec![0u32; 1];
        if let Some(image) = &inputs.depth {
            depth = words(&image.channel_f32(0, rw, rh, "depth")?);
            flags |= 2;
        }
        let mut motion = vec![0u32; 2];
        if let Some(image) = &inputs.motion {
            motion = words(&image.rg_f32(rw, rh, "motion vectors")?);
            flags |= 4;
        }
        let mut reactive = vec![0u32; 1];
        if let Some(image) = &inputs.reactive {
            reactive = words(&image.channel_f32(0, rw, rh, "reactive mask")?);
            flags |= 8;
        }
        if inputs.depth_reversed {
            flags |= 16;
        }
        let mut params = [
            rw,
            rh,
            ow,
            oh,
            inputs.jitter[0].to_bits(),
            inputs.jitter[1].to_bits(),
            inputs.exposure.to_bits(),
            inputs.motion_vector_scale[0].to_bits(),
            inputs.motion_vector_scale[1].to_bits(),
            flags,
            self.config.history_frames.to_bits(),
        ];
        let mut dst = vec![0u32; texels * 4];
        let mut history = std::mem::take(&mut self.history);
        let mut new_history = vec![0u32; texels * 4];
        let ran = self.compute.dispatch(
            shaders::TEMPORAL,
            &mut [
                &mut color,
                &mut depth,
                &mut motion,
                &mut reactive,
                &mut history,
                &mut dst,
                &mut new_history,
                &mut params,
            ],
            groups_for(ow, oh),
        );
        if let Err(error) = ran {
            self.history_size = [0, 0];
            return Err(error);
        }
        self.history = new_history;

        if inputs.sharpness > 0.0 {
            let mut out = vec![0u32; texels * 4];
            let mut rcas = [
                ow,
                oh,
                linear as u32,
                linear as u32,
                rcas_lobe(inputs.sharpness).to_bits(),
            ];
            self.compute.dispatch(
                shaders::RCAS,
                &mut [&mut dst, &mut out, &mut rcas],
                groups_for(ow, oh),
            )?;
            dst = out;
        }
        Image::from_rgba_f32(ow, oh, inputs.output_format, &floats(&dst))
    }

    fn reset(&mut self) {
        self.history.clear();
        self.history_size = [0, 0];
    }
}
