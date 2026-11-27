//! When the real and the generated frames reach the screen: a [`Sequencer`] turns the arrival of a real frame into
//! timed presents (through `upscale::framegen::Pacer`), and a [`Presenter`] thread shows them on time.
//!
//! The game thread only captures the frame and queues its presents, so the game's own frame rate is not held back by
//! the wait for the generated frame's turn. The thread sleeps on a [`Clock`] (the high-resolution timer in
//! `pacing`, or a fake one in tests).

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use upscale::framegen::{Pacer, PresentPlan};

use crate::pacing::Clock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Real,
    Generated,
}

/// Something to show at `at` (on the presenter's clock).
pub struct Scheduled<T> {
    pub at: Duration,
    pub kind: Shown,
    pub item: T,
}

/// Plans the presents of every real frame. Shows never go out of order: a present is never earlier than the one
/// before it.
pub struct Sequencer {
    pacer: Pacer,
    tail: Duration,
}

impl Sequencer {
    /// `phase` is where the generated frame sits between two real ones: 0.5 for a doubled frame rate.
    pub fn new(phase: f32) -> Self {
        Sequencer {
            pacer: Pacer::new(phase),
            tail: Duration::ZERO,
        }
    }

    /// Feeds the arrival of a real frame to the pacer. `None` for the first frame and after a hitch: the frame is
    /// shown alone.
    pub fn on_real_frame(&mut self, now: Duration) -> Option<PresentPlan> {
        self.pacer.on_real_frame(now)
    }

    /// The presents for a frame that arrived at `now`. `generated` is given only together with a plan.
    pub fn entries<T>(
        &mut self,
        now: Duration,
        plan: Option<PresentPlan>,
        real: T,
        generated: Option<T>,
    ) -> Vec<Scheduled<T>> {
        let mut out = Vec::with_capacity(2);
        let mut last = self.tail;
        match (plan, generated) {
            (Some(plan), Some(generated)) => {
                let at = (now + plan.generated_delay).max(last);
                out.push(Scheduled {
                    at,
                    kind: Shown::Generated,
                    item: generated,
                });
                last = at;
                out.push(Scheduled {
                    at: (now + plan.real_delay).max(last),
                    kind: Shown::Real,
                    item: real,
                });
            }
            _ => out.push(Scheduled {
                at: now.max(last),
                kind: Shown::Real,
                item: real,
            }),
        }
        if let Some(end) = out.last() {
            self.tail = end.at;
        }
        out
    }

    /// Forget the rhythm (a cut, a menu, a size change).
    pub fn reset(&mut self) {
        self.pacer.reset();
    }
}

