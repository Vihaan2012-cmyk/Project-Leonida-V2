//! Stand-ins for tests: a decoder that makes test patterns and a stream header builder.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::decoder::{AudioTrackInfo, DecodeError, DecodedFrame, MovieDecoder, Plane, StreamInfo};
use super::header::HEADER_BYTES;

/// A header with the fixed part and `tracks` stereo 48 kHz descriptors (ids 0, 3, 6, ...).
pub fn stream_header(frames: u32, width: u32, height: u32, tracks: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; HEADER_BYTES + tracks as usize * 12];
    bytes[..4].copy_from_slice(b"KB2n");
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    bytes[8..12].copy_from_slice(&frames.to_le_bytes());
    bytes[0x10..0x14].copy_from_slice(&frames.to_le_bytes());
    bytes[0x14..0x18].copy_from_slice(&width.to_le_bytes());
    bytes[0x18..0x1C].copy_from_slice(&height.to_le_bytes());
    bytes[0x1C..0x20].copy_from_slice(&30u32.to_le_bytes());
    bytes[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
    bytes[0x28..0x2C].copy_from_slice(&tracks.to_le_bytes());
    let rates = HEADER_BYTES + tracks as usize * 4;
    let ids = rates + tracks as usize * 4;
    for t in 0..tracks as usize {
        bytes[rates + t * 4..rates + t * 4 + 2].copy_from_slice(&48000u16.to_le_bytes());
        bytes[rates + t * 4 + 2..rates + t * 4 + 4].copy_from_slice(&0x7000u16.to_le_bytes());
        bytes[ids + t * 4..ids + t * 4 + 4].copy_from_slice(&(t as u32 * 3).to_le_bytes());
    }
    bytes
}

/// Holds the fake decoder's `decode_next` until opened.
pub struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    pub fn new(open: bool) -> Arc<Gate> {
        Arc::new(Gate {
            open: Mutex::new(open),
            changed: Condvar::new(),
        })
    }

    pub fn set(&self, open: bool) {
        if let Ok(mut state) = self.open.lock() {
            *state = open;
        }
        self.changed.notify_all();
    }

    fn pass(&self) {
        let Ok(mut state) = self.open.lock() else {
            return;
        };
        while !*state {
            state = match self.changed.wait_timeout(state, Duration::from_secs(10)) {
                Ok((state, _)) => state,
                Err(_) => return,
            };
        }
    }
}

/// What a fake decoder was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Decode(u32),
    Seek(u32),
}

/// Stereo frames of audio the fake decoder makes per video frame.
pub const AUDIO_FRAMES: usize = 1600;

/// Makes pattern frames whose pixels and samples say which frame they belong to. Planes have a stride wider
/// than their width.
pub struct FakeDecoder {
    pub info: StreamInfo,
    next: u32,
    pub fail_at: Option<u32>,
    pub gate: Option<Arc<Gate>>,
    pub log: Arc<Mutex<Vec<Call>>>,
}

impl FakeDecoder {
    pub fn new(frames: u32, width: u32, height: u32, tracks: u32, alpha: bool) -> FakeDecoder {
        FakeDecoder {
            info: StreamInfo {
                width,
                height,
                frames,
                fps_num: 30,
                fps_den: 1,
                has_alpha: alpha,
                audio: (0..tracks)
                    .map(|t| AudioTrackInfo {
                        rate: 48000,
                        channels: 2,
                        id: t * 3,
                    })
                    .collect(),
            },
            next: 0,
            fail_at: None,
            gate: None,
            log: Arc::default(),
        }
    }

    /// The pixel plane `plane` (0 Y, 1 Cr, 2 Cb, 3 A) holds at (x, y) of frame `index`.
    pub fn pixel(plane: usize, index: u32, x: usize, y: usize) -> u8 {
        (x * 3 + y * 5 + index as usize * 7 + plane * 50) as u8
    }

    /// The `i`th sample of `track` for frame `index`.
    pub fn sample(track: usize, index: u32, i: usize) -> f32 {
        (track as f32 * 1000.0 + index as f32 * 10.0 + (i % 10) as f32) / 100_000.0
    }

    fn plane_dims(&self, plane: usize) -> (usize, usize) {
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        if plane == 1 || plane == 2 {
            (w.div_ceil(2), h.div_ceil(2))
        } else {
            (w, h)
        }
    }
}

impl MovieDecoder for FakeDecoder {
    fn info(&self) -> StreamInfo {
        self.info.clone()
    }

