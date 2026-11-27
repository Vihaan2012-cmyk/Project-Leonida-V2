//! The XMA contexts: 64-byte records in the shape-context memory that the title fills in and the coprocessor
//! works from. The title enables a context (HAL operation 5), the coprocessor decodes its input into the context's
//! output ring and moves the write offset, and the title reports what it took with operation 0x12.
//!
//! The words used, as the title writes them (little-endian, bit 0 first):
//!
//! | word | bits | field |
//! |---|---|---|
//! | 0 | 0-11 | packets in input buffer 0 |
//! | 0 | 12-19 | loop count (255 when the stream loops until stopped) |
//! | 0 | 22-26 | output ring size in 256-byte blocks (23) |
//! | 0 | 27-31 | output write offset in blocks, moved by the coprocessor |
//! | 1 | 27-28 | sample rate: 24000, 32000, 44100, 48000 |
//! | 1 | 29 | stereo |
//! | 2 | 0-25 | input read position in bits |
//! | 3, 4 | 0-25 | loop start and end, in bits |
//! | 5 | all | input buffer 0 (coprocessor address) |
//! | 7 | all | output ring (coprocessor address) |
//! | 9 | 0-4 | output read offset in blocks, moved by operation 0x12 |
//!
//! Output is 16-bit little-endian interleaved PCM written in whole 256-byte blocks. Loops are followed at packet
//! granularity: the frame offsets inside a loop's first and last packet are not applied.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use audio_codec::xma2::{Xma2Config, Xma2Decoder};

pub const CONTEXT_BYTES: usize = 64;
pub const BLOCK_BYTES: usize = 256;
const PACKET_BYTES: usize = 2048;
const PACKET_BITS: u32 = (PACKET_BYTES * 8) as u32;
const RATES: [u32; 4] = [24_000, 32_000, 44_100, 48_000];
const TICK: Duration = Duration::from_millis(4);
const PACKETS_PER_REFILL: usize = 2;

/// Where decoded samples come from. The real source is an XMA2 decoder; tests bring their own.
pub trait PcmSource: Send {
    /// Appends interleaved samples; false once the stream is over.
    fn refill(&mut self, out: &mut VecDeque<i16>) -> bool;
}

/// A context as the title set it up.
pub struct Setup {
    pub ring: usize,
    pub ring_blocks: u32,
    pub source: Box<dyn PcmSource>,
}

pub struct XmaSource {
    decoder: Xma2Decoder,
    data: Vec<u8>,
    next: usize,
    channels: usize,
    loop_start: usize,
    loop_end: Option<usize>,
    loops_left: Option<u32>,
}

impl XmaSource {
    fn push_next(&mut self) -> bool {
        let last = self.data.len() / PACKET_BYTES;
        let stop = self.loop_end.map_or(last, |end| (end + 1).min(last));
        if self.next >= stop {
            let again = match self.loops_left {
                Some(0) | None => false,
                Some(left) => {
                    self.loops_left = (left != 255).then(|| left - 1);
                    true
                }
            };
            if !again || self.loop_end.is_none() {
                return false;
            }
            self.next = self.loop_start;
            return self.decoder.restart_at_packet(self.next).is_ok();
        }
        let at = self.next * PACKET_BYTES;
        let _ = self.decoder.push_packet(&self.data[at..at + PACKET_BYTES]);
        self.next += 1;
        true
    }
}

impl PcmSource for XmaSource {
    fn refill(&mut self, out: &mut VecDeque<i16>) -> bool {
        let mut more = true;
        for _ in 0..PACKETS_PER_REFILL {
            more = self.push_next();
            if !more {
                break;
            }
        }
        let frames = self.decoder.available();
        if frames > 0 {
            let mut pcm = vec![0i16; frames * self.channels];
            let n = self.decoder.pull_i16(&mut pcm);
            out.extend(&pcm[..n * self.channels]);
        }
        more
    }
}

fn word(context: usize, index: usize) -> &'static AtomicU32 {
    // SAFETY: the title's contexts are 4-byte aligned words of the shape-context block, which the engine keeps
    // alive until the worker has stopped.
    unsafe { AtomicU32::from_ptr((context + index * 4) as *mut u32) }
}

