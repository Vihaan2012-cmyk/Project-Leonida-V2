//! Reading resource names out of an RPF9 archive without its table of contents.
//!
//! The table of contents is encrypted and is not read. The data area is zlib streams laid end to end on 16-byte
//! boundaries; a drawable's name (`name.#dr`) is plaintext inside its first inflated stream. The walk inflates every
//! stream once, hashes the inflated bytes as it goes and looks for names in the first megabyte.

use flate2::{Decompress, FlushDecompress, Status};
use memchr::memmem;
use sha2::{Digest, Sha256};

use crate::hash::ContentHash;

const ALIGN: usize = 16;
const NAME_WINDOW: usize = 1 << 20;
const SUFFIXES: [&[u8]; 3] = [b".#dr", b".#ft", b".#dd"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    /// Offset of the zlib stream in the archive.
    pub offset: u64,
    pub stored: u64,
    pub inflated: u64,
    /// Hash of the inflated bytes.
    pub hash: ContentHash,
    /// Resource names found in the first megabyte, with their suffix.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkSummary {
    pub streams: u64,
    pub named_streams: u64,
    pub bytes_walked: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WalkOptions {
    /// Where to start. Defaults to the data offset in the archive header, or 0 for a file with no RPF9 header.
    pub start: Option<u64>,
    /// How many bytes after the start to walk. Defaults to the whole archive.
    pub limit: Option<u64>,
}

fn is_zlib_header(data: &[u8], at: usize) -> bool {
    matches!(
        data.get(at..at + 2),
        Some([0x78, 0x01 | 0x5e | 0x9c | 0xda])
    )
}

/// Offset where the data area starts, from the RPF9 header (`u32` at `+4`); 0 when there is no such header.
pub fn data_start(data: &[u8]) -> u64 {
    match data.get(..8) {
        Some(head) if &head[..4] == b"RPF9" || &head[..4] == b"9FPR" => {
            u64::from(u32::from_le_bytes(head[4..8].try_into().unwrap()))
        }
        _ => 0,
    }
}

/// Inflates one stream starting at `at`. `None` when the bytes are not a clean zlib stream.
fn inflate(data: &[u8], at: usize) -> Option<Stream> {
    let mut decoder = Decompress::new(true);
    let mut out = vec![0u8; 1 << 16];
    let mut hasher = Sha256::new();
    let mut prefix: Vec<u8> = Vec::new();
    loop {
        let consumed = decoder.total_in() as usize;
        let before_out = decoder.total_out();
        let status = decoder
            .decompress(&data[at + consumed..], &mut out, FlushDecompress::None)
            .ok()?;
        let produced = (decoder.total_out() - before_out) as usize;
        hasher.update(&out[..produced]);
        if prefix.len() < NAME_WINDOW {
            let room = NAME_WINDOW - prefix.len();
            prefix.extend_from_slice(&out[..produced.min(room)]);
        }
        match status {
            Status::StreamEnd => break,
            Status::Ok | Status::BufError => {
                if produced == 0 && decoder.total_in() as usize == consumed {
                    return None;
                }
            }
        }
    }
    Some(Stream {
        offset: at as u64,
        stored: decoder.total_in(),
        inflated: decoder.total_out(),
        hash: ContentHash::from_digest(hasher.finalize().into()),
        names: find_names(&prefix),
    })
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

/// Every `name.#dr`, `name.#ft` or `name.#dd` spelled in the bytes.
pub fn find_names(data: &[u8]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for suffix in SUFFIXES {
        for hit in memmem::find_iter(data, suffix) {
            let mut start = hit;
            while start > 0 && hit - start < 200 && is_name_byte(data[start - 1]) {
                start -= 1;
            }
            if hit > start && data[start].is_ascii_alphanumeric() {
                let end = hit + suffix.len();
                if let Ok(name) = std::str::from_utf8(&data[start..end])
                    && !names.iter().any(|known| known == name)
                {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// Walks the zlib streams of an archive in order, calling `on_stream` for each.
pub fn walk(data: &[u8], options: WalkOptions, mut on_stream: impl FnMut(Stream)) -> WalkSummary {
    let start = options.start.unwrap_or_else(|| data_start(data)) as usize;
    let end = options.limit.map_or(data.len(), |limit| {
        data.len().min(start.saturating_add(limit as usize))
    });
    let mut summary = WalkSummary::default();
    let mut at = start;
    while at + 2 <= end {
        if is_zlib_header(data, at)
            && let Some(stream) = inflate(data, at)
        {
            let next = at + stream.stored as usize;
            summary.streams += 1;
            summary.named_streams += u64::from(!stream.names.is_empty());
            at = next.next_multiple_of(ALIGN);
            on_stream(stream);
            continue;
        }
        at += ALIGN;
    }
    summary.bytes_walked = at.min(end).saturating_sub(start) as u64;
    summary
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    pub fn stream(payload: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(payload).unwrap();
        encoder.finish().unwrap()
    }

    /// A header with the data offset at 16, then each stream on a 16-byte boundary, with `gap` filler bytes between.
    pub fn archive(parts: &[Vec<u8>], gap: usize) -> Vec<u8> {
        let mut out = b"RPF9".to_vec();
        out.extend_from_slice(&16u32.to_le_bytes());
        out.resize(16, 0);
        for part in parts {
            out.extend_from_slice(part);
            out.resize(out.len().next_multiple_of(16) + gap, 0xA5);
        }
        out
    }

    /// A payload that spells a name inside some other bytes, as a drawable's system page does.
    pub fn named_payload(name: &str, filler: usize) -> Vec<u8> {
        let mut payload = vec![0x11u8; filler];
        payload.extend_from_slice(name.as_bytes());
        payload.extend_from_slice(&[0, 0, 0, 0]);
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn names_are_found_in_the_payload() {
        let payload = named_payload("bodygenitalsgen_fr1_008_regular_ws00_full.#dr", 40);
        assert_eq!(
            find_names(&payload),
            vec!["bodygenitalsgen_fr1_008_regular_ws00_full.#dr".to_string()]
        );
        assert!(find_names(b"no names here .#dr").is_empty());
    }

    #[test]
    fn the_walk_reads_every_stream_across_gaps() {
        let first = named_payload("alpha_fr1_000.#dr", 100);
        let second = vec![7u8; 5000];
        let third = named_payload("beta_mr1_000.#ft", 10);
        let archive = archive(&[stream(&first), stream(&second), stream(&third)], 32);
        let mut seen = Vec::new();
        let summary = walk(&archive, WalkOptions::default(), |stream| seen.push(stream));
        assert_eq!(summary.streams, 3);
        assert_eq!(summary.named_streams, 2);
        assert_eq!(seen[0].names, vec!["alpha_fr1_000.#dr".to_string()]);
        assert_eq!(seen[1].inflated, 5000);
        assert_eq!(seen[1].hash, ContentHash::of(&second));
        assert_eq!(seen[2].names, vec!["beta_mr1_000.#ft".to_string()]);
    }

    #[test]
    fn limit_and_start_bound_the_walk() {
        let streams: Vec<Vec<u8>> = (0..4u8)
            .map(|n| stream(&vec![n; 3000 + usize::from(n)]))
            .collect();
        let archive = archive(&streams, 0);
        let mut count = 0;
        walk(
            &archive,
            WalkOptions {
                start: None,
                limit: Some(streams[0].len() as u64 + 8),
            },
            |_| count += 1,
        );
        assert_eq!(count, 1);
        let mut count = 0;
        walk(&archive, WalkOptions::default(), |_| count += 1);
        assert_eq!(count, 4);
    }

    #[test]
    fn random_bytes_that_look_like_a_header_are_skipped() {
        let mut parts = vec![vec![
            0x78, 0x9c, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14,
        ]];
        parts.push(stream(&named_payload("real_fr1_000.#dr", 8)));
        let archive = archive(&parts, 0);
        let mut names = Vec::new();
        walk(&archive, WalkOptions::default(), |stream| {
            names.extend(stream.names)
        });
        assert_eq!(names, vec!["real_fr1_000.#dr".to_string()]);
    }

    #[test]
    fn a_file_without_a_header_is_walked_from_zero() {
        let archive = stream(&named_payload("plain_fr1_000.#dr", 4));
        assert_eq!(data_start(&archive), 0);
        let mut count = 0;
        walk(&archive, WalkOptions::default(), |_| count += 1);
        assert_eq!(count, 1);
    }
}
