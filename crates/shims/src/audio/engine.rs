//! Everything the audio libraries share: the coprocessor address space, the command queue, the mixer and the
//! output that drives it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::apu::ApuSpace;
use super::hal::{Hal, HalCommand};
use super::mixer::{Buffer, BufferId, Completion, Looping, Mixer, VoiceId, VoiceParams};
use super::output::{Output, SharedMixer};
use super::xapu::XApu;
use super::xma::{self, DecodeError, OwnedXmaRequest, XmaRequest};
use super::xma_hw::{self, CONTEXT_BYTES as CTX_BYTES, XmaHw};

/// Rate the mixer runs at until a device sets its own.
pub const DEFAULT_RATE: u32 = 48_000;
/// Longest stretch the silent clock renders in one catch-up.
const MAX_CATCH_UP: Duration = Duration::from_secs(1);
const CHUNK_FRAMES: usize = 2048;
#[cfg(not(windows))]
const ACP_BYTES: usize = 4096;
const CONTEXT_BYTES: usize = 64;
const MAX_CONTEXTS: usize = 4096;

/// Least time between attempts to reopen a device that failed, so a missing device is not polled every frame.
const REOPEN_INTERVAL: Duration = Duration::from_secs(2);

/// Spaces out attempts to reopen the output device.
#[derive(Debug, Default)]
pub struct ReopenGate {
    last: Option<Instant>,
}

