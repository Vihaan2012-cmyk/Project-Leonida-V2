//! Opus data inside AWC banks, rewrapped as Ogg Opus without decoding or re-encoding.
//!
//! Plain streams store 2048-byte blocks of `[u16 length][packet]` (little-endian, a zero length pads the rest
//! of the block). Streamed music (tag `0x48`) interleaves per-channel runs of such blocks in chunks.

use crate::awc::{Awc, AwcStream, Codec, TAG_DATA, TAG_STREAMED_MUSIC};
use crate::ogg::{FLAG_BOS, FLAG_EOS, OggWriter};
use crate::{Error, Result};

pub const BLOCK_SIZE: usize = 2048;
/// Encoder lookahead measured on real streams; the bank does not store it.
pub const DEFAULT_PRE_SKIP: u16 = 312;
const MUSIC_RECORD_SIZE: usize = 24;

/// Samples per channel at 48 kHz in an Opus packet, from its table-of-contents byte and frame count code.
pub fn packet_frames(packet: &[u8]) -> Result<u32> {
    let toc = *packet
        .first()
        .ok_or_else(|| Error::Invalid("empty Opus packet".into()))?;
    let config = toc >> 3;
    let per_frame: u32 = match config {
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        12..=15 => [480, 960][(config % 2) as usize],
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => {
            let n = u32::from(*packet.get(1).ok_or_else(|| Error::Invalid("Opus packet without a frame count".into()))? & 0x3F);
            if n == 0 {
                return Err(Error::Invalid("Opus packet with zero frames".into()));
            }
            n
        }
    };
    Ok(per_frame * count)
}

/// Channels of an Opus packet from the TOC stereo bit.
pub fn packet_channels(packet: &[u8]) -> u8 {
    if packet.first().is_some_and(|t| t & 4 != 0) {
        2
    } else {
        1
    }
}

/// The packets of a run of 2048-byte blocks.
pub fn split_blocks(data: &[u8]) -> Result<Vec<&[u8]>> {
    let mut packets = Vec::new();
    for block in data.chunks(BLOCK_SIZE) {
        let mut at = 0;
        while at + 2 <= block.len() {
            let len = usize::from(u16::from_le_bytes([block[at], block[at + 1]]));
            if len == 0 {
                break;
            }
            let p = block.get(at + 2..at + 2 + len).ok_or_else(|| {
                Error::Invalid(format!("Opus packet of {len} bytes runs past its block"))
            })?;
            packets.push(p);
            at += 2 + len;
        }
    }
    Ok(packets)
}

/// Declared per-channel frames of a plain Opus stream: the format tag's `+0x0C` count when it agrees with the
/// sample count, else `samples / channels`.
pub fn declared_frames(samples: u32, opus_frames: u32, channels: u8) -> u64 {
    let ch = u64::from(channels.max(1));
    if u64::from(opus_frames) * ch == u64::from(samples) {
        u64::from(opus_frames)
    } else {
        u64::from(samples) / ch
    }
}

/// Build an Ogg Opus file from stored packets. `frames` is the declared per-channel length (48 kHz);
/// the end-of-stream page is written at the packet that reaches `pre_skip + frames` and later packets are
/// dropped.
pub fn ogg_opus(
    packets: &[&[u8]],
    channels: u8,
    input_rate: u32,
    pre_skip: u16,
    frames: u64,
) -> Result<Vec<u8>> {
    if packets.is_empty() {
        return Err(Error::Invalid("no Opus packets".into()));
    }
    let durations: Vec<u32> = packets.iter().map(|p| packet_frames(p)).collect::<Result<_>>()?;
    let total: u64 = durations.iter().map(|&d| u64::from(d)).sum();
    let pre_skip = u64::from(pre_skip).min(total);
    let target = pre_skip + frames;

    let mut head = Vec::new();
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(channels);
    head.extend_from_slice(&(pre_skip as u16).to_le_bytes());
    head.extend_from_slice(&input_rate.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    let vendor = b"leonida audio-codec";
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());

    let mut w = OggWriter::new(0x4C45_4F4E);
    w.page(FLAG_BOS, 0, &[&head]);
    w.page(0, 0, &[&tags]);

    let mut cumulative = 0u64;
    let mut used = packets.len();
    for (i, &d) in durations.iter().enumerate() {
        cumulative += u64::from(d);
        if cumulative >= target {
            used = i + 1;
            break;
        }
    }
    let lengths: Vec<usize> = packets[..used].iter().map(|p| p.len()).collect();
    let mut start = 0;
    let mut running = 0u64;
    while start < used {
        let n = OggWriter::packets_per_page(&lengths[start..]).min(40);
        let end = start + n;
        running += durations[start..end].iter().map(|&d| u64::from(d)).sum::<u64>();
        let last = end == used;
        let granule = if last { running.min(target) } else { running };
        w.page(
            if last { FLAG_EOS } else { 0 },
            granule as i64,
            &packets[start..end],
        );
        start = end;
    }
    Ok(w.finish())
}

