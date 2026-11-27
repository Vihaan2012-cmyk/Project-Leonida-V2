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
        let frames_before = u32::from(table[i] / 512);
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
