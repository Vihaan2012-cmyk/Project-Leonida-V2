//! BCPack: the console's entropy-coded preconditioner for BCn data.
//!
//! A chunk is one interleaved rANS lane array (12 lanes for BC1/BC3/BC4/BC5, 16 for BC6H/BC7) followed by a
//! block stage that turns the lane symbols into BCn blocks: endpoint deltas against an earlier block or a built-in
//! history entry, and index codebooks. [`decode`] runs both. Codecs whose block stage is not covered by the
//! measured facts return [`BcPackError::Unsupported`] so the caller keeps its placeholder; a chunk that needs a
//! built-in history entry that is only provisional, or not recovered, returns [`BcPackError::History`]. All BC7
//! history entries are provisional.

use thiserror::Error;

mod bc6h;
#[cfg(test)]
mod bc6h_real_tests;
#[cfg(test)]
mod bc6h_tests;
mod bc7;
mod bc7_history;
#[cfg(test)]
mod bc7_tests;
#[cfg(test)]
mod block_tests;
mod blocks;
mod bptc;
mod codebooks;
#[cfg(test)]
mod lane_tests;
mod lanes;
mod model;
#[cfg(test)]
mod real_tests;
#[cfg(test)]
mod survey_tests;
mod tables;
#[cfg(test)]
mod testenc;

pub use lanes::LaneError;

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

/// How much a missing built-in history entry is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryStatus {
    /// Fitted but not verified; usable only with [`DecodeOptions::allow_provisional`].
    Provisional,
    /// Not recovered at all.
    Unknown,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BcPackError {
    #[error("BCPack {codec:?}: decoded size {size} is not a whole number of blocks")]
    BadSize { codec: BcPackCodec, size: usize },
    #[error("BCPack stream is shorter than its 48-byte lane header")]
    Truncated,
    #[error("BCPack {codec:?} has no block stage in the measured facts")]
    Unsupported { codec: BcPackCodec },
    #[error("BCPack lane array: {0}")]
    Lanes(#[from] LaneError),
    #[error(
        "BCPack {codec:?} block {block}: lane-15 signature is outside the recovered table or unconfirmed"
    )]
    Signature { codec: BcPackCodec, block: usize },
    #[error(
        "BCPack {codec:?}: {blocks} blocks break the format's anchor or p-bit rules (first: block {block})"
    )]
    Inconsistent {
        codec: BcPackCodec,
        block: usize,
        blocks: usize,
    },
    #[error("BCPack {codec:?} lane {lane} is non-zero")]
    UnexpectedLane { codec: BcPackCodec, lane: usize },
    #[error("BCPack {codec:?} block {block} needs built-in history entry {entry} ({status:?})")]
    History {
        codec: BcPackCodec,
        block: usize,
        entry: u8,
        status: HistoryStatus,
    },
}

impl BcPackError {
    /// The codec of the chunk the error is about (BC1 for stream-level errors that name none).
    pub fn codec(&self) -> BcPackCodec {
        match self {
            BcPackError::BadSize { codec, .. }
            | BcPackError::Unsupported { codec }
            | BcPackError::UnexpectedLane { codec, .. }
            | BcPackError::Signature { codec, .. }
            | BcPackError::Inconsistent { codec, .. }
            | BcPackError::History { codec, .. } => *codec,
            BcPackError::Truncated | BcPackError::Lanes(_) => BcPackCodec::Bc1,
        }
    }

    /// True when the chunk is a valid stream that this decoder cannot finish for lack of recovered constants
    /// (including BC7 signatures and chunks outside what the self-contained BC7 stage covers), as opposed to a
    /// corrupt one.
    pub fn is_missing_constants(&self) -> bool {
        matches!(
            self,
            BcPackError::Unsupported { .. }
                | BcPackError::History { .. }
                | BcPackError::Signature { .. }
                | BcPackError::Inconsistent { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeOptions {
    /// Use built-in history entries upstream marks provisional (fitted, not verified). Off in `Default`.
    pub allow_provisional: bool,
}

impl DecodeOptions {
    /// The options the runtime uses: provisional entries are on, since real textures decoded with them agree with
    /// their next mip; `LEONIDA_BCPACK_PROVISIONAL=0` turns them off.
    pub fn from_env() -> DecodeOptions {
        static ALLOW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let allow = *ALLOW
            .get_or_init(|| std::env::var("LEONIDA_BCPACK_PROVISIONAL").map_or(true, |v| v != "0"));
        DecodeOptions {
            allow_provisional: allow,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    pub data: Vec<u8>,
    /// Blocks whose endpoints trace back to a provisional history entry.
    pub provisional_blocks: usize,
}

/// Whether [`decode`] has a block stage for this codec (a given chunk can still need missing history).
pub fn is_supported(codec: BcPackCodec) -> bool {
    matches!(
        codec,
        BcPackCodec::Bc1
            | BcPackCodec::Bc3
            | BcPackCodec::Bc4
            | BcPackCodec::Bc5
            | BcPackCodec::Bc7
    )
}

/// Decode one BCPack chunk into `out_size` bytes of tiled BCn blocks, with provisional entries refused.
pub fn decode(codec: BcPackCodec, packed: &[u8], out_size: usize) -> Result<Vec<u8>, BcPackError> {
    decode_with(codec, packed, out_size, &DecodeOptions::default()).map(|d| d.data)
}

pub fn decode_with(
    codec: BcPackCodec,
    packed: &[u8],
    out_size: usize,
    opts: &DecodeOptions,
) -> Result<Decoded, BcPackError> {
    if out_size == 0 || !out_size.is_multiple_of(codec.block_bytes()) {
        return Err(BcPackError::BadSize {
            codec,
            size: out_size,
        });
    }
    if packed.len() < 48 {
        return Err(BcPackError::Truncated);
    }
    let stage = match codec {
        BcPackCodec::Bc1 => blocks::bc1,
        BcPackCodec::Bc3 => blocks::bc3,
        BcPackCodec::Bc4 => blocks::bc4,
        BcPackCodec::Bc5 => blocks::bc5,
        BcPackCodec::Bc6h if bc6h::has_history() => bc6h::bc6h,
        BcPackCodec::Bc7 => bc7::bc7,
        _ => return Err(BcPackError::Unsupported { codec }),
    };
    let widths = codec.lane_widths().expect("supported codecs have lanes");
    let symbols = lanes::decode_slice(packed, widths, out_size / 8)?;
    let mut data = vec![0u8; out_size];
    let provisional_blocks = stage(&symbols, &mut data, opts)?;
    Ok(Decoded {
        data,
        provisional_blocks,
    })
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
    fn codecs_without_a_block_stage_stay_unsupported() {
        assert_eq!(
            decode(BcPackCodec::Bc2, &[0; 64], 4096),
            Err(BcPackError::Unsupported {
                codec: BcPackCodec::Bc2
            })
        );
        assert!(matches!(
            decode(BcPackCodec::Bc1, &[0; 64], 100),
            Err(BcPackError::BadSize { .. })
        ));
        assert_eq!(
            decode(BcPackCodec::Bc1, &[0; 10], 4096),
            Err(BcPackError::Truncated)
        );
    }
}