/// Rewrap a plain Opus stream (codec `0x0B`, tag `0x55`) as Ogg Opus.
pub fn stream_to_ogg(awc: &Awc<'_>, stream: &AwcStream, pre_skip: u16) -> Result<Vec<u8>> {
    let fmt = awc.format(stream)?;
    if fmt.codec != Codec::Opus {
        return Err(Error::Invalid(format!("stream is {:?}, not Opus", fmt.codec)));
    }
    let data = awc.data(stream)?;
    let packets = split_blocks(data)?;
    let channels = fmt.channels.max(1);
    let frames = declared_frames(fmt.samples, fmt.opus_frames, channels);
    ogg_opus(&packets, channels, u32::from(fmt.sample_rate), pre_skip, frames)
}

/// One channel group of a streamed-music stream: its packets with the per-chunk repeats removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicChannel {
    pub id: u32,
    pub sample_rate: u16,
    /// Per-channel frames at 48 kHz.
    pub frames: u64,
    pub channels: u8,
    pub packets: Vec<Vec<u8>>,
}

fn rd32(d: &[u8], at: usize) -> Result<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Error::Truncated {
            what: "streamed music",
            offset: at,
            need: 4,
            have: d.len().saturating_sub(at),
        })
}

/// Decode the `0x48` descriptor and chunk layout of a streamed-music stream into one packet list per channel.
pub fn music_channels(awc: &Awc<'_>, stream: &AwcStream) -> Result<Vec<MusicChannel>> {
    let desc_tag = stream
        .tag(TAG_STREAMED_MUSIC)
        .ok_or(Error::MissingTag("streamed music (0x48)"))?;
    let desc = awc.tag_data(desc_tag)?;
    let blocks = rd32(desc, 0)? as usize;
    let chunk_size = rd32(desc, 4)? as usize;
    let nch = rd32(desc, 8)? as usize;
    if nch == 0 || nch > 64 || chunk_size == 0 {
        return Err(Error::Invalid("streamed music descriptor has an impossible shape".into()));
    }
    let mut info = Vec::new();
    for c in 0..nch {
        let at = 12 + c * 16;
        let rec = desc.get(at..at + 16).ok_or(Error::Truncated {
            what: "streamed music channel table",
            offset: at,
            need: 16,
            have: desc.len().saturating_sub(at),
        })?;
        info.push((
            rd32(rec, 0)? & 0x1FFF_FFFF,
            rd32(rec, 4)?,
            u16::from_le_bytes([rec[10], rec[11]]),
            rec[12],
        ));
    }
    if info.iter().any(|i| Codec::from_id(i.3) != Codec::Opus) {
        return Err(Error::Unsupported("streamed music that is not Opus".into()));
    }
    let data = awc.data(stream)?;
    let header = (nch * MUSIC_RECORD_SIZE).div_ceil(BLOCK_SIZE) * BLOCK_SIZE;
    let mut chans: Vec<Vec<Vec<u8>>> = vec![Vec::new(); nch];
    let mut prev_chunk: Vec<Vec<Vec<u8>>> = vec![Vec::new(); nch];
    for b in 0..blocks {
        let start = b * chunk_size;
        let end = (start + chunk_size).min(data.len());
        let chunk = data
            .get(start..end)
            .filter(|c| c.len() >= header)
            .ok_or_else(|| Error::Invalid(format!("streamed music chunk {b} is outside the data")))?;
        let mut at = header;
        let mut this_chunk: Vec<Vec<Vec<u8>>> = Vec::new();
        for c in 0..nch {
            let subblocks = rd32(chunk, c * MUSIC_RECORD_SIZE + 4)? as usize;
            let skip = u64::from(rd32(chunk, c * MUSIC_RECORD_SIZE + 8)?);
            let run = chunk
                .get(at..at + subblocks * BLOCK_SIZE)
                .ok_or_else(|| Error::Invalid(format!("streamed music chunk {b} channel {c} overruns its chunk")))?;
            at += subblocks * BLOCK_SIZE;
            let packets: Vec<Vec<u8>> = split_blocks(run)?.into_iter().map(<[u8]>::to_vec).collect();
            let mut dropped = 0;
            let mut covered = 0u64;
            while covered < skip {
                let p = packets
                    .get(dropped)
                    .ok_or_else(|| Error::Invalid("repeated prefix longer than the chunk".into()))?;
                covered += u64::from(packet_frames(p)?);
                dropped += 1;
            }
            if covered != skip {
                return Err(Error::Invalid("repeated prefix does not end on a packet boundary".into()));
            }
            let prev = &prev_chunk[c];
            if dropped > prev.len() || packets[..dropped] != prev[prev.len() - dropped..] {
                return Err(Error::Invalid("repeated prefix differs from the previous chunk's tail".into()));
            }
            chans[c].extend(packets[dropped..].iter().cloned());
            this_chunk.push(packets);
        }
        prev_chunk = this_chunk;
    }
    info.into_iter()
        .zip(chans)
        .map(|((id, samples, rate, _), packets)| {
            let first = packets
                .first()
                .ok_or_else(|| Error::Invalid("streamed music channel without packets".into()))?;
            let channels = packet_channels(first);
            Ok(MusicChannel {
                id,
                sample_rate: rate,
                frames: u64::from(samples) / u64::from(channels),
                channels,
                packets,
            })
        })
        .collect()
}

