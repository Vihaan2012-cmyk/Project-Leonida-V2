//! The title's output stage, without the operating system: what it does with the buffers it hands over each period.
//!
//! The title mixes on the CPU and delivers the result through the Windows spatial audio client (0x1e7f580,
//! 0x43ade50, 0x43ae310): one *static bed* object per speaker position (the `AudioObjectType` bit it registers,
//! 0x5f0bd40), no dynamic objects. Each period it waits on an event, asks the stream for a batch of frames, copies
//! one batch of its own ring buffers into every object's buffer, and ends the update. This module is the other
//! side of that exchange: [`downmix`] turns the speaker buffers into stereo and [`Pacer`] raises the event at the
//! rate the mixer drains. The COM objects that carry the calls are in `spatial_com`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::mixer::{Buffer, Looping, VoiceId, VoiceParams};
use super::output::SharedMixer;

/// Sample rate of the objects' buffers: the one format the enumerator offers.
pub const OBJECT_RATE: u32 = 48_000;
/// Frames the stream asks the title to deliver per update (10 ms).
pub const PERIOD_FRAMES: usize = 480;
/// Periods kept queued ahead of the device before the event is raised for another.
pub const QUEUED_PERIODS: usize = 3;
const POLL: Duration = Duration::from_millis(2);

/// `AudioObjectType` bits, as the title registers them.
pub mod kind {
    pub const DYNAMIC: u32 = 0x1;
    pub const FRONT_LEFT: u32 = 0x2;
    pub const FRONT_RIGHT: u32 = 0x4;
    pub const FRONT_CENTER: u32 = 0x8;
    pub const LOW_FREQUENCY: u32 = 0x10;
    pub const SIDE_LEFT: u32 = 0x20;
    pub const SIDE_RIGHT: u32 = 0x40;
    pub const BACK_LEFT: u32 = 0x80;
    pub const BACK_RIGHT: u32 = 0x100;
    pub const TOP_FRONT_LEFT: u32 = 0x200;
    pub const TOP_FRONT_RIGHT: u32 = 0x400;
    pub const TOP_BACK_LEFT: u32 = 0x800;
    pub const TOP_BACK_RIGHT: u32 = 0x1000;
    pub const BOTTOM_FRONT_LEFT: u32 = 0x2000;
    pub const BOTTOM_FRONT_RIGHT: u32 = 0x4000;
    pub const BOTTOM_BACK_LEFT: u32 = 0x8000;
    pub const BOTTOM_BACK_RIGHT: u32 = 0x10000;
    pub const BACK_CENTER: u32 = 0x20000;
}

const HALF_POWER: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// How much of one speaker position goes to the left and to the right ear.
pub fn pan_of(object: u32) -> (f32, f32) {
    use kind::*;
    match object {
        FRONT_LEFT => (1.0, 0.0),
        FRONT_RIGHT => (0.0, 1.0),
        FRONT_CENTER | DYNAMIC => (HALF_POWER, HALF_POWER),
        LOW_FREQUENCY => (0.5, 0.5),
        SIDE_LEFT | BACK_LEFT => (HALF_POWER, 0.0),
        SIDE_RIGHT | BACK_RIGHT => (0.0, HALF_POWER),
        TOP_FRONT_LEFT | TOP_BACK_LEFT | BOTTOM_FRONT_LEFT | BOTTOM_BACK_LEFT => (0.5, 0.0),
        TOP_FRONT_RIGHT | TOP_BACK_RIGHT | BOTTOM_FRONT_RIGHT | BOTTOM_BACK_RIGHT => (0.0, 0.5),
        BACK_CENTER => (0.5, 0.5),
        _ => (0.0, 0.0),
    }
}

/// One speaker position's samples for a period and the volume set on it.
pub struct Speaker<'a> {
    pub object: u32,
    pub volume: f32,
    pub samples: &'a [f32],
}