/// Builds a context's job from what the title wrote. `input` is the copy of its input buffer.
pub fn setup_from_context(
    context: usize,
    input: Vec<u8>,
    ring: usize,
) -> Result<Setup, audio_codec::Error> {
    let w0 = word(context, 0).load(Ordering::Acquire);
    let w1 = word(context, 1).load(Ordering::Acquire);
    let w2 = word(context, 2).load(Ordering::Acquire);
    let loop_start = word(context, 3).load(Ordering::Acquire) & 0x03ff_ffff;
    let loop_end = word(context, 4).load(Ordering::Acquire) & 0x03ff_ffff;
    let channels = if w1 >> 29 & 1 == 1 { 2 } else { 1 };
    let rate = RATES[(w1 >> 27 & 3) as usize];
    let ring_blocks = (w0 >> 22) & 0x1f;
    let decoder = Xma2Decoder::new(Xma2Config::new(rate, channels, None))?;
    let first = (w2 & 0x03ff_ffff) / PACKET_BITS;
    let looping = loop_end > loop_start;
    let mut source = XmaSource {
        decoder,
        data: input,
        next: first as usize,
        channels,
        loop_start: (loop_start / PACKET_BITS) as usize,
        loop_end: looping.then_some((loop_end / PACKET_BITS) as usize),
        loops_left: Some((w0 >> 12) & 0xff),
    };
    if first != 0 {
        source.decoder.restart_at_packet(first as usize)?;
    }
    Ok(Setup {
        ring,
        ring_blocks,
        source: Box::new(source),
    })
}

/// Marks `blocks` output blocks as read: moves the read offset (word 9) around the ring.
pub fn output_read(context: usize, blocks: u32) {
    let ring = (word(context, 0).load(Ordering::Acquire) >> 22) & 0x1f;
    if ring == 0 {
        return;
    }
    let _ = word(context, 9).fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        let read = ((v & 0x1f) + blocks) % ring;
        Some((v & !0x1f) | read)
    });
}

enum Message {
    Enable(usize, Setup),
    Disable(usize),
    Stop,
}

struct Job {
    ring: usize,
    ring_blocks: u32,
    source: Box<dyn PcmSource>,
    pending: VecDeque<i16>,
    done: bool,
}

/// The worker that decodes enabled contexts into their rings.
#[derive(Default)]
pub struct XmaHw {
    link: Option<(Sender<Message>, JoinHandle<()>)>,
}

impl XmaHw {
    pub fn enable(&mut self, context: usize, setup: Setup) {
        if self.link.is_none() {
            let (tx, rx) = channel();
            let handle = std::thread::Builder::new()
                .name("leonida-xma-hw".into())
                .spawn(move || run(rx))
                .ok();
            self.link = handle.map(|h| (tx, h));
        }
        if let Some((tx, _)) = &self.link {
            let _ = tx.send(Message::Enable(context, setup));
        }
    }

    pub fn disable(&mut self, context: usize) {
        if let Some((tx, _)) = &self.link {
            let _ = tx.send(Message::Disable(context));
        }
    }

    /// Stops the worker and waits for it: nothing writes to the title's memory afterwards.
    pub fn shutdown(&mut self) {
        if let Some((tx, handle)) = self.link.take() {
            let _ = tx.send(Message::Stop);
            let _ = handle.join();
        }
    }
}

impl Drop for XmaHw {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(rx: Receiver<Message>) {
    let mut jobs: HashMap<usize, Job> = HashMap::new();
    loop {
        match rx.recv_timeout(TICK) {
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Message::Enable(context, setup)) => {
                jobs.insert(
                    context,
                    Job {
                        ring: setup.ring,
                        ring_blocks: setup.ring_blocks,
                        source: setup.source,
                        pending: VecDeque::new(),
                        done: false,
                    },
                );
                continue;
            }
            Ok(Message::Disable(context)) => {
                jobs.remove(&context);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        for (&context, job) in &mut jobs {
            tick(context, job);
        }
    }
}

/// Moves what the ring has room for from the job's decoded samples into the ring.
fn tick(context: usize, job: &mut Job) {
    let ring = job.ring_blocks;
    if ring < 2 || job.ring == 0 {
        return;
    }
    let write = word(context, 0).load(Ordering::Acquire) >> 27;
    let read = word(context, 9).load(Ordering::Acquire) & 0x1f;
    let used = (write + ring - read) % ring;
    let free = (ring - 1 - used) as usize;
    let block_samples = BLOCK_BYTES / 2;
    while !job.done && job.pending.len() < free * block_samples {
        if !job.source.refill(&mut job.pending) {
            job.done = true;
        }
    }
    let blocks = free.min(job.pending.len() / block_samples);
    if blocks == 0 {
        return;
    }
    let mut at = write;
    for _ in 0..blocks {
        let dst = job.ring + at as usize * BLOCK_BYTES;
        for i in 0..block_samples {
            let sample = job.pending.pop_front().unwrap_or(0);
            // SAFETY: the ring is `ring_blocks` blocks of the title's output buffer, which stays allocated until
            // the title has disabled the context or the engine has shut the worker down.
            unsafe { ((dst + i * 2) as *mut i16).write_unaligned(sample.to_le()) };
        }
        at = (at + 1) % ring;
    }
    let _ = word(context, 0).fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        Some((v & 0x07ff_ffff) | (at << 27))
    });
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    struct Counter {
        next: i16,
        left: usize,
    }

