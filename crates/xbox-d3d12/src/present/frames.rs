//! Frame events: the vblank grid, origin events, tokens, frames in flight and statistics.
//!
//! The state machine takes the time as an argument so it runs without a clock or a window; `FrameEvents` wraps it with a
//! real clock, a mutex and a condition variable for the blocking `WaitFrameEventX`.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
pub const E_FAIL: i32 = 0x8000_4005_u32 as i32;
pub const DXGI_ERROR_UNSUPPORTED: i32 = 0x887A_0004_u32 as i32;
/// `HRESULT_FROM_WIN32(ERROR_INVALID_STATE)`.
pub const E_INVALID_STATE: i32 = 0x8007_139F_u32 as i32;

pub const FRAME_INTERVAL_60_HZ: u32 = 16_667;
pub const FRAME_INTERVAL_30_HZ: u32 = 33_333;
pub const FRAME_INTERVAL_120_HZ: u32 = 8_333;

/// The origin event, the only one the public samples use.
pub const FRAME_EVENT_ORIGIN: u32 = 0;
/// `WaitFrameEventX` writes the token on a timeout only with this flag (measured on another runtime).
pub const WAIT_FLAG_TOKEN_ON_TIMEOUT: u32 = 2;

const TOKEN_MASK: u64 = 0xFFFF_FFFF_FFFF;
const KEEP: usize = 64;

/// What the pacing policy decides about a frame rate the title asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    /// The period of the vblank grid in microseconds; 0 takes the cap off (no grid, no throttle).
    pub interval_us: u32,
    pub sync: SyncMode,
}

/// How a frame reaches the display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    #[default]
    Vsync,
    /// The newest frame replaces a waiting one; no tearing.
    Mailbox,
    Immediate,
}

/// The seam to the pacing module: `SetFrameIntervalX` asks it what rate to run at.
pub trait FramePolicy: Send + Sync {
    fn pacing(&self, requested_interval_us: u32) -> Pacing;
}

/// Runs at the rate the title asked for, synchronised to the display.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsolePolicy;

impl FramePolicy for ConsolePolicy {
    fn pacing(&self, requested_interval_us: u32) -> Pacing {
        Pacing {
            interval_us: requested_interval_us,
            sync: SyncMode::Vsync,
        }
    }
}

/// How a late frame is put on the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlipKind {
    Vblank,
    /// Flipped at once, mid-scan; carries the scan position as a percentage.
    Immediate(f32),
    /// Not paced: shown as soon as the presenter gets to it.
    Unpaced,
}

