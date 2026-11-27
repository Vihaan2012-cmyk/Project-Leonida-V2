//! Frame generation: a frame between the previous real frame and the newest one, built from the game's motion
//! vectors and depth, and [`Pacer`], which tells the presenter when to show it.
//!
//! The generator keeps the previous real frame (colour and depth) itself. For a phase `t` between 0 (previous) and
//! 1 (newest), three passes run on the GPU:
//!
//! 1. every display texel of the newest frame is splatted forward along `(1 - t) * motion`; the nearest surface wins
//!    per target texel (an atomic max over depth, `shaders/fg_key.wgsl`), then its motion is written out
//!    (`shaders/fg_field.wgsl`). Texels nothing lands on are disocclusions.
//! 2. each target texel samples the newest frame backwards along `-(1 - t) * motion` and the previous frame along
//!    `+t * motion` (`shaders/fg_interp.wgsl`). A sample is trusted only if the depth found there agrees with the
//!    splatted surface's depth, so a revealed background takes the frame that still shows it and a covered one the
//!    frame that does not; when both agree the two are mixed by `t`. Holes take the farthest (background) motion
//!    within six texels.
//!
//! Buffer layouts: key pass `[depth, motion, key, params]`, field pass `[depth, motion, key, field, params]`,
//! interpolation `[colour, previous colour, depth, previous depth, key, field, destination, params]`; params
//! `[w, h, render w, render h, phase, flags (1 reversed depth), depth tolerance, near / far, mv scale x, mv scale y]`.
//!
//! Optional DLSS frame generation is not offered: NVIDIA ships it through Streamline's swap-chain proxy, which cannot
//! be driven from public documentation alone and is not loaded here.

use std::time::Duration;

use crate::compute::{SharedCompute, floats, groups_for, words};
use crate::{FrameGenInputs, FrameGenerator, Image, UpscaleError, shaders};

struct Previous {
    size: [u32; 2],
    render_size: [u32; 2],
    color: Vec<u32>,
    depth: Vec<u32>,
}

pub struct InterpolatingFrameGenerator {
    compute: SharedCompute,
    previous: Option<Previous>,
}

impl InterpolatingFrameGenerator {
    pub fn new(compute: SharedCompute) -> Self {
        InterpolatingFrameGenerator {
            compute,
            previous: None,
        }
    }
}

impl FrameGenerator for InterpolatingFrameGenerator {
    fn name(&self) -> &'static str {
        "Leonida frame interpolation"
    }

    fn generate(&mut self, inputs: &FrameGenInputs<'_>) -> Result<Option<Image>, UpscaleError> {
        let [w, h] = [inputs.color.width, inputs.color.height];
        let [rw, rh] = inputs.render_size;
        if w == 0 || h == 0 || rw == 0 || rh == 0 {
            return Err(UpscaleError::BadInput {
                what: "sizes",
                reason: "a size is zero".into(),
            });
        }
        if !(inputs.phase > 0.0 && inputs.phase < 1.0) {
            return Err(UpscaleError::BadInput {
                what: "phase",
                reason: "must be between 0 and 1, exclusive".into(),
            });
        }
        if !inputs.output_format.is_color() {
            return Err(UpscaleError::UnsupportedFormat {
                format: inputs.output_format,
                what: "the generated frame",
            });
        }
        let color = words(&inputs.color.rgba_f32(w, h, "colour")?);
        let depth = words(&inputs.depth.channel_f32(0, rw, rh, "depth")?);
        let mut motion = words(&inputs.motion.rg_f32(rw, rh, "motion vectors")?);

        let previous = self
            .previous
            .take()
            .filter(|p| !inputs.reset && p.size == [w, h] && p.render_size == [rw, rh]);
        let generated = match &previous {
            None => None,
            Some(previous) => {
                let k = if inputs.camera_far.is_finite() && inputs.camera_far > inputs.camera_near {
                    inputs.camera_near / inputs.camera_far
                } else {
                    0.0
                };
                let mut params = [
                    w,
                    h,
                    rw,
                    rh,
                    inputs.phase.to_bits(),
                    inputs.depth_reversed as u32,
                    inputs.depth_tolerance.to_bits(),
                    k.to_bits(),
                    inputs.motion_vector_scale[0].to_bits(),
                    inputs.motion_vector_scale[1].to_bits(),
                ];
                let texels = (w * h) as usize;
                let groups = groups_for(w, h);
                let mut key = vec![0u32; texels];
                let mut field = vec![0u32; texels * 2];
                let mut cur_depth = depth.clone();
                self.compute.dispatch(
                    shaders::FG_KEY,
                    &mut [&mut cur_depth, &mut motion, &mut key, &mut params],
                    groups,
                )?;
                self.compute.dispatch(
                    shaders::FG_FIELD,
                    &mut [
                        &mut cur_depth,
                        &mut motion,
                        &mut key,
                        &mut field,
                        &mut params,
                    ],
                    groups,
                )?;
                let mut cur_color = color.clone();
                let mut prev_color = previous.color.clone();
                let mut prev_depth = previous.depth.clone();
                let mut dst = vec![0u32; texels * 4];
                self.compute.dispatch(
                    shaders::FG_INTERP,
                    &mut [
                        &mut cur_color,
                        &mut prev_color,
                        &mut cur_depth,
                        &mut prev_depth,
                        &mut key,
                        &mut field,
                        &mut dst,
                        &mut params,
                    ],
                    groups,
                )?;
                Some(Image::from_rgba_f32(
                    w,
                    h,
                    inputs.output_format,
                    &floats(&dst),
                )?)
            }
        };
        self.previous = Some(Previous {
            size: [w, h],
            render_size: [rw, rh],
            color,
            depth,
        });
        Ok(generated)
    }

    fn reset(&mut self) {
        self.previous = None;
    }
}

