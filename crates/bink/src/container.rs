//! The Bink 2 file header, frame index and the split of a frame into audio packets and video data.

use std::sync::Arc;

use crate::error::{Error, Result};

const FIXED: usize = 0x2C;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    pub rate: u32,
    pub channels: u16,
    pub id: u32,
    /// Bit 0x1000 of the stored flags: the DCT codec rather than the RDFT one.
    pub dct: bool,
    pub max_decoded_bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub has_alpha: bool,
    pub audio: Vec<AudioTrack>,
    /// The revision letter after `KB2`.
    pub version: u8,
    /// The word at +0x24: bit 0x100000 is alpha, the low two bits pick the slice count.
    pub flags: u32,
    pub largest_frame: u32,
}

#[derive(Debug, Clone, Copy)]
struct IndexEntry {
    offset: usize,
    key: bool,
}

pub struct Container {
    data: Arc<[u8]>,
    pub header: Header,
    index: Vec<IndexEntry>,
}

/// One frame cut into its audio packets and its video data.
pub struct FramePackets<'a> {
    /// Per track, the bytes after the 4-byte packet size: a 4-byte decoded byte count, then the codec data.
    pub audio: Vec<&'a [u8]>,
    pub video: &'a [u8],
    pub key: bool,
}

fn word(data: &[u8], at: usize) -> Result<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or(Error::Truncated)
}

impl Container {
    pub fn parse(data: Arc<[u8]>) -> Result<Container> {
        if data.len() < 4 || &data[..3] != b"KB2" {
            return Err(Error::NotBink);
        }
        let version = data[3];
        if !version.is_ascii_lowercase() {
            return Err(Error::NotBink);
        }
        let frames = word(&data, 8)?;
        let largest_frame = word(&data, 0x0C)?;
        let width = word(&data, 0x14)?;
        let height = word(&data, 0x18)?;
        let fps_num = word(&data, 0x1C)?;
        let fps_den = word(&data, 0x20)?;
        let flags = word(&data, 0x24)?;
        let tracks = word(&data, 0x28)? as usize;
        if frames == 0 || frames > 1 << 22 || frames != word(&data, 0x10)? {
            return Err(Error::Header("frame count"));
        }
        if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
            return Err(Error::Header("picture size"));
        }
        if fps_num == 0 || fps_den == 0 {
            return Err(Error::Header("frame rate"));
        }
        if tracks > 256 {
            return Err(Error::Header("audio track count"));
        }
        let mut at = FIXED;
        if version >= b'i' {
            at += 4;
        }
        let max_sizes = at;
        at += 4 * tracks;
        let rates = at;
        at += 4 * tracks;
        let ids = at;
        at += 4 * tracks;
        let mut audio = Vec::with_capacity(tracks);
        for t in 0..tracks {
            let rate_flags = word(&data, rates + 4 * t)?;
            let track_flags = (rate_flags >> 16) as u16;
            audio.push(AudioTrack {
                rate: rate_flags & 0xFFFF,
                channels: if track_flags & 0x2000 != 0 { 2 } else { 1 },
                id: word(&data, ids + 4 * t)?,
                dct: track_flags & 0x1000 != 0,
                max_decoded_bytes: word(&data, max_sizes + 4 * t)?,
            });
        }
        let mut index = Vec::with_capacity(frames as usize);
        for f in 0..frames as usize {
            let raw = word(&data, at + 4 * f)? as usize;
            index.push(IndexEntry {
                offset: raw & !1,
                key: raw & 1 != 0,
            });
        }
        let first = at + 4 * frames as usize;
        let mut previous = first;
        for entry in &index {
            if entry.offset < previous || entry.offset > data.len() {
                return Err(Error::Header("frame index"));
            }
            previous = entry.offset;
        }
        Ok(Container {
            header: Header {
                width,
                height,
                frames,
                fps_num,
                fps_den,
                has_alpha: flags & 0x10_0000 != 0,
                audio,
                version,
                flags,
                largest_frame,
            },
            index,
            data,
        })
    }

    pub fn is_key(&self, frame: u32) -> bool {
        self.index.get(frame as usize).is_some_and(|e| e.key)
    }

    /// The nearest keyframe at or before `frame`.
    pub fn key_at_or_before(&self, frame: u32) -> u32 {
        (0..=frame.min(self.header.frames - 1))
            .rev()
            .find(|f| self.index[*f as usize].key)
            .unwrap_or(0)
    }

    pub fn packets(&self, frame: u32) -> Result<FramePackets<'_>> {
        let entry = self
            .index
            .get(frame as usize)
            .ok_or(Error::NoSuchFrame(frame))?;
        let end = self
            .index
            .get(frame as usize + 1)
            .map_or(self.data.len(), |e| e.offset);
        let mut rest = &self.data[entry.offset..end];
        let mut audio = Vec::with_capacity(self.header.audio.len());
        for _ in &self.header.audio {
            let size = word(rest, 0)? as usize;
            let body = rest.get(4..4 + size).ok_or(Error::Truncated)?;
            audio.push(body);
            rest = &rest[4 + size..];
        }
        Ok(FramePackets {
            audio,
            video: rest,
            key: entry.key,
        })
    }
}
