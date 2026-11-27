//! BCPack: the console's entropy-coded preconditioner for BCn data.
//!
//! What the measured documentation gives is the container side (chunk codec ids, decoded sizes,
//! the lane layout per codec and the fragment header). It does not give the rANS probability
//! tables, the index codebooks or the built-in endpoint histories a decoder needs, so
//! [`decode`] returns [`BcPackError::Unsupported`] and the caller substitutes a placeholder.
//! Nothing here guesses block data.

use thiserror::Error;

#[cfg(test)]
mod lane_tests;
mod lanes;
mod model;
mod tables;
#[cfg(test)]
mod testenc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BcPackCodec {
    Bc1 = 1,
    Bc2 = 2,
    Bc3 = 3,
    Bc4 = 4,
    Bc5 = 5,
    Bc6h = 6,
    Bc7 = 7,
}

impl BcPackCodec {
    /// Chunk codec ids 1..=7 are BCPack modes; 0 is raw and 8 is zlib.
    pub fn from_chunk_codec(codec: u8) -> Option<BcPackCodec> {
        Some(match codec {
            1 => BcPackCodec::Bc1,
            2 => BcPackCodec::Bc2,
            3 => BcPackCodec::Bc3,
            4 => BcPackCodec::Bc4,
            5 => BcPackCodec::Bc5,
            6 => BcPackCodec::Bc6h,
            7 => BcPackCodec::Bc7,
            _ => return None,
        })
    }

    pub const fn block_bytes(self) -> usize {
        match self {
            BcPackCodec::Bc1 | BcPackCodec::Bc4 => 8,
            _ => 16,
        }
    }

    /// Interleaved rANS lanes in the stream.
    pub const fn lane_count(self) -> usize {
        match self {
            BcPackCodec::Bc6h | BcPackCodec::Bc7 => 16,
            _ => 12,
        }
    }

    /// Lane elements per block: 16-byte formats consume two.
    pub const fn elements_per_block(self) -> usize {
        self.block_bytes() / 8
    }

    /// Default symbol width of each lane, in bits. BC2 has no measured layout.
    pub fn lane_widths(self) -> Option<&'static [u8]> {
        Some(match self {
            BcPackCodec::Bc1 => &[8, 8, 5, 6, 5, 5, 6, 5, 8, 8, 8, 8],
            BcPackCodec::Bc3 => &[4, 4, 8, 6, 6, 6, 6, 5, 6, 5, 8, 8],
            BcPackCodec::Bc4 => &[8, 8, 8, 8, 6, 6, 6, 6, 6, 6, 6, 6],
            BcPackCodec::Bc5 => &[4, 4, 8, 8, 6, 6, 6, 6, 6, 6, 6, 6],
            BcPackCodec::Bc6h => &[4, 4, 8, 8, 8, 5, 5, 5, 6, 6, 6, 8, 8, 8, 8, 5],
            BcPackCodec::Bc7 => &[4, 4, 8, 8, 8, 8, 8, 7, 7, 5, 6, 8, 8, 8, 8, 5],
            BcPackCodec::Bc2 => return None,
        })
    }
}

/// First byte of a lane fragment: `[z:2][long length form:1][flush:1][q:4]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentHeader {
    /// Symbol width reduction: the width is the lane default minus `z`.
    pub z: u8,
    pub flush: bool,
    /// Probability model 0..=15 (0 uniform, 15 concentrated on symbol 0).
    pub q: u8,
    /// First-fragment payload budget in bytes.
    pub budget: u32,
    /// Header size: 2 bytes for the short form, 3 for the long form.
    pub len: usize,
}

impl FragmentHeader {
    pub fn parse(bytes: &[u8]) -> Option<FragmentHeader> {
        let h0 = *bytes.first()?;
        let long = h0 & 0x20 != 0;
        let (budget, len) = if long {
            let (lo, hi) = (u32::from(*bytes.get(1)?), u32::from(*bytes.get(2)?));
            (lo | hi << 8, 3)
        } else {
            (u32::from(*bytes.get(1)?), 2)
        };
        Some(FragmentHeader {
            z: h0 >> 6,
            flush: h0 & 0x10 != 0,
            q: h0 & 0x0F,
            budget: if long { budget + 257 } else { budget + 1 },
            len,
        })
    }
}

