//! The audio DSP behind `chad.dll`: `XDspConnectWithMaximumStreamLimit`, `XDspActivate`, `XDspSubmitCommand`,
//! `XDspDeactivate` and `XDspDisconnect`.
//!
//! What the title does with it, read from its code (RVAs are image offsets):
//!
//! - It never sends its final mix here. The title mixes on the CPU in 256-sample blocks and delivers the result
//!   through the system's own audio client (MMDevice enumerator, `IAudioClient`, `ISpatialAudioClient`, 0x43aea20
//!   and 0x1e7f580); the host provides those. The DSP carries only the **convolution reverb** (the strings
//!   `CONVOLUTION_REVERB_` and `audImpulseResponseSound` sit next to the node that opens it, 0x3b770 and 0x20cfaa0).
//! - `XDspConnectWithMaximumStreamLimit(memory, size, version, limit, 1, &handle)` (0x5b52600): `memory` is a
//!   0x60000-byte block the title allocated (0x1d6bdf0) and carved into 48 pairs of 0x1000-byte buffers, 8 in its
//!   first table and 40 in its second; `version` is 0x18 or 0x1e, `limit` is 0x80. The title keeps the pairs and
//!   hands their addresses back in commands, so the DSP needs no use of `memory`. The handle goes to `+0x488` of
//!   the title's device record.
//! - `XDspActivate(handle, &info, &status, &stream)` (0x11d4750) opens one stream. `info` is 0x1c bytes with the
//!   fields of [`StreamInfo`]; the reverb node (0x3b770) sends kind 1 with the impulse response's address and
//!   length. The DSP writes the address of a status block to `status` and the stream's handle to `stream`.
//! - `XDspSubmitCommand(stream, &command, &fence)` (0x5b200 passes its third register through untouched): the
//!   command is 0x20 bytes, see [`Command`]. The title copies one batch of input into the pair's first buffer,
//!   submits, and on its next audio frame reads the pair's second buffer. It checks the status block first: the
//!   batch counts as done when `done == fence` and `error == 0` (0x4e5acc0, 0x43b2a40). Anything else counts as a
//!   miss, and after a few misses the node switches itself off, so a submit finishes its work before it returns
//!   and the fence is already complete when the title looks.
//! - `XDspDeactivate(stream)` (0x1166010) ends one stream; `XDspDisconnect(handle)` (0x43ae9c0) ends the device.
//!
//! The batch the title packs is 1024 samples for a mono stream (four 256-sample blocks in a ring) and 256 samples
//! per channel for a stereo stream (`[left][right]`, left first). The DSP side of that is inferred from how the
//! title packs and unpacks the buffers: the info's block field (`0x400`) is the same for both. A stereo impulse
//! response is read as two planar halves; the gain pair (both 1.0 in every command seen) scales channel 0 and
//! channel 1.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::convolve::Convolver;

pub const INFO_BYTES: usize = 0x1c;
pub const COMMAND_BYTES: usize = 0x20;
/// Samples in one mono batch.
pub const MONO_BATCH: usize = 1024;
/// Samples per channel in one stereo batch.
pub const STEREO_BATCH: usize = 256;
/// Longest impulse response taken, in samples; a longer one is cut.
pub const MAX_IR_SAMPLES: usize = 1 << 21;
/// Streams one connection may hold when the title gives no limit.
const DEFAULT_LIMIT: usize = 0x80;

pub const KIND_CONVOLUTION: u32 = 1;
pub const KIND_PASS_THROUGH: u32 = 2;
/// `StreamInfo::flags`: the stream carries two channels.
pub const FLAG_STEREO_INPUT: u32 = 2;
/// `StreamInfo::flags`: the impulse response has two channels.
pub const FLAG_STEREO_RESPONSE: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DspError {
    #[error("no such DSP connection {0:#x}")]
    UnknownConnection(u64),
    #[error("no such DSP stream {0:#x}")]
    UnknownStream(u64),
    #[error("the connection already holds {0} streams")]
    TooManyStreams(usize),
    #[error("stream kind {0} is not one the title has been seen to open")]
    UnknownKind(u32),
    #[error("the impulse response is missing")]
    NoResponse,
    #[error("a command named a null buffer")]
    NullBuffer,
}

