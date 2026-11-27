//! Everything the audio libraries share: the coprocessor address space, the command queue, the mixer and the
//! output that drives it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::apu::ApuSpace;
use super::mixer::{Buffer, BufferId, Completion, Looping, Mixer, VoiceId, VoiceParams};
use super::output::{Output, SharedMixer};
use super::queue::CommandQueue;
use super::xma::{self, DecodeError, XmaRequest};

/// Rate the mixer runs at until a device sets its own.
pub const DEFAULT_RATE: u32 = 48_000;
/// Longest stretch the silent clock renders in one catch-up.
const MAX_CATCH_UP: Duration = Duration::from_secs(1);
const CHUNK_FRAMES: usize = 2048;
const ACP_BYTES: usize = 4096;
const CONTEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Open the default device when the title activates the DSP; run silently when there is none.
    Auto,
    /// Never open a device; the mixer follows a silent clock.
    Off,
}

pub struct Engine {
    pub apu: ApuSpace,
    pub queue: CommandQueue,
    mixer: SharedMixer,
    output: Option<Output>,
    mode: OutputMode,
    active: bool,
    started: Option<Instant>,
    silent_frames: u64,
    acp: Option<Box<[u64]>>,
    contexts: Option<Box<[u64]>>,
    next_buffer: BufferId,
    completions: Vec<Completion>,
    noted: HashSet<String>,
}

fn zeroed(bytes: usize) -> Box<[u64]> {
    vec![0u64; bytes / 8].into_boxed_slice()
}

impl Engine {
    pub fn new(mode: OutputMode) -> Engine {
        Engine {
            apu: ApuSpace::new(),
            queue: CommandQueue::default(),
            mixer: Arc::new(Mutex::new(Mixer::new(DEFAULT_RATE))),
            output: None,
            mode,
            active: false,
            started: None,
            silent_frames: 0,
            acp: None,
            contexts: None,
            next_buffer: 1,
            completions: Vec::new(),
            noted: HashSet::new(),
        }
    }

    pub fn set_mode(&mut self, mode: OutputMode) {
        self.mode = mode;
    }

    /// Records an unmeasured detail once per key.
    pub fn note_once(&mut self, key: &str, text: impl FnOnce() -> String) {
        if self.noted.insert(key.to_string()) {
            crate::trace::note(text());
        }
    }

    pub fn mixer(&self) -> SharedMixer {
        self.mixer.clone()
    }

    /// The address of the hardware layer's handle block: allocated on first use, stable until reset.
    pub fn acp_create(&mut self) -> usize {
        self.acp.get_or_insert_with(|| zeroed(ACP_BYTES)).as_ptr() as usize
    }

    pub fn allocate_shape_contexts(&mut self) -> usize {
        self.contexts
            .get_or_insert_with(|| zeroed(CONTEXT_BYTES))
            .as_ptr() as usize
    }

