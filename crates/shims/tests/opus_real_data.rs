//! Decodes the first packets of real Opus streams. Skipped when `LEONIDA_GAME_DIR` is unset or the package
//! is not the 1.0.2.1 build the bank offset was measured on.

use std::io::{Read, Seek, SeekFrom};

use audio_codec::awc::{Awc, Codec, TAG_STREAMED_MUSIC};
use audio_codec::opus::{packet_channels, split_blocks};
use shims::audio::opus::open;

const FIRST_BANK: u64 = 0x129B000;

fn read_first_bank() -> Option<Vec<u8>> {
    let dir = std::env::var_os("LEONIDA_GAME_DIR")?;
    let mut f = std::fs::File::open(std::path::Path::new(&dir).join("audio.rpf")).ok()?;
    f.seek(SeekFrom::Start(FIRST_BANK)).ok()?;
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
    f.seek(SeekFrom::Start(FIRST_BANK)).ok()?;
    let mut bank = vec![0u8; end];
    f.read_exact(&mut bank).ok()?;
    Some(bank)
}

#[test]
fn real_opus_packets_decode_to_audible_pcm() {
    let Some(bytes) = read_first_bank() else {
        eprintln!("skipped: no readable first bank");
        return;
    };
    let awc = Awc::parse(&bytes).unwrap();
    let mut audible = 0;
    let mut checked = 0;
    for s in &awc.streams {
        let Ok(fmt) = awc.format(s) else { continue };
        if fmt.codec != Codec::Opus || s.tag(TAG_STREAMED_MUSIC).is_some() {
            continue;
        }
        let data = awc.data(s).unwrap();
        let packets = split_blocks(data).unwrap();
        let Some(first) = packets.first() else {
            continue;
        };
        let channels = packet_channels(first);
        let mut stream = open(channels).unwrap();
        let mut peak = 0.0f32;
        for packet in packets.iter().take(20) {
            let mut out = vec![0.0f32; 5760 * usize::from(channels)];
            let frames = stream.decode(packet, 5760, &mut out).unwrap();
            assert!(frames > 0);
            assert!(out.iter().all(|v| v.is_finite()));
            peak = out.iter().fold(peak, |p, v| p.max(v.abs()));
        }
        checked += 1;
        audible += usize::from(peak > 1e-3);
        if checked == 20 {
            break;
        }
    }
    assert!(checked > 0);
    assert!(audible > 0, "{checked} streams decoded, all silent");
}
