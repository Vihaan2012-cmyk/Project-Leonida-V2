//! Walk the data area of an archive without its encrypted table of contents: zlib streams, Bink 2 videos and AWC
//! banks lie end to end, with zero padding and unreadable bytes between them.

use crate::inflate::{Inflater, MAX_STREAM};

/// One piece of the data area, in archive order.
pub(crate) enum Piece<'a> {
    /// A zlib stream that inflated cleanly (checksum included); `data` is the whole inflated stream.
    Zlib {
        offset: u64,
        stored: u32,
        data: &'a [u8],
    },
    /// A validated Bink 2 video, stored as is.
    Bink2 { offset: u64, bytes: &'a [u8] },
    /// A validated AWC bank, stored as is.
    Awc { offset: u64, bytes: &'a [u8] },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkStats {
    pub streams: u64,
    pub stored_bytes: u64,
    pub inflated_bytes: u64,
    pub bink2: u64,
    pub awc: u64,
    /// Bytes between pieces that are all zero.
    pub zero_padding: u64,
    /// Other bytes between pieces, in `unread_ranges` runs.
    pub unread_bytes: u64,
    pub unread_ranges: u64,
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

pub(crate) fn is_zlib_header(first: u8, second: u8) -> bool {
    first == 0x78 && matches!(second, 0x01 | 0x5E | 0x9C | 0xDA)
}

/// Length of the Bink 2 video at the start of `bytes`. The stored size is the field at +4 plus 8; the frame count
/// is repeated at +0x10 and the picture is 16 to 8192 pixels each way.
pub(crate) fn bink2_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 0x2C || &bytes[..3] != b"KB2" || !bytes[3].is_ascii_lowercase() {
        return None;
    }
    let stored = u64::from(u32_at(bytes, 4)) + 8;
    let frames = u32_at(bytes, 8);
    let (width, height) = (u32_at(bytes, 0x14), u32_at(bytes, 0x18));
    let sane = (64..=bytes.len() as u64).contains(&stored)
        && frames > 0
        && frames == u32_at(bytes, 0x10)
        && (16..=8192).contains(&width)
        && (16..=8192).contains(&height);
    sane.then_some(stored as usize)
}

/// Length of the AWC (`ADAT`) bank at the start of `bytes`: the header, or the end of the furthest tag. Every
/// stream's tags must lie inside the header. A bank without streams must fill its header exactly.
pub(crate) fn awc_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 16 || &bytes[..4] != b"ADAT" {
        return None;
    }
    let flags = u32_at(bytes, 4);
    let count = u32_at(bytes, 8) as usize;
    let header = u32_at(bytes, 12) as usize;
    if count > 1 << 22 || !(16..=bytes.len()).contains(&header) || header > 256 << 20 {
        return None;
    }
    let mut at = 16;
    if flags & 0x0040_0000 != 0 {
        at += 4;
    }
    if flags & 0x0001_0000 != 0 {
        at += count * 2;
    }
    if flags & 0x0020_0000 != 0 {
        at += (count + 1) * 4;
    }
    let table = at;
    at += count * 4;
    if at > header {
        return None;
    }
    let mut end = header;
    for stream in 0..count {
        let tags = (u32_at(bytes, table + stream * 4) >> 29) as usize;
        for _ in 0..tags {
            if at + 8 > header {
                return None;
            }
            let tag = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
            at += 8;
            let size = ((tag >> 28) & 0x0FFF_FFFF) as usize;
            let offset = (tag & 0x0FFF_FFFF) as usize;
            if offset + size > bytes.len() {
                return None;
            }
            end = end.max(offset + size);
        }
    }
    (count > 0 || at == header).then_some(end)
}

fn account_gap(data: &[u8], from: usize, to: usize, stats: &mut WalkStats) {
    if to <= from {
        return;
    }
    let len = (to - from) as u64;
    if data[from..to].iter().all(|&byte| byte == 0) {
        stats.zero_padding += len;
    } else {
        stats.unread_bytes += len;
        stats.unread_ranges += 1;
    }
}

