//! Frame pacing: the frame rate policy (console cap, a chosen cap, uncapped, vsync) behind the console frame events.
//!
//! The title registers a frame interval (`SetFrameIntervalX`, device slot 78), arms the origin event
//! (`ScheduleFrameEventX`, slot 79) and blocks in `WaitFrameEventX` (slot 80) once per frame. The pacer decides
//! when that wait returns. The calls it needs from `present.rs`:
//!
//! - [`Pacer::set_frame_interval`], [`Pacer::schedule_frame_event`], [`Pacer::wait_frame_event`]: the three device calls;
//! - [`Pacer::frame_presented`]: from `PresentX`, with the token the plane carries;
//! - [`Pacer::suspend`] / [`Pacer::resume`]: from `SuspendX` / `ResumeX`;
//! - [`Pacer::sync_hint`]: which present mode fits the policy;
//! - [`Pacer::frame_record`] / [`Pacer::stats`] / [`Pacer::latest_token`]: for `GetFrameStatisticsX`, the overlay and
//!   the title's non-blocking poll of the origin event.
//!
//! Time comes from a [`Clock`], so the logic is tested with a fake one. [`SystemClock`] sleeps on a
//! high-resolution waitable timer and spins for the last stretch.

use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Frame records kept for `GetFrameStatisticsX`.
const RECORDS: usize = 64;
/// Frame times kept for the statistics.
const WINDOW: usize = 240;
/// A blocked wait looks at the suspended flag at least this often.
const SLICE: Duration = Duration::from_millis(50);

/// What the frame rate is held to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameLimit {
    /// What the title asked for in `SetFrameIntervalX` (30 or 60 Hz).
    Console,
    /// A fixed number of frames per second, whatever the title asked.
    Cap(f64),
    /// No wait at all and unsynchronised presents.
    Uncapped,
    /// No wait; the swapchain's own vertical sync holds the rate.
    VSync,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "frame limit must be console, uncapped, vsync or a number of frames per second above 0: {0:?}"
)]
pub struct ParseLimitError(String);

impl FromStr for FrameLimit {
    type Err = ParseLimitError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let trimmed = text.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "console" | "default" => Ok(Self::Console),
            "uncapped" | "unlimited" | "off" | "0" => Ok(Self::Uncapped),
            "vsync" => Ok(Self::VSync),
            other => match other.parse::<f64>() {
                Ok(fps) if fps.is_finite() && fps > 0.0 => Ok(Self::Cap(fps)),
                _ => Err(ParseLimitError(trimmed.to_owned())),
            },
        }
    }
}

/// What the presenter should ask of the swapchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncHint {
    /// Wait for the display's vertical blank (FIFO).
    Vsync,
    /// The pacer holds the rate itself: mailbox when available, FIFO otherwise.
    Paced,
    /// Present at once (immediate, or mailbox when tearing is unwanted).
    Immediate,
}

/// Why a wait gave no frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WaitError {
    #[error("the wait timed out")]
    TimedOut,
    #[error("no frame interval or origin event is registered")]
    NotScheduled,
    #[error("the queue is suspended")]
    Suspended,
}

/// Time source and precise sleeping.
pub trait Clock: Send + Sync {
    /// Time since the clock started.
    fn now(&self) -> Duration;
    /// Returns once `now()` has reached `deadline`.
    fn sleep_until(&self, deadline: Duration);
}