/// Whether a frame that is `late_us` past its vblank is flipped at once.
///
/// `threshold` is `ImmediateThresholdPercent`: a late frame is shown immediately only when the scan has not passed that
/// percentage of the screen. 0, negative and NaN never tear, 100 or more tears wherever the scan is.
pub fn decide_present_sync(late_us: u64, interval_us: u64, threshold: f32) -> FlipKind {
    if late_us == 0 || interval_us == 0 || threshold.is_nan() || threshold <= 0.0 {
        return FlipKind::Vblank;
    }
    let scanned = (late_us % interval_us) as f32 / interval_us as f32 * 100.0;
    if threshold >= 100.0 || scanned <= threshold {
        FlipKind::Immediate(scanned)
    } else {
        FlipKind::Vblank
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRecord {
    pub token: u64,
    pub origin_us: u64,
    pub issued_us: u64,
    pub present_us: Option<u64>,
    pub display_us: Option<u64>,
    pub percent_scanned: f32,
}

/// The kinds of statistic `GetFrameStatisticsX` can be asked for. The mask bits are ours: the console's are unmeasured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatKind {
    Interval,
    Event,
    Render,
    Present,
    Display,
}

impl StatKind {
    pub fn bit(self) -> u32 {
        1 << self as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatEntry {
    pub kind: StatKind,
    pub token: u64,
    pub time_us: u64,
    pub percent_scanned: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Poll {
    Ready(u64),
    /// Nothing before this time (microseconds on the state's clock).
    Pending(u64),
    Invalid,
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub flip: FlipKind,
    /// When the frame is on the screen.
    pub display_us: u64,
    pub late_us: u64,
}

pub struct FrameState {
    interval_us: u64,
    latency: u32,
    base_us: u64,
    offset_us: Option<u64>,
    uncapped: bool,
    suspended: bool,
    last_origin: Option<i64>,
    next_token: u64,
    in_flight: VecDeque<u64>,
    last_display: Option<i64>,
    records: VecDeque<FrameRecord>,
    presented: u64,
    immediate: u64,
}

impl Default for FrameState {
    fn default() -> Self {
        FrameState {
            interval_us: 0,
            latency: 1,
            base_us: 0,
            offset_us: None,
            uncapped: false,
            suspended: false,
            last_origin: None,
            next_token: 1,
            in_flight: VecDeque::new(),
            last_display: None,
            records: VecDeque::new(),
            presented: 0,
            immediate: 0,
        }
    }
}

impl FrameState {
    pub fn registered(&self) -> bool {
        self.interval_us != 0 || self.uncapped
    }

    pub fn armed(&self) -> bool {
        self.registered() && self.offset_us.is_some() && !self.suspended
    }

    pub fn interval_us(&self) -> u64 {
        self.interval_us
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    pub fn presented_frames(&self) -> u64 {
        self.presented
    }

    pub fn immediate_flips(&self) -> u64 {
        self.immediate
    }

    /// `SetFrameIntervalX`. Calling it again keeps an armed origin event and re-bases the grid on `now`.
    pub fn set_interval(&mut self, now: u64, pacing: Pacing, latency: u32) {
        self.uncapped = pacing.interval_us == 0;
        self.interval_us = u64::from(pacing.interval_us);
        self.latency = latency;
        self.base_us = now;
        self.last_origin = None;
    }

    /// `ScheduleFrameEventX` for the origin event.
    pub fn schedule_origin(&mut self, offset_us: u64) -> bool {
        if !self.registered() || self.suspended {
            return false;
        }
        self.offset_us = Some(offset_us);
        true
    }

    pub fn suspend(&mut self) {
        self.suspended = true;
        self.offset_us = None;
        self.interval_us = 0;
        self.uncapped = false;
        self.in_flight.clear();
        self.last_origin = None;
        self.last_display = None;
    }

    /// Presents are allowed again; waits fail until the interval and the event are registered again.
    pub fn resume(&mut self) {
        self.suspended = false;
    }

    fn origin_us(&self, index: i64) -> u64 {
        let offset = self.offset_us.unwrap_or(0) as i64;
        (self.base_us as i64 + index * self.interval_us as i64 + offset).max(0) as u64
    }

    fn purge(&mut self, now: u64) {
        while self.in_flight.front().is_some_and(|d| *d <= now) {
            self.in_flight.pop_front();
        }
    }

    /// One look at `WaitFrameEventX`: a token now, or the time to look again.
    pub fn poll_wait(&mut self, now: u64) -> Poll {
        if self.suspended {
            return Poll::Suspended;
        }
        if !self.armed() {
            return Poll::Invalid;
        }
        let origin;
        let index;
        if self.uncapped {
            index = self.last_origin.map_or(0, |i| i + 1);
            origin = now;
        } else {
            self.purge(now);
            if self.latency > 0 && self.in_flight.len() >= self.latency as usize {
                return Poll::Pending(self.in_flight[self.in_flight.len() - self.latency as usize]);
            }
            let interval = self.interval_us as i64;
            let offset = self.offset_us.unwrap_or(0) as i64;
            let first = match self.last_origin {
                Some(last) => last + 1,
                None => {
                    let since = now as i64 - self.base_us as i64 - offset;
                    since.div_euclid(interval) + i64::from(since.rem_euclid(interval) != 0)
                }
            };
            let first = first.max(0);
            if self.origin_us(first) > now {
                if self.last_origin.is_none() {
                    self.last_origin = Some(first - 1);
                }
                return Poll::Pending(self.origin_us(first));
            }
            let newest =
                ((now as i64 - self.base_us as i64 - offset).div_euclid(interval)).max(first);
            index = newest;
            origin = self.origin_us(newest);
        }
        let token = self.next_token;
        self.next_token = if self.next_token >= TOKEN_MASK {
            1
        } else {
            self.next_token + 1
        };
        self.last_origin = Some(index);
        self.records.push_back(FrameRecord {
            token,
            origin_us: origin,
            issued_us: now,
            present_us: None,
            display_us: None,
            percent_scanned: 0.0,
        });
        while self.records.len() > KEEP {
            self.records.pop_front();
        }
        Poll::Ready(token)
    }

    /// Whether a present may carry this token: the null token, or one that a wait handed out.
    pub fn token_valid(&self, token: u64) -> bool {
        token == 0 || token < self.next_token
    }

    /// Records a present and decides when the frame is on the screen.
    pub fn present(&mut self, now: u64, token: u64, immediate_threshold: f32) -> Timing {
        self.presented += 1;
        let paced = !self.uncapped && self.interval_us != 0 && self.offset_us.is_some();
        let record = self.records.iter().position(|r| r.token == token);
        let (Some(record), true, true) = (record, paced, token != 0) else {
            if let Some(i) = record {
                self.records[i].present_us = Some(now);
                self.records[i].display_us = Some(now);
            }
            return Timing {
                flip: FlipKind::Unpaced,
                display_us: now,
                late_us: 0,
            };
        };
        self.purge(now);
        let interval = self.interval_us as i64;
        let origin = self.records[record].origin_us as i64;
        let origin_index = (origin - self.base_us as i64 - self.offset_us.unwrap_or(0) as i64)
            .div_euclid(interval);
        let due = self.base_us as i64 + (origin_index + 1) * interval;
        let late = (now as i64 - due).max(0) as u64;
        let queued = !self.in_flight.is_empty();
        let mut flip = FlipKind::Vblank;
        if !queued {
            flip = decide_present_sync(late, self.interval_us, immediate_threshold);
        }
        let timing = match flip {
            FlipKind::Immediate(_) => {
                self.immediate += 1;
                Timing {
                    flip,
                    display_us: now,
                    late_us: late,
                }
            }
            _ => {
                let since = now as i64 - self.base_us as i64;
                let mut j = since.div_euclid(interval) + i64::from(since.rem_euclid(interval) != 0);
                if queued && let Some(last) = self.last_display {
                    j = j.max(last + 1);
                }
                self.last_display = Some(j);
                let at = (self.base_us as i64 + j * interval) as u64;
                self.in_flight.push_back(at);
                Timing {
                    flip: FlipKind::Vblank,
                    display_us: at,
                    late_us: late,
                }
            }
        };
        let record = &mut self.records[record];
        record.present_us = Some(now);
        record.display_us = Some(timing.display_us);
        if let FlipKind::Immediate(scanned) = timing.flip {
            record.percent_scanned = scanned;
        }
        timing
    }

    /// `GetFrameStatisticsX`: at most `max` entries for `token` (0 for every kept frame) of the kinds in `mask`, with
    /// only what has happened by `now`.
    pub fn statistics(&self, now: u64, token: u64, mask: u32, max: usize) -> Vec<StatEntry> {
        let mut out = Vec::new();
        if mask & StatKind::Interval.bit() != 0 && self.interval_us != 0 && max > 0 {
            out.push(StatEntry {
                kind: StatKind::Interval,
                token: 0,
                time_us: self.interval_us,
                percent_scanned: 0.0,
            });
        }
        for r in self
            .records
            .iter()
            .filter(|r| token == 0 || r.token == token)
        {
            let mut push = |kind: StatKind, t: u64, pct: f32| {
                if mask & kind.bit() != 0 && out.len() < max {
                    out.push(StatEntry {
                        kind,
                        token: r.token,
                        time_us: t,
                        percent_scanned: pct,
                    });
                }
            };
            push(StatKind::Event, r.origin_us, 0.0);
            if let Some(p) = r.present_us {
                push(StatKind::Present, p, 0.0);
            }
            if let Some(d) = r.display_us.filter(|d| *d <= now) {
                push(StatKind::Display, d, r.percent_scanned);
            }
        }
        out
    }
}

/// A monotonic microsecond clock.
pub trait Clock: Send + Sync {
    fn now_us(&self) -> u64;
}

pub struct SystemClock {
    start: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock {
            start: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

/// `FrameState` behind a lock, with blocking waits.
pub struct FrameEvents {
    state: Mutex<FrameState>,
    changed: Condvar,
    clock: Box<dyn Clock>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaitResult {
    pub hresult: i32,
    pub token: u64,
}

impl FrameEvents {
    pub fn new(clock: Box<dyn Clock>) -> FrameEvents {
        FrameEvents {
            state: Mutex::new(FrameState::default()),
            changed: Condvar::new(),
            clock,
        }
    }

    pub fn now_us(&self) -> u64 {
        self.clock.now_us()
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut FrameState, u64) -> R) -> R {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.clock.now_us();
        let r = f(&mut state, now);
        self.changed.notify_all();
        r
    }

    /// `WaitFrameEventX`: `timeout_ms` of `u32::MAX` waits for ever. A timeout gives `S_FALSE`; an event that was never
    /// scheduled gives `E_INVALIDARG`; a suspended queue gives `ERROR_INVALID_STATE`, also to a wait that was blocked.
    pub fn wait(&self, timeout_ms: u32, flags: u32) -> WaitResult {
        let deadline =
            (timeout_ms != u32::MAX).then(|| self.clock.now_us() + u64::from(timeout_ms) * 1000);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let now = self.clock.now_us();
            let wake = match state.poll_wait(now) {
                Poll::Ready(token) => {
                    return WaitResult {
                        hresult: S_OK,
                        token,
                    };
                }
                Poll::Invalid => {
                    return WaitResult {
                        hresult: E_INVALIDARG,
                        token: 0,
                    };
                }
                Poll::Suspended => {
                    return WaitResult {
                        hresult: E_INVALID_STATE,
                        token: 0,
                    };
                }
                Poll::Pending(at) => at,
            };
            if deadline.is_some_and(|d| now >= d) {
                let token = if flags & WAIT_FLAG_TOKEN_ON_TIMEOUT != 0 {
                    state.records.back().map_or(0, |r| r.token)
                } else {
                    0
                };
                return WaitResult {
                    hresult: S_FALSE,
                    token,
                };
            }
            let until = deadline.map_or(wake, |d| d.min(wake));
            let sleep = Duration::from_micros(until.saturating_sub(now).max(1));
            state = self
                .changed
                .wait_timeout(state, sleep)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    const I: u32 = 16_000;

    fn capped(latency: u32) -> FrameState {
        let mut s = FrameState::default();
        s.set_interval(
            1_000,
            Pacing {
                interval_us: I,
                sync: SyncMode::Vsync,
            },
            latency,
        );
        s.schedule_origin(0);
        s
    }

    #[test]
    fn first_wait_returns_at_the_next_origin_and_tokens_count_up() {
        let mut s = capped(0);
        assert_eq!(s.poll_wait(1_000), Poll::Ready(1));
        assert_eq!(s.poll_wait(1_001), Poll::Pending(1_000 + u64::from(I)));
        assert_eq!(s.poll_wait(1_000 + u64::from(I)), Poll::Ready(2));
    }

    #[test]
    fn origin_offset_delays_the_event_inside_the_frame() {
        let mut s = FrameState::default();
        s.set_interval(
            0,
            Pacing {
                interval_us: I,
                sync: SyncMode::Vsync,
            },
            0,
        );
        s.schedule_origin(4_000);
        assert_eq!(s.poll_wait(0), Poll::Pending(4_000));
        assert_eq!(s.poll_wait(4_000), Poll::Ready(1));
        assert_eq!(s.poll_wait(4_001), Poll::Pending(20_000));
    }

    #[test]
    fn a_late_wait_takes_the_newest_origin_and_skips_the_missed_ones() {
        let mut s = capped(0);
        assert_eq!(s.poll_wait(1_000), Poll::Ready(1));
        let late = 1_000 + 5 * u64::from(I) + 10;
        assert_eq!(s.poll_wait(late), Poll::Ready(2));
        assert_eq!(
            s.poll_wait(late + 1),
            Poll::Pending(1_000 + 6 * u64::from(I))
        );
    }

    #[test]
    fn unregistered_or_unscheduled_waits_are_invalid() {
        let mut s = FrameState::default();
        assert_eq!(s.poll_wait(0), Poll::Invalid);
        assert!(!s.schedule_origin(0));
        s.set_interval(
            0,
            Pacing {
                interval_us: I,
                sync: SyncMode::Vsync,
            },
            1,
        );
        assert_eq!(s.poll_wait(0), Poll::Invalid);
        assert!(s.schedule_origin(0));
        assert_eq!(s.poll_wait(0), Poll::Ready(1));
    }

    #[test]
    fn latency_blocks_the_wait_while_frames_wait_for_their_vblank() {
        let mut s = capped(1);
        let Poll::Ready(t1) = s.poll_wait(1_000) else {
            panic!()
        };
        let timing = s.present(2_000, t1, 0.0);
        assert_eq!(timing.flip, FlipKind::Vblank);
        assert_eq!(timing.display_us, 1_000 + u64::from(I));
        assert_eq!(s.poll_wait(2_001), Poll::Pending(1_000 + u64::from(I)));
        assert_eq!(s.poll_wait(1_000 + u64::from(I)), Poll::Ready(2));
    }

    #[test]
    fn latency_two_lets_a_second_frame_queue() {
        let mut s = capped(2);
        let Poll::Ready(t1) = s.poll_wait(1_000) else {
            panic!()
        };
        s.present(1_100, t1, 0.0);
        assert_eq!(s.poll_wait(1_200), Poll::Pending(1_000 + u64::from(I)));
        let mut s = capped(2);
        s.poll_wait(1_000);
        let t = s.present(1_100, 1, 0.0);
        let t2 = s.present(1_200, 1, 0.0);
        assert_eq!(t2.display_us, t.display_us + u64::from(I));
        let Poll::Pending(at) = s.poll_wait(1_300) else {
            panic!()
        };
        assert_eq!(at, t.display_us);
    }

    #[test]
    fn presents_before_the_origin_is_armed_are_not_paced() {
        let mut s = FrameState::default();
        s.set_interval(
            0,
            Pacing {
                interval_us: I,
                sync: SyncMode::Vsync,
            },
            1,
        );
        let t = s.present(500, 0, 0.0);
        assert_eq!(t.flip, FlipKind::Unpaced);
        assert_eq!(t.display_us, 500);
        s.schedule_origin(0);
        assert_eq!(s.poll_wait(500), Poll::Pending(u64::from(I)));
    }

    #[test]
    fn tokens_that_were_never_handed_out_are_invalid() {
        let mut s = capped(0);
        assert!(s.token_valid(0));
        assert!(!s.token_valid(1));
        s.poll_wait(1_000);
        assert!(s.token_valid(1));
        assert!(s.token_valid(0));
        assert!(!s.token_valid(2));
    }

    #[test]
    fn decide_present_sync_follows_the_threshold_rules() {
        assert_eq!(decide_present_sync(0, 16_000, 100.0), FlipKind::Vblank);
        assert_eq!(decide_present_sync(4_000, 16_000, 0.0), FlipKind::Vblank);
        assert_eq!(decide_present_sync(4_000, 16_000, -5.0), FlipKind::Vblank);
        assert_eq!(
            decide_present_sync(4_000, 16_000, f32::NAN),
            FlipKind::Vblank
        );
        assert_eq!(
            decide_present_sync(4_000, 16_000, 30.0),
            FlipKind::Immediate(25.0)
        );
        assert_eq!(decide_present_sync(4_000, 16_000, 20.0), FlipKind::Vblank);
        assert!(matches!(
            decide_present_sync(15_000, 16_000, 100.0),
            FlipKind::Immediate(_)
        ));
    }

    #[test]
    fn a_late_frame_flips_at_once_only_inside_the_threshold() {
        let mut s = capped(0);
        let Poll::Ready(t) = s.poll_wait(1_000) else {
            panic!()
        };
        let due = 1_000 + u64::from(I);
        let late = s.present(due + 4_000, t, 50.0);
        assert_eq!(late.flip, FlipKind::Immediate(25.0));
        assert_eq!(late.display_us, due + 4_000);
        assert_eq!(late.late_us, 4_000);
        assert_eq!(s.immediate_flips(), 1);

        let mut s = capped(0);
        s.poll_wait(1_000);
        let t = s.present(due + 4_000, 1, 10.0);
        assert_eq!(t.flip, FlipKind::Vblank);
        assert_eq!(t.display_us, due + u64::from(I));

        let mut s = capped(0);
        s.poll_wait(1_000);
        let t = s.present(due - 1, 1, 100.0);
        assert_eq!(t.flip, FlipKind::Vblank);
        assert_eq!(t.late_us, 0);
    }

    #[test]
    fn a_frame_behind_a_waiting_one_never_tears() {
        let mut s = capped(0);
        s.poll_wait(1_000);
        s.poll_wait(1_000 + u64::from(I));
        let due = 1_000 + u64::from(I);
        let first = s.present(due - 10, 1, 100.0);
        assert_eq!(first.flip, FlipKind::Vblank);
        let second = s.present(due - 5, 2, 100.0);
        assert_eq!(second.flip, FlipKind::Vblank);
        assert_eq!(second.display_us, first.display_us + u64::from(I));
    }

    #[test]
    fn uncapped_waits_return_at_once_with_fresh_tokens() {
        let mut s = FrameState::default();
        s.set_interval(
            0,
            Pacing {
                interval_us: 0,
                sync: SyncMode::Immediate,
            },
            1,
        );
        assert_eq!(s.poll_wait(0), Poll::Invalid);
        s.schedule_origin(0);
        assert_eq!(s.poll_wait(5), Poll::Ready(1));
        assert_eq!(s.present(6, 1, 0.0).flip, FlipKind::Unpaced);
        assert_eq!(s.poll_wait(7), Poll::Ready(2));
        assert_eq!(s.poll_wait(7), Poll::Ready(3));
    }

    #[test]
    fn suspend_wakes_waits_with_invalid_state_and_resume_needs_a_new_registration() {
        let mut s = capped(1);
        s.poll_wait(1_000);
        s.suspend();
        assert_eq!(s.poll_wait(2_000), Poll::Suspended);
        s.resume();
        assert_eq!(s.poll_wait(2_000), Poll::Invalid);
        s.set_interval(
            2_000,
            Pacing {
                interval_us: I,
                sync: SyncMode::Vsync,
            },
            1,
        );
        s.schedule_origin(0);
        assert_eq!(s.poll_wait(2_000), Poll::Ready(2));
    }

    #[test]
    fn setting_the_interval_again_rebases_the_grid_and_keeps_the_event() {
        let mut s = capped(0);
        s.poll_wait(1_000);
        s.set_interval(
            10_000,
            Pacing {
                interval_us: I,
                sync: SyncMode::Vsync,
            },
            0,
        );
        assert_eq!(s.poll_wait(10_000), Poll::Ready(2));
    }

    #[test]
    fn statistics_report_event_present_and_display_times() {
        let mut s = capped(0);
        let Poll::Ready(t) = s.poll_wait(1_000) else {
            panic!()
        };
        s.present(1_500, t, 0.0);
        let all = StatKind::Event.bit() | StatKind::Present.bit() | StatKind::Display.bit();
        let early = s.statistics(2_000, t, all, 8);
        assert_eq!(
            early.iter().map(|e| e.kind).collect::<Vec<_>>(),
            vec![StatKind::Event, StatKind::Present]
        );
        let later = s.statistics(1_000 + u64::from(I), t, all, 8);
        assert_eq!(later.len(), 3);
        assert_eq!(later[2].kind, StatKind::Display);
        assert_eq!(later[2].time_us, 1_000 + u64::from(I));
        assert_eq!(s.statistics(2_000, t, all, 1).len(), 1);
        assert!(s.statistics(2_000, 99, all, 8).is_empty());
        assert_eq!(
            s.statistics(0, 0, StatKind::Interval.bit(), 4)[0].time_us,
            u64::from(I)
        );
    }

    #[test]
    fn only_the_last_frames_are_kept() {
        let mut s = capped(0);
        for i in 0..200u64 {
            s.poll_wait(1_000 + i * u64::from(I));
        }
        assert!(s.statistics(u64::MAX, 0, StatKind::Event.bit(), 1000).len() <= KEEP);
        assert!(s.token_valid(200));
    }

    struct Manual(Arc<AtomicU64>);

    impl Clock for Manual {
        fn now_us(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn blocking_wait_times_out_with_s_false_and_reports_unscheduled_events() {
        let events = FrameEvents::new(Box::new(SystemClock::default()));
        assert_eq!(events.wait(0, 0).hresult, E_INVALIDARG);
        events.with(|s, now| {
            s.set_interval(
                now,
                Pacing {
                    interval_us: 200_000,
                    sync: SyncMode::Vsync,
                },
                0,
            );
            s.schedule_origin(100_000);
        });
        let r = events.wait(5, 0);
        assert_eq!(
            r,
            WaitResult {
                hresult: S_FALSE,
                token: 0
            }
        );
        let r = events.wait(1_000, 0);
        assert_eq!(r.hresult, S_OK);
        assert_eq!(r.token, 1);
    }

    #[test]
    fn real_time_waits_run_at_the_grid_rate() {
        let events = FrameEvents::new(Box::new(SystemClock::default()));
        events.with(|s, now| {
            s.set_interval(
                now,
                Pacing {
                    interval_us: 10_000,
                    sync: SyncMode::Vsync,
                },
                0,
            );
            s.schedule_origin(0);
        });
        let start = Instant::now();
        let mut last = 0;
        for _ in 0..6 {
            let r = events.wait(u32::MAX, 0);
            assert_eq!(r.hresult, S_OK);
            assert!(r.token > last);
            last = r.token;
        }
        let spent = start.elapsed();
        assert!(spent >= Duration::from_millis(45), "{spent:?}");
        assert!(spent < Duration::from_millis(400), "{spent:?}");
    }

    #[test]
    fn suspend_releases_a_blocked_wait() {
        let events = Arc::new(FrameEvents::new(Box::new(SystemClock::default())));
        events.with(|s, now| {
            s.set_interval(
                now,
                Pacing {
                    interval_us: 5_000_000,
                    sync: SyncMode::Vsync,
                },
                0,
            );
            s.schedule_origin(2_000_000);
        });
        let waiter = {
            let events = events.clone();
            std::thread::spawn(move || events.wait(u32::MAX, 0))
        };
        std::thread::sleep(Duration::from_millis(30));
        events.with(|s, _| s.suspend());
        assert_eq!(waiter.join().unwrap().hresult, E_INVALID_STATE);
    }

    #[test]
    fn manual_clock_drives_the_blocking_wrapper_without_sleeping_when_due() {
        let time = Arc::new(AtomicU64::new(0));
        let events = FrameEvents::new(Box::new(Manual(time.clone())));
        events.with(|s, now| {
            s.set_interval(
                now,
                Pacing {
                    interval_us: 1_000,
                    sync: SyncMode::Vsync,
                },
                0,
            );
            s.schedule_origin(0);
        });
        time.store(5_000, Ordering::SeqCst);
        assert_eq!(events.wait(0, 0).hresult, S_OK);
    }
}
