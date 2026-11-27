//! Pure-Rust decoder for Bink 2 movies (`KB2` streams) and their Bink audio tracks.

mod bits;
mod container;
mod error;
mod tables;
mod video;
mod vlc;

use std::sync::Arc;

pub use container::{AudioTrack, Container, Header};
pub use error::{Error, Result};
pub use video::{Frame, Plane};

use video::VideoDecoder;

pub struct Movie {
    container: Container,
    video: VideoDecoder,
    next: u32,
    current: Option<Frame>,
    audio: Vec<Vec<f32>>,
}

impl Movie {
    pub fn open(data: Arc<[u8]>) -> Result<Movie> {
        let container = Container::parse(data)?;
        let video = VideoDecoder::new(&container.header)?;
        let tracks = container.header.audio.len();
        Ok(Movie {
            container,
            video,
            next: 0,
            current: None,
            audio: vec![Vec::new(); tracks],
        })
    }

    pub fn header(&self) -> &Header {
        &self.container.header
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
        Ok(())
    }

    /// Interleaved samples carried by the last decoded frame for `track`. Audio decoding is not implemented, so
    /// the result is always empty.
    pub fn audio(&mut self, track: usize) -> Vec<f32> {
        self.audio
            .get_mut(track)
            .map(std::mem::take)
            .unwrap_or_default()
    }
}