/// When to show the frames of one real-frame interval. `now` below is the moment the newest real frame arrived;
/// the previous real frame is presented at `now` by the previous plan's `real_delay`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentPlan {
    pub generated_delay: Duration,
    pub real_delay: Duration,
    pub phase: f32,
}

/// Smooths the real-frame interval and places the generated frame at `phase` of it. The presenter feeds it the
/// arrival time of every real frame (any monotonic clock, as a duration since some start).
pub struct Pacer {
    phase: f32,
    smoothing: f64,
    max_interval: Duration,
    last: Option<Duration>,
    interval: Option<f64>,
}

impl Pacer {
    pub fn new(phase: f32) -> Self {
        Pacer {
            phase: phase.clamp(0.05, 0.95),
            smoothing: 0.2,
            max_interval: Duration::from_millis(125),
            last: None,
            interval: None,
        }
    }

    /// `None` for the first frame and after a hitch longer than 125 ms: show the real frame alone.
    pub fn on_real_frame(&mut self, now: Duration) -> Option<PresentPlan> {
        let last = self.last.replace(now)?;
        let interval = now.checked_sub(last)?;
        if interval > self.max_interval || interval.is_zero() {
            self.interval = None;
            return None;
        }
        let seconds = interval.as_secs_f64();
        let smoothed = match self.interval {
            None => seconds,
            Some(previous) => previous + (seconds - previous) * self.smoothing,
        };
        self.interval = Some(smoothed);
        Some(PresentPlan {
            generated_delay: Duration::from_secs_f64(smoothed * self.phase as f64),
            real_delay: Duration::from_secs_f64(smoothed),
            phase: self.phase,
        })
    }

    pub fn reset(&mut self) {
        self.last = None;
        self.interval = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_pacer_places_the_generated_frame_inside_the_smoothed_interval() {
        let mut pacer = Pacer::new(0.5);
        assert_eq!(pacer.on_real_frame(ms(0)), None);
        let plan = pacer.on_real_frame(ms(33)).unwrap();
        assert_eq!(plan.real_delay, ms(33));
        assert_eq!(plan.generated_delay.as_micros(), 16_500);
        let plan = pacer.on_real_frame(ms(83)).unwrap();
        assert!(plan.real_delay > ms(33) && plan.real_delay < ms(50));
        assert_eq!(
            plan.generated_delay.as_secs_f64() * 2.0,
            plan.real_delay.as_secs_f64()
        );
    }

    #[test]
    fn a_hitch_or_a_reset_restarts_pacing() {
        let mut pacer = Pacer::new(0.5);
        pacer.on_real_frame(ms(0));
        pacer.on_real_frame(ms(16)).unwrap();
        assert_eq!(pacer.on_real_frame(ms(400)), None);
        assert_eq!(pacer.on_real_frame(ms(416)).unwrap().real_delay, ms(16));
        pacer.reset();
        assert_eq!(pacer.on_real_frame(ms(432)), None);
    }
}