impl DspError {
    /// The `HRESULT` the shims hand back.
    pub fn code(&self) -> u32 {
        match self {
            DspError::UnknownConnection(_) | DspError::UnknownStream(_) | DspError::NullBuffer => {
                0x8007_0057
            }
            DspError::TooManyStreams(_) => 0x8007_000E,
            DspError::UnknownKind(_) => 0x8000_4001,
            DspError::NoResponse => 0x8007_0057,
        }
    }
}

/// The 0x1c-byte description of a stream the title passes to `XDspActivate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub kind: u32,
    pub flags: u32,
    /// 0x400 in everything the title sends.
    pub block: u32,
    /// The reverb node's channel count.
    pub mode: u32,
    /// The impulse response's length in 32-bit words.
    pub words: u32,
    /// The impulse response's address (unaligned 64-bit field at `+0x14`).
    pub data: u64,
}

impl StreamInfo {
    pub fn parse(bytes: &[u8; INFO_BYTES]) -> StreamInfo {
        let word =
            |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        StreamInfo {
            kind: word(0),
            flags: word(4),
            block: word(8),
            mode: word(0xc),
            words: word(0x10),
            data: u64::from_le_bytes(bytes[0x14..0x1c].try_into().expect("eight bytes")),
        }
    }

    pub fn to_bytes(&self) -> [u8; INFO_BYTES] {
        let mut bytes = [0u8; INFO_BYTES];
        for (at, value) in [
            (0, self.kind),
            (4, self.flags),
            (8, self.block),
            (0xc, self.mode),
            (0x10, self.words),
        ] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[0x14..].copy_from_slice(&self.data.to_le_bytes());
        bytes
    }

    pub fn stereo(&self) -> bool {
        self.flags & FLAG_STEREO_INPUT != 0 || self.mode == 2
    }
}

/// The 0x20-byte command of `XDspSubmitCommand`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Command {
    pub gains: [f32; 2],
    pub input: u64,
    pub output: u64,
}

impl Command {
    pub fn parse(bytes: &[u8; COMMAND_BYTES]) -> Command {
        let float =
            |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        let address =
            |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"));
        Command {
            gains: [float(0), float(4)],
            input: address(8),
            output: address(0x10),
        }
    }

    pub fn to_bytes(&self) -> [u8; COMMAND_BYTES] {
        let mut bytes = [0u8; COMMAND_BYTES];
        bytes[0..4].copy_from_slice(&self.gains[0].to_le_bytes());
        bytes[4..8].copy_from_slice(&self.gains[1].to_le_bytes());
        bytes[8..0x10].copy_from_slice(&self.input.to_le_bytes());
        bytes[0x10..0x18].copy_from_slice(&self.output.to_le_bytes());
        bytes
    }
}

/// The block the title polls. Word 0 is 1 while the stream runs, word 1 the fence of the last finished command,
/// word 2 the fence of the last accepted one and word 3 an error flag (0 when nothing went wrong).
#[repr(C)]
pub struct StatusBlock {
    words: [AtomicU32; 4],
}

impl StatusBlock {
    fn new() -> StatusBlock {
        let block = StatusBlock {
            words: Default::default(),
        };
        block.words[0].store(1, Ordering::Release);
        block
    }

    pub fn running(&self) -> bool {
        self.words[0].load(Ordering::Acquire) != 0
    }

    pub fn done(&self) -> u32 {
        self.words[1].load(Ordering::Acquire)
    }

    pub fn accepted(&self) -> u32 {
        self.words[2].load(Ordering::Acquire)
    }

    pub fn error(&self) -> u32 {
        self.words[3].load(Ordering::Acquire)
    }
}

enum Processor {
    Reverb {
        channels: Vec<Convolver>,
        batch: usize,
    },
    PassThrough {
        batch: usize,
    },
}