/// Walk `data[start..end]`, handing each piece to `sink` in archive order.
pub(crate) fn walk(
    data: &[u8],
    start: usize,
    end: usize,
    sink: &mut dyn FnMut(Piece<'_>),
) -> WalkStats {
    let mut stats = WalkStats::default();
    let mut inflater = Inflater::default();
    let mut inflated = Vec::new();
    let mut pos = start;
    let mut gap_start = start;
    while pos < end {
        let Some(found) = memchr::memchr3(0x78, b'K', b'A', &data[pos..end]) else {
            break;
        };
        let at = pos + found;
        let region = &data[at..end];
        pos = at + 1;
        match region[0] {
            0x78 if region.len() > 2 && is_zlib_header(region[0], region[1]) => {
                let Ok(done) = inflater.inflate(region, true, MAX_STREAM, &mut inflated) else {
                    continue;
                };
                account_gap(data, gap_start, at, &mut stats);
                stats.streams += 1;
                stats.stored_bytes += done.consumed as u64;
                stats.inflated_bytes += inflated.len() as u64;
                sink(Piece::Zlib {
                    offset: at as u64,
                    stored: done.consumed as u32,
                    data: &inflated,
                });
                pos = at + done.consumed;
            }
            b'K' => {
                let Some(len) = bink2_len(region) else {
                    continue;
                };
                account_gap(data, gap_start, at, &mut stats);
                stats.bink2 += 1;
                stats.stored_bytes += len as u64;
                sink(Piece::Bink2 {
                    offset: at as u64,
                    bytes: &region[..len],
                });
                pos = at + len;
            }
            b'A' => {
                let Some(len) = awc_len(region) else {
                    continue;
                };
                account_gap(data, gap_start, at, &mut stats);
                stats.awc += 1;
                stats.stored_bytes += len as u64;
                sink(Piece::Awc {
                    offset: at as u64,
                    bytes: &region[..len],
                });
                pos = at + len;
            }
            _ => continue,
        }
        gap_start = pos;
    }
    account_gap(data, gap_start, end, &mut stats);
    stats
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::io::Write as _;

    pub fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    pub fn deflate(data: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    pub fn bink2(frames: u32, total: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; total];
        bytes[..4].copy_from_slice(b"KB2n");
        bytes[4..8].copy_from_slice(&(total as u32 - 8).to_le_bytes());
        bytes[8..12].copy_from_slice(&frames.to_le_bytes());
        bytes[0x10..0x14].copy_from_slice(&frames.to_le_bytes());
        bytes[0x14..0x18].copy_from_slice(&1280u32.to_le_bytes());
        bytes[0x18..0x1C].copy_from_slice(&720u32.to_le_bytes());
        bytes
    }

    /// A bank with one stream carrying a `0x55` tag of `payload` bytes after a 2048-byte header.
    pub fn awc(payload: usize) -> Vec<u8> {
        let header = 2048usize;
        let mut bytes = vec![0u8; header + payload];
        bytes[..4].copy_from_slice(b"ADAT");
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&(header as u32).to_le_bytes());
        bytes[16..20].copy_from_slice(&((1u32 << 29) | 7).to_le_bytes());
        let tag = (0x55u64 << 56) | ((payload as u64) << 28) | header as u64;
        bytes[20..28].copy_from_slice(&tag.to_le_bytes());
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn collect(data: &[u8]) -> (Vec<(String, u64, usize)>, WalkStats) {
        let mut found = Vec::new();
        let stats = walk(data, 0, data.len(), &mut |piece| match piece {
            Piece::Zlib { offset, data, .. } => found.push(("zlib".to_string(), offset, data.len())),
            Piece::Bink2 { offset, bytes } => found.push(("bink2".to_string(), offset, bytes.len())),
            Piece::Awc { offset, bytes } => found.push(("awc".to_string(), offset, bytes.len())),
        });
        (found, stats)
    }

    #[test]
    fn finds_streams_between_padding_and_junk() {
        let a = zlib(&[7u8; 5000]);
        let b = zlib(b"second stream");
        let mut data = vec![0u8; 64];
        let first = data.len();
        data.extend_from_slice(&a);
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(b"junk\x78\x00junk"); // a false zlib start
        let second = data.len();
        data.extend_from_slice(&b);
        let (found, stats) = collect(&data);
        assert_eq!(
            found,
            vec![
                ("zlib".to_string(), first as u64, 5000),
                ("zlib".to_string(), second as u64, 13),
            ]
        );
        assert_eq!(stats.streams, 2);
        assert_eq!(stats.zero_padding, 64);
        assert_eq!(stats.unread_bytes, 12 + 10);
        assert_eq!(stats.unread_ranges, 1);
        assert_eq!(stats.stored_bytes, (a.len() + b.len()) as u64);
    }

    #[test]
    fn a_corrupt_checksum_is_not_a_stream() {
        let mut stream = zlib(&[1u8; 300]);
        let last = stream.len() - 1;
        stream[last] ^= 0xFF;
        let (found, stats) = collect(&stream);
        assert!(found.is_empty());
        assert_eq!(stats.streams, 0);
        assert_eq!(stats.unread_bytes, stream.len() as u64);
    }

    #[test]
    fn a_truncated_stream_is_not_a_stream() {
        let stream = zlib(&(0..9000u32).flat_map(u32::to_le_bytes).collect::<Vec<_>>());
        let (found, _) = collect(&stream[..stream.len() - 6]);
        assert!(found.is_empty());
    }

    #[test]
    fn finds_bink2_and_awc_between_streams() {
        let video = bink2(30, 400);
        let bank = awc(5000);
        let mut data = zlib(b"head");
        let video_at = data.len();
        data.extend_from_slice(&video);
        let bank_at = data.len();
        data.extend_from_slice(&bank);
        data.extend_from_slice(&zlib(b"tail"));
        let (found, stats) = collect(&data);
        let kinds: Vec<_> = found.iter().map(|(kind, _, _)| kind.as_str()).collect();
        assert_eq!(kinds, ["zlib", "bink2", "awc", "zlib"]);
        assert_eq!(found[1], ("bink2".to_string(), video_at as u64, 400));
        assert_eq!(found[2], ("awc".to_string(), bank_at as u64, 2048 + 5000));
        assert_eq!((stats.bink2, stats.awc, stats.unread_bytes), (1, 1, 0));
    }

    #[test]
    fn bink2_needs_a_sane_header() {
        let mut video = bink2(30, 400);
        video[0x14..0x18].copy_from_slice(&4u32.to_le_bytes());
        assert!(bink2_len(&video).is_none());
        let mut video = bink2(30, 400);
        video[0x10] ^= 1;
        assert!(bink2_len(&video).is_none());
        assert_eq!(bink2_len(&bink2(2, 100)), Some(100));
    }

    #[test]
    fn awc_zero_stream_bank_fills_its_header() {
        let mut bank = vec![0u8; 20];
        bank[..4].copy_from_slice(b"ADAT");
        bank[4..8].copy_from_slice(&0xff21_0001u32.to_le_bytes());
        bank[12..16].copy_from_slice(&20u32.to_le_bytes());
        assert_eq!(awc_len(&bank), Some(20));
        bank[12..16].copy_from_slice(&19u32.to_le_bytes());
        assert_eq!(awc_len(&bank), None);
    }

    #[test]
    fn awc_tags_must_stay_inside_the_bank() {
        let mut bank = awc(100);
        bank.truncate(2048 + 50);
        assert_eq!(awc_len(&bank), None);
    }

    #[test]
    fn adat_inside_a_word_of_a_stream_is_not_a_bank() {
        let (found, _) = collect(&zlib(b"<METADATA>"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "zlib");
    }
}
