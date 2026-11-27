//! AWC audio banks (`ADAT` container): stream table, tags, and the per-stream companion tables.

use crate::{Error, Result};

pub const TAG_PEAKS: u8 = 0x36;
pub const TAG_DATA: u8 = 0x55;
pub const TAG_GRANULAR_GRAINS: u8 = 0x5a;
pub const TAG_FORMAT: u8 = 0x76;
pub const TAG_SEEK_TABLE: u8 = 0xa3;
pub const TAG_LOUDNESS: u8 = 0xb2;
pub const TAG_MARKERS: u8 = 0xbd;
pub const TAG_GRANULAR_LOOPS: u8 = 0xd9;
pub const TAG_STREAMED_MUSIC: u8 = 0x48;
pub const TAG_FACIAL_CLIP: u8 = 0x5c;

const FLAG_TABLE_4: u32 = 0x0040_0000;
const FLAG_TABLE_N2: u32 = 0x0001_0000;
const FLAG_TABLE_N1X4: u32 = 0x0020_0000;

/// Jenkins one-at-a-time hash of the lowercased text, the game's name hash.
pub fn joaat(text: &str) -> u32 {
    let mut h: u32 = 0;
    for b in text.bytes() {
        h = h.wrapping_add(u32::from(b.to_ascii_lowercase()));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwcTag {
    pub kind: u8,
    pub size: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwcStream {
    /// Low 29 bits of the name hash.
    pub id: u32,
    pub tags: Vec<AwcTag>,
}

impl AwcStream {
    pub fn tag(&self, kind: u8) -> Option<&AwcTag> {
        self.tags.iter().find(|t| t.kind == kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Pcm16,
    PcmF32,
    Xma2,
    Opus,
    Other(u8),
}

impl Codec {
    pub fn from_id(id: u8) -> Self {
        match id {
            0 => Codec::Pcm16,
            2 => Codec::PcmF32,
            5 => Codec::Xma2,
            0x0B => Codec::Opus,
            other => Codec::Other(other),
        }
    }
}

/// The `0x76` format tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFormat {
    /// Samples; for Opus these are 48 kHz frames interleaved across channels.
    pub samples: u32,
    pub sample_rate: u16,
    /// Opus per-channel frame count (`+0x0C`).
    pub opus_frames: u32,
    pub codec: Codec,
    pub channels: u8,
}

/// A cue record of the `0xbd` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub name: u32,
    pub value: u32,
    /// Sample position in the stream.
    pub offset: u32,
    pub extra: u32,
}

impl Marker {
    pub fn value_f32(&self) -> f32 {
        f32::from_bits(self.value)
    }
}

/// A region cue: a `start` record followed by an `end` record of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub name: u32,
    pub start: u32,
    pub end: u32,
}

/// Pairs `start`/`end` cues of each name inside one marker list, in stored order.
pub fn pair_regions(markers: &[Marker]) -> Vec<Region> {
    let start_hash = joaat("start");
    let end_hash = joaat("end");
    let mut regions = Vec::new();
    let mut names: Vec<u32> = markers
        .iter()
        .filter(|m| m.value == start_hash || m.value == end_hash)
        .map(|m| m.name)
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let mut same = markers.iter().filter(|m| m.name == name).peekable();
        while let Some(m) = same.next() {
            if m.value == start_hash {
                if let Some(e) = same.peek() {
                    if e.value == end_hash && e.offset >= m.offset {
                        regions.push(Region {
                            name,
                            start: m.offset,
                            end: e.offset,
                        });
                        same.next();
                    }
                }
            }
        }
    }
    regions.sort_by_key(|r| (r.start, r.end));
    regions
}

/// One `0xb2` loudness point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessPoint {
    pub sample: u32,
    pub db: f32,
}

fn rd_u32(data: &[u8], at: usize, what: &'static str) -> Result<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Error::Truncated {
            what,
            offset: at,
            need: 4,
            have: data.len().saturating_sub(at),
        })
}

/// A parsed bank over the bytes starting at its `ADAT` magic (it may be followed by more archive data).
pub struct Awc<'a> {
    data: &'a [u8],
    pub flags: u32,
    pub header_size: u32,
    pub streams: Vec<AwcStream>,
}