/// The real clock: a monotonic timer, slept on with a high-resolution waitable timer.
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep_until(&self, deadline: Duration) {
        if self.now() >= deadline {
            return;
        }
        let timer = sys::Timer::new();
        let spin = if timer.as_ref().is_some_and(sys::Timer::high_resolution) {
            Duration::from_micros(600)
        } else {
            Duration::from_millis(16)
        };
        loop {
            let now = self.now();
            if now >= deadline {
                return;
            }
            let remaining = deadline - now;
            if remaining > spin {
                match &timer {
                    Some(timer) => timer.sleep(remaining - spin),
                    None => std::thread::sleep(remaining - spin),
                }
            } else if remaining > Duration::from_micros(50) {
                std::thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
        TIMER_ALL_ACCESS, WaitForSingleObject,
    };

    pub struct Timer {
        handle: HANDLE,
        high_resolution: bool,
    }

    impl Timer {
        pub fn new() -> Option<Self> {
            // SAFETY: null attributes and name are allowed; the handle is closed in `drop`.
            let handle = unsafe {
                CreateWaitableTimerExW(
                    std::ptr::null(),
                    std::ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            };
            if !handle.is_null() {
                return Some(Self {
                    handle,
                    high_resolution: true,
                });
            }
            // SAFETY: as above, without the flag that older Windows versions refuse.
            let handle = unsafe {
                CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), 0, TIMER_ALL_ACCESS)
            };
            (!handle.is_null()).then_some(Self {
                handle,
                high_resolution: false,
            })
        }

        pub fn high_resolution(&self) -> bool {
            self.high_resolution
        }

        pub fn sleep(&self, duration: Duration) {
            let ticks = i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX);
            let due = -ticks;
            // SAFETY: `handle` is a live timer; `due` outlives the call; no completion routine is given.
            unsafe {
                if SetWaitableTimer(self.handle, &due, 0, None, std::ptr::null(), 0) != 0 {
                    WaitForSingleObject(self.handle, INFINITE);
                }
            }
        }
    }

    impl Drop for Timer {
        fn drop(&mut self) {
            // SAFETY: the handle came from `CreateWaitableTimerExW` and is closed once.
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use std::time::Duration;

    pub struct Timer;

    impl Timer {
        pub fn new() -> Option<Self> {
            None
        }

        pub fn high_resolution(&self) -> bool {
            false
        }

        pub fn sleep(&self, duration: Duration) {
            std::thread::sleep(duration);
        }
    }
}

/// When one frame came out of the wait and what became of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRecord {
    pub token: u64,
    /// Which origin of the interval grid the wait returned for (0 without a grid).
    pub origin: u64,
    /// When the wait returned.
    pub event: Duration,
    /// When `PresentX` was called for the token.
    pub present: Option<Duration>,
    /// When the frame was shown: the next origin after the present on a grid, the present time without one.
    pub display: Option<Duration>,
}

/// Frame-time statistics over the last [`WINDOW`] frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    /// Frames handed out since the pacer started.
    pub frames: u64,
    /// Frame times in the window.
    pub window: usize,
    pub last: Duration,
    pub mean: Duration,
    pub min: Duration,
    pub max: Duration,
    /// The frame time 99% of the frames were at or below.
    pub p99: Duration,
    /// Frames per second at the mean frame time.
    pub fps: f64,
    /// Origins the title missed (its frame ran past them).
    pub skipped: u64,
    /// Waits that found their origin already passed.
    pub late: u64,
}

struct State {
    limit: FrameLimit,
    title_interval: Option<Duration>,
    latency: u32,
    registered: bool,
    grid_base: Option<Duration>,
    scheduled: bool,
    offset: Duration,
    next_token: u64,
    last_origin: Option<u64>,
    last_return: Option<Duration>,
    pending: VecDeque<Duration>,
    records: VecDeque<FrameRecord>,
    frame_times: VecDeque<Duration>,
    frames: u64,
    skipped: u64,
    late: u64,
}

impl State {
    fn interval(&self) -> Option<Duration> {
        match self.limit {
            FrameLimit::Console => self.title_interval,
            FrameLimit::Cap(fps) => Some(Duration::from_nanos((1e9 / fps).round().max(1.0) as u64)),
            FrameLimit::Uncapped | FrameLimit::VSync => None,
        }
    }

    fn rebase(&mut self) {
        self.grid_base = None;
        self.last_origin = None;
        self.pending.clear();
    }

    /// The time of the first origin strictly after `time`.
    fn next_origin_after(&self, time: Duration, interval: Duration) -> Duration {
        let base = self.grid_base.unwrap_or(time);
        if time < base {
            return base;
        }
        let k = (time - base).as_nanos() / interval.as_nanos().max(1);
        base + Self::steps(interval, k as u64 + 1)
    }

    fn steps(interval: Duration, count: u64) -> Duration {
        Duration::from_nanos(
            (interval.as_nanos() * u128::from(count)).min(u128::from(u64::MAX)) as u64,
        )
    }

    fn origin_time(&self, index: u64, interval: Duration) -> Duration {
        self.grid_base.unwrap_or_default() + Self::steps(interval, index) + self.offset
    }
}

enum Plan {
    Return { origin: u64 },
    Sleep { until: Duration },
}

/// Holds the frame rate policy and hands out frame tokens.
pub struct Pacer<C: Clock = SystemClock> {
    clock: C,
    state: Mutex<State>,
    suspended: AtomicBool,
}

impl Pacer<SystemClock> {
    pub fn new(limit: FrameLimit) -> Self {
        Self::with_clock(SystemClock::new(), limit)
    }
}

