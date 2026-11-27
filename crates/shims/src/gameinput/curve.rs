//! What the mouse has to push the right stick through to turn the camera by a fixed angle per count.
//!
//! The executable (build 1.0.2.1) turns a raw stick into the value the rest of the game reads in
//! `sub_001fede0`, a radial dead zone with a rescale: the raw length `r` becomes `clamp((r - inner) / (1 - inner -
//! outer), 0, 1)` along the same direction, and everything inside `inner` becomes zero. The two zones are the
//! settings object's override fields at `+0x470` to `+0x47c` of the pad object, set up as 0.25 inner and 0.0 outer
//! (constants at `0x617c040`). What the camera then does with that value (its look curve and its speed up while the
//! stick is held) is data driven and has not been measured, so [`StickResponse::exponent`] stays 1.0 and
//! [`LookSettings`] holds the full-deflection turn rate as a setting.

/// The game's stick processing, forward and backward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StickResponse {
    pub inner: f32,
    pub outer: f32,
    /// Power applied to the rescaled length by whatever reads it; 1.0 is linear.
    pub exponent: f32,
}

impl Default for StickResponse {
    fn default() -> Self {
        StickResponse {
            inner: 0.25,
            outer: 0.0,
            exponent: 1.0,
        }
    }
}

impl StickResponse {
    fn span(&self) -> f32 {
        (1.0 - self.inner - self.outer).max(1e-3)
    }

    /// The length the game reads for a raw stick length.
    pub fn forward(&self, raw: f32) -> f32 {
        let t = ((raw - self.inner) / self.span()).clamp(0.0, 1.0);
        t.powf(self.exponent)
    }

    /// The raw stick length that makes the game read `effective`. Zero stays zero, and the smallest movement
    /// starts just outside the dead zone.
    pub fn inverse(&self, effective: f32) -> f32 {
        if effective <= 0.0 {
            return 0.0;
        }
        let t = effective.min(1.0).powf(1.0 / self.exponent.max(1e-3));
        self.inner + t * self.span()
    }

    /// The raw stick vector that makes the game read `effective` (a vector of length at most 1).
    pub fn inverse_vector(&self, effective: [f32; 2]) -> [f32; 2] {
        let length = effective[0].hypot(effective[1]);
        if length <= 0.0 {
            return [0.0; 2];
        }
        let scale = self.inverse(length) / length;
        [effective[0] * scale, effective[1] * scale]
    }
}

/// How mouse counts become a turn rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookSettings {
    pub response: StickResponse,
    /// Degrees one count turns the camera at sensitivity 1.0.
    pub degrees_per_count: f32,
    /// Degrees per second the camera turns at full deflection, as far as the game's own look speed is known.
    pub full_rate: f32,
    pub sensitivity: [f32; 2],
    pub invert: [bool; 2],
    /// 0 to 1; how long the movement is averaged, from one frame to about a tenth of a second.
    pub smoothing: f32,
}

impl Default for LookSettings {
    fn default() -> Self {
        LookSettings {
            response: StickResponse::default(),
            degrees_per_count: 0.0225,
            full_rate: 180.0,
            sensitivity: [1.0, 1.0],
            invert: [false, false],
            smoothing: 0.0,
        }
    }
}

const WINDOW_US: u64 = 16_000;
const SMOOTHING_US: f32 = 100_000.0;

/// Turns the movement of the mouse into a right stick position, with no acceleration: the stick is
/// `inverse(rate / full_rate)`, so the camera turns by the same angle for the same counts at any speed.
#[derive(Debug, Clone, Default)]
pub struct MouseLook {
    samples: Vec<(u64, f32, f32)>,
    smoothed: [f32; 2],
    last: Option<u64>,
}

impl MouseLook {
    pub fn push(&mut self, now: u64, dx: i64, dy: i64) {
        if dx != 0 || dy != 0 {
            self.samples.push((now, dx as f32, dy as f32));
        }
    }

    pub fn reset(&mut self) {
        *self = MouseLook::default();
    }

    /// The stick for time `now` in microseconds, +Y up.
    pub fn step(&mut self, now: u64, look: &LookSettings) -> [f32; 2] {
        self.samples.retain(|s| s.0 + WINDOW_US > now);
        let (sx, sy) = self
            .samples
            .iter()
            .fold((0.0, 0.0), |a, s| (a.0 + s.1, a.1 + s.2));
        let seconds = WINDOW_US as f32 / 1e6;
        let rate = [sx / seconds, -sy / seconds];
        let dt = self.last.map_or(0, |t| now.saturating_sub(t)) as f32;
        self.last = Some(now);
        let tau = look.smoothing.clamp(0.0, 1.0) * SMOOTHING_US;
        let alpha = if tau <= 0.0 || dt <= 0.0 {
            1.0
        } else {
            1.0 - (-dt / tau).exp()
        };
        #[allow(clippy::needless_range_loop)]
        for axis in 0..2 {
            self.smoothed[axis] += alpha * (rate[axis] - self.smoothed[axis]);
        }
        let mut turn = [0.0; 2];
        #[allow(clippy::needless_range_loop)]
        for axis in 0..2 {
            let sign = if look.invert[axis] { -1.0 } else { 1.0 };
            turn[axis] =
                sign * self.smoothed[axis] * look.sensitivity[axis] * look.degrees_per_count
                    / look.full_rate.max(1.0);
        }
        let clipped = clip_length(turn);
        let raw = look.response.inverse_vector(clipped);
        if self.samples.is_empty() && self.smoothed.iter().all(|v| v.abs() < 1e-3) {
            self.smoothed = [0.0; 2];
            return [0.0; 2];
        }
        raw
    }
}

