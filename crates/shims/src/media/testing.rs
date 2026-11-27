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