impl<'a> Awc<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if data.get(..4) != Some(b"ADAT") {
            return Err(Error::Invalid("not an ADAT bank".into()));
        }
        let flags = rd_u32(data, 4, "ADAT header")?;
        let count = rd_u32(data, 8, "ADAT header")? as usize;
        let header_size = rd_u32(data, 12, "ADAT header")?;
        if header_size as usize > data.len() || header_size < 16 {
            return Err(Error::Invalid(format!("ADAT header size {header_size} out of range")));
        }
        if count > (header_size as usize - 16) / 4 {
            return Err(Error::Invalid(format!("ADAT stream count {count} does not fit the header")));
        }
        let mut at = 16usize;
        if flags & FLAG_TABLE_4 != 0 {
            at += 4;
        }
        if flags & FLAG_TABLE_N2 != 0 {
            at += count * 2;
        }
        if flags & FLAG_TABLE_N1X4 != 0 {
            at += (count + 1) * 4;
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let w = rd_u32(data, at + i * 4, "ADAT stream table")?;
            entries.push((w & 0x1FFF_FFFF, (w >> 29) as usize));
        }
        at += count * 4;
        let mut streams = Vec::with_capacity(count);
        for (id, tag_count) in entries {
            let mut tags = Vec::with_capacity(tag_count);
            for _ in 0..tag_count {
                let b = data.get(at..at + 8).ok_or(Error::Truncated {
                    what: "ADAT tag",
                    offset: at,
                    need: 8,
                    have: data.len().saturating_sub(at),
                })?;
                let w = u64::from_le_bytes(b.try_into().expect("8 bytes"));
                tags.push(AwcTag {
                    kind: (w >> 56) as u8,
                    size: ((w >> 28) & 0x0FFF_FFFF) as u32,
                    offset: (w & 0x0FFF_FFFF) as u32,
                });
                at += 8;
            }
            streams.push(AwcStream { id, tags });
        }
        if at > header_size as usize {
            return Err(Error::Invalid("ADAT tables run past the declared header size".into()));
        }
        Ok(Self {
            data,
            flags,
            header_size,
            streams,
        })
    }

    pub fn stream_by_id(&self, id: u32) -> Option<&AwcStream> {
        self.streams.iter().find(|s| s.id == id)
    }

    /// Bytes of a tag; errors when the tag reaches past the bytes we hold.
    pub fn tag_data(&self, tag: &AwcTag) -> Result<&'a [u8]> {
        let start = tag.offset as usize;
        let end = start + tag.size as usize;
        self.data.get(start..end).ok_or(Error::Truncated {
            what: "AWC tag data",
            offset: start,
            need: tag.size as usize,
            have: self.data.len().saturating_sub(start),
        })
    }

    fn require<'s>(&self, stream: &'s AwcStream, kind: u8, name: &'static str) -> Result<&'s AwcTag> {
        stream.tag(kind).ok_or(Error::MissingTag(name))
    }

    pub fn format(&self, stream: &AwcStream) -> Result<StreamFormat> {
        let d = self.tag_data(self.require(stream, TAG_FORMAT, "format (0x76)")?)?;
        if d.len() < 32 {
            return Err(Error::Truncated {
                what: "AWC format tag",
                offset: 0,
                need: 32,
                have: d.len(),
            });
        }
        Ok(StreamFormat {
            samples: rd_u32(d, 0, "format")?,
            sample_rate: u16::from_le_bytes([d[8], d[9]]),
            opus_frames: rd_u32(d, 12, "format")?,
            codec: Codec::from_id(d[28]),
            channels: d[29],
        })
    }

    pub fn data(&self, stream: &AwcStream) -> Result<&'a [u8]> {
        self.tag_data(self.require(stream, TAG_DATA, "data (0x55)")?)
    }

    pub fn markers(&self, stream: &AwcStream) -> Result<Vec<Marker>> {
        let Some(tag) = stream.tag(TAG_MARKERS) else {
            return Ok(Vec::new());
        };
        let d = self.tag_data(tag)?;
        if d.len() % 16 != 0 {
            return Err(Error::Invalid("marker tag size is not a multiple of 16".into()));
        }
        Ok(d.chunks_exact(16)
            .map(|c| {
                let w = |i: usize| u32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
                Marker {
                    name: w(0),
                    value: w(4),
                    offset: w(8),
                    extra: w(12),
                }
            })
            .collect())
    }

    /// First sample of each 2048-byte block (`0xa3`); measured on XMA2 streams, where each entry is the
    /// running total of decoded samples at the packet start.
    pub fn seek_table(&self, stream: &AwcStream) -> Result<Vec<u32>> {
        let d = self.tag_data(self.require(stream, TAG_SEEK_TABLE, "seek table (0xa3)")?)?;
        Ok(d.chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }

    pub fn peaks(&self, stream: &AwcStream) -> Result<Vec<u16>> {
        let d = self.tag_data(self.require(stream, TAG_PEAKS, "peaks (0x36)")?)?;
        Ok(d.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect())
    }

    pub fn loudness(&self, stream: &AwcStream) -> Result<Vec<LoudnessPoint>> {
        let d = self.tag_data(self.require(stream, TAG_LOUDNESS, "loudness (0xb2)")?)?;
        Ok(d.chunks_exact(8)
            .map(|c| LoudnessPoint {
                sample: u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                db: f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            })
            .collect())
    }
}