    impl PcmSource for Counter {
        fn refill(&mut self, out: &mut VecDeque<i16>) -> bool {
            for _ in 0..512.min(self.left) {
                out.push_back(self.next);
                self.next += 1;
                self.left -= 1;
            }
            self.left > 0
        }
    }

    fn context_words(ring_blocks: u32) -> Box<[u32; 16]> {
        let mut w = Box::new([0u32; 16]);
        w[0] = ring_blocks << 22;
        w
    }

    fn job(ring: usize, ring_blocks: u32, samples: usize) -> Job {
        Job {
            ring,
            ring_blocks,
            source: Box::new(Counter {
                next: 0,
                left: samples,
            }),
            pending: VecDeque::new(),
            done: false,
        }
    }

    fn ring_samples(ring: &[u8], first_block: usize, blocks: usize) -> Vec<i16> {
        ring[first_block * BLOCK_BYTES..(first_block + blocks) * BLOCK_BYTES]
            .chunks(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    #[test]
    fn a_tick_fills_the_free_blocks_and_moves_the_write_offset() {
        let mut ctx = context_words(23);
        let mut ring = vec![0u8; 23 * BLOCK_BYTES];
        let mut j = job(ring.as_mut_ptr() as usize, 23, 100_000);
        tick(ctx.as_mut_ptr() as usize, &mut j);
        assert_eq!(ctx[0] >> 27, 22, "one block stays free: full is not empty");
        let samples = ring_samples(&ring, 0, 22);
        assert!(samples.iter().enumerate().all(|(i, s)| *s == i as i16));
        tick(ctx.as_mut_ptr() as usize, &mut j);
        assert_eq!(ctx[0] >> 27, 22, "nothing moves until the title reads");
    }

    #[test]
    fn reading_blocks_makes_room_and_the_ring_wraps() {
        let mut ctx = context_words(23);
        let mut ring = vec![0u8; 23 * BLOCK_BYTES];
        let mut j = job(ring.as_mut_ptr() as usize, 23, 100_000);
        let c = ctx.as_mut_ptr() as usize;
        tick(c, &mut j);
        output_read(c, 10);
        assert_eq!(ctx[9] & 0x1f, 10);
        tick(c, &mut j);
        assert_eq!(ctx[0] >> 27, (22 + 10) % 23);
        assert_eq!(ring_samples(&ring, 22, 1)[0], (22 * 128) as i16);
        assert_eq!(
            ring_samples(&ring, 0, 1)[0],
            (23 * 128) as i16,
            "the 24th block continues at the start of the ring"
        );
        output_read(c, 20);
        assert_eq!(ctx[9] & 0x1f, 7, "the read offset wraps at the ring size");
    }

    #[test]
    fn a_short_stream_ends_without_padding_partial_blocks() {
        let mut ctx = context_words(23);
        let mut ring = vec![0xAAu8; 23 * BLOCK_BYTES];
        let mut j = job(ring.as_mut_ptr() as usize, 23, 300);
        tick(ctx.as_mut_ptr() as usize, &mut j);
        assert_eq!(
            ctx[0] >> 27,
            2,
            "300 samples are two whole 128-sample blocks"
        );
        assert!(j.done);
        assert_eq!(j.pending.len(), 44);
    }

    #[test]
    fn the_worker_follows_enable_and_disable_and_stops_before_returning() {
        let mut ctx = context_words(8);
        let mut ring = vec![0u8; 8 * BLOCK_BYTES];
        let c = ctx.as_mut_ptr() as usize;
        let mut hw = XmaHw::default();
        hw.enable(
            c,
            Setup {
                ring: ring.as_mut_ptr() as usize,
                ring_blocks: 8,
                source: Box::new(Counter {
                    next: 0,
                    left: 10_000,
                }),
            },
        );
        let until = Instant::now() + Duration::from_secs(5);
        while word(c, 0).load(Ordering::Acquire) >> 27 != 7 {
            assert!(Instant::now() < until, "the worker never filled the ring");
            std::thread::sleep(Duration::from_millis(1));
        }
        hw.disable(c);
        hw.shutdown();
        assert_eq!(ring_samples(&ring, 0, 1)[5], 5);
    }

    #[test]
    fn a_context_is_read_back_into_a_decoder_setup() {
        let mut ctx = context_words(23);
        ctx[0] |= 3 | (255 << 12);
        ctx[1] = (1 << 29) | (3 << 27);
        let setup = setup_from_context(ctx.as_mut_ptr() as usize, vec![0; 2048 * 3], 0x1000);
        let mut setup = setup.expect("a stereo 48 kHz setup");
        assert_eq!((setup.ring, setup.ring_blocks), (0x1000, 23));
        let mut out = VecDeque::new();
        let _ = setup.source.refill(&mut out);
    }
}