/// One open stream.
pub struct Stream {
    status: Box<StatusBlock>,
    fence: u32,
    processor: Processor,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl Stream {
    fn new(info: &StreamInfo, response: &[f32]) -> Stream {
        let stereo = info.stereo();
        let (processor, floats) = match info.kind {
            KIND_CONVOLUTION => {
                let (channels, batch) = if stereo {
                    (2, STEREO_BATCH)
                } else {
                    (1, MONO_BATCH)
                };
                let planes = if info.flags & FLAG_STEREO_RESPONSE != 0 {
                    2
                } else {
                    1
                };
                let each = response.len() / planes;
                let convolvers = (0..channels)
                    .map(|c| {
                        let plane = c.min(planes - 1);
                        Convolver::new(&response[plane * each..(plane + 1) * each], batch)
                    })
                    .collect();
                (
                    Processor::Reverb {
                        channels: convolvers,
                        batch,
                    },
                    channels * batch,
                )
            }
            _ => {
                let batch = (info.block as usize).clamp(1, 0x400);
                (Processor::PassThrough { batch }, batch)
            }
        };
        Stream {
            status: Box::new(StatusBlock::new()),
            fence: 0,
            processor,
            input: vec![0.0; floats],
            output: vec![0.0; floats],
        }
    }

    /// The address of the status block the title polls.
    pub fn status_address(&self) -> usize {
        &*self.status as *const StatusBlock as usize
    }

    pub fn status(&self) -> &StatusBlock {
        &self.status
    }

    /// Floats a command reads and writes.
    pub fn floats(&self) -> usize {
        self.input.len()
    }

    /// Runs one batch: `input` holds [`Stream::floats`] samples, the result lands in `output`. The fence of the
    /// finished command is returned and already visible in the status block.
    pub fn process(&mut self, gains: [f32; 2], input: &[f32], output: &mut [f32]) -> u32 {
        self.fence = self.fence.wrapping_add(1).max(1);
        self.status.words[2].store(self.fence, Ordering::Release);
        match &mut self.processor {
            Processor::Reverb { channels, batch } => {
                for (c, convolver) in channels.iter_mut().enumerate() {
                    let range = c * *batch..(c + 1) * *batch;
                    convolver.process(&input[range.clone()], &mut output[range.clone()]);
                    let gain = gains[c.min(1)];
                    if gain != 1.0 {
                        output[range].iter_mut().for_each(|s| *s *= gain);
                    }
                }
            }
            Processor::PassThrough { batch } => {
                output[..*batch].copy_from_slice(&input[..*batch]);
            }
        }
        self.status.words[1].store(self.fence, Ordering::Release);
        self.fence
    }

    /// Runs a command whose buffers are addresses in the title's memory.
    ///
    /// # Safety
    /// `command.input` and `command.output` must each address at least [`Stream::floats`] floats the title owns,
    /// as the buffers of the pair it took from its pool do (0x1000 bytes each).
    pub unsafe fn run(&mut self, command: &Command) -> Result<u32, DspError> {
        if command.input == 0 || command.output == 0 {
            return Err(DspError::NullBuffer);
        }
        let floats = self.input.len();
        let mut input = std::mem::take(&mut self.input);
        let mut output = std::mem::take(&mut self.output);
        // SAFETY: the caller guarantees both addresses hold `floats` floats; the input is copied out first so the
        // two buffers are never borrowed together.
        unsafe {
            std::ptr::copy_nonoverlapping(command.input as *const f32, input.as_mut_ptr(), floats);
        }
        let fence = self.process(command.gains, &input, &mut output);
        // SAFETY: as above.
        unsafe {
            std::ptr::copy_nonoverlapping(output.as_ptr(), command.output as *mut f32, floats);
        }
        self.input = input;
        self.output = output;
        Ok(fence)
    }
}

struct Connection {
    limit: usize,
    streams: Vec<u64>,
}

/// Every connection and stream the title has open.
#[derive(Default)]
pub struct Dsp {
    connections: HashMap<u64, Connection>,
    streams: HashMap<u64, Arc<Mutex<Stream>>>,
    next: u64,
    noted: HashSet<String>,
}

/// A stream's handle and the address of its status block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened {
    pub stream: u64,
    pub status: usize,
}

impl Dsp {
    fn fresh_handle(&mut self) -> u64 {
        self.next += 1;
        0x4453_5000_0000 + self.next
    }

    fn note_once(&mut self, key: &str, text: impl FnOnce() -> String) {
        if self.noted.insert(key.to_string()) {
            crate::trace::note(text());
        }
    }

