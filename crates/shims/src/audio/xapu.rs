//! The Opus decoder behind `XApuEnqueueCommand`, `XApuDequeueResult` and `XApuGetResultCount`.
//!
//! The title never decodes Opus itself: it opens a decoder stream on the coprocessor, copies each packet (the
//! two-byte length prefix removed) into a slot of a shared region and asks for 960 frames (20 ms at 48 kHz). The
//! region comes back from `XApuConnect`: 128 input slots of 0x280 bytes, then output slots of 0x1e00 bytes from
//! offset 0x14000 (960 frames of two `f32` channels). A command is 0x40 bytes and starts with the operation and
//! the stream slot (0 to 255):
//!
//! | op | command | result |
//! |---|---|---|
//! | 1 | open: channel count at +0x18 | op 1 |
//! | 2 | close | op 2 |
//! | 3 | decode: frames +0x24, input size +0x28, input address +0x2c, output size +0x34, output address +0x38 | op 3, bytes written at +0x34 |
//! | 5 | reset | op 5 |
//!
//! A result is 0x40 bytes: the operation, the slot, and at +0x34 the byte count of a decode. Everything runs on one
//! worker thread so the title's thread only copies a packet in and takes results out; results come back in order.
//! The output is interleaved `f32`: the title converts it to 16-bit samples as one flat run.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

pub const COMMAND_BYTES: usize = 0x40;
pub const RESULT_BYTES: usize = 0x40;
pub const INPUT_SLOT_BYTES: usize = 0x280;
pub const INPUT_SLOTS: usize = 128;
pub const OUTPUT_BASE: usize = 0x14000;
pub const OUTPUT_SLOT_BYTES: usize = 0x1e00;
pub const OUTPUT_SLOTS: usize = 256;
pub const REGION_BYTES: usize = OUTPUT_BASE + OUTPUT_SLOTS * OUTPUT_SLOT_BYTES;
/// Results kept before the oldest is dropped.
pub const RESULT_LIMIT: usize = 4096;
const MAX_STREAMS: usize = 256;
const MAX_FRAMES: usize = 5760;

/// The memory the title and the decoder share.
pub struct Region {
    ptr: *mut u8,
    layout: Layout,
}

// SAFETY: the region is plain memory; the title and the worker only touch disjoint slots the title hands out.
unsafe impl Send for Region {}
// SAFETY: as above.
unsafe impl Sync for Region {}

impl Region {
    fn new() -> Region {
        let layout = Layout::from_size_align(REGION_BYTES, 0x1000).expect("region layout");
        // SAFETY: the layout has a non-zero size.
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "the host refused the audio region");
        Region { ptr, layout }
    }

    pub fn base(&self) -> usize {
        self.ptr as usize
    }

    fn contains(&self, address: u64, len: usize) -> bool {
        let base = self.base() as u64;
        address >= base && address.saturating_add(len as u64) <= base + REGION_BYTES as u64
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with this layout.
        unsafe { dealloc(self.ptr, self.layout) };
    }
}

/// One Opus stream's decoder.
pub trait OpusStream: Send {
    /// Decodes one packet into interleaved `f32`; returns the frames written (at most `out.len() / channels`).
    fn decode(&mut self, packet: &[u8], frames: usize, out: &mut [f32]) -> Result<usize, String>;
    fn reset(&mut self) {}
}

pub type OpusFactory = fn(channels: u8) -> Option<Box<dyn OpusStream>>;

static FACTORY: Mutex<Option<OpusFactory>> = Mutex::new(None);

/// Installs the Opus decoder the worker opens streams with. `None` restores silence.
pub fn set_opus_factory(factory: Option<OpusFactory>) {
    if let Ok(mut slot) = FACTORY.lock() {
        *slot = factory;
    }
}