impl<C: Clock> Pacer<C> {
    pub fn with_clock(clock: C, limit: FrameLimit) -> Self {
        Self {
            clock,
            state: Mutex::new(State {
                limit,
                title_interval: None,
                latency: 0,
                registered: false,
                grid_base: None,
                scheduled: false,
                offset: Duration::ZERO,
                next_token: 1,
                last_origin: None,
                last_return: None,
                pending: VecDeque::new(),
                records: VecDeque::new(),
                frame_times: VecDeque::new(),
                frames: 0,
                skipped: 0,
                late: 0,
            }),
            suspended: AtomicBool::new(false),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn limit(&self) -> FrameLimit {
        self.state().limit
    }

    /// Changes the policy at once; the interval grid starts again from the next wait.
    pub fn set_limit(&self, limit: FrameLimit) {
        let mut state = self.state();
        if state.limit != limit {
            state.limit = limit;
            state.rebase();
        }
    }

    pub fn sync_hint(&self) -> SyncHint {
        match self.state().limit {
            FrameLimit::VSync => SyncHint::Vsync,
            FrameLimit::Console | FrameLimit::Cap(_) => SyncHint::Paced,
            FrameLimit::Uncapped => SyncHint::Immediate,
        }
    }

    /// `SetFrameIntervalX`: the interval in microseconds (0 means as fast as possible) and the number of
    /// presented frames allowed to wait for their display. Starts the interval grid at this moment.
    pub fn set_frame_interval(&self, interval_us: u32, latency: u32) {
        let now = self.clock.now();
        let mut state = self.state();
        state.title_interval =
            (interval_us != 0).then(|| Duration::from_micros(u64::from(interval_us)));
        state.latency = latency;
        state.registered = true;
        state.rebase();
        state.grid_base = Some(now);
    }

    /// `ScheduleFrameEventX`: arms the origin event, delayed by `offset_us` inside the frame.
    pub fn schedule_frame_event(&self, offset_us: u32) {
        let mut state = self.state();
        state.scheduled = true;
        state.offset = Duration::from_micros(u64::from(offset_us));
    }

    /// `SuspendX`: drops the interval, the origin event and the frames in flight, and wakes blocked waits.
    pub fn suspend(&self) {
        self.suspended.store(true, Ordering::SeqCst);
        let mut state = self.state();
        state.registered = false;
        state.scheduled = false;
        state.rebase();
    }

    /// `ResumeX`: waits fail until the interval and the origin event are registered again. Tokens keep counting.
    pub fn resume(&self) {
        self.suspended.store(false, Ordering::SeqCst);
    }

    /// `WaitFrameEventX`: blocks until the next frame origin and returns its token. `None` waits for ever.
    pub fn wait_frame_event(&self, timeout: Option<Duration>) -> Result<u64, WaitError> {
        let started = self.clock.now();
        let deadline = timeout.map(|t| started + t);
        loop {
            if self.suspended.load(Ordering::SeqCst) {
                return Err(WaitError::Suspended);
            }
            let plan = {
                let mut state = self.state();
                if !state.registered || !state.scheduled {
                    return Err(WaitError::NotScheduled);
                }
                self.plan(&mut state)
            };
            match plan {
                Plan::Return { origin } => return Ok(self.finish(origin)),
                Plan::Sleep { until } => {
                    let now = self.clock.now();
                    if deadline.is_some_and(|d| now >= d) {
                        return Err(WaitError::TimedOut);
                    }
                    let mut stop = until.min(now + SLICE);
                    if let Some(d) = deadline {
                        stop = stop.min(d);
                    }
                    self.clock.sleep_until(stop);
                }
            }
        }
    }

    fn plan(&self, state: &mut State) -> Plan {
        let now = self.clock.now();
        let Some(interval) = state.interval() else {
            return Plan::Return { origin: 0 };
        };
        let base = *state.grid_base.get_or_insert(now);
        let first = base + state.offset;
        let newest =
            (now >= first).then(|| ((now - first).as_nanos() / interval.as_nanos().max(1)) as u64);
        let wanted = state.last_origin.map_or(0, |last| last + 1);
        let mut origin = match newest {
            Some(newest) if newest >= wanted => newest,
            _ => wanted,
        };
        let mut at = state.origin_time(origin, interval);
        while let Some(due) = Self::throttle(state, at) {
            at = due;
        }
        if at >= first {
            origin = origin.max(((at - first).as_nanos() / interval.as_nanos().max(1)) as u64);
        }
        if at > now {
            Plan::Sleep { until: at }
        } else {
            Plan::Return { origin }
        }
    }

    /// The display time of a frame that must be shown before a wait at `at` may return: more frames than the
    /// latency allows are still waiting for their vertical blank.
    fn throttle(state: &State, at: Duration) -> Option<Duration> {
        let waiting: Vec<Duration> = state.pending.iter().copied().filter(|d| *d > at).collect();
        (waiting.len() > state.latency as usize)
            .then(|| waiting[waiting.len() - 1 - state.latency as usize])
    }

    fn finish(&self, origin: u64) -> u64 {
        let now = self.clock.now();
        let mut state = self.state();
        let token = state.next_token;
        state.next_token = token.checked_add(1).unwrap_or(1);
        if let Some(interval) = state.interval() {
            let wanted = state.last_origin.map_or(0, |last| last + 1);
            if origin > wanted {
                state.skipped += origin - wanted;
                state.late += 1;
            } else if now > state.origin_time(origin, interval) + interval / 4 {
                state.late += 1;
            }
            state.last_origin = Some(origin);
        }
        if let Some(previous) = state.last_return {
            if state.frame_times.len() == WINDOW {
                state.frame_times.pop_front();
            }
            state.frame_times.push_back(now - previous);
        }
        state.last_return = Some(now);
        state.frames += 1;
        if state.records.len() == RECORDS {
            state.records.pop_front();
        }
        state.records.push_back(FrameRecord {
            token,
            origin,
            event: now,
            present: None,
            display: None,
        });
        token
    }

    /// The token the last successful wait returned, 0 before the first. For the title's polling wait, which
    /// reads the current token without blocking.
    pub fn latest_token(&self) -> u64 {
        self.state().records.back().map_or(0, |r| r.token)
    }

    /// `PresentX` was called for the frame with this token.
    pub fn frame_presented(&self, token: u64) {
        let now = self.clock.now();
        let mut state = self.state();
        let display = match state.interval() {
            Some(interval) => state.next_origin_after(now, interval),
            None => now,
        };
        if let Some(record) = state.records.iter_mut().find(|r| r.token == token) {
            record.present = Some(now);
            record.display = Some(display);
        }
        if state.interval().is_some() {
            state.pending.push_back(display);
        }
        while state.pending.front().is_some_and(|d| *d <= now) {
            state.pending.pop_front();
        }
    }

    pub fn frame_record(&self, token: u64) -> Option<FrameRecord> {
        self.state()
            .records
            .iter()
            .find(|r| r.token == token)
            .copied()
    }

    pub fn stats(&self) -> FrameStats {
        let state = self.state();
        let mut times: Vec<Duration> = state.frame_times.iter().copied().collect();
        let mut stats = FrameStats {
            frames: state.frames,
            window: times.len(),
            skipped: state.skipped,
            late: state.late,
            ..FrameStats::default()
        };
        let Some(last) = times.last().copied() else {
            return stats;
        };
        stats.last = last;
        let total: Duration = times.iter().sum();
        stats.mean = total / times.len() as u32;
        times.sort_unstable();
        stats.min = times[0];
        stats.max = times[times.len() - 1];
        stats.p99 = times[((times.len() * 99).div_ceil(100))
            .saturating_sub(1)
            .min(times.len() - 1)];
        stats.fps = if stats.mean.is_zero() {
            0.0
        } else {
            1.0 / stats.mean.as_secs_f64()
        };
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct FakeClock(Arc<Mutex<Duration>>);

    impl FakeClock {
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            *self.0.lock().unwrap()
        }

        fn sleep_until(&self, deadline: Duration) {
            let mut now = self.0.lock().unwrap();
            if *now < deadline {
                *now = deadline;
            }
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn pacer(limit: FrameLimit) -> (Pacer<FakeClock>, FakeClock) {
        let clock = FakeClock::default();
        (Pacer::with_clock(clock.clone(), limit), clock)
    }

    fn registered(
        limit: FrameLimit,
        interval_us: u32,
        latency: u32,
    ) -> (Pacer<FakeClock>, FakeClock) {
        let (pacer, clock) = pacer(limit);
        pacer.set_frame_interval(interval_us, latency);
        pacer.schedule_frame_event(0);
        (pacer, clock)
    }

    #[test]
    fn limits_parse() {
        assert_eq!("console".parse(), Ok(FrameLimit::Console));
        assert_eq!("Uncapped".parse(), Ok(FrameLimit::Uncapped));
        assert_eq!("0".parse(), Ok(FrameLimit::Uncapped));
        assert_eq!(" vsync ".parse(), Ok(FrameLimit::VSync));
        assert_eq!("144".parse(), Ok(FrameLimit::Cap(144.0)));
        assert_eq!("97.5".parse(), Ok(FrameLimit::Cap(97.5)));
        assert!("fast".parse::<FrameLimit>().is_err());
        assert!("-5".parse::<FrameLimit>().is_err());
        assert!("NaN".parse::<FrameLimit>().is_err());
    }

    #[test]
    fn console_limit_follows_the_title_interval() {
        let (pacer, clock) = registered(FrameLimit::Console, 33_333, 1);
        let first = pacer.wait_frame_event(None).unwrap();
        let t0 = clock.now();
        let second = pacer.wait_frame_event(None).unwrap();
        assert_eq!((first, second), (1, 2));
        assert_eq!(clock.now() - t0, Duration::from_micros(33_333));
        pacer.set_frame_interval(16_667, 1);
        pacer.schedule_frame_event(0);
        pacer.wait_frame_event(None).unwrap();
        let t1 = clock.now();
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now() - t1, Duration::from_micros(16_667));
    }

    #[test]
    fn a_chosen_cap_ignores_the_title_interval() {
        let (pacer, clock) = registered(FrameLimit::Cap(100.0), 33_333, 1);
        pacer.wait_frame_event(None).unwrap();
        let t0 = clock.now();
        for _ in 0..10 {
            pacer.wait_frame_event(None).unwrap();
        }
        assert_eq!(clock.now() - t0, ms(100));
    }

    #[test]
    fn uncapped_and_vsync_never_sleep() {
        for limit in [FrameLimit::Uncapped, FrameLimit::VSync] {
            let (pacer, clock) = registered(limit, 33_333, 1);
            for expected in 1..=50 {
                assert_eq!(pacer.wait_frame_event(Some(Duration::ZERO)), Ok(expected));
            }
            assert_eq!(clock.now(), Duration::ZERO);
        }
    }

    #[test]
    fn a_zero_interval_from_the_title_does_not_cap_the_console_policy() {
        let (pacer, clock) = registered(FrameLimit::Console, 0, 1);
        pacer.wait_frame_event(None).unwrap();
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now(), Duration::ZERO);
    }

    #[test]
    fn the_origin_offset_delays_each_frame() {
        let (pacer, clock) = pacer(FrameLimit::Console);
        pacer.set_frame_interval(16_000, 1);
        pacer.schedule_frame_event(4_000);
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now(), ms(4));
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now(), ms(20));
    }

