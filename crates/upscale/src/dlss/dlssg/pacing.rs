//! How the frames of DLSS frame generation reach the screen.
//!
//! NVIDIA's guide (section 7) states the rules this module follows:
//!
//! - One evaluation makes one generated frame. A call per real frame is 2x; `n` calls with `multiFrameIndex` 1 to `n` are
//!   `n + 1` times the frame rate. Only GeForce RTX 50 series GPUs offer `n > 1` (`DLSSG.MultiFrameCountMax`).
//! - The generated frame lies between the previous real frame and the newest one, so it needs the newest one: the
//!   newest real frame is held back and the generated frames are shown first, then the real frame.
//! - Frames are shown at equal spacing. The first generated frame goes out as soon as its evaluation is done; the
//!   real frame is kept (the `OutputReal` copy, or the caller's own image) until its turn.
//! - NGX does not pace or present: the application does, from a thread of its own so the game's render thread is not
//!   held up.
//!
//! [`schedule`] turns that into times. Every frame is shown one hold after the moment its content happened, so motion
//! stays even: the content of generated frame `j` of `n` is `j / (n + 1)` of the way from the previous real frame to the
//! newest, and the hold is the distance from the first generated frame's content to the arrival of the newest real
//! frame, plus a guard for the GPU time of the evaluation.

use std::time::Duration;

/// What a scheduled present shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// Generated frame `index` of the interval, counted from 1.
    Generated {
        index: u32,
    },
    Real,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Present {
    /// After the moment the newest real frame arrived.
    pub delay: Duration,
    pub shown: Shown,
}

/// Where generated frame `index` of `count` sits between the real frames, from 0 (previous) to 1 (newest).
pub fn phase(index: u32, count: u32) -> f32 {
    index as f32 / (count + 1) as f32
}

/// The presents of one real-frame interval of `interval` with `count` generated frames, in the order they are shown.
/// `guard` is the time kept between the arrival and the first generated frame (the evaluation's GPU time); it is held
/// to at most a quarter of the spacing.
pub fn schedule(interval: Duration, count: u32, guard: Duration) -> Vec<Present> {
    let count = count.max(1);
    let seconds = interval.as_secs_f64();
    let spacing = seconds / f64::from(count + 1);
    let guard = guard.as_secs_f64().min(spacing * 0.25);
    let mut out: Vec<Present> = (1..=count)
        .map(|index| Present {
            delay: Duration::from_secs_f64(guard + spacing * f64::from(index - 1)),
            shown: Shown::Generated { index },
        })
        .collect();
    out.push(Present {
        delay: Duration::from_secs_f64(guard + spacing * f64::from(count)),
        shown: Shown::Real,
    });
    out
}

/// How many frames are shown per real frame when `count` are generated.
pub fn frames_per_real(count: u32) -> u32 {
    count.max(1) + 1
}
