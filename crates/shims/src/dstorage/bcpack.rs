//! BCPack decompression for DirectStorage requests.
//!
//! The console decompresses BCPack in the I/O path: a request whose compression format is a BCPack mode carries
//! the entropy-coded chunk as its source and expects tiled BCn blocks in the destination. The numeric
//! `DSTORAGE_COMPRESSION_FORMAT` values that stand for the seven `DSTORAGE_BCPACK_MODE`s are not measured, so the
//! caller registers each one with [`set_bcpack_format`]; the decoder is `assets::texture::bcpack`.

use super::engine::{self, Decompress, E_FAIL, E_NOTIMPL};
use crate::trace::note;
use assets::texture::bcpack::{self, BcPackCodec, DecodeOptions};

fn decode_mode<const MODE: u8>(input: &[u8], output: &mut [u8]) -> Result<(), i32> {
    let codec = BcPackCodec::from_chunk_codec(MODE).expect("modes 1..=7");
    match bcpack::decode_with(codec, input, output.len(), &DecodeOptions::from_env()) {
        Ok(decoded) => {
            output.copy_from_slice(&decoded.data);
            Ok(())
        }
        Err(error) => {
            note(format!(
                "DirectStorage BCPack {codec:?} request failed: {error}"
            ));
            Err(if error.is_missing_constants() {
                E_NOTIMPL
            } else {
                E_FAIL
            })
        }
    }
}

fn decompressor(mode: u8) -> Option<Decompress> {
    let function: Decompress = match mode {
        1 => decode_mode::<1>,
        2 => decode_mode::<2>,
        3 => decode_mode::<3>,
        4 => decode_mode::<4>,
        5 => decode_mode::<5>,
        6 => decode_mode::<6>,
        7 => decode_mode::<7>,
        _ => return None,
    };
    Some(function)
}

/// Makes requests with compression `format` decompress as BCPack `mode` (1 = BC1 ... 7 = BC7). Returns whether
/// `mode` names a BCPack mode.
pub fn set_bcpack_format(format: u8, mode: u8) -> bool {
    match decompressor(mode) {
        Some(function) => {
            engine::set_decompressor(format, function);
            true
        }
        None => false,
    }
}
