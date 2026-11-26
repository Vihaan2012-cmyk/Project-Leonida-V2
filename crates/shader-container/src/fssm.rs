use crate::ContainerError;
use crate::dxbc::{u16_at, u32_at};

/// `s_endpgm`, 0xBF810000 little-endian.
pub const ENDPGM: [u8; 4] = [0x00, 0x00, 0x81, 0xBF];

const CODE_OFFSET_FIELD: usize = 0xC0;
const CODE_START_BIAS: usize = 0x28;
const WAVE_FLAGS: usize = 0xD4;
const FLAG_WAVE32: u32 = 0x20;
const MIN_LEN: usize = 0xD8;
const MAX_DATA_DWORDS: usize = 1 << 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WaveSize {
    Wave32,
    Wave64,
}

/// Where a blob's instructions are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRange {
    /// From the header's code offset: instructions in `start..end`.
    Header {
        start: usize,
        end: usize,
    },
    /// The header offset did not lead to code; only the end of the first aligned s_endpgm is known.
    Scan {
        end: usize,
    },
    NotFound,
}

/// A precompiled RDNA2 program with its metadata, as unpacked from XBCH.
#[derive(Debug, Clone, Copy)]
pub struct Fssm<'a> {
    bytes: &'a [u8],
}

impl<'a> Fssm<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ContainerError> {
        if bytes.len() < MIN_LEN {
            return Err(ContainerError::Truncated("FSSM header"));
        }
        if &bytes[..4] != b"FSSM" {
            return Err(ContainerError::Invalid("not an FSSM blob".into()));
        }
        Ok(Self { bytes })
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn version(&self) -> u16 {
        u16_at(self.bytes, 4)
    }

    /// Bit 5 of the u32 at 0xD4: set for wave32, clear for wave64.
    pub fn wave_size(&self) -> WaveSize {
        if u32_at(self.bytes, WAVE_FLAGS) & FLAG_WAVE32 != 0 {
            WaveSize::Wave32
        } else {
            WaveSize::Wave64
        }
    }

    /// A compute shader's COMPUTE_PGM_RSRC2 (u32 at 0x138) and thread-group size (u32s at 0x13C, 0x140, 0x144).
    pub fn compute_setup(&self) -> Option<(u32, [u32; 3])> {
        let word = |at: usize| {
            self.bytes
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
        };
        Some((word(0x138)?, [word(0x13C)?, word(0x140)?, word(0x144)?]))
    }

    pub fn code_range(&self) -> CodeRange {
        code_range(self.bytes)
    }
}

/// The code section offset is the u32 at 0xC0; instructions start 0x28 bytes into it and run to the last
/// s_endpgm aligned to the start, minus a trailing constant-data block. Without a usable offset, only the first
/// 4-byte-aligned s_endpgm is known.
fn code_range(b: &[u8]) -> CodeRange {
    let off = u32_at(b, CODE_OFFSET_FIELD) as usize;
    let start = off + CODE_START_BIAS;
    if off > 0 && start < b.len() {
        let last = b[start..]
            .chunks_exact(4)
            .rposition(|word| word == ENDPGM.as_slice());
        if let Some(index) = last {
            let end = start + 4 * index + 4;
            return CodeRange::Header {
                start,
                end: data_block_start(b, start, end),
            };
        }
    }
    match b.chunks_exact(4).position(|word| word == ENDPGM.as_slice()) {
        Some(index) => CodeRange::Scan { end: 4 * index + 4 },
        None => CodeRange::NotFound,
    }
}

/// Where a constant-data block begins at the end of `start..end`, or `end` if there is none. Such a block is
/// s_endpgm, a dword count N, N data dwords, then the closing s_endpgm the range ends on. It is recognised only
/// when N matches the distance to the closing s_endpgm and the dword before the count is s_endpgm.
fn data_block_start(b: &[u8], start: usize, end: usize) -> usize {
    let last = end - 4;
    let dwords = (last - start) / 4;
    if dwords == 0 {
        return end;
    }
    for n in 0..=MAX_DATA_DWORDS.min(dwords - 1) {
        let count = last - 4 * (n + 1);
        if count < start + 4 {
            break;
        }
        if u32_at(b, count) as usize == n && b[count - 4..count] == ENDPGM {
            return count;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fssm;

    const NOP: u32 = 0xBF80_0000; // s_nop 0
    const END: u32 = 0xBF81_0000; // s_endpgm

    #[test]
    fn wave_size_from_flag() {
        assert_eq!(
            Fssm::parse(&fssm(true, &[NOP])).unwrap().wave_size(),
            WaveSize::Wave32
        );
        assert_eq!(
            Fssm::parse(&fssm(false, &[NOP])).unwrap().wave_size(),
            WaveSize::Wave64
        );
        assert_eq!(Fssm::parse(&fssm(false, &[NOP])).unwrap().version(), 1);
    }

    #[test]
    fn code_range_from_header() {
        // NOP at 0x128, NOP at 0x12C, s_endpgm at 0x130
        let blob = fssm(false, &[NOP, NOP]);
        assert_eq!(
            Fssm::parse(&blob).unwrap().code_range(),
            CodeRange::Header {
                start: 0x128,
                end: 0x134
            }
        );
    }

    #[test]
    fn trailing_constant_data_is_cut() {
        // 0x128 NOP, 0x12C s_endpgm, 0x130 count 2, 0x134 data, 0x138 data, 0x13C s_endpgm
        let blob = fssm(false, &[NOP, END, 2, 0xBEEF, 0xDEAD]);
        assert_eq!(
            Fssm::parse(&blob).unwrap().code_range(),
            CodeRange::Header {
                start: 0x128,
                end: 0x130
            }
        );
    }

    #[test]
    fn falls_back_to_first_aligned_endpgm() {
        let mut blob = fssm(false, &[NOP, NOP]);
        blob[0xC0..0xC4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            Fssm::parse(&blob).unwrap().code_range(),
            CodeRange::Scan { end: 0x134 }
        );
    }

    #[test]
    fn no_endpgm_anywhere() {
        let mut blob = vec![0u8; 0xD8];
        blob[..4].copy_from_slice(b"FSSM");
        assert_eq!(
            Fssm::parse(&blob).unwrap().code_range(),
            CodeRange::NotFound
        );
    }

    #[test]
    fn rejects_short_or_wrong_blobs() {
        assert_eq!(
            Fssm::parse(b"FSSM").unwrap_err(),
            ContainerError::Truncated("FSSM header")
        );
        let mut blob = fssm(false, &[NOP]);
        blob[0] = b'X';
        assert!(matches!(
            Fssm::parse(&blob),
            Err(ContainerError::Invalid(_))
        ));
    }
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    use crate::testutil::fssm;

    #[test]
    fn compute_setup_reads_rsrc2_and_group_size() {
        let mut blob = fssm(true, &[0xBF81_0000]);
        blob.resize(blob.len().max(0x148), 0);
        for (at, value) in [(0x138, 0x998u32), (0x13C, 32), (0x140, 4), (0x144, 1)] {
            blob[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let parsed = Fssm::parse(&blob).unwrap();
        assert_eq!(parsed.compute_setup(), Some((0x998, [32, 4, 1])));
        let short = fssm(true, &[0xBF81_0000]);
        if short.len() < 0x148 {
            assert_eq!(Fssm::parse(&short).unwrap().compute_setup(), None);
        }
    }
}