    pub fn release_shape_contexts(&mut self) {
        self.contexts = None;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn has_device(&self) -> bool {
        self.output.is_some()
    }

    /// The title turned the DSP on: start the device if there is one and the mode allows.
    pub fn activate(&mut self) {
        self.active = true;
        self.started.get_or_insert_with(Instant::now);
        if self.mode == OutputMode::Auto && self.output.is_none() {
            match Output::start(self.mixer.clone()) {
                Ok(output) => self.output = Some(output),
                Err(error) => {
                    self.note_once("output", || {
                        format!("no audio output ({error}): audio runs silent")
                    });
                }
            }
        }
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.output = None;
        self.started = None;
        self.silent_frames = 0;
    }

    /// Without a device, renders the time that has passed into nothing so voices finish when they should.
    pub fn pump(&mut self) {
        if let Some(started) = self.started {
            self.pump_to(started.elapsed());
        }
    }

    /// As [`Engine::pump`], with the time since activation given.
    pub fn pump_to(&mut self, elapsed: Duration) {
        if self.output.is_none() {
            let Ok(mut mixer) = self.mixer.lock() else {
                return;
            };
            let rate = u64::from(mixer.rate());
            let due = (elapsed.as_nanos() * u128::from(rate) / 1_000_000_000) as u64;
            let longest = MAX_CATCH_UP.as_secs() * rate;
            if due > self.silent_frames + longest {
                self.silent_frames = due - longest;
            }
            let mut todo = due.saturating_sub(self.silent_frames);
            let mut scratch = vec![0.0f32; CHUNK_FRAMES * 2];
            while todo > 0 {
                let n = (todo as usize).min(CHUNK_FRAMES);
                mixer.render(&mut scratch[..n * 2]);
                todo -= n as u64;
                self.silent_frames += n as u64;
            }
        }
        if let Ok(mut mixer) = self.mixer.lock() {
            self.completions.extend(mixer.take_completions());
        }
    }

    /// Buffers that finished since the last call.
    pub fn take_completions(&mut self) -> Vec<Completion> {
        if let Ok(mut mixer) = self.mixer.lock() {
            self.completions.extend(mixer.take_completions());
        }
        std::mem::take(&mut self.completions)
    }

    pub fn new_buffer_id(&mut self) -> BufferId {
        self.next_buffer += 1;
        self.next_buffer - 1
    }

    pub fn add_voice(&mut self, params: VoiceParams) -> VoiceId {
        self.mixer
            .lock()
            .map(|mut m| m.add_voice(params))
            .unwrap_or(0)
    }

    pub fn queue_buffer(&mut self, voice: VoiceId, buffer: Buffer) -> bool {
        self.mixer
            .lock()
            .map(|mut m| m.queue(voice, buffer))
            .unwrap_or(false)
    }

    /// Decodes an XMA stream and starts it on a new voice; the buffer id comes back with the voice.
    pub fn play_xma(
        &mut self,
        request: &XmaRequest<'_>,
        looping: Looping,
        gain: f32,
    ) -> Result<(VoiceId, BufferId), DecodeError> {
        let decoded = xma::decode(request)?;
        let id = self.new_buffer_id();
        let buffer = Buffer::from_i16(id, &decoded.pcm, decoded.channels, looping);
        let mut params = VoiceParams::new(decoded.sample_rate);
        params.gain = gain;
        let voice = self.add_voice(params);
        self.queue_buffer(voice, buffer);
        Ok((voice, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_blocks_are_zeroed_and_stable() {
        let mut e = Engine::new(OutputMode::Off);
        let a = e.acp_create();
        assert_eq!(a, e.acp_create());
        let c = e.allocate_shape_contexts();
        assert_ne!(a, c);
        // SAFETY: the context block is CONTEXT_BYTES long and owned by the engine.
        let bytes = unsafe { std::slice::from_raw_parts(c as *const u8, CONTEXT_BYTES) };
        assert!(bytes.iter().all(|b| *b == 0));
        e.release_shape_contexts();
        assert_ne!(e.allocate_shape_contexts(), 0);
    }

    #[test]
    fn the_silent_clock_completes_voices_on_time() {
        let mut e = Engine::new(OutputMode::Off);
        e.activate();
        assert!(e.is_active() && !e.has_device());
        let voice = e.add_voice(VoiceParams::new(DEFAULT_RATE));
        let id = e.new_buffer_id();
        let pcm = vec![1000i16; DEFAULT_RATE as usize / 2];
        e.queue_buffer(voice, Buffer::from_i16(id, &pcm, 1, Looping::Once));
        e.pump_to(Duration::from_millis(400));
        assert!(e.take_completions().is_empty(), "not done at 400 ms");
        e.pump_to(Duration::from_millis(500));
        let done = e.take_completions();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].buffer, id);
        assert_eq!(done[0].frame, 24_000);
    }

    #[test]
    fn a_long_stall_is_caught_up_in_bounded_work() {
        let mut e = Engine::new(OutputMode::Off);
        e.activate();
        let voice = e.add_voice(VoiceParams::new(DEFAULT_RATE));
        let pcm = vec![0i16; 10];
        let id = e.new_buffer_id();
        e.queue_buffer(voice, Buffer::from_i16(id, &pcm, 1, Looping::Once));
        e.pump_to(Duration::from_secs(3600));
        assert_eq!(e.take_completions().len(), 1);
        let rendered = e.mixer().lock().unwrap().rendered_frames();
        assert_eq!(rendered, u64::from(DEFAULT_RATE));
        e.pump_to(Duration::from_secs(3601));
        assert_eq!(
            e.mixer().lock().unwrap().rendered_frames(),
            2 * u64::from(DEFAULT_RATE)
        );
    }

    #[test]
    fn an_xma_stream_plays_as_silence_without_a_decoder() {
        let mut e = Engine::new(OutputMode::Off);
        e.activate();
        let request = XmaRequest {
            data: &[0; 64],
            sample_rate: 24_000,
            channels: 1,
            frames: 12_000,
        };
        let (voice, buffer) = e.play_xma(&request, Looping::Once, 1.0).unwrap();
        assert_ne!(voice, 0);
        e.pump_to(Duration::from_millis(499));
        assert!(e.take_completions().is_empty());
        e.pump_to(Duration::from_millis(501));
        let done = e.take_completions();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].buffer, buffer);
    }

    #[test]
    fn deactivating_restarts_the_clock() {
        let mut e = Engine::new(OutputMode::Off);
        e.activate();
        e.pump_to(Duration::from_secs(1));
        e.deactivate();
        assert!(!e.is_active());
        e.activate();
        e.pump_to(Duration::from_millis(10));
        assert_eq!(e.mixer().lock().unwrap().rendered_frames(), 48_000 + 480);
    }
}
