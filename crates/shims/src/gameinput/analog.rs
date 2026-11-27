//! The keyboard as an analog stick, and the mouse wheel as button presses.

/// How the movement keys push the left stick.
#[derive(Debug, Clone, PartialEq)]
pub struct Movement {
    pub level: f32,
    pub walk_level: f32,
    pub run_level: f32,
    pub ramp_up_ms: u32,
    pub ramp_down_ms: u32,
    /// Virtual-key codes whose being held selects the walk or the run level; run wins when both are held.
    pub walk_keys: Vec<u8>,
    pub run_keys: Vec<u8>,
}

impl Default for Movement {
    fn default() -> Self {
        Movement {
            level: 1.0,
            walk_level: 0.4,
            run_level: 1.0,
            ramp_up_ms: 80,
            ramp_down_ms: 50,
            walk_keys: vec![0x12],
            run_keys: vec![0x10],
        }
    }
}

impl Movement {
    /// The stick length for the movement keys given which keys are down.
    pub fn level_for(&self, keys: &[bool; 256]) -> f32 {
        let held = |codes: &[u8]| codes.iter().any(|&k| keys[k as usize]);
        if held(&self.run_keys) {
            self.run_level
        } else if held(&self.walk_keys) {
            self.walk_level
        } else {
            self.level
        }
    }
}

/// The left stick as the keys drive it: it moves toward the target by at most a full deflection per ramp time.
#[derive(Debug, Clone, Default)]
pub struct AnalogMove {
    current: [f32; 2],
    last: Option<u64>,
}

impl AnalogMove {
    pub fn reset(&mut self) {
        *self = AnalogMove::default();
    }

    pub fn current(&self) -> [f32; 2] {
        self.current
    }

    pub fn step(&mut self, now: u64, target: [f32; 2], movement: &Movement) -> [f32; 2] {
        let dt_ms = self
            .last
            .map_or(0.0, |t| now.saturating_sub(t) as f32 / 1000.0);
        self.last = Some(now);
        #[allow(clippy::needless_range_loop)]
        for axis in 0..2 {
            let (to, from) = (target[axis], self.current[axis]);
            let rising = to.abs() > from.abs() && to * from >= 0.0;
            let ramp = if rising {
                movement.ramp_up_ms
            } else {
                movement.ramp_down_ms
            };
            let reach = if ramp == 0 {
                f32::INFINITY
            } else {
                dt_ms / ramp as f32
            };
            let delta = to - from;
            self.current[axis] = if delta.abs() <= reach {
                to
            } else {
                from + reach.copysign(delta)
            };
        }
        self.current
    }
}

const PRESS_US: u64 = 60_000;
const GAP_US: u64 = 40_000;

/// One press for each notch of the wheel, long enough for a frame to see it, with a gap between two.
#[derive(Debug, Clone, Default)]
pub struct WheelPulses {
    pending: [u32; 2],
    active: Option<(usize, u64)>,
    quiet_until: u64,
}

impl WheelPulses {
    /// Direction 0 is up, 1 is down.
    pub fn push(&mut self, direction: usize, notches: u32) {
        self.pending[direction] = (self.pending[direction] + notches).min(16);
    }

    pub fn reset(&mut self) {
        *self = WheelPulses::default();
    }

    /// Which directions are pressed at `now`.
    pub fn step(&mut self, now: u64) -> [bool; 2] {
        if let Some((_, until)) = self.active
            && now >= until
        {
            self.active = None;
            self.quiet_until = now + GAP_US;
        }
        if self.active.is_none() && now >= self.quiet_until {
            for direction in 0..2 {
                if self.pending[direction] > 0 {
                    self.pending[direction] -= 1;
                    self.active = Some((direction, now + PRESS_US));
                    break;
                }
            }
        }
        let mut out = [false; 2];
        if let Some((direction, _)) = self.active {
            out[direction] = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_and_run_keys_pick_the_level() {
        let m = Movement::default();
        let mut keys = [false; 256];
        assert_eq!(m.level_for(&keys), 1.0);
        keys[0x12] = true;
        assert_eq!(m.level_for(&keys), 0.4);
        keys[0x10] = true;
        assert_eq!(m.level_for(&keys), 1.0);
        let slow = Movement { level: 0.7, ..m };
        keys[0x10] = false;
        keys[0x12] = false;
        assert_eq!(slow.level_for(&keys), 0.7);
    }

    #[test]
    fn the_stick_ramps_up_and_down_at_the_set_times() {
        let m = Movement::default();
        let mut stick = AnalogMove::default();
        stick.step(0, [0.0, 0.0], &m);
        let half = stick.step(40_000, [0.0, 1.0], &m);
        assert!((half[1] - 0.5).abs() < 1e-5);
        assert_eq!(stick.step(100_000, [0.0, 1.0], &m)[1], 1.0);
        let down = stick.step(125_000, [0.0, 0.0], &m);
        assert!((down[1] - 0.5).abs() < 1e-5);
        assert_eq!(stick.step(200_000, [0.0, 0.0], &m)[1], 0.0);
    }

    #[test]
    fn no_ramp_jumps_straight_to_the_target() {
        let m = Movement {
            ramp_up_ms: 0,
            ramp_down_ms: 0,
            ..Movement::default()
        };
        let mut stick = AnalogMove::default();
        stick.step(0, [0.0; 2], &m);
        assert_eq!(stick.step(1000, [0.4, -0.4], &m), [0.4, -0.4]);
        assert_eq!(stick.step(2000, [0.0; 2], &m), [0.0, 0.0]);
    }

    #[test]
    fn a_walk_level_target_is_reached_and_held() {
        let m = Movement::default();
        let mut stick = AnalogMove::default();
        stick.step(0, [0.0; 2], &m);
        for t in 1..20u64 {
            stick.step(t * 10_000, [0.0, 0.4], &m);
        }
        assert_eq!(stick.current(), [0.0, 0.4]);
    }

    #[test]
    fn each_wheel_notch_is_its_own_press() {
        let mut wheel = WheelPulses::default();
        wheel.push(1, 2);
        let mut presses = 0;
        let mut was = false;
        for ms in 0..400u64 {
            let down = wheel.step(ms * 1000)[1];
            if down && !was {
                presses += 1;
            }
            assert!(!wheel.step(ms * 1000)[0]);
            was = down;
        }
        assert_eq!(presses, 2);
        assert_eq!(wheel.step(500_000), [false, false]);
    }

    #[test]
    fn a_press_lasts_long_enough_to_be_seen() {
        let mut wheel = WheelPulses::default();
        wheel.push(0, 1);
        let held = (0..200u64).filter(|ms| wheel.step(ms * 1000)[0]).count();
        assert!((55..=65).contains(&held), "{held}");
    }
}