fn clip_length(v: [f32; 2]) -> [f32; 2] {
    let length = v[0].hypot(v[1]);
    if length > 1.0 {
        [v[0] / length, v[1] / length]
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_is_a_rescaled_radial_dead_zone() {
        let r = StickResponse::default();
        assert_eq!(r.forward(0.0), 0.0);
        assert_eq!(r.forward(0.25), 0.0);
        assert!((r.forward(0.625) - 0.5).abs() < 1e-6);
        assert_eq!(r.forward(1.0), 1.0);
        assert_eq!(r.forward(2.0), 1.0);
    }

    #[test]
    fn inverse_undoes_forward_over_the_whole_range() {
        for exponent in [1.0, 1.5, 2.0] {
            let r = StickResponse {
                exponent,
                ..StickResponse::default()
            };
            for i in 1..=100 {
                let effective = i as f32 / 100.0;
                let raw = r.inverse(effective);
                assert!(raw > r.inner && raw <= 1.0 + 1e-6, "{raw}");
                assert!((r.forward(raw) - effective).abs() < 1e-4, "{exponent} {i}");
            }
            let mut last = 0.0;
            for i in 1..=100 {
                let raw = r.inverse(i as f32 / 100.0);
                assert!(raw > last);
                last = raw;
            }
        }
        assert_eq!(StickResponse::default().inverse(0.0), 0.0);
    }

    #[test]
    fn a_vector_keeps_its_direction_and_gets_the_scalar_inverse_length() {
        let r = StickResponse::default();
        let raw = r.inverse_vector([0.3, 0.4]);
        let length = raw[0].hypot(raw[1]);
        assert!((length - r.inverse(0.5)).abs() < 1e-6);
        assert!((raw[0] / raw[1] - 0.75).abs() < 1e-5);
        assert_eq!(r.inverse_vector([0.0, 0.0]), [0.0, 0.0]);
    }

    fn drive(look: &LookSettings, counts_per_ms: f32, millis: u64) -> Vec<[f32; 2]> {
        let mut mouse = MouseLook::default();
        let mut out = Vec::new();
        let mut owed = 0.0;
        for ms in 0..millis {
            owed += counts_per_ms;
            let whole = owed.floor();
            owed -= whole;
            let now = 1_000_000 + ms * 1000;
            mouse.push(now, whole as i64, 0);
            out.push(mouse.step(now, look));
        }
        out
    }

    #[test]
    fn steady_motion_gives_a_stick_the_game_turns_by_the_same_angle_per_count() {
        let look = LookSettings::default();
        for counts_per_ms in [0.5f32, 1.0, 2.0] {
            let out = drive(&look, counts_per_ms, 100);
            let raw = out.last().unwrap()[0];
            let effective = look.response.forward(raw);
            let degrees_per_second = effective * look.full_rate;
            let expected = counts_per_ms * 1000.0 * look.degrees_per_count;
            assert!(
                (degrees_per_second - expected).abs() < 0.05 * expected,
                "{counts_per_ms}: {degrees_per_second} vs {expected}"
            );
        }
    }

    #[test]
    fn a_stopped_mouse_gives_exactly_zero_and_the_dead_zone_is_never_entered_by_motion() {
        let look = LookSettings::default();
        let mut out = drive(&look, 1.0, 50);
        assert!(out.iter().skip(20).all(|s| s[0] > look.response.inner));
        let mut mouse = MouseLook::default();
        mouse.push(1_000_000, 5, 0);
        mouse.step(1_000_000, &look);
        out.clear();
        for ms in 1..40 {
            out.push(mouse.step(1_000_000 + ms * 1000, &look));
        }
        assert_eq!(*out.last().unwrap(), [0.0, 0.0]);
    }

    #[test]
    fn sensitivity_is_per_axis_and_inversion_flips_only_y() {
        let mut look = LookSettings {
            sensitivity: [1.0, 0.5],
            ..LookSettings::default()
        };
        let mut mouse = MouseLook::default();
        for ms in 0..30u64 {
            mouse.push(1_000_000 + ms * 1000, 1, 1);
            mouse.step(1_000_000 + ms * 1000, &look);
        }
        let s = mouse.step(1_030_000, &look);
        let length = s[0].hypot(s[1]);
        let strength = look.response.forward(length);
        let (x, y) = (strength * s[0] / length, strength * s[1].abs() / length);
        assert!((x / y - 2.0).abs() < 0.1, "{x} {y}");
        assert!(s[1] < 0.0, "mouse down pushes the stick down");
        look.invert[1] = true;
        let mut mouse = MouseLook::default();
        for ms in 0..30u64 {
            mouse.push(1_000_000 + ms * 1000, 0, 1);
            mouse.step(1_000_000 + ms * 1000, &look);
        }
        assert!(mouse.step(1_030_000, &look)[1] > 0.0);
    }

    #[test]
    fn smoothing_delays_the_rise_and_the_fall() {
        let sharp = LookSettings::default();
        let soft = LookSettings {
            smoothing: 1.0,
            ..sharp
        };
        let a = drive(&sharp, 1.0, 40);
        let b = drive(&soft, 1.0, 40);
        assert!(b[30][0] < a[30][0]);
        assert!(b[30][0] > 0.0);
    }
}