/// Block index for a sample position from a seek table: entry `i` is the first sample of block `i`, so this is
/// the last block that starts at or before `sample`.
pub fn seek_block(table: &[u32], sample: u32) -> usize {
    table.partition_point(|&start| start <= sample).saturating_sub(1)
}

#[cfg(test)]
pub(crate) mod build {
    //! Synthetic bank builder for tests.

    pub struct StreamSpec {
        pub id: u32,
        pub tags: Vec<(u8, Vec<u8>)>,
    }

    pub fn bank(flags: u32, streams: &[StreamSpec]) -> Vec<u8> {
        let n = streams.len();
        let mut header_len = 16;
        if flags & 0x0040_0000 != 0 {
            header_len += 4;
        }
        if flags & 0x0001_0000 != 0 {
            header_len += n * 2;
        }
        if flags & 0x0020_0000 != 0 {
            header_len += (n + 1) * 4;
        }
        header_len += n * 4 + streams.iter().map(|s| s.tags.len() * 8).sum::<usize>();
        let mut out = Vec::new();
        out.extend_from_slice(b"ADAT");
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        out.extend_from_slice(&(header_len as u32).to_le_bytes());
        out.resize(header_len - n * 4 - streams.iter().map(|s| s.tags.len() * 8).sum::<usize>(), 0);
        for s in streams {
            out.extend_from_slice(&(s.id | ((s.tags.len() as u32) << 29)).to_le_bytes());
        }
        let data_at = header_len.div_ceil(16) * 16;
        let mut payload = Vec::new();
        for s in streams {
            for (kind, bytes) in &s.tags {
                let w = (u64::from(*kind) << 56)
                    | ((bytes.len() as u64) << 28)
                    | (data_at + payload.len()) as u64;
                out.extend_from_slice(&w.to_le_bytes());
                payload.extend_from_slice(bytes);
            }
        }
        out.resize(data_at, 0);
        out.extend_from_slice(&payload);
        out
    }

