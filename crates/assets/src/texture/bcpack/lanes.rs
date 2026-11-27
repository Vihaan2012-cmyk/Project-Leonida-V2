//! The interleaved rANS lane array: 12 or 16 lanes decoded in lockstep from one shared byte stream.
//!
//! Each lane is a small state machine clocked once per cycle (header bytes, fragment payload, drain). Which lane
//! gets which byte of the shared stream is decided by the clock: a lane with an empty one-byte input buffer asks
//! for a byte, at most [`FEED_WINDOW`] requests are served per cycle in lane order, a lane that asked and was not
//! served freezes for the cycle, and a lane may only hand over its next symbol once every lane has handed over
//! the current one. The decoder therefore replays the clock; the symbols come out lane by lane.

use super::model::{L, MAX_EAB, Model, model};
use thiserror::Error;

/// Bytes the stream hands out per cycle.
pub const FEED_WINDOW: usize = 8;
const MAX_LANES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Hdr0,
    Hdr1,
    Hdr2,
    Proc,
    Drain,
    Idle,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LaneError {
    #[error("lane {lane}: fragment header selects an alphabet wider than the lane")]
    BadHeader { lane: usize },
    #[error("the stream ran out of bytes after {consumed}")]
    Truncated { consumed: usize },
    #[error("no lane made progress after {consumed} bytes")]
    NoProgress { consumed: usize },
    #[error("lanes {lanes:?} ended outside the legal rANS end states")]
    EndState { lanes: Vec<usize> },
    #[error("non-zero bytes follow the last lane at {consumed}")]
    TrailingData { consumed: usize },
}

/// Where the shared stream's bytes come from.
pub trait ByteSource {
    /// The next byte, handed to `lane`; `None` when the stream is exhausted.
    fn next(&mut self, lane: usize) -> Option<u8>;
    /// Bytes handed out so far.
    fn consumed(&self) -> usize;
}

pub struct SliceSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        SliceSource { data, pos: 0 }
    }
}

impl ByteSource for SliceSource<'_> {
    #[inline(always)]
    fn next(&mut self, _lane: usize) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn consumed(&self) -> usize {
        self.pos
    }
}

/// Decoded symbols, lane-major.
pub struct LaneSymbols {
    per_lane: usize,
    symbols: Vec<u8>,
}

impl LaneSymbols {
    pub fn lane(&self, i: usize) -> &[u8] {
        &self.symbols[i * self.per_lane..(i + 1) * self.per_lane]
    }
}

struct Lane {
    state: State,
    width_bits: u8,
    in_buf: u8,
    in_full: bool,
    sym: u8,
    sym_full: bool,
    in_rem: u32,
    out_rem: u32,
    p0: u32,
    p1: u32,
    long_header: bool,
    flush: bool,
    q: u8,
    eab: u8,
    model: &'static Model,
    done: bool,
    pushed: u32,
    final_state: u32,
}

fn is_legal_end(x: u32) -> bool {
    x == L || x == L >> 8 || x == L >> 16
}

/// Decode `per_lane` symbols for each lane. `widths` are the lanes' default symbol widths in bits.
pub fn decode_lanes<S: ByteSource>(
    src: &mut S,
    widths: &[u8],
    per_lane: usize,
) -> Result<LaneSymbols, LaneError> {
    let n = widths.len();
    assert!(n > 0 && n <= MAX_LANES && per_lane > 0);
    let m0 = model(0, 0);
    let mut lanes: Vec<Lane> = widths
        .iter()
        .map(|&w| Lane {
            state: State::Hdr0,
            width_bits: w - 1,
            in_buf: 0,
            in_full: false,
            sym: 0,
            sym_full: false,
            in_rem: 0,
            out_rem: per_lane as u32,
            p0: 0,
            p1: 0,
            long_header: false,
            flush: false,
            q: 0,
            eab: 0,
            model: m0,
            done: false,
            pushed: 0,
            final_state: 0,
        })
        .collect();
    let mut symbols = vec![0u8; n * per_lane];
    let mut slots = 0u32;
    let mut cycles = 0u64;
    let mut watch = (usize::MAX, u64::MAX);
    let mut idle_checks = 0;
    let mut pushed_total = 0u64;

    loop {
        let mut served = 0usize;
        let mut active = 0u32;
        let mut finished = true;
        for (i, ln) in lanes.iter_mut().enumerate() {
            let bit = 1u32 << i;
            let blocked = ln.sym_full && slots & bit != 0;
            let wants = !ln.in_full
                && match ln.state {
                    State::Hdr0 | State::Hdr1 | State::Hdr2 => true,
                    State::Proc => ln.in_rem > 0,
                    _ => false,
                };
            let mut fed = None;
            if wants && !blocked && served < FEED_WINDOW {
                match src.next(i) {
                    Some(b) => fed = Some(b),
                    None => {
                        return Err(LaneError::Truncated {
                            consumed: src.consumed(),
                        });
                    }
                }
            }
            let stall = (wants && fed.is_none()) || blocked;
            if !stall {
                if matches!(ln.state, State::Proc | State::Drain) {
                    ln.p0 = if ln.sym_full {
                        ln.model.advance(ln.p1, ln.sym)
                    } else {
                        ln.p1
                    };
                }
                if let Some(b) = fed {
                    ln.in_buf = b;
                    ln.in_full = true;
                    served += 1;
                }
                if ln.sym_full {
                    slots |= bit;
                    symbols[i * per_lane + ln.pushed as usize] = ln.sym;
                    ln.pushed += 1;
                    pushed_total += 1;
                    ln.sym_full = false;
                }
                step(i, ln)?;
            }
            if !(ln.done && !ln.sym_full && slots & bit == 0) {
                active |= bit;
            }
            finished &= ln.done && !ln.sym_full;
        }
        if slots != 0 && slots & active == active {
            slots = 0;
        }
        cycles += 1;
        if finished {
            break;
        }
        if cycles & 0x3FF == 0 {
            let now = (src.consumed(), pushed_total);
            if now == watch {
                idle_checks += 1;
                if idle_checks >= 4 {
                    return Err(LaneError::NoProgress { consumed: now.0 });
                }
            } else {
                watch = now;
                idle_checks = 0;
            }
        }
    }
    let bad: Vec<usize> = (0..n)
        .filter(|&i| !is_legal_end(lanes[i].final_state))
        .collect();
    if !bad.is_empty() {
        return Err(LaneError::EndState { lanes: bad });
    }
    Ok(LaneSymbols { per_lane, symbols })
}

