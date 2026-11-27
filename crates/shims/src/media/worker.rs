//! The decode thread: runs the decoder ahead of playback into a small queue of frames (with the audio that
//! belongs to each), so no call the game makes waits for a decode. The consumer side only takes the queue's
//! lock for a moment; a late frame is waited for only when the caller asks to, and for a bounded time.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::decoder::{DecodeError, DecodedFrame, MovieDecoder};

/// How far past the decoder's position a frame is still reached by decoding on rather than seeking.
const REACH: u32 = 8;

/// Frames decoded ahead of the one playing.
pub const QUEUE_FRAMES: usize = 4;

/// How long `stop` waits for a worker stuck in the decoder before leaving it to finish by itself.
const STOP_PATIENCE: Duration = Duration::from_millis(500);

pub type Loader = Box<dyn FnOnce() -> Result<Box<dyn MovieDecoder>, DecodeError> + Send>;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub index: u32,
    pub frame: DecodedFrame,
    /// Per selected track (index into the decoder's tracks): the samples of this frame.
    pub audio: Vec<(usize, Vec<f32>)>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Ready(Item),
    /// Not decoded yet.
    NotReady,
    /// The queue has moved past the frame asked for; it needs a seek.
    Behind,
    Failed(String),
    /// The frame asked for is past the end of the stream.
    End,
}

#[derive(Debug)]
pub struct Taken {
    /// Frames before the one asked for, in order; the player still plays their audio.
    pub skipped: Vec<Item>,
    pub outcome: Outcome,
}

struct State {
    queue: VecDeque<Item>,
    epoch: u64,
    seek: Option<u32>,
    /// The frame the decoder produces next, as the consumer sees the stream.
    next: u32,
    frames: u32,
    stop: bool,
    failure: Option<String>,
    ready: bool,
}

struct Shared {
    state: Mutex<State>,
    produced: Condvar,
    room: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, DecodeError>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.0),
        Err(_) => Err(format!("the decoder panicked in {what}")),
    }
}

enum Job {
    Seek(u32),
    Decode(u32, u64),
}

fn run(shared: Arc<Shared>, loader: Loader, tracks: Vec<usize>, capacity: usize) {
    let fail = |message: String| {
        shared.lock().failure.get_or_insert(message);
        shared.produced.notify_all();
    };
    let mut decoder = match guarded("opening", loader) {
        Ok(decoder) => decoder,
        Err(message) => return fail(message),
    };
    let frames = decoder.info().frames;
    {
        let mut state = shared.lock();
        state.frames = frames;
        state.ready = true;
    }
    shared.produced.notify_all();
    loop {
        let job = {
            let mut state = shared.lock();
            loop {
                if state.stop {
                    return;
                }
                if let Some(frame) = state.seek.take() {
                    break Job::Seek(frame);
                }
                if state.queue.len() < capacity && state.next < state.frames {
                    break Job::Decode(state.next, state.epoch);
                }
                state = shared.room.wait(state).unwrap_or_else(|p| p.into_inner());
            }
        };
        match job {
            Job::Seek(frame) => {
                if let Err(message) = guarded("seeking", || decoder.seek(frame)) {
                    return fail(message);
                }
            }
            Job::Decode(index, epoch) => {
                let decoded = guarded("decoding", || decoder.decode_next());
                let state = shared.lock();
                if state.epoch != epoch || state.stop {
                    continue;
                }
                match decoded {
                    Ok(frame) => {
                        drop(state);
                        let audio = tracks
                            .iter()
                            .map(|&t| {
                                (
                                    t,
                                    catch_unwind(AssertUnwindSafe(|| decoder.audio(t)))
                                        .unwrap_or_default(),
                                )
                            })
                            .collect();
                        let mut state = shared.lock();
                        if state.epoch == epoch && !state.stop {
                            state.queue.push_back(Item {
                                index,
                                frame,
                                audio,
                            });
                            state.next = index + 1;
                            shared.produced.notify_all();
                        }
                    }
                    Err(message) => {
                        drop(state);
                        return fail(message);
                    }
                }
            }
        }
    }
}

