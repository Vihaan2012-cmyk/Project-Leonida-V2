//! Where a movie's bytes come from, and the loader that reads the rest of a stream on the decode thread.

use std::io::Read;
use std::sync::Arc;

use super::decoder::{DecodeError, DecoderFactory};
use super::worker::Loader;

pub trait ByteSource: Send {
    /// Fills as much of `buf` as the source has left; fewer bytes than asked for means the end.
    fn read(&mut self, buf: &mut [u8]) -> usize;
}

pub fn read_fully(source: &mut dyn ByteSource, buf: &mut [u8]) -> usize {
    let mut done = 0;
    while done < buf.len() {
        let n = source.read(&mut buf[done..]);
        if n == 0 {
            break;
        }
        done += n;
    }
    done
}

pub struct FileSource(pub std::fs::File);

impl ByteSource for FileSource {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        self.0.read(buf).unwrap_or(0)
    }
}

/// A stream the caller holds in memory (`BinkOpen`'s from-memory flag), valid until the movie is closed.
pub struct MemorySource {
    ptr: usize,
    len: usize,
    pos: usize,
}

impl MemorySource {
    /// # Safety
    /// `ptr` must stay valid for reads of the whole stream, whose size the header gives, until this source is dropped.
    pub unsafe fn new(ptr: *const u8, len: usize) -> MemorySource {
        MemorySource {
            ptr: ptr as usize,
            len,
            pos: 0,
        }
    }
}

impl ByteSource for MemorySource {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.len - self.pos);
        // SAFETY: `new`'s guarantee covers `len` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping((self.ptr + self.pos) as *const u8, buf.as_mut_ptr(), n)
        };
        self.pos += n;
        n
    }
}

const CHUNK: usize = 1 << 20;

/// The most a stream may claim to be.
const MAX_STREAM: u64 = 1 << 32;

/// A loader that reads the rest of the stream after `prefix` (the bytes already read) and gives it to the
/// decoder. The source is dropped, and so closed, as soon as it has been read.
pub fn loader(
    mut source: Box<dyn ByteSource>,
    prefix: Vec<u8>,
    total: u64,
    factory: DecoderFactory,
) -> Loader {
    Box::new(move || {
        if total < prefix.len() as u64 || total > MAX_STREAM {
            return Err(DecodeError(format!(
                "a stream of {total} bytes is not plausible"
            )));
        }
        let mut bytes = prefix;
        let start = bytes.len();
        bytes.resize(total as usize, 0);
        let mut at = start;
        while at < bytes.len() {
            let end = (at + CHUNK).min(bytes.len());
            let n = read_fully(source.as_mut(), &mut bytes[at..end]);
            at += n;
            if at < end {
                return Err(DecodeError(format!(
                    "the stream ends at {at} of {total} bytes"
                )));
            }
        }
        drop(source);
        factory(Arc::from(bytes))
    })
}

#[cfg(test)]
pub(crate) struct VecSource(pub Vec<u8>, pub usize);

#[cfg(test)]
impl ByteSource for VecSource {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.0.len() - self.1).min(7);
        buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
        self.1 += n;
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::decoder::MovieDecoder;
    use crate::media::testing::FakeDecoder;

    fn factory(bytes: Arc<[u8]>) -> Result<Box<dyn MovieDecoder>, DecodeError> {
        Ok(Box::new(FakeDecoder::new(
            bytes.len() as u32,
            16,
            16,
            0,
            false,
        )))
    }

    #[test]
    fn the_rest_of_a_stream_is_read_in_pieces_and_handed_to_the_decoder() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| i as u8).collect();
        let source = VecSource(data.clone(), 100);
        let load = loader(
            Box::new(source),
            data[..100].to_vec(),
            data.len() as u64,
            factory,
        );
        let decoder = load().unwrap();
        assert_eq!(
            decoder.info().frames,
            3_000_000,
            "the decoder saw every byte"
        );
    }

    #[test]
    fn a_stream_that_ends_early_or_claims_too_much_is_an_error() {
        let short = loader(
            Box::new(VecSource(vec![0; 50], 10)),
            vec![0; 10],
            100,
            factory,
        );
        assert!(short().is_err());
        let huge = loader(
            Box::new(VecSource(vec![], 0)),
            vec![0; 10],
            1 << 33,
            factory,
        );
        assert!(huge().is_err());
    }

    #[test]
    fn a_stream_in_memory_reads_up_to_its_length() {
        let data = [1u8, 2, 3, 4, 5];
        // SAFETY: `data` outlives the source.
        let mut source = unsafe { MemorySource::new(data.as_ptr(), 5) };
        let mut buf = [0u8; 4];
        assert_eq!(read_fully(&mut source, &mut buf), 4);
        assert_eq!(read_fully(&mut source, &mut buf), 1);
        assert_eq!(buf[0], 5);
    }
}
