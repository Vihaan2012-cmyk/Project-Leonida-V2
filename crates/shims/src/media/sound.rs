//! Where a movie's audio goes: one voice per selected track on the shared mixer.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::audio::{Buffer, Looping, SharedMixer, VoiceId, VoiceParams, with_engine};

pub trait AudioSink: Send {
    /// Appends the samples (interleaved, `-1..1`) of the next stretch of `track`.
    fn push(&mut self, track: usize, rate: u32, channels: u8, samples: Vec<f32>);
    /// Drops everything not yet played, as a seek does.
    fn flush(&mut self);
    fn set_gain(&mut self, track: usize, gain: f32);
    fn pause(&mut self, paused: bool);
    /// How long the audio of the lowest-numbered track has played since the last flush; `None` while nothing has been
    /// pushed. It stands still when the device does not consume or the voice has run dry.
    fn position(&self) -> Option<Duration> {
        None
    }
}

/// Plays on the shared audio engine's mixer; its voices are removed when it is dropped.
pub struct EngineSink {
    mixer: SharedMixer,
    voices: BTreeMap<usize, (VoiceId, VoiceParams)>,
    /// Source frames each voice had played when the sink was last flushed.
    bases: BTreeMap<usize, f64>,
    gains: BTreeMap<usize, f32>,
    paused: bool,
}

impl EngineSink {
    pub fn new() -> EngineSink {
        EngineSink::on(with_engine(|e| e.mixer()))
    }

    pub fn on(mixer: SharedMixer) -> EngineSink {
        EngineSink {
            mixer,
            voices: BTreeMap::new(),
            bases: BTreeMap::new(),
            gains: BTreeMap::new(),
            paused: false,
        }
    }
}

impl Default for EngineSink {
    fn default() -> Self {
        EngineSink::new()
    }
}

impl AudioSink for EngineSink {
    fn push(&mut self, track: usize, rate: u32, channels: u8, samples: Vec<f32>) {
        if samples.is_empty() {
            return;
        }
        let id = with_engine(|e| e.new_buffer_id());
        let Ok(mut mixer) = self.mixer.lock() else {
            return;
        };
        let (voice, _) = *self.voices.entry(track).or_insert_with(|| {
            let mut params = VoiceParams::new(rate);
            params.gain = self.gains.get(&track).copied().unwrap_or(1.0);
            let voice = mixer.add_voice(params);
            mixer.set_paused(voice, self.paused);
            (voice, params)
        });
        mixer.queue(
            voice,
            Buffer {
                id,
                samples: Arc::from(samples),
                channels,
                looping: Looping::Once,
            },
        );
    }

    fn flush(&mut self) {
        if let Ok(mut mixer) = self.mixer.lock() {
            for (track, (voice, _)) in &self.voices {
                mixer.flush(*voice);
                self.bases
                    .insert(*track, mixer.voice_played(*voice).unwrap_or(0.0));
            }
        }
    }

    fn position(&self) -> Option<Duration> {
        let (track, (voice, params)) = self.voices.iter().next()?;
        let played = self.mixer.lock().ok()?.voice_played(*voice)?;
        let base = self.bases.get(track).copied().unwrap_or(0.0);
        Some(Duration::from_secs_f64(
            (played - base).max(0.0) / f64::from(params.rate.max(1)),
        ))
    }

    fn set_gain(&mut self, track: usize, gain: f32) {
        self.gains.insert(track, gain);
        if let Some((voice, params)) = self.voices.get_mut(&track) {
            params.gain = gain;
            if let Ok(mut mixer) = self.mixer.lock() {
                mixer.set_params(*voice, *params);
            }
        }
    }

    fn pause(&mut self, paused: bool) {
        self.paused = paused;
        if let Ok(mut mixer) = self.mixer.lock() {
            for (voice, _) in self.voices.values() {
                mixer.set_paused(*voice, paused);
            }
        }
    }
}

impl Drop for EngineSink {
    fn drop(&mut self) {
        if let Ok(mut mixer) = self.mixer.lock() {
            for (voice, _) in self.voices.values() {
                mixer.remove_voice(*voice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::audio::Mixer;

    fn mixer() -> SharedMixer {
        Arc::new(Mutex::new(Mixer::new(48000)))
    }

    fn render(mixer: &SharedMixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        mixer.lock().unwrap().render(&mut out);
        out
    }

    #[test]
    fn pushed_samples_play_in_order_on_one_voice_per_track() {
        let m = mixer();
        let mut sink = EngineSink::on(m.clone());
        sink.push(0, 48000, 2, vec![0.25, -0.25, 0.5, -0.5]);
        sink.push(0, 48000, 2, vec![0.75, -0.75]);
        sink.push(1, 48000, 2, vec![0.1, 0.1]);
        assert_eq!(m.lock().unwrap().voice_count(), 2);
        let out = render(&m, 4);
        assert_eq!(&out[..6], &[0.35, -0.15, 0.5, -0.5, 0.75, -0.75]);
    }

    #[test]
    fn flushing_drops_what_is_waiting_and_dropping_removes_the_voices() {
        let m = mixer();
        let mut sink = EngineSink::on(m.clone());
        sink.push(0, 48000, 2, vec![0.5; 4000]);
        sink.flush();
        assert!(render(&m, 8).iter().all(|&s| s == 0.0));
        sink.push(0, 48000, 2, vec![0.5; 4]);
        assert_eq!(render(&m, 1)[0], 0.5);
        drop(sink);
        assert_eq!(m.lock().unwrap().voice_count(), 0);
    }

    #[test]
    fn gain_and_pause_reach_the_voice_even_before_it_exists() {
        let m = mixer();
        let mut sink = EngineSink::on(m.clone());
        sink.set_gain(0, 0.5);
        sink.pause(true);
        sink.push(0, 48000, 2, vec![0.8; 8]);
        assert!(render(&m, 2).iter().all(|&s| s == 0.0), "paused");
        sink.pause(false);
        assert!((render(&m, 1)[0] - 0.4).abs() < 1e-6);
        sink.set_gain(0, 0.25);
        assert!((render(&m, 1)[0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn the_position_is_what_the_mixer_has_played_since_the_last_flush() {
        let m = mixer();
        let mut sink = EngineSink::on(m.clone());
        assert_eq!(sink.position(), None);
        sink.push(1, 48000, 2, vec![0.1; 2 * 4800]);
        sink.push(0, 48000, 2, vec![0.1; 2 * 4800]);
        assert_eq!(sink.position(), Some(Duration::ZERO));
        render(&m, 2400);
        assert_eq!(
            sink.position(),
            Some(Duration::from_millis(50)),
            "the lowest track is the clock"
        );
        sink.flush();
        assert_eq!(sink.position(), Some(Duration::ZERO));
        sink.push(0, 48000, 2, vec![0.1; 2 * 4800]);
        render(&m, 480);
        assert_eq!(sink.position(), Some(Duration::from_millis(10)));
        sink.pause(true);
        render(&m, 480);
        assert_eq!(sink.position(), Some(Duration::from_millis(10)), "paused");
    }
}
