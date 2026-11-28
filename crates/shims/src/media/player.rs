//! One open movie: the Bink calls' behaviour without the pointers. Frame numbers the game sees count from 1;
//! the decoder's count from 0.
//!
//! Timing follows the movie's own frame rate on a media clock that stops while the movie is paused: after
//! frame `k` is decoded, the next one is due once `k` frame periods of media time have passed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::decoder::StreamInfo;
use super::layout;
use super::sound::AudioSink;
use super::worker::{Item, Loader, Outcome, QUEUE_FRAMES, Worker};
use crate::trace::note;

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

pub struct SystemClock(Instant);

impl SystemClock {
    pub fn new() -> SystemClock {
        SystemClock(Instant::now())
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// The longest a call that was told to wait for a frame forever waits.
pub const WAIT_CAP: Duration = Duration::from_secs(2);

/// How much of a new reading of the audio against the wall clock moves the clock's lead.
const LEAD_SMOOTHING: f64 = 0.1;

/// A reading this far (seconds) from the lead is a break in the audio, not drift: the lead follows it at once.
const LEAD_SNAP: f64 = 0.1;

/// Past this many seconds the audio is not trusted as the clock.
const LEAD_LIMIT: f64 = 1.0;

/// What the audio engine says about movie time. The movie's own frame period keeps time between readings, and
/// each frame installed corrects it by how far the audio has got, so the picture follows the sound through
/// device clock drift and stalls but never waits for a device that plays nothing.
#[derive(Default)]
struct AudioClock {
    /// Movie time of the first sample played after the last flush.
    origin: Duration,
    /// The audio's position at the last reading.
    seen: Option<Duration>,
    /// Seconds the audio's movie time is ahead of the wall-based clock; `None` while the audio does not drive it.
    lead: Option<f64>,
}

pub struct Player {
    info: StreamInfo,
    /// Decoder track indices the movie plays, in the order the game numbers them.
    selected: Vec<usize>,
    clock: Arc<dyn Clock>,
    sink: Box<dyn AudioSink>,
    loader: Option<Loader>,
    worker: Option<Worker>,
    decoderless: bool,
    cur: u32,
    last: Option<u32>,
    /// The decoder frame (from 0) whose pixels are in the buffers.
    installed: Option<u32>,
    pending: bool,
    paused: bool,
    will_loop: bool,
    ended: bool,
    started: bool,
    media: Duration,
    running_since: Option<Duration>,
    audio_clock: AudioClock,
    registered: Option<usize>,
    slot: u32,
}

impl Player {
    /// `loader` makes the decoder on the decode thread when the first frame is asked for; without one the
    /// movie has no frames to show and every frame is due at once.
    pub fn new(
        info: StreamInfo,
        selected: Vec<usize>,
        loader: Option<Loader>,
        sink: Box<dyn AudioSink>,
        clock: Arc<dyn Clock>,
    ) -> Player {
        Player {
            decoderless: loader.is_none(),
            info,
            selected,
            clock,
            sink,
            loader,
            worker: None,
            cur: 1,
            last: None,
            installed: None,
            pending: false,
            paused: false,
            will_loop: false,
            ended: false,
            started: false,
            media: Duration::ZERO,
            running_since: None,
            audio_clock: AudioClock::default(),
            registered: None,
            slot: 0,
        }
    }

    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    pub fn current(&self) -> u32 {
        self.cur
    }

    /// The word the game reads as the last decoded frame: all ones before the first.
    pub fn last_word(&self) -> u32 {
        self.last.unwrap_or(u32::MAX)
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn track_count(&self) -> u32 {
        self.selected.len() as u32
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    fn period(&self, frames: u32) -> Duration {
        let rate = f64::from(self.info.fps_num.max(1));
        Duration::from_secs_f64(f64::from(frames) * f64::from(self.info.fps_den.max(1)) / rate)
    }

    fn wall_media(&self) -> Duration {
        self.media
            + self
                .running_since
                .map_or(Duration::ZERO, |s| self.clock.now().saturating_sub(s))
    }

    fn media_now(&self) -> Duration {
        let wall = self.wall_media();
        match self.audio_clock.lead {
            Some(lead) => Duration::from_secs_f64((wall.as_secs_f64() + lead).max(0.0)),
            None => wall,
        }
    }

    /// Reads how far the audio has played and corrects the clock by it.
    fn read_audio(&mut self) {
        let clock = &mut self.audio_clock;
        let Some(played) = self.sink.position() else {
            clock.seen = None;
            clock.lead = None;
            return;
        };
        let advancing = clock
            .seen
            .map_or(played > Duration::ZERO, |seen| played > seen);
        clock.seen = Some(played);
        if self.paused || !self.started {
            return;
        }
        if !advancing {
            clock.lead = None;
            return;
        }
        let wall = self.media
            + self
                .running_since
                .map_or(Duration::ZERO, |s| self.clock.now().saturating_sub(s));
        let reading = (clock.origin + played).as_secs_f64() - wall.as_secs_f64();
        clock.lead = match clock.lead {
            _ if reading.abs() > LEAD_LIMIT => None,
            Some(lead) if (reading - lead).abs() <= LEAD_SNAP => {
                Some(lead + (reading - lead) * LEAD_SMOOTHING)
            }
            _ => Some(reading),
        };
    }

    /// # Safety
    /// `out` must be valid for [`layout::SIZE`] bytes of writes.
    pub unsafe fn describe(&self, out: *mut u8) {
        // SAFETY: the caller's guarantee.
        unsafe { layout::describe(out, &self.info) }
    }

    /// Takes the buffers the game mapped and shows black in them until the first frame is decoded.
    ///
    /// # Safety
    /// `description` must stay a valid frame buffer description, with valid buffers, until the movie is closed.
    pub unsafe fn register(&mut self, description: usize) {
        self.registered = Some(description);
        // SAFETY: the caller's guarantee.
        unsafe {
            if layout::registered(description as *const u8) {
                layout::fill_black(description as *mut u8);
            }
        }
        self.slot = 0;
    }

    fn ensure_worker(&mut self) {
        if self.worker.is_some() {
            return;
        }
        if let Some(loader) = self.loader.take() {
            let worker = Worker::spawn(loader, self.selected.clone(), QUEUE_FRAMES);
            if self.cur > 1 {
                worker.seek(self.cur - 1);
            }
            self.worker = Some(worker);
        }
    }

    /// Starts decoding the current frame without waiting for it (`BinkDoFrameAsyncMulti`).
    pub fn do_frame_start(&mut self) {
        self.pending = true;
        self.ensure_worker();
    }

    /// Finishes the frame `do_frame_start` began, waiting at most `patience` for the decoder. True when the
    /// frame is in the buffers (or the movie has ended).
    pub fn do_frame_wait(&mut self, patience: Duration) -> bool {
        if !self.pending {
            return true;
        }
        if self.decoderless || self.ended {
            self.last = Some(self.cur);
            self.pending = false;
            return true;
        }
        let want = self.cur - 1;
        if self.installed == Some(want) {
            self.last = Some(self.cur);
            self.pending = false;
            return true;
        }
        let taken = match &self.worker {
            Some(worker) => worker.take(want, patience),
            None => {
                self.fail("the decode thread could not start".to_string());
                return true;
            }
        };
        for item in taken.skipped.iter() {
            self.play_audio(item);
        }
        match taken.outcome {
            Outcome::Ready(item) => {
                self.play_audio(&item);
                self.install(&item);
                self.last = Some(self.cur);
                self.installed = Some(want);
                self.pending = false;
                true
            }
            Outcome::NotReady => false,
            Outcome::Behind => {
                if let Some(worker) = &self.worker {
                    worker.seek(want);
                }
                false
            }
            Outcome::Failed(message) => {
                self.fail(message);
                true
            }
            Outcome::End => {
                self.pending = false;
                true
            }
        }
    }

    fn fail(&mut self, message: String) {
        note(format!(
            "Bink: a movie's decoder failed and the movie ends here: {message}"
        ));
        self.ended = true;
        self.cur = self.info.frames.max(1);
        self.last = Some(self.cur);
        self.pending = false;
        self.sink.flush();
    }

    fn play_audio(&mut self, item: &Item) {
        for (track, samples) in &item.audio {
            if let Some(info) = self.info.audio.get(*track) {
                self.sink
                    .push(*track, info.rate, info.channels, samples.clone());
            }
        }
    }

    fn install(&mut self, item: &Item) {
        if let Some(description) = self.registered {
            let set = self.slot;
            // SAFETY: `register`'s guarantee.
            unsafe { layout::fill(description as *mut u8, &item.frame, set) };
            self.slot = (set + 1) % layout::TOTAL_FRAMES;
        }
        if !self.started {
            self.started = true;
            self.running_since = (!self.paused).then(|| self.clock.now());
        }
        self.read_audio();
    }

    /// Decodes the current frame, waiting for it a bounded time (`BinkDoFrame`).
    pub fn do_frame(&mut self) {
        self.do_frame_start();
        self.do_frame_wait(WAIT_CAP);
    }

    pub fn next_frame(&mut self) {
        if self.cur < self.info.frames {
            self.cur += 1;
        } else if self.will_loop && !self.ended {
            self.goto(1);
        }
    }

    pub fn goto(&mut self, frame: u32) {
        if self.ended {
            return;
        }
        self.cur = frame.clamp(1, self.info.frames.max(1));
        self.installed = None;
        self.media = self.period(self.cur - 1);
        self.running_since = (self.started && !self.paused).then(|| self.clock.now());
        self.sink.flush();
        self.audio_clock = AudioClock {
            origin: self.media,
            ..AudioClock::default()
        };
        if let Some(worker) = &self.worker
            && !worker.reaches(self.cur - 1)
        {
            worker.seek(self.cur - 1);
        }
    }

    /// Returns the new pause state.
    pub fn pause(&mut self, pause: bool) -> bool {
        if pause != self.paused {
            if pause {
                self.media = self.media_now();
                self.running_since = None;
            } else if self.started {
                self.running_since = Some(self.clock.now());
            }
            self.paused = pause;
            self.sink.pause(pause);
        }
        self.paused
    }

    pub fn set_will_loop(&mut self, on: bool) {
        self.will_loop = on;
    }

    /// True while the next frame is not due yet (`BinkWait`).
    pub fn wait(&self) -> bool {
        if self.decoderless || self.ended {
            return false;
        }
        let Some(done) = self.installed else {
            return false;
        };
        if self.paused {
            return true;
        }
        if done + 1 >= self.info.frames {
            return false;
        }
        self.media_now() < self.period(done + 1)
    }

    /// True when the next frame is already a whole frame period overdue (`BinkShouldSkip`).
    pub fn should_skip(&self) -> bool {
        if self.decoderless || self.ended || self.paused {
            return false;
        }
        let Some(done) = self.installed else {
            return false;
        };
        self.media_now().saturating_sub(self.period(done + 1)) >= self.period(1)
    }

    /// `volume` is 0 to 0x10000, the range the game clamps its own to.
    pub fn set_volume(&mut self, track: usize, volume: i32) {
        if let Some(&decoder_track) = self.selected.get(track) {
            self.sink
                .set_gain(decoder_track, (volume.clamp(0, 0x10000) as f32) / 65536.0);
        }
    }

    pub fn close(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
        self.registered = None;
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::testing::{
        Call, FakeDecoder, FakeGame, Gate, ManualClock, RecordingSink, SinkEvent,
    };

    const FRAME: Duration = Duration::from_millis(34);

    struct Rig {
        player: Player,
        clock: Arc<ManualClock>,
        sink: RecordingSink,
        log: Arc<std::sync::Mutex<Vec<Call>>>,
    }

    fn rig_with(decoder: FakeDecoder, selected: Vec<usize>) -> Rig {
        let clock = Arc::new(ManualClock::default());
        let sink = RecordingSink::default();
        let info = decoder.info.clone();
        let log = decoder.log.clone();
        let player = Player::new(
            info,
            selected,
            Some(Box::new(move || Ok(Box::new(decoder) as _))),
            Box::new(sink.clone()),
            clock.clone(),
        );
        Rig {
            player,
            clock,
            sink,
            log,
        }
    }

    fn rig(frames: u32) -> Rig {
        rig_with(FakeDecoder::new(frames, 32, 16, 1, false), vec![0])
    }

    #[test]
    fn frames_are_due_at_the_movies_frame_rate_and_not_while_paused() {
        let mut r = rig(10);
        assert!(!r.player.wait(), "nothing waits before the first frame");
        r.player.do_frame();
        assert_eq!(r.player.last_word(), 1);
        assert!(r.player.wait());
        r.clock.advance(Duration::from_millis(20));
        assert!(r.player.wait(), "frame 2 is due at 33 ms");
        r.clock.advance(Duration::from_millis(14));
        assert!(!r.player.wait());
        r.player.next_frame();
        r.player.do_frame();
        assert_eq!(r.player.last_word(), 2);
        assert!(r.player.wait());
        assert!(r.player.pause(true));
        r.clock.advance(Duration::from_secs(5));
        assert!(r.player.wait(), "a paused movie has no frame due");
        r.player.pause(false);
        r.clock.advance(Duration::from_millis(30));
        assert!(r.player.wait(), "the pause did not count as playing time");
        r.clock.advance(FRAME);
        assert!(!r.player.wait());
    }

    #[test]
    fn a_frame_two_periods_late_is_to_be_skipped() {
        let mut r = rig(10);
        r.player.do_frame();
        assert!(!r.player.should_skip());
        r.clock.advance(Duration::from_millis(60));
        assert!(!r.player.should_skip());
        r.clock.advance(Duration::from_millis(40));
        assert!(r.player.should_skip());
        r.player.pause(true);
        assert!(!r.player.should_skip());
    }

    #[test]
    fn decoded_frames_fill_the_registered_buffers_in_turn() {
        let mut r = rig_with(FakeDecoder::new(10, 32, 16, 0, true), vec![]);
        let mut game = FakeGame::new(r.player.info());
        // SAFETY: the game's memory outlives the player.
        unsafe { r.player.register(game.ptr()) };
        assert_eq!(game.plane(0, 0, 4)[0], 0, "black until the first frame");
        assert_eq!(game.plane(0, 1, 4)[0], 0x80);
        for n in 0..4u32 {
            r.player.do_frame();
            assert_eq!(game.frame_num(), n % 3);
            let set = (n % 3) as usize;
            for p in 0..4 {
                let (w, h) = if p == 1 || p == 2 { (16, 8) } else { (32, 16) };
                let memory = game.plane(set, p, 4);
                for y in 0..h {
                    for x in 0..w {
                        assert_eq!(memory[y * 64 + x], FakeDecoder::pixel(p, n, x, y));
                    }
                }
            }
            r.player.next_frame();
        }
    }

    #[test]
    fn going_to_a_frame_seeks_unless_it_is_already_on_its_way() {
        let mut r = rig(40);
        r.player.do_frame();
        r.player.next_frame();
        r.player.goto(3);
        assert!(
            !r.log
                .lock()
                .unwrap()
                .iter()
                .any(|c| matches!(c, Call::Seek(_)))
        );
        r.player.do_frame();
        assert_eq!(r.player.last_word(), 3);
        r.player.goto(30);
        assert_eq!(r.player.current(), 30);
        assert!(!r.player.wait(), "the frame after a goto is due at once");
        r.player.do_frame();
        assert_eq!(r.player.last_word(), 30);
        assert!(r.log.lock().unwrap().contains(&Call::Seek(29)));
        r.player.goto(999);
        assert_eq!(r.player.current(), 40);
        r.player.goto(0);
        assert_eq!(r.player.current(), 1);
    }

    #[test]
    fn looping_wraps_to_the_first_frame_and_decodes_it_again() {
        let mut r = rig(3);
        r.player.set_will_loop(true);
        for _ in 0..3 {
            r.player.do_frame();
            r.player.next_frame();
        }
        assert_eq!(r.player.current(), 1);
        r.player.do_frame();
        assert_eq!(r.player.last_word(), 1);
        assert!(r.log.lock().unwrap().contains(&Call::Seek(0)));
    }

    #[test]
    fn closing_stops_the_decoder_and_a_second_close_is_harmless() {
        let mut r = rig(10);
        r.player.do_frame();
        r.player.close();
        r.player.close();
    }

    #[test]
    fn audio_follows_the_frames_played_including_skipped_ones_and_stops_for_a_seek() {
        let mut r = rig_with(FakeDecoder::new(20, 16, 16, 2, false), vec![1]);
        r.player.do_frame();
        r.player.next_frame();
        r.player.next_frame();
        r.player.do_frame();
        let events = r.sink.0.lock().unwrap().clone();
        let pushes: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                SinkEvent::Push {
                    track,
                    rate,
                    channels,
                    samples,
                } => Some((*track, *rate, *channels, samples[0])),
                _ => None,
            })
            .collect();
        assert_eq!(
            pushes,
            vec![
                (1, 48000, 2, FakeDecoder::sample(1, 0, 0)),
                (1, 48000, 2, FakeDecoder::sample(1, 1, 0)),
                (1, 48000, 2, FakeDecoder::sample(1, 2, 0)),
            ],
            "frame 1's audio went too, though the picture skipped it"
        );
        r.player.goto(10);
        assert_eq!(r.sink.0.lock().unwrap().last(), Some(&SinkEvent::Flush));
        r.player.set_volume(0, 0x8000);
        r.player.set_volume(5, 0);
        r.player.pause(true);
        let events = r.sink.0.lock().unwrap().clone();
        assert_eq!(
            &events[events.len() - 2..],
            [SinkEvent::Gain(1, 0.5), SinkEvent::Pause(true)]
        );
    }

    fn set_audio(r: &Rig, played: Option<Duration>) {
        *r.sink.1.lock().unwrap() = played;
    }

    #[test]
    fn the_picture_follows_the_audio_when_it_runs_ahead_of_the_wall_clock() {
        let mut r = rig(10);
        set_audio(&r, Some(Duration::ZERO));
        r.player.do_frame();
        r.clock.advance(Duration::from_millis(33));
        set_audio(&r, Some(Duration::from_millis(40)));
        r.player.next_frame();
        r.player.do_frame();
        r.clock.advance(Duration::from_millis(27));
        assert!(
            !r.player.wait(),
            "the audio is 7 ms ahead, so frame 3 (due at 66.7 ms) is due at 60 ms of wall time"
        );
        let mut plain = rig(10);
        plain.player.do_frame();
        plain.clock.advance(Duration::from_millis(33));
        plain.player.next_frame();
        plain.player.do_frame();
        plain.clock.advance(Duration::from_millis(27));
        assert!(plain.player.wait(), "without audio the wall clock decides");
    }

    #[test]
    fn a_stalled_audio_device_gives_the_clock_back_to_the_wall() {
        let mut r = rig(10);
        set_audio(&r, Some(Duration::ZERO));
        r.player.do_frame();
        r.clock.advance(Duration::from_millis(33));
        set_audio(&r, Some(Duration::from_millis(80)));
        r.player.next_frame();
        r.player.do_frame();
        assert!(r.player.audio_clock.lead.is_some());
        r.clock.advance(Duration::from_millis(33));
        r.player.next_frame();
        r.player.do_frame();
        assert_eq!(r.player.audio_clock.lead, None, "the audio did not move");
        r.clock.advance(Duration::from_millis(34));
        assert!(!r.player.wait(), "frame 4 is due after 100 ms of wall time");
    }

    #[test]
    fn a_seek_restarts_the_audio_clock_at_the_frames_movie_time() {
        let mut r = rig(40);
        set_audio(&r, Some(Duration::ZERO));
        r.player.do_frame();
        r.clock.advance(Duration::from_millis(33));
        set_audio(&r, Some(Duration::from_millis(33)));
        r.player.next_frame();
        r.player.do_frame();
        r.player.goto(31);
        assert_eq!(r.player.audio_clock.lead, None);
        assert_eq!(r.player.audio_clock.origin, Duration::from_secs(1));
        set_audio(&r, Some(Duration::ZERO));
        r.player.do_frame();
        r.clock.advance(Duration::from_millis(33));
        set_audio(&r, Some(Duration::from_millis(33)));
        r.player.next_frame();
        r.player.do_frame();
        let lead = r.player.audio_clock.lead.expect("audio plays again");
        assert!(
            lead.abs() < 0.01,
            "audio at 1.033 s and wall at 1.033 s: {lead}"
        );
    }

    #[test]
    fn a_paused_movie_keeps_its_audio_lead_and_a_missing_position_drops_it() {
        let mut r = rig(10);
        set_audio(&r, Some(Duration::ZERO));
        r.player.do_frame();
        r.clock.advance(Duration::from_millis(33));
        set_audio(&r, Some(Duration::from_millis(43)));
        r.player.next_frame();
        r.player.do_frame();
        let lead = r.player.audio_clock.lead.unwrap();
        r.player.pause(true);
        r.clock.advance(Duration::from_secs(3));
        assert_eq!(r.player.audio_clock.lead, Some(lead));
        set_audio(&r, None);
        r.player.pause(false);
        r.player.next_frame();
        r.player.do_frame();
        assert_eq!(r.player.audio_clock.lead, None);
    }

    #[test]
    fn nothing_the_game_calls_waits_for_a_stuck_decoder() {
        let mut decoder = FakeDecoder::new(20, 16, 16, 1, false);
        let gate = Gate::new(false);
        decoder.gate = Some(gate.clone());
        let mut r = rig_with(decoder, vec![0]);
        let start = Instant::now();
        r.player.do_frame_start();
        assert!(!r.player.do_frame_wait(Duration::ZERO));
        r.player.next_frame();
        r.player.goto(7);
        r.player.pause(true);
        r.player.pause(false);
        r.player.set_volume(0, 100);
        assert!(!r.player.wait());
        assert!(!r.player.should_skip());
        assert!(start.elapsed() < Duration::from_millis(300));
        assert!(!r.player.do_frame_wait(Duration::from_millis(50)));
        let closing = Instant::now();
        r.player.close();
        assert!(closing.elapsed() < Duration::from_secs(3));
        gate.set(true);
    }

    #[test]
    fn a_decoder_error_ends_the_movie_cleanly() {
        let mut decoder = FakeDecoder::new(20, 16, 16, 0, false);
        decoder.fail_at = Some(2);
        let mut r = rig_with(decoder, vec![]);
        let mut guard = 0;
        while r.player.current() < 20 && !r.player.ended() {
            r.player.do_frame();
            r.player.next_frame();
            guard += 1;
            assert!(guard < 100);
        }
        assert!(r.player.ended());
        assert_eq!((r.player.current(), r.player.last_word()), (20, 20));
        assert!(!r.player.wait());
        r.player.goto(3);
        assert_eq!(r.player.current(), 20, "an ended movie stays at its end");
        r.player.do_frame();
    }

    #[test]
    fn a_decoder_that_will_not_open_ends_it_too() {
        let clock = Arc::new(ManualClock::default());
        let info = FakeDecoder::new(5, 16, 16, 0, false).info;
        let mut p = Player::new(
            info,
            vec![],
            Some(Box::new(|| {
                Err(crate::media::DecodeError("unsupported".into()))
            })),
            Box::new(RecordingSink::default()),
            clock,
        );
        p.do_frame();
        assert!(p.ended());
        assert_eq!(p.last_word(), 5);
    }

    #[test]
    fn without_a_decoder_every_frame_is_due_at_once_and_black() {
        let clock = Arc::new(ManualClock::default());
        let info = FakeDecoder::new(4, 16, 16, 0, false).info;
        let mut p = Player::new(
            info.clone(),
            vec![],
            None,
            Box::new(RecordingSink::default()),
            clock,
        );
        let mut game = FakeGame::new(&info);
        // SAFETY: the game's memory outlives the player.
        unsafe { p.register(game.ptr()) };
        let mut shown = 0;
        loop {
            assert!(!p.wait());
            p.do_frame();
            shown += 1;
            if p.last_word() == 4 {
                break;
            }
            p.next_frame();
        }
        assert_eq!(shown, 4);
        assert_eq!(game.plane(0, 0, 3)[0], 0);
    }
}