struct State<T> {
    queue: VecDeque<Scheduled<T>>,
    showing: bool,
    quit: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Shows scheduled items in order, each when the clock reaches its time.
pub struct Presenter<T: Send + 'static> {
    shared: Arc<Shared<T>>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Presenter<T> {
    pub fn new(clock: Arc<dyn Clock>, mut show: impl FnMut(Scheduled<T>) + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                showing: false,
                quit: false,
            }),
            changed: Condvar::new(),
        });
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("leonida-frame-generation".into())
            .spawn(move || {
                loop {
                    let entry = {
                        let mut state = worker.lock();
                        loop {
                            if state.quit {
                                return;
                            }
                            if let Some(entry) = state.queue.pop_front() {
                                state.showing = true;
                                break entry;
                            }
                            state = worker
                                .changed
                                .wait(state)
                                .unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    clock.sleep_until(entry.at);
                    show(entry);
                    worker.lock().showing = false;
                    worker.changed.notify_all();
                }
            })
            .ok();
        Presenter { shared, thread }
    }

    pub fn schedule(&self, entries: impl IntoIterator<Item = Scheduled<T>>) {
        self.shared.lock().queue.extend(entries);
        self.shared.changed.notify_all();
    }

    /// Blocks until at most `limit` presents are waiting or being shown.
    pub fn wait_at_most(&self, limit: usize) {
        let mut state = self.shared.lock();
        while state.queue.len() + usize::from(state.showing) > limit {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Blocks until everything scheduled has been shown.
    pub fn flush(&self) {
        self.wait_at_most(0);
    }
}

impl<T: Send + 'static> Drop for Presenter<T> {
    fn drop(&mut self) {
        self.shared.lock().quit = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct FakeClock(Arc<Mutex<Duration>>);

    impl FakeClock {
        fn set(&self, to: Duration) {
            let mut now = self.0.lock().unwrap();
            if *now < to {
                *now = to;
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            *self.0.lock().unwrap()
        }

        fn sleep_until(&self, deadline: Duration) {
            self.set(deadline);
        }
    }

    type Log = Arc<Mutex<Vec<(Shown, Duration, u32)>>>;

    fn rig() -> (FakeClock, Sequencer, Presenter<u32>, Log) {
        let clock = FakeClock::default();
        let log: Log = Arc::default();
        let (c, l) = (clock.clone(), log.clone());
        let presenter = Presenter::new(Arc::new(clock.clone()), move |s: Scheduled<u32>| {
            l.lock().unwrap().push((s.kind, c.now(), s.item));
        });
        (clock, Sequencer::new(0.5), presenter, log)
    }

    /// One real frame arrives at `at`; `generated` says whether a generated frame is made for it.
    fn arrive(
        (clock, sequencer, presenter, _): &mut (FakeClock, Sequencer, Presenter<u32>, Log),
        at: Duration,
        id: u32,
        generated: bool,
    ) {
        presenter.flush();
        clock.set(at);
        let now = clock.now();
        let plan = sequencer.on_real_frame(now);
        let partner = (generated && plan.is_some()).then_some(1000 + id);
        presenter.schedule(sequencer.entries(now, plan, id, partner));
    }

    fn us(n: u64) -> Duration {
        Duration::from_micros(n)
    }

    #[test]
    fn real_and_generated_frames_alternate_at_even_intervals() {
        let mut rig = rig();
        let interval = 16_667;
        for k in 0..40u32 {
            arrive(&mut rig, us(u64::from(k) * interval), k, true);
        }
        rig.2.flush();
        let log = rig.3.lock().unwrap().clone();
        // the first real frame is alone, then each real frame is preceded by its generated frame
        assert_eq!(log[0].0, Shown::Real);
        assert_eq!(log[0].2, 0);
        let rest = &log[1..];
        assert_eq!(rest.len(), 2 * 39);
        for (i, pair) in rest.chunks(2).enumerate() {
            assert_eq!(pair[0].0, Shown::Generated, "pair {i}");
            assert_eq!(pair[1].0, Shown::Real, "pair {i}");
            assert_eq!(
                pair[0].2,
                1000 + pair[1].2,
                "the generated frame belongs to its real frame"
            );
        }
        let times: Vec<Duration> = rest.iter().map(|e| e.1).collect();
        for pair in times.windows(2).skip(2) {
            let gap = (pair[1] - pair[0]).as_micros() as i64;
            assert!(
                (gap - interval as i64 / 2).abs() <= 2,
                "presents {gap} us apart, wanted {}",
                interval / 2
            );
        }
        let reals: Vec<_> = rest.iter().filter(|e| e.0 == Shown::Real).collect();
        let ids: Vec<u32> = reals.iter().map(|e| e.2).collect();
        assert_eq!(
            ids,
            (1..40).collect::<Vec<_>>(),
            "real frames keep their order"
        );
    }

    #[test]
    fn the_added_latency_is_half_an_interval_plus_the_guard() {
        let mut rig = rig();
        let interval = 16_667u64;
        for k in 0..20u32 {
            arrive(&mut rig, us(u64::from(k) * interval), k, true);
        }
        rig.2.flush();
        let log = rig.3.lock().unwrap().clone();
        for e in log.iter().filter(|e| e.0 == Shown::Real && e.2 >= 5) {
            let arrived = us(u64::from(e.2) * interval);
            let delay = e.1 - arrived;
            assert!(
                delay >= us(interval / 2) && delay <= us(interval / 2 + 2_100),
                "frame {} waited {delay:?}",
                e.2
            );
        }
    }

    #[test]
    fn frames_without_a_generated_partner_are_shown_at_once_and_in_order() {
        let mut rig = rig();
        for k in 0..8u32 {
            arrive(&mut rig, us(u64::from(k) * 16_667), k, false);
        }
        rig.2.flush();
        let log = rig.3.lock().unwrap().clone();
        assert!(log.iter().all(|e| e.0 == Shown::Real));
        for e in &log {
            assert_eq!(e.1, us(u64::from(e.2) * 16_667));
        }
    }

    #[test]
    fn a_menu_between_two_game_stretches_restarts_the_rhythm() {
        let mut rig = rig();
        for k in 0..6u32 {
            arrive(&mut rig, us(u64::from(k) * 16_667), k, true);
        }
        // menu frames: shown alone, and the sequencer forgets the rhythm
        for k in 6..9u32 {
            rig.2.flush();
            rig.1.reset();
            let now = us(u64::from(k) * 16_667);
            rig.0.set(now);
            rig.2.schedule(rig.1.entries(now, None, k, None));
        }
        for k in 9..14u32 {
            arrive(&mut rig, us(u64::from(k) * 16_667), k, true);
        }
        rig.2.flush();
        let log = rig.3.lock().unwrap().clone();
        let generated: Vec<u32> = log
            .iter()
            .filter(|e| e.0 == Shown::Generated)
            .map(|e| e.2 - 1000)
            .collect();
        assert_eq!(generated, vec![1, 2, 3, 4, 5, 10, 11, 12, 13]);
    }

    #[test]
    fn a_long_hitch_shows_the_frame_alone_and_pacing_recovers() {
        let mut rig = rig();
        for k in 0..4u32 {
            arrive(&mut rig, us(u64::from(k) * 16_667), k, true);
        }
        arrive(&mut rig, us(500_000), 4, true);
        for k in 1..4u32 {
            arrive(&mut rig, us(500_000 + u64::from(k) * 16_667), 4 + k, true);
        }
        rig.2.flush();
        let log = rig.3.lock().unwrap().clone();
        let after: Vec<_> = log.iter().filter(|e| e.1 >= us(500_000)).collect();
        assert_eq!(after[0].0, Shown::Real, "the hitching frame is alone");
        assert_eq!(after[0].2, 4);
        assert_eq!(after.iter().filter(|e| e.0 == Shown::Generated).count(), 3);
    }
}
