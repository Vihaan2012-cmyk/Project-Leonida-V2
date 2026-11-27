//! Mixes the voices the title submits into stereo `f32` at the device rate.
//!
//! A voice plays a queue of buffers of interleaved `f32` frames at its own sample rate; the mixer resamples
//! linearly, applies gain, pitch and balance, sums, and clamps. Each buffer that finishes is reported as a
//! [`Completion`] at the output frame where it ended, so completion follows the output clock.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

pub type VoiceId = u32;
pub type BufferId = u64;

const MIN_PITCH: f32 = 0.01;
const MAX_PITCH: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Looping {
    Once,
    /// Plays this many more times after the first.
    Times(u32),
    Forever,
}

#[derive(Debug, Clone)]
pub struct Buffer {
    pub id: BufferId,
    /// Interleaved frames.
    pub samples: Arc<[f32]>,
    pub channels: u8,
    pub looping: Looping,
}

impl Buffer {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// 16-bit PCM scaled to `-1..1`.
    pub fn from_i16(id: BufferId, pcm: &[i16], channels: u8, looping: Looping) -> Buffer {
        let samples: Arc<[f32]> = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
        Buffer {
            id,
            samples,
            channels,
            looping,
        }
    }

    fn frame(&self, index: usize) -> (f32, f32) {
        let channels = usize::from(self.channels.max(1));
        let frame = &self.samples[index * channels..(index + 1) * channels];
        match channels {
            1 => (frame[0], frame[0]),
            2 => (frame[0], frame[1]),
            n => {
                let (mut left, mut right) = (0.0, 0.0);
                for (c, s) in frame.iter().enumerate() {
                    if c % 2 == 0 {
                        left += s;
                    } else {
                        right += s;
                    }
                }
                (left / n.div_ceil(2) as f32, right / (n / 2) as f32)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceParams {
    /// Sample rate of the voice's buffers.
    pub rate: u32,
    pub gain: f32,
    /// `-1` is hard left, `1` hard right.
    pub pan: f32,
    /// Playback rate multiplier.
    pub pitch: f32,
}

impl VoiceParams {
    pub fn new(rate: u32) -> VoiceParams {
        VoiceParams {
            rate,
            gain: 1.0,
            pan: 0.0,
            pitch: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    pub voice: VoiceId,
    pub buffer: BufferId,
    /// The output frame, counted from the mixer's start, at which the buffer ended.
    pub frame: u64,
}

struct Queued {
    buffer: Buffer,
    repeats: Looping,
}

struct Voice {
    params: VoiceParams,
    queue: VecDeque<Queued>,
    /// Position in the front buffer, in its frames.
    position: f64,
    paused: bool,
}

pub struct Mixer {
    rate: u32,
    master: f32,
    voices: BTreeMap<VoiceId, Voice>,
    next: VoiceId,
    completions: Vec<Completion>,
    rendered: u64,
}

impl Mixer {
    pub fn new(rate: u32) -> Mixer {
        Mixer {
            rate: rate.max(1),
            master: 1.0,
            voices: BTreeMap::new(),
            next: 1,
            completions: Vec::new(),
            rendered: 0,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
    }

    pub fn set_master(&mut self, gain: f32) {
        self.master = gain;
    }

    /// Output frames rendered so far.
    pub fn rendered_frames(&self) -> u64 {
        self.rendered
    }

    pub fn add_voice(&mut self, params: VoiceParams) -> VoiceId {
        let id = self.next;
        self.next += 1;
        self.voices.insert(
            id,
            Voice {
                params,
                queue: VecDeque::new(),
                position: 0.0,
                paused: false,
            },
        );
        id
    }

    pub fn remove_voice(&mut self, id: VoiceId) -> bool {
        self.voices.remove(&id).is_some()
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn set_params(&mut self, id: VoiceId, params: VoiceParams) -> bool {
        self.voices
            .get_mut(&id)
            .map(|v| v.params = params)
            .is_some()
    }

    pub fn set_paused(&mut self, id: VoiceId, paused: bool) -> bool {
        self.voices
            .get_mut(&id)
            .map(|v| v.paused = paused)
            .is_some()
    }

    /// Adds a buffer after the ones already queued. A buffer with no frames completes at the next render.
    pub fn queue(&mut self, id: VoiceId, buffer: Buffer) -> bool {
        let Some(voice) = self.voices.get_mut(&id) else {
            return false;
        };
        let repeats = buffer.looping;
        voice.queue.push_back(Queued { buffer, repeats });
        true
    }

    /// Drops everything queued without reporting completions.
    pub fn flush(&mut self, id: VoiceId) -> bool {
        self.voices
            .get_mut(&id)
            .map(|v| {
                v.queue.clear();
                v.position = 0.0;
            })
            .is_some()
    }

    pub fn queued_buffers(&self, id: VoiceId) -> usize {
        self.voices.get(&id).map_or(0, |v| v.queue.len())
    }

    pub fn take_completions(&mut self) -> Vec<Completion> {
        std::mem::take(&mut self.completions)
    }

    /// Fills `out` with interleaved stereo frames.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / 2;
        let rate = f64::from(self.rate);
        let start = self.rendered;
        for (&id, voice) in &mut self.voices {
            if voice.paused {
                continue;
            }
            let params = voice.params;
            let step =
                f64::from(params.rate) / rate * f64::from(params.pitch.clamp(MIN_PITCH, MAX_PITCH));
            let left_gain = params.gain * (1.0 - params.pan.clamp(-1.0, 1.0).max(0.0));
            let right_gain = params.gain * (1.0 + params.pan.clamp(-1.0, 1.0).min(0.0));
            for n in 0..frames {
                let Some((left, right)) =
                    voice.advance(id, step, start + n as u64, &mut self.completions)
                else {
                    break;
                };
                out[2 * n] += left * left_gain;
                out[2 * n + 1] += right * right_gain;
            }
        }
        for sample in out.iter_mut() {
            *sample = (*sample * self.master).clamp(-1.0, 1.0);
        }
        self.rendered += frames as u64;
    }
}

impl Voice {
    /// The voice's next output frame, or `None` when nothing is left to play.
    fn advance(
        &mut self,
        id: VoiceId,
        step: f64,
        frame: u64,
        completions: &mut Vec<Completion>,
    ) -> Option<(f32, f32)> {
        loop {
            let front = self.queue.front_mut()?;
            let frames = front.buffer.frames();
            if frames == 0 || self.position >= frames as f64 {
                let again = match front.repeats {
                    Looping::Forever => frames > 0,
                    Looping::Times(n) if n > 0 => {
                        front.repeats = Looping::Times(n - 1);
                        true
                    }
                    _ => false,
                };
                if again {
                    self.position -= frames as f64;
                } else {
                    completions.push(Completion {
                        voice: id,
                        buffer: front.buffer.id,
                        frame,
                    });
                    self.position = (self.position - frames as f64).max(0.0);
                    self.queue.pop_front();
                }
                continue;
            }
            let index = self.position as usize;
            let fraction = (self.position - index as f64) as f32;
            let looping = !matches!(front.repeats, Looping::Once | Looping::Times(0));
            let next = if index + 1 < frames {
                index + 1
            } else if looping {
                0
            } else {
                index
            };
            let (l0, r0) = front.buffer.frame(index);
            let (l1, r1) = front.buffer.frame(next);
            let frame_out = (l0 + (l1 - l0) * fraction, r0 + (r1 - r0) * fraction);
            self.position += step;
            let repeats = matches!(front.repeats, Looping::Forever | Looping::Times(1..));
            if self.position >= frames as f64 && !repeats {
                completions.push(Completion {
                    voice: id,
                    buffer: front.buffer.id,
                    frame: frame + 1,
                });
                self.position -= frames as f64;
                self.queue.pop_front();
            }
            return Some(frame_out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize, channels: u8) -> Arc<[f32]> {
        (0..frames * usize::from(channels))
            .map(|i| (i / usize::from(channels)) as f32 / 1000.0)
            .collect()
    }

    fn buffer(id: BufferId, frames: usize, looping: Looping) -> Buffer {
        Buffer {
            id,
            samples: ramp(frames, 1),
            channels: 1,
            looping,
        }
    }

    #[test]
    fn mono_at_the_output_rate_plays_unchanged_and_completes_on_time() {
        let mut m = Mixer::new(48_000);
        let v = m.add_voice(VoiceParams::new(48_000));
        assert!(m.queue(v, buffer(7, 100, Looping::Once)));
        let mut out = vec![0.0; 2 * 150];
        m.render(&mut out);
        assert_eq!(out[2 * 10], 0.010);
        assert_eq!(out[2 * 10 + 1], 0.010);
        assert_eq!(out[2 * 99], 0.099);
        assert_eq!(out[2 * 100], 0.0, "silent after the end");
        let done = m.take_completions();
        assert_eq!(
            done,
            vec![Completion {
                voice: v,
                buffer: 7,
                frame: 100
            }]
        );
        assert_eq!(m.queued_buffers(v), 0);
        assert_eq!(m.rendered_frames(), 150);
    }

    #[test]
    fn a_half_rate_voice_takes_twice_as_long() {
        let mut m = Mixer::new(48_000);
        let v = m.add_voice(VoiceParams::new(24_000));
        m.queue(v, buffer(1, 100, Looping::Once));
        let mut out = vec![0.0; 2 * 199];
        m.render(&mut out);
        assert!(m.take_completions().is_empty());
        assert!((out[2] - 0.0005).abs() < 1e-6, "interpolated");
        let mut out = vec![0.0; 2 * 2];
        m.render(&mut out);
        assert_eq!(m.take_completions()[0].frame, 200);
    }

    #[test]
    fn buffers_play_back_to_back_and_loops_repeat() {
        let mut m = Mixer::new(1000);
        let v = m.add_voice(VoiceParams::new(1000));
        m.queue(v, buffer(1, 10, Looping::Times(2)));
        m.queue(v, buffer(2, 5, Looping::Once));
        let mut out = vec![0.0; 2 * 40];
        m.render(&mut out);
        let ids: Vec<(BufferId, u64)> = m
            .take_completions()
            .iter()
            .map(|c| (c.buffer, c.frame))
            .collect();
        assert_eq!(ids, vec![(1, 30), (2, 35)]);
        assert_eq!(out[2 * 10], 0.0, "the loop restarts");
        assert_eq!(out[2 * 30], 0.0);
        assert_eq!(out[2 * 31], 0.001, "the next buffer follows");
    }

    #[test]
    fn forever_never_completes_and_flush_drops_silently() {
        let mut m = Mixer::new(1000);
        let v = m.add_voice(VoiceParams::new(1000));
        m.queue(v, buffer(1, 10, Looping::Forever));
        let mut out = vec![0.0; 2 * 100];
        m.render(&mut out);
        assert!(m.take_completions().is_empty());
        assert_eq!(out[2 * 95], 0.005);
        assert!(m.flush(v));
        m.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
        assert!(m.take_completions().is_empty());
    }

    #[test]
    fn gain_pan_pause_and_clamp() {
        let mut m = Mixer::new(1000);
        let mut params = VoiceParams::new(1000);
        params.gain = 0.5;
        params.pan = 1.0;
        let v = m.add_voice(params);
        let constant = Buffer {
            id: 1,
            samples: vec![0.8f32; 10].into(),
            channels: 1,
            looping: Looping::Once,
        };
        m.queue(v, constant.clone());
        let mut out = vec![0.0; 2 * 4];
        m.set_paused(v, true);
        m.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
        m.set_paused(v, false);
        m.render(&mut out);
        assert_eq!(out[0], 0.0, "hard right silences the left");
        assert!((out[1] - 0.4).abs() < 1e-6);
        let loud = m.add_voice(VoiceParams::new(1000));
        m.queue(loud, constant.clone());
        let again = m.add_voice(VoiceParams::new(1000));
        m.queue(again, constant);
        m.render(&mut out);
        assert_eq!(out[0], 1.0, "sum is clamped");
    }

    #[test]
    fn stereo_and_multichannel_sources_map_to_two_channels() {
        let mut m = Mixer::new(1000);
        let v = m.add_voice(VoiceParams::new(1000));
        m.queue(
            v,
            Buffer::from_i16(1, &[16384, -16384, 8192, -8192], 2, Looping::Once),
        );
        let mut out = vec![0.0; 2 * 2];
        m.render(&mut out);
        assert!((out[0] - 0.5).abs() < 1e-6 && (out[1] + 0.5).abs() < 1e-6);
        let w = m.add_voice(VoiceParams::new(1000));
        let six = Buffer {
            id: 2,
            samples: vec![0.2, 0.4, 0.2, 0.4, 0.2, 0.4].into(),
            channels: 6,
            looping: Looping::Once,
        };
        m.queue(w, six);
        m.flush(v);
        m.render(&mut out);
        assert!((out[0] - 0.2).abs() < 1e-6 && (out[1] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn an_empty_buffer_completes_at_once() {
        let mut m = Mixer::new(1000);
        let v = m.add_voice(VoiceParams::new(1000));
        m.queue(v, buffer(9, 0, Looping::Forever));
        let mut out = vec![0.0; 2];
        m.render(&mut out);
        assert_eq!(m.take_completions().len(), 1);
        assert!(m.remove_voice(v));
        assert_eq!(m.voice_count(), 0);
    }
}