fn factory() -> Option<OpusFactory> {
    FACTORY.lock().ok().and_then(|f| *f)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Open {
        slot: u32,
        channels: u32,
    },
    Close {
        slot: u32,
    },
    Reset {
        slot: u32,
    },
    Decode {
        slot: u32,
        frames: u32,
        packet: Vec<u8>,
        out_bytes: u32,
        out_address: u64,
    },
    /// An operation the title has not been seen to use; it still gets its result.
    Other {
        op: u32,
        slot: u32,
    },
}

impl Command {
    pub fn op(&self) -> u32 {
        match self {
            Command::Open { .. } => 1,
            Command::Close { .. } => 2,
            Command::Decode { .. } => 3,
            Command::Reset { .. } => 5,
            Command::Other { op, .. } => *op,
        }
    }
}

fn word(bytes: &[u8; COMMAND_BYTES], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn word64(bytes: &[u8; COMMAND_BYTES], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XApuResult {
    pub op: u32,
    pub slot: u32,
    pub bytes: u32,
}

impl XApuResult {
    pub fn to_bytes(self) -> [u8; RESULT_BYTES] {
        let mut out = [0u8; RESULT_BYTES];
        out[0..4].copy_from_slice(&self.op.to_le_bytes());
        out[4..8].copy_from_slice(&self.slot.to_le_bytes());
        out[0x34..0x38].copy_from_slice(&self.bytes.to_le_bytes());
        out
    }
}

type Results = Arc<Mutex<VecDeque<XApuResult>>>;

enum Job {
    Command(Command),
    Stop,
}

struct Connection {
    region: Arc<Region>,
    jobs: Sender<Job>,
    results: Results,
    worker: Option<JoinHandle<()>>,
    handle: u64,
}

/// What a connection hands back: positive and non-zero, so it reads as valid whether the title tests it for
/// zero or for a negative value.
const HANDLE: u64 = 0x4150_5500;

#[derive(Default)]
pub struct XApu {
    connection: Option<Connection>,
    /// Used instead of the installed factory when set.
    factory: Option<OpusFactory>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl XApu {
    pub fn with_factory(factory: OpusFactory) -> XApu {
        XApu {
            connection: None,
            factory: Some(factory),
        }
    }

    /// Opens the connection (once) and returns the handle and the shared region's address.
    pub fn connect(&mut self) -> (u64, usize) {
        if self.connection.is_none() {
            let region = Arc::new(Region::new());
            let results: Results = Arc::default();
            let (jobs, rx) = channel();
            let worker = {
                let region = region.clone();
                let results = results.clone();
                let own = self.factory;
                std::thread::Builder::new()
                    .name("leonida-xapu".into())
                    .spawn(move || work(rx, region, results, own))
                    .ok()
            };
            self.connection = Some(Connection {
                region,
                jobs,
                results,
                worker,
                handle: HANDLE,
            });
        }
        let c = self.connection.as_ref().expect("connected above");
        (c.handle, c.region.base())
    }

    pub fn disconnect(&mut self) {
        if let Some(mut c) = self.connection.take() {
            let _ = c.jobs.send(Job::Stop);
            if let Some(worker) = c.worker.take() {
                let _ = worker.join();
            }
        }
    }

    pub fn connected(&self) -> bool {
        self.connection.is_some()
    }

    /// Reads a command the title built and hands it to the worker. Returns the operation.
    pub fn enqueue(&self, bytes: &[u8; COMMAND_BYTES]) -> Option<u32> {
        let c = self.connection.as_ref()?;
        let command = parse(bytes, &c.region);
        let op = command.op();
        c.jobs.send(Job::Command(command)).ok()?;
        Some(op)
    }

    pub fn result_count(&self) -> usize {
        self.connection
            .as_ref()
            .map_or(0, |c| lock(&c.results).len())
    }

    pub fn dequeue(&self) -> Option<XApuResult> {
        lock(&self.connection.as_ref()?.results).pop_front()
    }
}

impl Drop for XApu {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// Builds a command from the title's bytes. A decode whose packet lies outside the shared region carries an
/// empty packet and comes back with no bytes written.
pub fn parse(bytes: &[u8; COMMAND_BYTES], region: &Region) -> Command {
    let op = word(bytes, 0);
    let slot = word(bytes, 4);
    match op {
        1 => Command::Open {
            slot,
            channels: word(bytes, 0x18),
        },
        2 => Command::Close { slot },
        5 => Command::Reset { slot },
        3 => {
            let size = word(bytes, 0x28) as usize;
            let at = word64(bytes, 0x2c);
            let packet = if region.contains(at, size) {
                // SAFETY: the range lies inside the region, which lives as long as the connection.
                unsafe { std::slice::from_raw_parts(at as *const u8, size) }.to_vec()
            } else {
                Vec::new()
            };
            Command::Decode {
                slot,
                frames: word(bytes, 0x24),
                packet,
                out_bytes: word(bytes, 0x34),
                out_address: word64(bytes, 0x38),
            }
        }
        _ => Command::Other { op, slot },
    }
}

struct Stream {
    channels: usize,
    decoder: Option<Box<dyn OpusStream>>,
}

fn work(rx: Receiver<Job>, region: Arc<Region>, results: Results, own: Option<OpusFactory>) {
    let mut streams: HashMap<u32, Stream> = HashMap::new();
    let mut scratch: Vec<f32> = Vec::new();
    while let Ok(Job::Command(command)) = rx.recv() {
        let mut result = XApuResult {
            op: command.op(),
            slot: match &command {
                Command::Open { slot, .. }
                | Command::Close { slot }
                | Command::Reset { slot }
                | Command::Decode { slot, .. }
                | Command::Other { slot, .. } => *slot,
            },
            bytes: 0,
        };
        match command {
            Command::Open { slot, channels } => {
                let channels = channels.clamp(1, 2) as usize;
                if streams.len() < MAX_STREAMS || streams.contains_key(&slot) {
                    let decoder = own.or_else(factory).and_then(|f| f(channels as u8));
                    streams.insert(slot, Stream { channels, decoder });
                }
            }
            Command::Close { slot } => {
                streams.remove(&slot);
            }
            Command::Reset { slot } => {
                if let Some(Stream {
                    decoder: Some(d), ..
                }) = streams.get_mut(&slot)
                {
                    d.reset();
                }
            }
            Command::Decode {
                slot,
                frames,
                packet,
                out_bytes,
                out_address,
            } => {
                result.bytes = decode(
                    streams.get_mut(&slot),
                    frames,
                    &packet,
                    out_bytes,
                    out_address,
                    &region,
                    &mut scratch,
                );
            }
            Command::Other { .. } => {}
        }
        let mut queue = lock(&results);
        if queue.len() == RESULT_LIMIT {
            queue.pop_front();
        }
        queue.push_back(result);
    }
}

/// Decodes into the output slot and returns the bytes written; silence where there is no decoder or it fails.
fn decode(
    stream: Option<&mut Stream>,
    frames: u32,
    packet: &[u8],
    out_bytes: u32,
    out_address: u64,
    region: &Region,
    scratch: &mut Vec<f32>,
) -> u32 {
    let Some(stream) = stream else { return 0 };
    let frames = (frames as usize).min(MAX_FRAMES);
    let room = out_bytes as usize / 4;
    let samples = (frames * stream.channels).min(room);
    if samples == 0 || !region.contains(out_address, samples * 4) {
        return 0;
    }
    scratch.clear();
    scratch.resize(samples, 0.0);
    if let Some(decoder) = stream.decoder.as_mut()
        && !packet.is_empty()
        && decoder.decode(packet, frames, scratch).is_err()
    {
        scratch.fill(0.0);
    }
    // SAFETY: the range lies inside the region, which the worker keeps alive; the title does not read an
    // output slot before the result for it arrives.
    unsafe {
        std::ptr::copy_nonoverlapping(
            scratch.as_ptr().cast::<u8>(),
            out_address as *mut u8,
            samples * 4,
        );
    }
    (samples * 4) as u32
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn command(op: u32, slot: u32) -> [u8; COMMAND_BYTES] {
        let mut c = [0u8; COMMAND_BYTES];
        c[0..4].copy_from_slice(&op.to_le_bytes());
        c[4..8].copy_from_slice(&slot.to_le_bytes());
        c
    }

    fn put(c: &mut [u8; COMMAND_BYTES], at: usize, bytes: &[u8]) {
        c[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn decode_command(
        base: usize,
        slot: u32,
        input_slot: usize,
        output_slot: usize,
        len: u32,
        channels: u32,
    ) -> [u8; COMMAND_BYTES] {
        let mut c = command(3, slot);
        put(&mut c, 0x24, &960u32.to_le_bytes());
        put(&mut c, 0x28, &len.to_le_bytes());
        let input = (base + input_slot * INPUT_SLOT_BYTES) as u64;
        put(&mut c, 0x2c, &input.to_le_bytes());
        put(&mut c, 0x34, &(channels * 0xf00).to_le_bytes());
        let output = (base + OUTPUT_BASE + output_slot * OUTPUT_SLOT_BYTES) as u64;
        put(&mut c, 0x38, &output.to_le_bytes());
        c
    }

    fn wait_results(x: &XApu, n: usize) {
        let until = Instant::now() + Duration::from_secs(5);
        while x.result_count() < n {
            assert!(Instant::now() < until, "worker did not answer");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    struct Ramp {
        channels: usize,
        resets: u32,
    }

    impl OpusStream for Ramp {
        fn decode(
            &mut self,
            packet: &[u8],
            frames: usize,
            out: &mut [f32],
        ) -> Result<usize, String> {
            if packet[0] == 0xff {
                return Err("bad".into());
            }
            for (i, s) in out.iter_mut().enumerate() {
                *s = f32::from(packet[0]) + (i / self.channels) as f32 / 1000.0;
            }
            Ok(frames.min(out.len() / self.channels))
        }
        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    #[test]
    fn open_decode_reset_close_each_answer_in_order() {
        let mut x = XApu::default();
        let (handle, base) = x.connect();
        assert!(handle > 0 && handle < 0x8000_0000);
        assert_eq!(x.connect(), (handle, base));
        assert_eq!(x.result_count(), 0);

        let mut open = command(1, 7);
        put(&mut open, 0x18, &2u32.to_le_bytes());
        assert_eq!(x.enqueue(&open), Some(1));
        // SAFETY: slot 3 of the input region is ours.
        unsafe { ((base + 3 * INPUT_SLOT_BYTES) as *mut u8).write(9) };
        let decode = decode_command(base, 7, 3, 5, 120, 2);
        assert_eq!(x.enqueue(&decode), Some(3));
        assert_eq!(x.enqueue(&command(5, 7)), Some(5));
        assert_eq!(x.enqueue(&command(2, 7)), Some(2));
        wait_results(&x, 4);

        let got: Vec<_> = std::iter::from_fn(|| x.dequeue()).collect();
        let expected = [(1, 0), (3, 960 * 2 * 4), (5, 0), (2, 0)];
        assert_eq!(got.len(), 4);
        for (r, (op, bytes)) in got.iter().zip(expected) {
            assert_eq!((r.op, r.slot, r.bytes), (op, 7, bytes));
        }
        // SAFETY: the output slot lies in the region.
        let out = unsafe {
            std::slice::from_raw_parts(
                (base + OUTPUT_BASE + 5 * OUTPUT_SLOT_BYTES) as *const f32,
                1920,
            )
        };
        assert!(
            out.iter().all(|s| *s == 0.0),
            "no decoder installed: silence"
        );
        assert_eq!(x.dequeue(), None);
        x.disconnect();
        assert!(!x.connected());
        assert_eq!(x.enqueue(&command(1, 0)), None);
    }

    #[test]
    fn result_bytes_land_at_the_offsets_the_title_reads() {
        let r = XApuResult {
            op: 3,
            slot: 9,
            bytes: 7680,
        }
        .to_bytes();
        assert_eq!(&r[0..8], &[3, 0, 0, 0, 9, 0, 0, 0]);
        assert_eq!(
            u32::from_le_bytes([r[0x34], r[0x35], r[0x36], r[0x37]]),
            7680
        );
        assert!(r[8..0x34].iter().all(|b| *b == 0));
    }

    #[test]
    fn a_decode_outside_the_region_or_on_a_closed_stream_writes_nothing() {
        let mut x = XApu::default();
        let (_, base) = x.connect();
        let mut stray = vec![0u8; 64];
        let mut c = decode_command(base, 1, 0, 0, 10, 1);
        put(&mut c, 0x38, &(stray.as_mut_ptr() as u64).to_le_bytes());
        assert_eq!(x.enqueue(&command(1, 1)), Some(1));
        assert_eq!(x.enqueue(&c), Some(3));
        let closed = decode_command(base, 2, 0, 0, 10, 1);
        assert_eq!(x.enqueue(&closed), Some(3));
        wait_results(&x, 3);
        let bytes: Vec<u32> = std::iter::from_fn(|| x.dequeue())
            .map(|r| r.bytes)
            .collect();
        assert_eq!(bytes, [0, 0, 0]);
        assert!(stray.iter().all(|b| *b == 0));
    }

    #[test]
    fn a_title_thread_never_waits_on_the_worker() {
        let mut x = XApu::default();
        let (_, base) = x.connect();
        x.enqueue(&command(1, 0));
        let start = Instant::now();
        for i in 0..2000 {
            let c = decode_command(base, 0, i % INPUT_SLOTS, i % OUTPUT_SLOTS, 100, 2);
            x.enqueue(&c);
        }
        assert!(start.elapsed() < Duration::from_secs(2));
        wait_results(&x, 2001);
        assert_eq!(x.result_count(), 2001);
    }

    #[test]
    fn an_installed_decoder_writes_interleaved_floats_and_failures_are_silent() {
        let mut x = XApu::with_factory(|channels| {
            Some(Box::new(Ramp {
                channels: usize::from(channels),
                resets: 0,
            }))
        });
        let (_, base) = x.connect();
        let mut open = command(1, 2);
        put(&mut open, 0x18, &1u32.to_le_bytes());
        x.enqueue(&open);
        // SAFETY: input slots 0 and 1 are ours.
        unsafe {
            (base as *mut u8).write(4);
            ((base + INPUT_SLOT_BYTES) as *mut u8).write(0xff);
        }
        x.enqueue(&decode_command(base, 2, 0, 0, 50, 1));
        x.enqueue(&decode_command(base, 2, 1, 1, 50, 1));
        wait_results(&x, 3);
        let results: Vec<_> = std::iter::from_fn(|| x.dequeue()).collect();
        assert_eq!(results[1].bytes, 960 * 4);
        assert_eq!(results[2].bytes, 960 * 4);
        // SAFETY: output slots 0 and 1 are in the region.
        let (good, bad) = unsafe {
            (
                std::slice::from_raw_parts((base + OUTPUT_BASE) as *const f32, 960),
                std::slice::from_raw_parts(
                    (base + OUTPUT_BASE + OUTPUT_SLOT_BYTES) as *const f32,
                    960,
                ),
            )
        };
        assert_eq!(good[0], 4.0);
        assert!((good[500] - 4.5).abs() < 1e-4);
        assert!(bad.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn an_unknown_operation_still_gets_its_result() {
        let mut x = XApu::default();
        x.connect();
        assert_eq!(x.enqueue(&command(9, 4)), Some(9));
        wait_results(&x, 1);
        assert_eq!(
            x.dequeue(),
            Some(XApuResult {
                op: 9,
                slot: 4,
                bytes: 0
            })
        );
    }
}