/// Banded exponential model: symbol `s` of a lane belongs to band 0 when `s == 0`, otherwise to
/// band `bit_length(s)`, so band `p >= 1` covers `[2^(p-1), 2^p)`.
pub fn symbol_band(symbol: u32) -> u32 {
    32 - symbol.leading_zeros()
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BcPackError {
    #[error("BCPack {codec:?}: decoded size {size} is not a whole number of blocks")]
    BadSize { codec: BcPackCodec, size: usize },
    #[error("BCPack stream is shorter than its 48-byte lane header")]
    Truncated,
    #[error(
        "BCPack {codec:?} decoding needs probability tables and codebooks the measured documentation does not provide"
    )]
    Unsupported { codec: BcPackCodec },
}

/// Whether [`decode`] can produce data for this codec.
pub fn is_supported(_codec: BcPackCodec) -> bool {
    false
}

/// Decode one BCPack chunk into `out_size` bytes of tiled BCn blocks.
pub fn decode(codec: BcPackCodec, packed: &[u8], out_size: usize) -> Result<Vec<u8>, BcPackError> {
    if out_size == 0 || !out_size.is_multiple_of(codec.block_bytes()) {
        return Err(BcPackError::BadSize {
            codec,
            size: out_size,
        });
    }
    if packed.len() < 48 {
        return Err(BcPackError::Truncated);
    }
    Err(BcPackError::Unsupported { codec })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_codec_ids_map_to_modes() {
        assert_eq!(BcPackCodec::from_chunk_codec(1), Some(BcPackCodec::Bc1));
        assert_eq!(BcPackCodec::from_chunk_codec(7), Some(BcPackCodec::Bc7));
        assert_eq!(BcPackCodec::from_chunk_codec(0), None);
        assert_eq!(BcPackCodec::from_chunk_codec(8), None);
    }

    #[test]
    fn lane_tables_match_lane_counts() {
        for c in [
            BcPackCodec::Bc1,
            BcPackCodec::Bc3,
            BcPackCodec::Bc4,
            BcPackCodec::Bc5,
            BcPackCodec::Bc6h,
            BcPackCodec::Bc7,
        ] {
            assert_eq!(c.lane_widths().unwrap().len(), c.lane_count());
        }
        assert!(BcPackCodec::Bc2.lane_widths().is_none());
    }

    #[test]
    fn short_and_long_fragment_headers() {
        let h = FragmentHeader::parse(&[0x1f, 0xde]).unwrap();
        assert_eq!(
            (h.z, h.flush, h.q, h.budget, h.len),
            (0, true, 15, 0xde + 1, 2)
        );
        let h = FragmentHeader::parse(&[0x35, 0x10, 0x01]).unwrap();
        assert_eq!(
            (h.z, h.flush, h.q, h.budget, h.len),
            (0, true, 5, 0x110 + 257, 3)
        );
        let h = FragmentHeader::parse(&[0xc0, 0x00]).unwrap();
        assert_eq!((h.z, h.flush, h.q, h.budget), (3, false, 0, 1));
        assert!(FragmentHeader::parse(&[0x20, 1]).is_none());
        assert!(FragmentHeader::parse(&[]).is_none());
    }

    #[test]
    fn bands() {
        assert_eq!(symbol_band(0), 0);
        assert_eq!(symbol_band(1), 1);
        assert_eq!(symbol_band(2), 2);
        assert_eq!(symbol_band(3), 2);
        assert_eq!(symbol_band(255), 8);
    }

    #[test]
    fn decode_is_unsupported_not_guessed() {
        assert_eq!(
            decode(BcPackCodec::Bc1, &[0; 64], 4096),
            Err(BcPackError::Unsupported {
                codec: BcPackCodec::Bc1
            })
        );
        assert!(matches!(
            decode(BcPackCodec::Bc7, &[0; 64], 100),
            Err(BcPackError::BadSize { .. })
        ));
        assert_eq!(
            decode(BcPackCodec::Bc1, &[0; 10], 4096),
            Err(BcPackError::Truncated)
        );
    }
}
