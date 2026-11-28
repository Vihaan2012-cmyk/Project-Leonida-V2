//! Pure-Rust decoder for Bink 2 movies (`KB2` streams) and their Bink audio tracks.
//!
//! Video frames come out as the game's own Bink library produces them: motion compensation, the inverse DCT and the
//! in-loop block filters follow that library's arithmetic (see the module docs of `video` for what was read where),
//! so a long chain of inter frames stays in step with the encoder.

mod audio;
mod bits;
mod container;
mod error;
mod tables;
mod video;
mod vlc;

use std::sync::Arc;

pub use audio::AudioDecoder;
pub use container::{AudioTrack, Container, Header};
pub use error::{Error, Result};
pub use video::{Frame, Plane};

use video::VideoDecoder;

pub struct Movie {
    container: Container,
    video: VideoDecoder,
    next: u32,
    current: Option<Frame>,
    audio: audio::Audio,
}

impl Movie {
    pub fn open(data: Arc<[u8]>) -> Result<Movie> {
        let container = Container::parse(data)?;
        let video = VideoDecoder::new(&container.header)?;
        let audio = audio::Audio::new(&container.header.audio);
        Ok(Movie {
            container,
            video,
            next: 0,
            current: None,
            audio,
        })
    }

    pub fn header(&self) -> &Header {
        &self.container.header
    }

    /// How many track packets were replaced by silence because they could not be decoded.
    pub fn audio_failures(&self) -> u32 {
        self.audio.failures()
    }

    /// Decodes the next frame; after the last one it reports `NoSuchFrame`.
    pub fn decode_next(&mut self) -> Result<&Frame> {
        let index = self.next;
        if index >= self.container.header.frames {
            return Err(Error::NoSuchFrame(index));
        }
        let packets = self.container.packets(index)?;
        let key = packets.key;
        let frame = self
            .video
            .decode(index, packets.video, key, self.current.as_ref())?;
        self.audio.decode_frame(&packets.audio);
        self.next += 1;
        Ok(self.current.insert(frame))
    }

    /// Positions the movie at the nearest keyframe at or before `frame`, then decodes forward to it.
    pub fn seek(&mut self, frame: u32) -> Result<()> {
        if frame >= self.container.header.frames {
            return Err(Error::NoSuchFrame(frame));
        }
        self.next = self.container.key_at_or_before(frame);
        self.current = None;
        while self.next <= frame {
            self.decode_next()?;
        }
        self.audio.seek(&self.container, self.next);
        Ok(())
    }

    /// Interleaved samples in `-1..1` carried by the last decoded frame for `track`; empty when the frame has none,
    /// and taken: a second call gives nothing until the next frame is decoded.
    pub fn audio(&mut self, track: usize) -> Vec<f32> {
        self.audio.take(track)
    }
}