impl Worker {
    /// Starts a thread that runs `loader` and then decodes `tracks`' audio with every frame.
    pub fn spawn(loader: Loader, tracks: Vec<usize>, capacity: usize) -> Worker {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                epoch: 0,
                seek: None,
                next: 0,
                frames: 0,
                stop: false,
                failure: None,
                ready: false,
            }),
            produced: Condvar::new(),
            room: Condvar::new(),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("bink-decode".to_string())
                .spawn(move || run(shared, loader, tracks, capacity.max(1)))
                .ok()
        };
        if thread.is_none() {
            shared.lock().failure = Some("the decode thread could not start".to_string());
        }
        Worker { shared, thread }
    }

    /// Makes `frame` (from 0) the next one produced and drops everything queued. Never waits.
    pub fn seek(&self, frame: u32) {
        let mut state = self.shared.lock();
        state.queue.clear();
        state.seek = Some(frame);
        state.next = frame;
        state.epoch += 1;
        drop(state);
        self.shared.room.notify_all();
    }

    /// Whether `frame` is queued or close enough ahead of the decoder that decoding on is cheaper than a seek.
    pub fn reaches(&self, frame: u32) -> bool {
        let state = self.shared.lock();
        let front = state.queue.front().map_or(state.next, |i| i.index);
        frame >= front && frame <= state.next + REACH
    }

    /// Takes frame `wanted`, dropping the earlier ones. Waits at most `patience` for it to be decoded.
    pub fn take(&self, wanted: u32, patience: Duration) -> Taken {
        let deadline = Instant::now() + patience;
        let mut skipped = Vec::new();
        let mut state = self.shared.lock();
        let outcome = loop {
            while state.queue.front().is_some_and(|i| i.index < wanted) {
                skipped.extend(state.queue.pop_front());
            }
            if let Some(front) = state.queue.front() {
                break if front.index == wanted {
                    state
                        .queue
                        .pop_front()
                        .map_or(Outcome::NotReady, Outcome::Ready)
                } else {
                    Outcome::Behind
                };
            }
            if let Some(message) = &state.failure {
                break Outcome::Failed(message.clone());
            }
            if state.ready && wanted >= state.frames {
                break Outcome::End;
            }
            if state.ready && wanted < state.next {
                break Outcome::Behind;
            }
            let now = Instant::now();
            if now >= deadline {
                break Outcome::NotReady;
            }
            state = self
                .shared
                .produced
                .wait_timeout(state, deadline - now)
                .map(|(state, _)| state)
                .unwrap_or_else(|p| p.into_inner().0);
        };
        drop(state);
        self.shared.room.notify_all();
        Taken { skipped, outcome }
    }

    /// The failure the decoder ended with, if it has.
    pub fn failure(&self) -> Option<String> {
        self.shared.lock().failure.clone()
    }

    /// Asks the thread to stop and waits a short while for it. A thread stuck inside the decoder is left to
    /// finish on its own, so closing a movie never hangs the caller.
    pub fn stop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.room.notify_all();
        if let Some(thread) = self.thread.take() {
            let deadline = Instant::now() + STOP_PATIENCE;
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::testing::{Call, FakeDecoder, Gate};

    const WAIT: Duration = Duration::from_secs(5);

    fn spawn(decoder: FakeDecoder, tracks: Vec<usize>) -> Worker {
        Worker::spawn(Box::new(move || Ok(Box::new(decoder) as _)), tracks, 3)
    }

    fn ready(taken: Taken) -> Item {
        match taken.outcome {
            Outcome::Ready(item) => item,
            other => panic!("not ready: {other:?}"),
        }
    }

    #[test]
    fn frames_come_out_in_order_with_their_audio() {
        let w = spawn(FakeDecoder::new(6, 16, 16, 2, false), vec![0, 1]);
        for index in 0..6 {
            let item = ready(w.take(index, WAIT));
            assert_eq!(item.index, index);
            assert_eq!(item.frame.index, index);
            assert_eq!(item.audio.len(), 2);
            assert_eq!(item.audio[1].0, 1);
            assert_eq!(item.audio[1].1[3], FakeDecoder::sample(1, index, 3));
        }
        assert_eq!(w.take(6, WAIT).outcome, Outcome::End);
    }

    #[test]
    fn the_queue_never_holds_more_than_its_capacity() {
        let d = FakeDecoder::new(50, 16, 16, 0, false);
        let log = d.log.clone();
        let w = spawn(d, vec![]);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            log.lock().unwrap().len(),
            3,
            "decoded ahead by the capacity and no more"
        );
        ready(w.take(0, WAIT));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(log.lock().unwrap().len(), 4);
    }

    #[test]
    fn taking_a_later_frame_hands_back_the_skipped_ones() {
        let w = spawn(FakeDecoder::new(10, 16, 16, 1, false), vec![0]);
        let taken = w.take(2, WAIT);
        assert_eq!(
            taken.skipped.iter().map(|i| i.index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(matches!(
            taken.outcome,
            Outcome::Ready(Item { index: 2, .. })
        ));
    }

    #[test]
    fn a_seek_drops_the_queue_and_restarts_there() {
        let d = FakeDecoder::new(40, 16, 16, 0, false);
        let log = d.log.clone();
        let w = spawn(d, vec![]);
        ready(w.take(0, WAIT));
        assert!(w.reaches(2));
        assert!(!w.reaches(30));
        w.seek(30);
        assert_eq!(ready(w.take(30, WAIT)).index, 30);
        assert_eq!(ready(w.take(31, WAIT)).index, 31);
        assert!(log.lock().unwrap().contains(&Call::Seek(30)));
        assert_eq!(w.take(5, Duration::ZERO).outcome, Outcome::Behind);
    }

    #[test]
    fn a_decoder_error_is_reported_and_the_thread_ends() {
        let mut d = FakeDecoder::new(10, 16, 16, 0, false);
        d.fail_at = Some(3);
        let w = spawn(d, vec![]);
        for index in 0..3 {
            ready(w.take(index, WAIT));
        }
        assert_eq!(
            w.take(3, WAIT).outcome,
            Outcome::Failed("fake failure at frame 3".to_string())
        );
        assert!(w.failure().is_some());
    }

    #[test]
    fn a_decoder_that_cannot_open_fails_the_first_take() {
        let w = Worker::spawn(
            Box::new(|| Err(DecodeError("no such stream".to_string()))),
            vec![],
            3,
        );
        assert_eq!(
            w.take(0, WAIT).outcome,
            Outcome::Failed("no such stream".to_string())
        );
    }

    #[test]
    fn a_panicking_decoder_is_a_failure_not_a_crash() {
        struct Panics;
        impl MovieDecoder for Panics {
            fn info(&self) -> crate::media::StreamInfo {
                FakeDecoder::new(5, 16, 16, 0, false).info
            }
            fn decode_next(&mut self) -> Result<DecodedFrame, DecodeError> {
                panic!("corrupt frame");
            }
            fn seek(&mut self, _: u32) -> Result<(), DecodeError> {
                Ok(())
            }
            fn audio(&mut self, _: usize) -> Vec<f32> {
                Vec::new()
            }
        }
        let w = Worker::spawn(Box::new(|| Ok(Box::new(Panics) as _)), vec![], 3);
        assert!(matches!(w.take(0, WAIT).outcome, Outcome::Failed(m) if m.contains("panicked")));
    }

    #[test]
    fn asking_for_a_frame_that_is_not_there_yet_waits_only_as_long_as_told() {
        let mut d = FakeDecoder::new(10, 16, 16, 0, false);
        let gate = Gate::new(false);
        d.gate = Some(gate.clone());
        let w = spawn(d, vec![]);
        let start = Instant::now();
        assert_eq!(w.take(0, Duration::ZERO).outcome, Outcome::NotReady);
        assert!(start.elapsed() < Duration::from_millis(100));
        assert_eq!(
            w.take(0, Duration::from_millis(60)).outcome,
            Outcome::NotReady
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        gate.set(true);
        ready(w.take(0, WAIT));
    }

    #[test]
    fn stopping_does_not_wait_forever_for_a_stuck_decoder() {
        let mut d = FakeDecoder::new(10, 16, 16, 0, false);
        let gate = Gate::new(false);
        d.gate = Some(gate.clone());
        let mut w = spawn(d, vec![]);
        std::thread::sleep(Duration::from_millis(50));
        let start = Instant::now();
        w.stop();
        assert!(start.elapsed() < Duration::from_secs(3));
        gate.set(true);
    }
}