    fn decode_next(&mut self) -> Result<DecodedFrame, DecodeError> {
        if let Some(gate) = &self.gate {
            gate.pass();
        }
        let index = self.next;
        if self.fail_at == Some(index) {
            return Err(DecodeError(format!("fake failure at frame {index}")));
        }
        if index >= self.info.frames {
            return Err(DecodeError("past the last frame".to_string()));
        }
        if let Ok(mut log) = self.log.lock() {
            log.push(Call::Decode(index));
        }
        self.next += 1;
        let count = if self.info.has_alpha { 4 } else { 3 };
        let planes = (0..count)
            .map(|p| {
                let (width, height) = self.plane_dims(p);
                let stride = width + 7;
                let mut data = vec![0xEE; stride * height];
                for y in 0..height {
                    for x in 0..width {
                        data[y * stride + x] = FakeDecoder::pixel(p, index, x, y);
                    }
                }
                Plane {
                    data,
                    stride,
                    width,
                    height,
                }
            })
            .collect();
        Ok(DecodedFrame { index, planes })
    }

    fn seek(&mut self, frame: u32) -> Result<(), DecodeError> {
        if let Ok(mut log) = self.log.lock() {
            log.push(Call::Seek(frame));
        }
        self.next = frame;
        Ok(())
    }

    fn audio(&mut self, track: usize) -> Vec<f32> {
        let index = self.next.saturating_sub(1);
        (0..AUDIO_FRAMES * 2)
            .map(|i| FakeDecoder::sample(track, index, i))
            .collect()
    }
}

/// A clock the test moves by hand.
#[derive(Default)]
pub struct ManualClock(Mutex<Duration>);

impl ManualClock {
    pub fn advance(&self, by: Duration) {
        if let Ok(mut now) = self.0.lock() {
            *now += by;
        }
    }
}

impl super::player::Clock for ManualClock {
    fn now(&self) -> Duration {
        self.0.lock().map(|t| *t).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SinkEvent {
    Push {
        track: usize,
        rate: u32,
        channels: u8,
        samples: Vec<f32>,
    },
    Flush,
    Gain(usize, f32),
    Pause(bool),
}

/// Records what the player sends to the audio engine.
#[derive(Clone, Default)]
pub struct RecordingSink(pub Arc<Mutex<Vec<SinkEvent>>>);

impl super::sound::AudioSink for RecordingSink {
    fn push(&mut self, track: usize, rate: u32, channels: u8, samples: Vec<f32>) {
        self.0.lock().unwrap().push(SinkEvent::Push {
            track,
            rate,
            channels,
            samples,
        });
    }
    fn flush(&mut self) {
        self.0.lock().unwrap().push(SinkEvent::Flush);
    }
    fn set_gain(&mut self, track: usize, gain: f32) {
        self.0.lock().unwrap().push(SinkEvent::Gain(track, gain));
    }
    fn pause(&mut self, paused: bool) {
        self.0.lock().unwrap().push(SinkEvent::Pause(paused));
    }
}

/// The game's side of the frame buffers: a description with mapped plane memory at a pitch of 64.
pub struct FakeGame {
    desc: Vec<u64>,
    planes: Vec<Vec<u8>>,
}

pub const GAME_PITCH: usize = 64;

impl FakeGame {
    pub fn new(info: &StreamInfo) -> FakeGame {
        let mut desc = vec![0u64; super::layout::SIZE / 8];
        // SAFETY: `desc` is `SIZE` bytes.
        unsafe { super::layout::describe(desc.as_mut_ptr() as *mut u8, info) };
        let mut game = FakeGame {
            desc,
            planes: Vec::new(),
        };
        let (cw, ch) = super::layout::chroma_size(info.width, info.height);
        let _ = cw;
        for set in 0..super::layout::TOTAL_FRAMES as usize {
            for plane in 0..5usize {
                let at = 0x18 + set * 0x50 + plane * 0x10;
                let bytes = game.bytes();
                if u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()) == 0 {
                    continue;
                }
                let h = if plane == 1 || plane == 2 {
                    ch
                } else {
                    info.height
                } as usize;
                game.planes.push(vec![0x55; GAME_PITCH * h]);
                let ptr = game.planes.last_mut().unwrap().as_mut_ptr() as u64;
                let bytes = game.bytes();
                bytes[at..at + 8].copy_from_slice(&ptr.to_le_bytes());
                bytes[at + 12..at + 16].copy_from_slice(&(GAME_PITCH as u32).to_le_bytes());
            }
        }
        game
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: `desc` is `SIZE` bytes of u64.
        unsafe {
            std::slice::from_raw_parts_mut(self.desc.as_mut_ptr() as *mut u8, super::layout::SIZE)
        }
    }

    pub fn ptr(&mut self) -> usize {
        self.desc.as_mut_ptr() as usize
    }

    pub fn frame_num(&self) -> u32 {
        // SAFETY: inside `desc`.
        unsafe { ((self.desc.as_ptr() as *const u8).add(0x14) as *const u32).read() }
    }

    /// The memory of `plane` (0 Y, 1 Cr, 2 Cb, 3 A) of `set`, for a movie with `planes_per_set` planes.
    pub fn plane(&self, set: usize, plane: usize, planes_per_set: usize) -> &[u8] {
        &self.planes[set * planes_per_set + plane]
    }
}