    #[test]
    fn a_late_wait_returns_at_once_and_skips_the_missed_origins() {
        let (pacer, clock) = registered(FrameLimit::Cap(100.0), 16_667, 1);
        pacer.wait_frame_event(None).unwrap();
        clock.advance(ms(35));
        let before = clock.now();
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now(), before);
        let stats = pacer.stats();
        assert_eq!(stats.skipped, 2);
        assert_eq!(stats.late, 1);
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now(), ms(40));
    }

    #[test]
    fn timeouts_give_no_token_and_leave_the_grid_alone() {
        let (pacer, clock) = registered(FrameLimit::Cap(10.0), 16_667, 1);
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(
            pacer.wait_frame_event(Some(ms(30))),
            Err(WaitError::TimedOut)
        );
        assert_eq!(clock.now(), ms(30));
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now(), ms(100));
        assert_eq!(pacer.stats().frames, 2);
    }

    #[test]
    fn waits_fail_until_registered_and_after_suspend_until_registered_again() {
        let (pacer, _clock) = pacer(FrameLimit::Console);
        assert_eq!(pacer.wait_frame_event(None), Err(WaitError::NotScheduled));
        pacer.set_frame_interval(16_667, 1);
        assert_eq!(pacer.wait_frame_event(None), Err(WaitError::NotScheduled));
        pacer.schedule_frame_event(0);
        assert_eq!(pacer.wait_frame_event(None), Ok(1));
        pacer.suspend();
        assert_eq!(pacer.wait_frame_event(None), Err(WaitError::Suspended));
        pacer.resume();
        assert_eq!(pacer.wait_frame_event(None), Err(WaitError::NotScheduled));
        pacer.set_frame_interval(16_667, 1);
        pacer.schedule_frame_event(0);
        assert_eq!(pacer.wait_frame_event(None), Ok(2));
    }

    #[test]
    fn latency_holds_a_late_wait_until_the_presented_frame_is_displayed() {
        for (latency, returns_at) in [(0, ms(20)), (1, ms(12))] {
            let (pacer, clock) = registered(FrameLimit::Cap(100.0), 16_667, latency);
            let token = pacer.wait_frame_event(None).unwrap();
            clock.advance(ms(12));
            pacer.frame_presented(token);
            assert_eq!(pacer.frame_record(token).unwrap().display, Some(ms(20)));
            pacer.wait_frame_event(None).unwrap();
            assert_eq!(clock.now(), returns_at, "latency {latency}");
        }
    }

    #[test]
    fn records_carry_the_present_and_display_times() {
        let (pacer, clock) = registered(FrameLimit::Cap(50.0), 16_667, 2);
        let token = pacer.wait_frame_event(None).unwrap();
        clock.advance(ms(5));
        pacer.frame_presented(token);
        let record = pacer.frame_record(token).unwrap();
        assert_eq!(record.event, Duration::ZERO);
        assert_eq!(record.present, Some(ms(5)));
        assert_eq!(record.display, Some(ms(20)));
        assert_eq!(pacer.frame_record(99), None);
    }

    #[test]
    fn frame_time_statistics() {
        let (pacer, clock) = registered(FrameLimit::Uncapped, 0, 1);
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(pacer.stats().window, 0);
        for step in [10u64, 10, 10, 10, 10, 10, 10, 10, 10, 30] {
            clock.advance(ms(step));
            pacer.wait_frame_event(None).unwrap();
        }
        let stats = pacer.stats();
        assert_eq!(stats.frames, 11);
        assert_eq!(stats.window, 10);
        assert_eq!(stats.last, ms(30));
        assert_eq!(stats.min, ms(10));
        assert_eq!(stats.max, ms(30));
        assert_eq!(stats.mean, ms(12));
        assert_eq!(stats.p99, ms(30));
        assert!((stats.fps - 1000.0 / 12.0).abs() < 1e-6);
    }

    #[test]
    fn changing_the_limit_starts_the_grid_again() {
        let (pacer, clock) = registered(FrameLimit::Console, 16_667, 1);
        pacer.wait_frame_event(None).unwrap();
        pacer.set_limit(FrameLimit::Cap(20.0));
        assert_eq!(pacer.sync_hint(), SyncHint::Paced);
        clock.advance(ms(1));
        pacer.wait_frame_event(None).unwrap();
        let t0 = clock.now();
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(clock.now() - t0, ms(50));
        pacer.set_limit(FrameLimit::Uncapped);
        assert_eq!(pacer.sync_hint(), SyncHint::Immediate);
        pacer.set_limit(FrameLimit::VSync);
        assert_eq!(pacer.sync_hint(), SyncHint::Vsync);
    }

    #[test]
    fn the_latest_token_follows_the_waits() {
        let (pacer, _clock) = registered(FrameLimit::Uncapped, 0, 1);
        assert_eq!(pacer.latest_token(), 0);
        pacer.wait_frame_event(None).unwrap();
        pacer.wait_frame_event(None).unwrap();
        assert_eq!(pacer.latest_token(), 2);
    }

    #[test]
    fn tokens_are_never_zero() {
        let (pacer, _clock) = registered(FrameLimit::Uncapped, 0, 1);
        pacer.state().next_token = u64::MAX;
        assert_eq!(pacer.wait_frame_event(None), Ok(u64::MAX));
        assert_eq!(pacer.wait_frame_event(None), Ok(1));
    }

    #[test]
    fn the_system_clock_sleeps_to_within_a_millisecond() {
        let clock = SystemClock::new();
        let target = clock.now() + ms(8);
        clock.sleep_until(target);
        let over = clock.now() - target;
        assert!(over < ms(2), "overslept by {over:?}");
    }

    #[test]
    fn a_real_cap_holds_its_rate() {
        let pacer = Pacer::new(FrameLimit::Cap(200.0));
        pacer.set_frame_interval(16_667, 1);
        pacer.schedule_frame_event(0);
        let start = Instant::now();
        for _ in 0..21 {
            pacer.wait_frame_event(None).unwrap();
        }
        let took = start.elapsed();
        assert!(
            took >= ms(98) && took < ms(400),
            "20 frames at 200 fps took {took:?}"
        );
    }
}