/// The part of a cycle after the byte and symbol hand-over: header parsing, byte merge and symbol decode.
#[inline(always)]
fn step(i: usize, ln: &mut Lane) -> Result<(), LaneError> {
    match ln.state {
        State::Idle => {}
        State::Hdr0 => {
            if ln.in_full {
                let b = ln.in_buf;
                ln.in_full = false;
                ln.in_rem = 0;
                ln.state = State::Hdr1;
                ln.q = b & 15;
                ln.long_header = b & 0x20 != 0;
                ln.flush = b & 0x10 != 0;
                let eab = i32::from(ln.width_bits) - i32::from(b >> 6);
                if !(0..=MAX_EAB as i32).contains(&eab) {
                    return Err(LaneError::BadHeader { lane: i });
                }
                ln.eab = eab as u8;
            }
        }
        State::Hdr1 => {
            if ln.in_full {
                ln.in_full = false;
                ln.model = model(usize::from(ln.eab), usize::from(ln.q));
                if ln.long_header {
                    ln.in_rem = u32::from(ln.in_buf);
                    ln.state = State::Hdr2;
                } else {
                    ln.in_rem = u32::from(ln.in_buf) + 1;
                    ln.state = State::Proc;
                }
            }
        }
        State::Hdr2 => {
            if ln.in_full {
                ln.in_full = false;
                ln.in_rem = (u32::from(ln.in_buf) << 8 | (ln.in_rem & 0xFF)) + 257;
                ln.state = State::Proc;
            }
        }
        State::Proc | State::Drain => {
            let proc = ln.state == State::Proc;
            let merge = proc && ln.p0 < L && ln.in_rem > 0 && ln.in_full;
            let x = if merge {
                (ln.p0 & 0xFF_FFFF) << 8 | u32::from(ln.in_buf)
            } else {
                ln.p0
            };
            let decodes = x > L && ln.out_rem > 0;
            if decodes {
                ln.out_rem -= 1;
                ln.sym = ln.model.symbol(x);
            }
            ln.p1 = x;
            ln.sym_full = decodes;
            if proc {
                if merge {
                    ln.in_rem -= 1;
                    ln.in_full = false;
                }
                if ln.in_rem == 0 {
                    if ln.out_rem > 0 {
                        if x == L {
                            ln.state = State::Hdr0;
                            if ln.flush {
                                ln.p1 = 0;
                            }
                        } else {
                            ln.state = State::Drain;
                        }
                    } else {
                        finish(ln, x, decodes);
                    }
                }
            } else if x <= L {
                if ln.out_rem > 0 {
                    ln.state = State::Hdr0;
                    if ln.flush {
                        ln.p0 = 0;
                        ln.p1 = 0;
                    }
                } else {
                    finish(ln, x, decodes);
                }
            }
        }
    }
    Ok(())
}

#[inline(always)]
fn finish(ln: &mut Lane, x: u32, decoded: bool) {
    ln.done = true;
    ln.state = State::Idle;
    ln.final_state = if decoded {
        ln.model.advance(x, ln.sym)
    } else {
        x
    };
}

/// Decode the lane array from `data` and require the remainder to be zero padding.
pub fn decode_slice(data: &[u8], widths: &[u8], per_lane: usize) -> Result<LaneSymbols, LaneError> {
    let mut src = SliceSource::new(data);
    let out = decode_lanes(&mut src, widths, per_lane)?;
    let used = src.consumed();
    if data[used..].iter().any(|b| *b != 0) {
        return Err(LaneError::TrailingData { consumed: used });
    }
    Ok(out)
}