/// Sums the speakers into interleaved stereo; `out` holds `frames * 2` samples and is overwritten.
pub fn downmix(speakers: &[Speaker<'_>], out: &mut [f32]) {
    out.fill(0.0);
    for speaker in speakers {
        let (left, right) = pan_of(speaker.object);
        let (left, right) = (left * speaker.volume, right * speaker.volume);
        for (frame, sample) in out.chunks_exact_mut(2).zip(speaker.samples) {
            frame[0] += sample * left;
            frame[1] += sample * right;
        }
    }
}

/// Queues one period of stereo on `voice`. Buffer id 0 never reports a completion.
pub fn queue_period(mixer: &SharedMixer, voice: VoiceId, stereo: Vec<f32>) {
    if let Ok(mut mixer) = mixer.lock() {
        mixer.queue(
            voice,
            Buffer {
                id: 0,
                samples: Arc::from(stereo),
                channels: 2,
                looping: Looping::Once,
            },
        );
    }
}

/// Adds the voice a stream plays on.
pub fn add_voice(mixer: &SharedMixer) -> Option<VoiceId> {
    mixer
        .lock()
        .ok()
        .map(|mut m| m.add_voice(VoiceParams::new(OBJECT_RATE)))
}

/// The cadence: a thread that raises `notify` whenever the mixer has fewer than [`QUEUED_PERIODS`] periods
/// waiting, and runs `pump` every turn so a mixer with no device still follows the clock.
pub struct Pacer {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Pacer {
    pub fn start(
        mixer: SharedMixer,
        voice: VoiceId,
        notify: impl Fn() + Send + 'static,
        pump: impl Fn() + Send + 'static,
    ) -> Option<Pacer> {
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let (thread_stop, thread_paused) = (stop.clone(), paused.clone());
        let thread = std::thread::Builder::new()
            .name("leonida-spatial".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    pump();
                    if !thread_paused.load(Ordering::Acquire) {
                        let queued = mixer.lock().map_or(0, |m| m.queued_buffers(voice));
                        if queued < QUEUED_PERIODS {
                            notify();
                        }
                    }
                    std::thread::sleep(POLL);
                }
            })
            .ok()?;
        Some(Pacer {
            stop,
            paused,
            thread: Some(thread),
        })
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::audio::Mixer;

    fn mixer() -> SharedMixer {
        Arc::new(Mutex::new(Mixer::new(OBJECT_RATE)))
    }

    #[test]
    fn stereo_speakers_stay_on_their_side_and_the_centre_splits_evenly() {
        let ones = vec![1.0f32; 4];
        let mut out = vec![0.0f32; 8];
        downmix(
            &[
                Speaker {
                    object: kind::FRONT_LEFT,
                    volume: 1.0,
                    samples: &ones,
                },
                Speaker {
                    object: kind::FRONT_CENTER,
                    volume: 1.0,
                    samples: &ones,
                },
            ],
            &mut out,
        );
        assert!((out[0] - (1.0 + HALF_POWER)).abs() < 1e-6);
        assert!((out[1] - HALF_POWER).abs() < 1e-6);
        let mut right = vec![9.0f32; 8];
        downmix(
            &[Speaker {
                object: kind::FRONT_RIGHT,
                volume: 0.5,
                samples: &ones,
            }],
            &mut right,
        );
        assert_eq!(
            (right[0], right[1]),
            (0.0, 0.5),
            "the buffer is overwritten"
        );
    }

    #[test]
    fn every_registered_position_reaches_at_least_one_ear() {
        for bit in 0..18 {
            let object = 1u32 << bit;
            let (left, right) = pan_of(object);
            assert!(left + right > 0.0, "{object:#x}");
        }
        assert_eq!(pan_of(0x40000), (0.0, 0.0));
    }

    #[test]
    fn a_short_speaker_buffer_leaves_the_rest_of_the_period_silent() {
        let mut out = vec![1.0f32; 8];
        downmix(
            &[Speaker {
                object: kind::FRONT_LEFT,
                volume: 1.0,
                samples: &[0.25, 0.5],
            }],
            &mut out,
        );
        assert_eq!(out, [0.25, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn periods_queue_without_completions_and_the_pacer_asks_for_more_until_enough_wait() {
        let mixer = mixer();
        let voice = add_voice(&mixer).unwrap();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = wakes.clone();
        let pacer = Pacer::start(
            mixer.clone(),
            voice,
            move || {
                counted.fetch_add(1, Ordering::AcqRel);
            },
            || {},
        )
        .unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while wakes.load(Ordering::Acquire) < 3 {
            assert!(std::time::Instant::now() < until, "no wake-ups");
            std::thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..QUEUED_PERIODS {
            queue_period(&mixer, voice, vec![0.25; PERIOD_FRAMES * 2]);
        }
        std::thread::sleep(Duration::from_millis(20));
        let settled = wakes.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            wakes.load(Ordering::Acquire),
            settled,
            "a full queue asks for nothing"
        );
        pacer.set_paused(true);
        let mut out = vec![0.0f32; PERIOD_FRAMES * 2];
        mixer.lock().unwrap().render(&mut out);
        assert!((out[0] - 0.25).abs() < 1e-6);
        assert!(mixer.lock().unwrap().take_completions().is_empty());
        drop(pacer);
    }
}