impl ReopenGate {
    /// True when an attempt is due at `now`; records it.
    pub fn due(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.duration_since(last) < REOPEN_INTERVAL => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Open the default device when the title activates the DSP; run silently when there is none.
    Auto,
    /// Never open a device; the mixer follows a silent clock.
    Off,
}

pub struct Engine {
    pub apu: ApuSpace,
    pub xapu: XApu,
    hal: Hal,
    xma_hw: XmaHw,
    #[cfg(windows)]
    hal_object: Option<Box<super::hal::object::Object>>,
    mixer: SharedMixer,
    output: Option<Output>,
    mode: OutputMode,
    active: bool,
    started: Option<Instant>,
    silent_frames: u64,
    #[cfg(not(windows))]
    acp: Option<Box<[u64]>>,
    contexts: Option<Box<[u64]>>,
    next_buffer: BufferId,
    completions: Vec<Completion>,
    noted: HashSet<String>,
    reopen: ReopenGate,
}

fn zeroed(bytes: usize) -> Box<[u64]> {
    vec![0u64; bytes / 8].into_boxed_slice()
}

impl Engine {
    pub fn new(mode: OutputMode) -> Engine {
        Engine {
            apu: ApuSpace::new(),
            xapu: XApu::default(),
            hal: Hal::default(),
            xma_hw: XmaHw::default(),
            #[cfg(windows)]
            hal_object: None,
            mixer: Arc::new(Mutex::new(Mixer::new(DEFAULT_RATE))),
            output: None,
            mode,
            active: false,
            started: None,
            silent_frames: 0,
            #[cfg(not(windows))]
            acp: None,
            contexts: None,
            next_buffer: 1,
            completions: Vec::new(),
            noted: HashSet::new(),
            reopen: ReopenGate::default(),
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

    /// The shared mixer. Asking for it means a sink is about to play on it, so the device opens now.
    pub fn mixer(&mut self) -> SharedMixer {
        self.activate();
        self.mixer.clone()
    }

    pub fn hal_mut(&mut self) -> &mut Hal {
        &mut self.hal
    }

    /// Records a command the title sent through the hardware object and acts on it: an enabled XMA context starts
    /// decoding on the worker, a disabled one stops, and blocks the title read free room in the ring.
    pub fn apply_hal(&mut self, command: HalCommand) {
        self.hal.submit(command);
        let Some(base) = self.contexts.as_ref().map(|c| c.as_ptr() as usize) else {
            return;
        };
        let total = self
            .contexts
            .as_ref()
            .map_or(0, |c| c.len() * 8 / CTX_BYTES);
        let address = |context: u32| {
            ((context as usize) < total).then(|| base + context as usize * CTX_BYTES)
        };
        match command {
            HalCommand::Enable { context } => {
                if let Some(address) = address(context) {
                    self.enable_xma_context(address);
                }
            }
            HalCommand::Disable { context } => {
                if let Some(address) = address(context) {
                    self.xma_hw.disable(address);
                }
            }
            HalCommand::OutputRead { context, blocks } => {
                if let Some(address) = address(context) {
                    xma_hw::output_read(address, blocks);
                }
            }
            HalCommand::Configure | HalCommand::Other { .. } => {}
        }
    }

    pub fn stop_hal(&mut self) {
        self.xma_hw.shutdown();
        self.hal.stop();
    }

    fn enable_xma_context(&mut self, context: usize) {
        let word = |i: usize| {
            // SAFETY: `context` lies inside the context block this engine owns.
            unsafe { ((context + i * 4) as *const u32).read_volatile() }
        };
        let (packets, input_apu, ring_apu) = ((word(0) & 0xfff) as usize, word(5), word(7));
        let (Some(input), Some(ring)) = (self.apu.map_apu(input_apu), self.apu.map_apu(ring_apu))
        else {
            self.note_once("xma-unmapped", || {
                "an XMA context was enabled with an input or output address nothing mapped".into()
            });
            return;
        };
        if packets == 0 {
            return;
        }
        // SAFETY: the title's input buffer holds `packets` 2048-byte packets, as it declared in the context.
        let data =
            unsafe { std::slice::from_raw_parts(input as *const u8, packets * 2048) }.to_vec();
        match xma_hw::setup_from_context(context, data, ring) {
            Ok(setup) => self.xma_hw.enable(context, setup),
            Err(error) => {
                self.note_once("xma-setup", || {
                    format!("an XMA context could not start: {error}")
                });
            }
        }
    }

    /// The address of the hardware layer's object: allocated on first use, stable until reset. On Windows it
    /// starts with the vtable the title calls through; elsewhere it is a zeroed block.
    pub fn acp_create(&mut self) -> usize {
        #[cfg(windows)]
        {
            let object = self.hal_object.get_or_insert_with(super::hal::object::new);
            &**object as *const super::hal::object::Object as usize
        }
        #[cfg(not(windows))]
        {
            self.acp.get_or_insert_with(|| zeroed(ACP_BYTES)).as_ptr() as usize
        }
    }

    /// Zeroed coprocessor contexts: 64 bytes each, `count` of them.
    pub fn allocate_shape_contexts(&mut self, count: usize) -> usize {
        let bytes = count.clamp(1, MAX_CONTEXTS) * CONTEXT_BYTES;
        self.contexts.get_or_insert_with(|| zeroed(bytes)).as_ptr() as usize
    }

    pub fn release_shape_contexts(&mut self) {
        self.xma_hw.shutdown();
        self.contexts = None;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn has_device(&self) -> bool {
        self.output.is_some()
    }

    /// Something wants to play: start the device if there is one and the mode allows.
    pub fn activate(&mut self) {
        self.active = true;
        self.started.get_or_insert_with(Instant::now);
        self.open_device();
    }

    fn open_device(&mut self) {
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

    /// Drops a device that failed or was unplugged and, once the gate allows, opens the default one again. The
    /// mixer keeps its voices, so sound resumes where it was.
    pub fn check_device(&mut self, now: Instant) {
        if !self.active || self.mode != OutputMode::Auto {
            return;
        }
        if self.output.as_ref().is_some_and(Output::lost) {
            self.output = None;
            self.note_once("device-lost", || {
                "the audio device failed: reopening the default device".into()
            });
        }
        if self.output.is_none() && self.reopen.due(now) {
            self.open_device();
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
        self.check_device(Instant::now());
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

    /// Starts an XMA stream on a new voice without decoding on the calling thread: a worker decodes and queues the
    /// buffer, so the title's thread never waits for the decoder. The voice plays from when the buffer arrives.
    pub fn play_xma_async(
        &mut self,
        request: OwnedXmaRequest,
        looping: Looping,
        gain: f32,
    ) -> (VoiceId, BufferId) {
        let id = self.new_buffer_id();
        let mut params = VoiceParams::new(request.sample_rate);
        params.gain = gain;
        let voice = self.add_voice(params);
        let mixer = self.mixer.clone();
        let spawned = std::thread::Builder::new()
            .name("leonida-xma".into())
            .spawn(move || {
                let decoded = xma::decode(&request.borrow());
                if let (Ok(decoded), Ok(mut mixer)) = (decoded, mixer.lock()) {
                    let buffer = Buffer::from_i16(id, &decoded.pcm, decoded.channels, looping);
                    mixer.queue(voice, buffer);
                }
            });
        if spawned.is_err() {
            self.note_once("xma-thread", || "no thread for an XMA decode".into());
        }
        (voice, id)
    }

    /// Decodes an XMA stream on the calling thread and starts it on a new voice; the buffer id comes back with the voice.
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
        let c = e.allocate_shape_contexts(350);
        assert_ne!(a, c);
        // SAFETY: the context block is 350 contexts long and owned by the engine.
        let bytes = unsafe { std::slice::from_raw_parts(c as *const u8, 350 * CONTEXT_BYTES) };
        assert!(bytes.iter().all(|b| *b == 0));
        e.release_shape_contexts();
        assert_ne!(e.allocate_shape_contexts(1), 0);
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
    fn an_async_xma_stream_queues_when_the_worker_is_done_and_never_blocks_the_caller() {
        let mut e = Engine::new(OutputMode::Off);
        e.activate();
        let request = OwnedXmaRequest {
            data: vec![0; 64],
            sample_rate: 24_000,
            channels: 1,
            frames: 12_000,
        };
        let (voice, buffer) = e.play_xma_async(request, Looping::Once, 1.0);
        let until = Instant::now() + Duration::from_secs(5);
        while e.mixer().lock().unwrap().queued_buffers(voice) == 0 {
            assert!(Instant::now() < until, "worker never queued the buffer");
            std::thread::sleep(Duration::from_millis(1));
        }
        e.pump_to(Duration::from_millis(501));
        let done = e.take_completions();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].buffer, buffer);
    }

    #[test]
    fn enabling_a_context_maps_its_buffers_and_output_reads_move_its_read_offset() {
        let mut e = Engine::new(OutputMode::Off);
        e.apu.create_heap(0, 0x226e00);
        let (input, input_apu) = e.apu.alloc(2048 * 2, 0x800).unwrap();
        let (ring, ring_apu) = e.apu.alloc(0x1700, 0x800).unwrap();
        let contexts = e.allocate_shape_contexts(4);
        let ctx = contexts + CTX_BYTES;
        // SAFETY: context 1 lies inside the four contexts just allocated.
        unsafe {
            let w = ctx as *mut u32;
            w.write(2 | (255 << 12) | (23 << 22));
            w.add(1).write(3 << 27);
            w.add(5).write(input_apu);
            w.add(7).write(ring_apu);
        }
        assert_ne!((input, ring), (0, 0));
        e.apply_hal(HalCommand::Enable { context: 1 });
        assert!(e.hal_mut().is_enabled(1));
        e.apply_hal(HalCommand::OutputRead {
            context: 1,
            blocks: 5,
        });
        // SAFETY: as above.
        let read = unsafe { (ctx as *const u32).add(9).read() };
        assert_eq!(read & 0x1f, 5);
        e.apply_hal(HalCommand::Enable { context: 99 });
        e.stop_hal();
        e.release_shape_contexts();
    }

    #[test]
    fn reopen_attempts_are_spaced() {
        let mut gate = ReopenGate::default();
        let t = Instant::now();
        assert!(gate.due(t));
        assert!(!gate.due(t + Duration::from_millis(500)));
        assert!(!gate.due(t + Duration::from_millis(1999)));
        assert!(gate.due(t + Duration::from_secs(2)));
    }

    #[test]
    fn a_device_check_with_output_off_opens_nothing() {
        let mut e = Engine::new(OutputMode::Off);
        e.activate();
        e.check_device(Instant::now());
        assert!(!e.has_device());
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