    /// Opens a connection; `limit` is the title's stream limit (0 takes the default).
    pub fn connect(&mut self, version: u32, limit: u32) -> u64 {
        let handle = self.fresh_handle();
        self.note_once("connect", || {
            format!("XDspConnectWithMaximumStreamLimit version {version:#x}: the title's shared block is not used")
        });
        let limit = if limit == 0 {
            DEFAULT_LIMIT
        } else {
            limit as usize
        };
        self.connections.insert(
            handle,
            Connection {
                limit,
                streams: Vec::new(),
            },
        );
        handle
    }

    /// Ends a connection and every stream on it.
    pub fn disconnect(&mut self, connection: u64) -> Result<(), DspError> {
        let held = self
            .connections
            .remove(&connection)
            .ok_or(DspError::UnknownConnection(connection))?;
        for stream in held.streams {
            self.streams.remove(&stream);
        }
        Ok(())
    }

    /// Opens a stream; `response` is the impulse response already read from the title's memory.
    pub fn activate(
        &mut self,
        connection: u64,
        info: &StreamInfo,
        response: &[f32],
    ) -> Result<Opened, DspError> {
        let held = self
            .connections
            .get(&connection)
            .ok_or(DspError::UnknownConnection(connection))?;
        if held.streams.len() >= held.limit {
            return Err(DspError::TooManyStreams(held.limit));
        }
        match info.kind {
            KIND_CONVOLUTION => {
                if response.is_empty() {
                    return Err(DspError::NoResponse);
                }
                if info.flags & !(FLAG_STEREO_INPUT | FLAG_STEREO_RESPONSE) != 0 || info.block != 0x400 {
                    self.note_once("info-shape", || {
                        format!(
                            "a convolution stream opened with flags {:#x}, block {:#x}, mode {}: only the measured shapes are understood",
                            info.flags, info.block, info.mode
                        )
                    });
                }
            }
            KIND_PASS_THROUGH => self.note_once("kind-2", || {
                "a DSP stream of kind 2 was opened: what it computes is unmeasured, it copies its input".into()
            }),
            other => return Err(DspError::UnknownKind(other)),
        }
        let stream = Stream::new(info, response);
        let status = stream.status_address();
        let handle = self.fresh_handle();
        self.streams.insert(handle, Arc::new(Mutex::new(stream)));
        if let Some(held) = self.connections.get_mut(&connection) {
            held.streams.push(handle);
        }
        Ok(Opened {
            stream: handle,
            status,
        })
    }

    /// The stream behind a handle, for a command to run on outside this table's lock.
    pub fn stream(&self, handle: u64) -> Result<Arc<Mutex<Stream>>, DspError> {
        self.streams
            .get(&handle)
            .cloned()
            .ok_or(DspError::UnknownStream(handle))
    }

    pub fn deactivate(&mut self, stream: u64) -> Result<(), DspError> {
        let removed = self
            .streams
            .remove(&stream)
            .ok_or(DspError::UnknownStream(stream))?;
        removed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .words[0]
            .store(0, Ordering::Release);
        for held in self.connections.values_mut() {
            held.streams.retain(|s| *s != stream);
        }
        Ok(())
    }

    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}

/// Locks a stream, ignoring poisoning: a panicked command must not stall the title's next frame.
pub fn lock_stream(stream: &Mutex<Stream>) -> MutexGuard<'_, Stream> {
    stream.lock().unwrap_or_else(|p| p.into_inner())
}