    pub fn format_tag(samples: u32, rate: u16, codec: u8, channels: u8) -> Vec<u8> {
        let mut t = vec![0u8; 32];
        t[0..4].copy_from_slice(&samples.to_le_bytes());
        t[8..10].copy_from_slice(&rate.to_le_bytes());
        t[28] = codec;
        t[29] = channels;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn joaat_matches_documented_cue_hashes() {
        assert_eq!(joaat("start"), 0x84dc_271f);
        assert_eq!(joaat("end"), 0xb0c4_85d7);
        assert_eq!(joaat("beat"), 0xe89a_e78c);
        assert_eq!(joaat("TrackId"), 0xa6d9_3246);
        assert_eq!(joaat("intro"), 0xda8f_5f53);
        assert_eq!(joaat("outro"), 0x5d40_b28d);
    }

    #[test]
    fn parses_streams_tags_and_flag_tables() {
        for flags in [0, 0x0040_0000, 0x0001_0000, 0x0020_0000, 0x0061_0000] {
            let bytes = bank(
                flags,
                &[
                    StreamSpec {
                        id: 0x1234_5678,
                        tags: vec![
                            (TAG_FORMAT, format_tag(1000, 32000, 5, 1)),
                            (TAG_DATA, vec![1, 2, 3, 4, 5]),
                        ],
                    },
                    StreamSpec {
                        id: 7,
                        tags: vec![(TAG_FORMAT, format_tag(48, 48000, 0x0B, 2))],
                    },
                ],
            );
            let awc = Awc::parse(&bytes).unwrap();
            assert_eq!(awc.streams.len(), 2);
            let s = awc.stream_by_id(0x1234_5678).unwrap();
            let f = awc.format(s).unwrap();
            assert_eq!(f.samples, 1000);
            assert_eq!(f.sample_rate, 32000);
            assert_eq!(f.codec, Codec::Xma2);
            assert_eq!(f.channels, 1);
            assert_eq!(awc.data(s).unwrap(), &[1, 2, 3, 4, 5]);
            let f2 = awc.format(awc.stream_by_id(7).unwrap()).unwrap();
            assert_eq!(f2.codec, Codec::Opus);
            assert_eq!(f2.channels, 2);
            assert!(matches!(awc.data(awc.stream_by_id(7).unwrap()), Err(Error::MissingTag(_))));
        }
    }

    #[test]
    fn zero_stream_bank_with_n1_table_parses() {
        let mut b = Vec::new();
        b.extend_from_slice(b"ADAT");
        b.extend_from_slice(&0xff21_0001u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&20u32.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        let awc = Awc::parse(&b).unwrap();
        assert!(awc.streams.is_empty());
    }

    #[test]
    fn rejects_bad_headers() {
        assert!(Awc::parse(b"RIFF....").is_err());
        let mut b = Vec::new();
        b.extend_from_slice(b"ADAT");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&1000u32.to_le_bytes());
        b.extend_from_slice(&16u32.to_le_bytes());
        assert!(Awc::parse(&b).is_err());
    }

    #[test]
    fn tag_past_end_is_truncated() {
        let mut bytes = bank(
            0,
            &[StreamSpec {
                id: 1,
                tags: vec![(TAG_DATA, vec![0; 16])],
            }],
        );
        bytes.truncate(bytes.len() - 4);
        let awc = Awc::parse(&bytes).unwrap();
        assert!(matches!(awc.data(&awc.streams[0]), Err(Error::Truncated { .. })));
    }

    #[test]
    fn markers_seek_table_and_regions() {
        let start = joaat("start");
        let end = joaat("end");
        let mut m = Vec::new();
        let recs = [
            (0x11u32, 1.0f32.to_bits(), 100u32, 0u32),
            (0x22, start, 200, 0),
            (0x22, end, 900, 0),
            (0x22, start, 1000, 0),
            (0x22, end, 1500, 0),
            (0x33, end, 1600, 0),
        ];
        for r in recs {
            for v in [r.0, r.1, r.2, r.3] {
                m.extend_from_slice(&v.to_le_bytes());
            }
        }
        let seek: Vec<u8> = [0u32, 11520, 23040, 34560].iter().flat_map(|v| v.to_le_bytes()).collect();
        let bytes = bank(
            0,
            &[StreamSpec {
                id: 9,
                tags: vec![(TAG_MARKERS, m), (TAG_SEEK_TABLE, seek)],
            }],
        );
        let awc = Awc::parse(&bytes).unwrap();
        let s = &awc.streams[0];
        let markers = awc.markers(s).unwrap();
        assert_eq!(markers.len(), 6);
        assert_eq!(markers[0].value_f32(), 1.0);
        let regions = pair_regions(&markers);
        assert_eq!(
            regions,
            vec![
                Region { name: 0x22, start: 200, end: 900 },
                Region { name: 0x22, start: 1000, end: 1500 },
            ]
        );
        let table = awc.seek_table(s).unwrap();
        assert_eq!(table, vec![0, 11520, 23040, 34560]);
        assert_eq!(seek_block(&table, 0), 0);
        assert_eq!(seek_block(&table, 11519), 0);
        assert_eq!(seek_block(&table, 11520), 1);
        assert_eq!(seek_block(&table, 999_999), 3);
    }
}
