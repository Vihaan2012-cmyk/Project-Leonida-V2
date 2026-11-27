//! Checks against the player's game package. Skipped when `LEONIDA_GAME_DIR` is unset or the package is not
//! the 1.0.2.1 build these offsets were measured on.

use std::io::{Read, Seek, SeekFrom};

use audio_codec::awc::{Awc, Codec};
use audio_codec::decode::decode_stream;
use audio_codec::xma2::{PACKET_SIZE, PacketHeader, Xma2Decoder};

/// A bank of `audio.rpf` that holds three mono 32 kHz XMA2 streams.
const BANK_OFFSET: u64 = 0x6_9DBC_6000;
/// The first of them: 58,880 samples in eight packets.
const STREAM_ID: u32 = 0x026F_4E1C;

fn read_bank() -> Option<Vec<u8>> {
    let dir = std::env::var_os("LEONIDA_GAME_DIR")?;
    let mut f = std::fs::File::open(std::path::Path::new(&dir).join("audio.rpf")).ok()?;
    f.seek(SeekFrom::Start(BANK_OFFSET)).ok()?;
    let mut buf = vec![0u8; 2 << 20];
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..]).ok()? {
            0 => break,
            n => got += n,
        }
    }
    buf.truncate(got);
    (buf.get(..4) == Some(b"ADAT")).then_some(buf)
}

#[test]
fn real_xma2_stream_decodes_to_its_declared_length() {
    let Some(bytes) = read_bank() else {
        eprintln!("skipped: no 1.0.2.1 audio.rpf");
        return;
    };
    let awc = Awc::parse(&bytes).unwrap();
    let Some(stream) = awc.stream_by_id(STREAM_ID) else {
        eprintln!("skipped: stream not in this bank");
        return;
    };
    let fmt = awc.format(stream).unwrap();
    assert_eq!(fmt.codec, Codec::Xma2);
    assert_eq!(
        (fmt.sample_rate, fmt.channels, fmt.samples),
        (32000, 1, 58880)
    );

    let table = awc.seek_table(stream).unwrap();
    assert_eq!(table, [0, 7680, 15360, 23040, 30208, 37376, 44544, 51712]);

    let data = awc.data(stream).unwrap();
    assert_eq!(data.len(), table.len() * PACKET_SIZE);
    for (i, p) in data.chunks(PACKET_SIZE).enumerate() {
        let h = PacketHeader::parse(p).unwrap();
        assert_eq!(h.carry_bits, 0, "packet {i}");
        assert_eq!(h.skip_count, 0, "packet {i}");
        let frames_before = table[i] / 512;
        let frames_here = u32::from(h.frame_count);
        let next = table.get(i + 1).copied().unwrap_or(fmt.samples) / 512;
        assert_eq!(frames_before + frames_here, next, "packet {i}");
    }

    let pcm = decode_stream(&awc, stream).unwrap();
    assert_eq!(pcm.frames(), 58880);
    let peak = pcm.samples.iter().fold(0f32, |a, &b| a.max(b.abs()));
    assert!((0.5..1.0).contains(&peak), "peak {peak}");
    assert!(pcm.samples.iter().all(|v| v.is_finite()));
    // the encoder starts with silence
    assert!(pcm.samples[..384].iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn seeking_into_a_real_stream_matches_straight_decoding() {
    let Some(bytes) = read_bank() else {
        return;
    };
    let awc = Awc::parse(&bytes).unwrap();
    let Some(stream) = awc.stream_by_id(STREAM_ID) else {
        return;
    };
    let fmt = awc.format(stream).unwrap();
    let data = awc.data(stream).unwrap();
    let table = awc.seek_table(stream).unwrap();
    let all = decode_stream(&awc, stream).unwrap();

    let cfg = audio_codec::decode::xma2_config(&fmt);
    let mut d = Xma2Decoder::new(cfg).unwrap();
    let target = 40_000u64;
    let first = d.seek(&table, target).unwrap();
    for p in data.chunks(PACKET_SIZE).skip(first) {
        d.push_packet(p).unwrap();
    }
    let mut got = vec![0f32; 5000];
    assert_eq!(d.pull_f32(&mut got), 5000);
    let want = &all.samples[target as usize..target as usize + 5000];
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-6);
    }
}

/// First bank of the archive (right after the audio config); read in full when it is small enough.
fn read_first_bank() -> Option<Vec<u8>> {
    let dir = std::env::var_os("LEONIDA_GAME_DIR")?;
    let mut f = std::fs::File::open(std::path::Path::new(&dir).join("audio.rpf")).ok()?;
    f.seek(SeekFrom::Start(0x129B000)).ok()?;
    let mut head = vec![0u8; 1 << 16];
    f.read_exact(&mut head).ok()?;
    let header_size = u32::from_le_bytes(head[12..16].try_into().ok()?) as usize;
    let awc = Awc::parse(head.get(..header_size)?).ok()?;
    let end = awc
        .streams
        .iter()
        .flat_map(|s| s.tags.iter())
        .map(|t| t.offset as usize + t.size as usize)
        .max()?;
    if end > 64 << 20 {
        return None;
    }
    f.seek(SeekFrom::Start(0x129B000)).ok()?;
    let mut bank = vec![0u8; end];
    f.read_exact(&mut bank).ok()?;
    Some(bank)
}

#[test]
fn real_opus_streams_repackage_to_well_formed_ogg() {
    use audio_codec::ogg::{FLAG_BOS, FLAG_EOS, read_pages};
    use audio_codec::opus::{DEFAULT_PRE_SKIP, declared_frames, stream_to_ogg};
    let Some(bytes) = read_first_bank() else {
        eprintln!("skipped: no readable first bank");
        return;
    };
    let awc = Awc::parse(&bytes).unwrap();
    let mut checked = 0;
    for s in &awc.streams {
        let Ok(fmt) = awc.format(s) else { continue };
        if fmt.codec != Codec::Opus || s.tag(audio_codec::awc::TAG_STREAMED_MUSIC).is_some() {
            continue;
        }
        let ogg = stream_to_ogg(&awc, s, DEFAULT_PRE_SKIP).unwrap();
        let pages = read_pages(&ogg).unwrap();
        assert_eq!(pages[0].flags, FLAG_BOS);
        assert_eq!(&pages[0].packets[0][..8], b"OpusHead");
        let last = pages.last().unwrap();
        assert_eq!(last.flags, FLAG_EOS);
        let frames = declared_frames(fmt.samples, fmt.opus_frames, fmt.channels);
        assert_eq!(last.granule as u64, u64::from(DEFAULT_PRE_SKIP) + frames);
        checked += 1;
        if checked == 20 {
            break;
        }
    }
    assert!(checked > 0);
}