/// Rewrap each channel group of a streamed-music stream as its own Ogg Opus file.
pub fn music_to_ogg(awc: &Awc<'_>, stream: &AwcStream, pre_skip: u16) -> Result<Vec<(u32, Vec<u8>)>> {
    music_channels(awc, stream)?
        .into_iter()
        .map(|c| {
            let refs: Vec<&[u8]> = c.packets.iter().map(Vec::as_slice).collect();
            ogg_opus(&refs, c.channels, 48000, pre_skip, c.frames).map(|o| (c.id, o))
        })
        .collect()
}

/// Whether a stream carries data this module can rewrap: plain Opus or streamed music.
pub fn is_opus_stream(awc: &Awc<'_>, stream: &AwcStream) -> bool {
    stream.tag(TAG_STREAMED_MUSIC).is_some()
        || (stream.tag(TAG_DATA).is_some()
            && awc.format(stream).is_ok_and(|f| f.codec == Codec::Opus))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awc::build::*;
    use crate::awc::{TAG_DATA, TAG_FORMAT};
    use crate::ogg::read_pages;

    /// An Opus packet with a given TOC and filler body (never decoded here).
    fn packet(toc: u8, len: usize, tag: u8) -> Vec<u8> {
        let mut p = vec![tag; len];
        p[0] = toc;
        p
    }

    fn blocks(packets: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut block = Vec::new();
        for p in packets {
            if block.len() + 2 + p.len() > BLOCK_SIZE {
                block.resize(BLOCK_SIZE, 0);
                out.extend_from_slice(&block);
                block.clear();
            }
            block.extend_from_slice(&(p.len() as u16).to_le_bytes());
            block.extend_from_slice(p);
        }
        block.resize(BLOCK_SIZE, 0);
        out.extend_from_slice(&block);
        out
    }

    #[test]
    fn toc_durations() {
        // CELT 20 ms single frame, mono: config 31
        assert_eq!(packet_frames(&[31 << 3, 0]).unwrap(), 960);
        // CELT 2.5 ms, two frames code 1
        assert_eq!(packet_frames(&[(16 << 3) | 1, 0]).unwrap(), 240);
        // SILK 60 ms
        assert_eq!(packet_frames(&[3 << 3, 0]).unwrap(), 2880);
        // hybrid 10 ms
        assert_eq!(packet_frames(&[12 << 3, 0]).unwrap(), 480);
        // code 3 with 6 frames of 20 ms CELT
        assert_eq!(packet_frames(&[(19 << 3) | 3, 6]).unwrap(), 960 * 6);
        assert!(packet_frames(&[]).is_err());
        assert!(packet_frames(&[3, 0]).is_err());
        assert_eq!(packet_channels(&[(19 << 3) | 4]), 2);
        assert_eq!(packet_channels(&[19 << 3]), 1);
    }

    #[test]
    fn splits_blocks_and_rejects_overrun() {
        let ps = vec![packet(19 << 3, 100, 1), packet(19 << 3, 300, 2), packet(19 << 3, 1990, 3)];
        let data = blocks(&ps);
        assert_eq!(data.len(), 2 * BLOCK_SIZE);
        let got = split_blocks(&data).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[2], &ps[2][..]);
        let mut bad = vec![0u8; BLOCK_SIZE];
        bad[..2].copy_from_slice(&5000u16.to_le_bytes());
        assert!(split_blocks(&bad).is_err());
    }

    #[test]
    fn declared_frames_prefers_agreeing_count() {
        assert_eq!(declared_frames(200, 100, 2), 100);
        assert_eq!(declared_frames(200, 77, 2), 100);
        assert_eq!(declared_frames(200, 200, 1), 200);
    }

    #[test]
    fn ogg_ends_at_the_packet_reaching_the_declared_length() {
        // 20 ms packets, 10 of them = 9600 frames; declare 9000 frames with pre-skip 312
        let ps: Vec<Vec<u8>> = (0..10).map(|i| packet(19 << 3, 50 + i, i as u8)).collect();
        let refs: Vec<&[u8]> = ps.iter().map(Vec::as_slice).collect();
        let ogg = ogg_opus(&refs, 1, 48000, 312, 9000).unwrap();
        let pages = read_pages(&ogg).unwrap();
        assert_eq!(&pages[0].packets[0][..8], b"OpusHead");
        assert_eq!(pages[0].packets[0][9], 1);
        assert_eq!(u16::from_le_bytes([pages[0].packets[0][10], pages[0].packets[0][11]]), 312);
        assert_eq!(&pages[1].packets[0][..8], b"OpusTags");
        let audio: Vec<_> = pages[2..].iter().flat_map(|p| p.packets.clone()).collect();
        // 9312 samples needed: the 10th packet (9600) is the one that reaches it
        assert_eq!(audio.len(), 10);
        assert_eq!(audio, ps);
        let last = pages.last().unwrap();
        assert_eq!(last.flags, FLAG_EOS);
        assert_eq!(last.granule, 9312);
        for w in pages[2..].windows(2) {
            assert!(w[0].granule <= w[1].granule);
        }
        // a shorter declared length drops trailing packets
        let ogg = ogg_opus(&refs, 1, 48000, 312, 1000).unwrap();
        let pages = read_pages(&ogg).unwrap();
        let n: usize = pages[2..].iter().map(|p| p.packets.len()).sum();
        assert_eq!(n, 2);
        assert_eq!(pages.last().unwrap().granule, 1312);
    }

    #[test]
    fn plain_stream_through_a_bank() {
        let ps: Vec<Vec<u8>> = (0..6).map(|i| packet((19 << 3) | 4, 80, i)).collect();
        let samples = 5 * 960 * 2;
        let mut fmt = format_tag(samples, 48000, 0x0B, 2);
        fmt[12..16].copy_from_slice(&(5u32 * 960).to_le_bytes());
        let bank_bytes = bank(
            0,
            &[StreamSpec {
                id: 3,
                tags: vec![(TAG_FORMAT, fmt), (TAG_DATA, blocks(&ps))],
            }],
        );
        let awc = Awc::parse(&bank_bytes).unwrap();
        let s = &awc.streams[0];
        assert!(is_opus_stream(&awc, s));
        let ogg = stream_to_ogg(&awc, s, DEFAULT_PRE_SKIP).unwrap();
        let pages = read_pages(&ogg).unwrap();
        assert_eq!(pages[0].packets[0][9], 2);
        assert_eq!(pages.last().unwrap().granule, 312 + 4800);
        assert_eq!(pages[2..].iter().map(|p| p.packets.len()).sum::<usize>(), 6);
    }

    fn music_bank(chunks: &[(Vec<Vec<Vec<u8>>>, Vec<u32>)], samples: u32) -> Vec<u8> {
        let nch = chunks[0].0.len();
        let header = BLOCK_SIZE;
        let mut payload = Vec::new();
        let mut chunk_size = 0;
        let mut built = Vec::new();
        for (chans, skips) in chunks {
            let runs: Vec<Vec<u8>> = chans.iter().map(|ps| blocks(ps)).collect();
            let mut chunk = vec![0u8; header];
            for c in 0..nch {
                let off = c * MUSIC_RECORD_SIZE;
                chunk[off + 4..off + 8].copy_from_slice(&((runs[c].len() / BLOCK_SIZE) as u32).to_le_bytes());
                chunk[off + 8..off + 12].copy_from_slice(&skips[c].to_le_bytes());
            }
            for r in &runs {
                chunk.extend_from_slice(r);
            }
            chunk_size = chunk_size.max(chunk.len());
            built.push(chunk);
        }
        for mut c in built {
            c.resize(chunk_size, 0);
            payload.extend_from_slice(&c);
        }
        let mut desc = Vec::new();
        for v in [chunks.len() as u32, chunk_size as u32, nch as u32] {
            desc.extend_from_slice(&v.to_le_bytes());
        }
        for c in 0..nch {
            let mut rec = [0u8; 16];
            rec[0..4].copy_from_slice(&(100 + c as u32).to_le_bytes());
            rec[4..8].copy_from_slice(&samples.to_le_bytes());
            rec[10..12].copy_from_slice(&48000u16.to_le_bytes());
            rec[12] = 0x0B;
            desc.extend_from_slice(&rec);
        }
        bank(
            0,
            &[StreamSpec {
                id: 1,
                tags: vec![(TAG_STREAMED_MUSIC, desc), (TAG_DATA, payload)],
            }],
        )
    }

    #[test]
    fn streamed_music_drops_repeated_prefixes() {
        let mk = |tag: u8| packet((19 << 3) | 4, 60, tag);
        let ch0_a = vec![mk(1), mk(2), mk(3)];
        let ch0_b = vec![mk(2), mk(3), mk(4), mk(5)];
        let ch1_a = vec![mk(11), mk(12)];
        let ch1_b = vec![mk(12), mk(13)];
        let bytes = music_bank(
            &[
                (vec![ch0_a, ch1_a], vec![0, 0]),
                (vec![ch0_b, ch1_b], vec![2 * 960, 960]),
            ],
            2 * 960 * 5,
        );
        let awc = Awc::parse(&bytes).unwrap();
        let chans = music_channels(&awc, &awc.streams[0]).unwrap();
        assert_eq!(chans.len(), 2);
        assert_eq!(chans[0].id, 100);
        assert_eq!(chans[0].channels, 2);
        assert_eq!(chans[0].frames, 4800);
        let tags: Vec<u8> = chans[0].packets.iter().map(|p| p[1]).collect();
        assert_eq!(tags, vec![1, 2, 3, 4, 5]);
        let tags1: Vec<u8> = chans[1].packets.iter().map(|p| p[1]).collect();
        assert_eq!(tags1, vec![11, 12, 13]);
        let oggs = music_to_ogg(&awc, &awc.streams[0], 312).unwrap();
        assert_eq!(oggs.len(), 2);
        assert!(read_pages(&oggs[0].1).is_ok());
    }

    #[test]
    fn streamed_music_rejects_bad_repeats() {
        let mk = |tag: u8| packet((19 << 3) | 4, 60, tag);
        // second chunk claims 1 repeated packet whose bytes differ from the previous tail
        let bytes = music_bank(
            &[
                (vec![vec![mk(1), mk(2)]], vec![0]),
                (vec![vec![mk(9), mk(3)]], vec![960]),
            ],
            2 * 960 * 3,
        );
        let awc = Awc::parse(&bytes).unwrap();
        assert!(music_channels(&awc, &awc.streams[0]).is_err());
        // skip not ending on a packet boundary
        let bytes = music_bank(
            &[
                (vec![vec![mk(1), mk(2)]], vec![0]),
                (vec![vec![mk(2), mk(3)]], vec![500]),
            ],
            2 * 960 * 3,
        );
        let awc = Awc::parse(&bytes).unwrap();
        assert!(music_channels(&awc, &awc.streams[0]).is_err());
    }
}