/// Copies the impulse response a stream's info points at.
///
/// # Safety
/// `info.data` must be null or address `info.words` 32-bit floats the title keeps loaded for the call.
pub unsafe fn read_response(info: &StreamInfo) -> Vec<f32> {
    if info.data == 0 || info.words == 0 {
        return Vec::new();
    }
    let words = (info.words as usize).min(MAX_IR_SAMPLES);
    // SAFETY: guaranteed by the caller.
    unsafe { std::slice::from_raw_parts(info.data as *const f32, words) }.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(flags: u32, words: u32) -> StreamInfo {
        StreamInfo {
            kind: KIND_CONVOLUTION,
            flags,
            block: 0x400,
            mode: if flags & FLAG_STEREO_INPUT != 0 { 2 } else { 1 },
            words,
            data: 0,
        }
    }

    #[test]
    fn info_and_command_round_trip_through_their_byte_layouts() {
        let original = StreamInfo {
            data: 0x1122_3344_5566_7788,
            ..info(10, 500)
        };
        assert_eq!(StreamInfo::parse(&original.to_bytes()), original);
        let bytes = original.to_bytes();
        assert_eq!(&bytes[0x14..], &0x1122_3344_5566_7788u64.to_le_bytes());
        let command = Command {
            gains: [1.0, 0.5],
            input: 0x1000,
            output: 0x2000,
        };
        assert_eq!(Command::parse(&command.to_bytes()), command);
        assert_eq!(&command.to_bytes()[..4], &1.0f32.to_le_bytes());
    }

    #[test]
    fn a_mono_stream_convolves_batches_and_completes_each_fence_before_returning() {
        let mut dsp = Dsp::default();
        let connection = dsp.connect(0x18, 0x80);
        let mut response = vec![0.0f32; 1500];
        response[0] = 0.5;
        response[1200] = 0.25;
        let opened = dsp
            .activate(connection, &info(0, response.len() as u32), &response)
            .unwrap();
        let stream = dsp.stream(opened.stream).unwrap();
        let mut stream = lock_stream(&stream);
        assert_eq!(stream.floats(), MONO_BATCH);
        assert!(stream.status().running());
        assert_eq!((stream.status().done(), stream.status().error()), (0, 0));
        let mut impulse = vec![0.0f32; MONO_BATCH];
        impulse[0] = 1.0;
        let mut first = vec![0.0; MONO_BATCH];
        let fence = stream.process([1.0, 1.0], &impulse, &mut first);
        assert_eq!(fence, 1);
        assert_eq!(stream.status().done(), fence);
        assert!((first[0] - 0.5).abs() < 1e-4);
        assert!(first[1..].iter().all(|s| s.abs() < 1e-4));
        let mut second = vec![0.0; MONO_BATCH];
        let fence = stream.process([2.0, 2.0], &vec![0.0; MONO_BATCH], &mut second);
        assert_eq!(
            (fence, stream.status().done(), stream.status().accepted()),
            (2, 2, 2)
        );
        let tail = 1200 - MONO_BATCH;
        assert!(
            (second[tail] - 0.5).abs() < 1e-4,
            "0.25 at the second batch, doubled by the gain"
        );
        assert!(
            second
                .iter()
                .enumerate()
                .all(|(i, s)| i == tail || s.abs() < 1e-4)
        );
    }

    #[test]
    fn a_stereo_stream_keeps_channels_apart_and_reads_a_planar_response() {
        let mut dsp = Dsp::default();
        let connection = dsp.connect(0x1e, 0x80);
        let mut response = vec![0.0f32; 2 * 100];
        response[5] = 1.0;
        response[100 + 9] = 0.5;
        let opened = dsp
            .activate(
                connection,
                &info(FLAG_STEREO_INPUT | FLAG_STEREO_RESPONSE, 200),
                &response,
            )
            .unwrap();
        let stream = dsp.stream(opened.stream).unwrap();
        let mut stream = lock_stream(&stream);
        assert_eq!(stream.floats(), 2 * STEREO_BATCH);
        let mut input = vec![0.0f32; 2 * STEREO_BATCH];
        input[0] = 1.0;
        input[STEREO_BATCH + 2] = 1.0;
        let mut output = vec![0.0; 2 * STEREO_BATCH];
        stream.process([1.0, 1.0], &input, &mut output);
        assert!((output[5] - 1.0).abs() < 1e-4);
        assert!((output[STEREO_BATCH + 2 + 9] - 0.5).abs() < 1e-4);
        assert!(
            output[STEREO_BATCH + 5].abs() < 1e-4,
            "left's response does not reach the right channel"
        );
    }

    #[test]
    fn the_stream_limit_and_unknown_handles_are_refused_with_typed_errors() {
        let mut dsp = Dsp::default();
        let connection = dsp.connect(0x18, 2);
        let response = vec![1.0f32; 8];
        for _ in 0..2 {
            dsp.activate(connection, &info(0, 8), &response).unwrap();
        }
        assert_eq!(
            dsp.activate(connection, &info(0, 8), &response),
            Err(DspError::TooManyStreams(2))
        );
        assert_eq!(
            dsp.activate(77, &info(0, 8), &response),
            Err(DspError::UnknownConnection(77))
        );
        assert_eq!(
            dsp.activate(connection, &info(0, 0), &[]),
            Err(DspError::TooManyStreams(2))
        );
        let other = dsp.connect(0x18, 0x80);
        assert_eq!(
            dsp.activate(other, &info(0, 0), &[]),
            Err(DspError::NoResponse)
        );
        let odd = StreamInfo {
            kind: 9,
            ..info(0, 8)
        };
        assert_eq!(
            dsp.activate(other, &odd, &response),
            Err(DspError::UnknownKind(9))
        );
        assert!(matches!(dsp.stream(5), Err(DspError::UnknownStream(5))));
        assert_eq!(DspError::UnknownKind(9).code(), 0x8000_4001);
    }

    #[test]
    fn deactivating_and_disconnecting_release_streams_and_stop_their_status() {
        let mut dsp = Dsp::default();
        let connection = dsp.connect(0x18, 0x80);
        let a = dsp.activate(connection, &info(0, 4), &[1.0; 4]).unwrap();
        let b = dsp.activate(connection, &info(0, 4), &[1.0; 4]).unwrap();
        let kept = dsp.stream(a.stream).unwrap();
        dsp.deactivate(a.stream).unwrap();
        assert!(!lock_stream(&kept).status().running());
        assert_eq!(
            dsp.deactivate(a.stream),
            Err(DspError::UnknownStream(a.stream))
        );
        assert_eq!(dsp.stream_count(), 1);
        dsp.disconnect(connection).unwrap();
        assert_eq!((dsp.stream_count(), dsp.connection_count()), (0, 0));
        assert!(dsp.stream(b.stream).is_err());
        assert!(dsp.disconnect(connection).is_err());
    }

    #[test]
    fn a_pass_through_stream_copies_so_its_fence_still_completes() {
        let mut dsp = Dsp::default();
        let connection = dsp.connect(0x18, 0x80);
        let kind2 = StreamInfo {
            kind: KIND_PASS_THROUGH,
            flags: 0,
            block: 0x400,
            mode: 1,
            words: 0,
            data: 0,
        };
        let opened = dsp.activate(connection, &kind2, &[]).unwrap();
        let stream = dsp.stream(opened.stream).unwrap();
        let mut stream = lock_stream(&stream);
        let input: Vec<f32> = (0..0x400).map(|i| i as f32).collect();
        let mut output = vec![0.0; 0x400];
        let fence = stream.process([1.0, 1.0], &input, &mut output);
        assert_eq!(output, input);
        assert_eq!(stream.status().done(), fence);
    }

    #[test]
    fn running_a_command_reads_and_writes_the_titles_buffers_and_rejects_null_ones() {
        let mut dsp = Dsp::default();
        let connection = dsp.connect(0x18, 0x80);
        let mut response = vec![0.0f32; 4];
        response[2] = 1.0;
        let opened = dsp.activate(connection, &info(0, 4), &response).unwrap();
        let stream = dsp.stream(opened.stream).unwrap();
        let mut stream = lock_stream(&stream);
        let input: Vec<f32> = (0..MONO_BATCH).map(|i| (i % 7) as f32).collect();
        let mut output = vec![9.0f32; MONO_BATCH];
        let command = Command {
            gains: [1.0, 1.0],
            input: input.as_ptr() as u64,
            output: output.as_mut_ptr() as u64,
        };
        // SAFETY: both vectors hold MONO_BATCH floats.
        let fence = unsafe { stream.run(&command) }.unwrap();
        assert_eq!(fence, 1);
        assert!(output[0].abs() < 1e-4);
        assert!((output[2] - input[0]).abs() < 1e-3);
        assert!((output[10] - input[8]).abs() < 1e-3);
        let null = Command {
            input: 0,
            ..command
        };
        // SAFETY: a null address is refused before it is read.
        assert_eq!(unsafe { stream.run(&null) }, Err(DspError::NullBuffer));
        assert_eq!(
            stream.status().done(),
            1,
            "a refused command leaves the fence alone"
        );
    }
}
