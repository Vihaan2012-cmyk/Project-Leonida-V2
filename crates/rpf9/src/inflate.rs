use flate2::{Decompress, FlushDecompress, Status};

use crate::model::Codec;

/// Largest inflated size accepted for one stream. The largest measured stream is 32 MiB.
pub(crate) const MAX_STREAM: usize = 256 << 20;

/// Reusable zlib/deflate decoder.
pub(crate) struct Inflater {
    zlib: Decompress,
    deflate: Decompress,
}

impl Default for Inflater {
    fn default() -> Self {
        Inflater {
            zlib: Decompress::new(true),
            deflate: Decompress::new(false),
        }
    }
}

pub(crate) struct Inflated {
    /// Stored bytes the stream used, its checksum included.
    pub consumed: usize,
}

impl Inflater {
    /// Inflate one stream that starts `input` into `out` (cleared first). `input` may run on past the end of the
    /// stream. Errors on a corrupt or truncated stream, a failed checksum, or output beyond `limit`.
    pub(crate) fn inflate(
        &mut self,
        input: &[u8],
        zlib: bool,
        limit: usize,
        out: &mut Vec<u8>,
    ) -> Result<Inflated, String> {
        let decoder = if zlib {
            &mut self.zlib
        } else {
            &mut self.deflate
        };
        decoder.reset(zlib);
        out.clear();
        loop {
            if out.len() > limit {
                return Err(format!("more than {limit} bytes inflated"));
            }
            if out.capacity() == out.len() {
                if out.len() == limit {
                    return Err(format!("more than {limit} bytes inflated"));
                }
                out.reserve((out.len().max(1 << 16)).min(limit - out.len()));
            }
            let consumed = decoder.total_in() as usize;
            let before = (decoder.total_in(), decoder.total_out());
            let status = decoder
                .decompress_vec(
                    &input[consumed.min(input.len())..],
                    out,
                    FlushDecompress::None,
                )
                .map_err(|error| error.to_string())?;
            match status {
                Status::StreamEnd => {
                    return Ok(Inflated {
                        consumed: decoder.total_in() as usize,
                    });
                }
                Status::Ok => {}
                Status::BufError => {
                    let progressed = (decoder.total_in(), decoder.total_out()) != before;
                    if !progressed && out.capacity() > out.len() {
                        return Err("truncated stream".into());
                    }
                }
            }
        }
    }
}

/// Decode `stored` into exactly `size` bytes with `codec`.
pub(crate) fn decode(
    inflater: &mut Inflater,
    codec: Codec,
    stored: &[u8],
    size: usize,
) -> Result<Vec<u8>, String> {
    match codec {
        Codec::Raw => Ok(stored.to_vec()),
        Codec::Zlib | Codec::Deflate => {
            let mut out = Vec::with_capacity(size + 1);
            inflater.inflate(stored, codec == Codec::Zlib, size + 1, &mut out)?;
            Ok(out)
        }
        Codec::Zstd => {
            use std::io::Read as _;
            let mut decoder = ruzstd::decoding::StreamingDecoder::new(stored)
                .map_err(|error| error.to_string())?;
            let mut out = Vec::with_capacity(size);
            decoder
                .by_ref()
                .take(size as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|error| error.to_string())?;
            Ok(out)
        }
    }
}
