//! The `bink` crate behind the shim's decoder trait: what plays when no other decoder is installed.

use std::sync::Arc;

use bink::Movie;

use super::decoder::{AudioTrackInfo, DecodeError, DecodedFrame, MovieDecoder, Plane, StreamInfo};

pub struct BinkDecoder {
    data: Arc<[u8]>,
    movie: Movie,
    info: StreamInfo,
}

fn failed(error: bink::Error) -> DecodeError {
    DecodeError(error.to_string())
}

pub fn open(data: Arc<[u8]>) -> Result<Box<dyn MovieDecoder>, DecodeError> {
    let movie = Movie::open(data.clone()).map_err(failed)?;
    let header = movie.header();
    let info = StreamInfo {
        width: header.width,
        height: header.height,
        frames: header.frames,
        fps_num: header.fps_num,
        fps_den: header.fps_den,
        has_alpha: header.has_alpha,
        audio: header
            .audio
            .iter()
            .map(|t| AudioTrackInfo {
                rate: t.rate,
                channels: t.channels as u8,
                id: t.id,
            })
            .collect(),
    };
    Ok(Box::new(BinkDecoder { data, movie, info }))
}

impl MovieDecoder for BinkDecoder {
    fn info(&self) -> StreamInfo {
        self.info.clone()
    }

    fn decode_next(&mut self) -> Result<DecodedFrame, DecodeError> {
        let frame = self.movie.decode_next().map_err(failed)?;
        Ok(DecodedFrame {
            index: frame.index,
            planes: frame
                .planes
                .iter()
                .map(|p| Plane {
                    data: p.data.clone(),
                    stride: p.stride,
                    width: p.width as usize,
                    height: p.height as usize,
                })
                .collect(),
        })
    }

    /// `Movie::seek` decodes up to and including its frame, so the one before is asked for; the first frame has
    /// none before it and the movie is opened afresh.
    fn seek(&mut self, frame: u32) -> Result<(), DecodeError> {
        match frame.checked_sub(1) {
            Some(before) => self.movie.seek(before).map_err(failed),
            None => {
                self.movie = Movie::open(self.data.clone()).map_err(failed)?;
                Ok(())
            }
        }
    }

    fn audio(&mut self, track: usize) -> Vec<f32> {
        self.movie.audio(track)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::testing::stream_header;

    /// A stream of `frames` empty frames behind a header with `tracks` audio tracks.
    fn stream(frames: u32, tracks: u32) -> Arc<[u8]> {
        let mut bytes = stream_header(frames, 16, 16, tracks);
        let index = bytes.len();
        bytes.resize(index + 4 * frames as usize, 0);
        for f in 0..frames as usize {
            let at = bytes.len() as u32;
            bytes[index + 4 * f..index + 4 * f + 4].copy_from_slice(&(at | 1).to_le_bytes());
            bytes.extend(std::iter::repeat_n(0u8, 4 * tracks as usize));
        }
        bytes.into()
    }

    #[test]
    fn the_stream_info_comes_from_the_movies_header() {
        let decoder = open(stream(5, 2)).unwrap();
        let info = decoder.info();
        assert_eq!((info.width, info.height, info.frames), (16, 16, 5));
        assert_eq!((info.fps_num, info.fps_den), (30, 1));
        assert!(!info.has_alpha);
        let tracks: Vec<_> = info
            .audio
            .iter()
            .map(|t| (t.rate, t.channels, t.id))
            .collect();
        assert_eq!(tracks, [(48000, 2, 0), (48000, 2, 3)]);
    }

    #[test]
    fn a_stream_that_is_not_bink_is_an_error_and_audio_of_nothing_is_empty() {
        assert!(open(Arc::from(vec![0u8; 64])).is_err());
        let mut decoder = open(stream(3, 1)).unwrap();
        assert!(decoder.audio(0).is_empty());
        assert!(decoder.audio(9).is_empty());
    }

    #[test]
    fn seeking_to_the_first_frame_reopens_the_movie() {
        let mut decoder = open(stream(3, 1)).unwrap();
        decoder.seek(0).unwrap();
        assert!(decoder.seek(9).is_err(), "past the last frame");
    }
}
