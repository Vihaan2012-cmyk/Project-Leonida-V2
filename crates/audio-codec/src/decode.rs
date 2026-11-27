//! Decoding whole AWC streams to PCM and WAV.

use crate::awc::{Awc, AwcStream, Codec, StreamFormat, pair_regions};
use crate::pcm::Pcm;
use crate::wav::{WavExtras, WavLoop, f32_wav, pcm16_wav};
use crate::xma2::{Xma2Config, Xma2Decoder};
use crate::{Error, Result};

/// The XMA2 decoder settings an AWC format tag implies: the declared sample count is the output length.
pub fn xma2_config(fmt: &StreamFormat) -> Xma2Config {
    Xma2Config::new(
        u32::from(fmt.sample_rate),
        usize::from(fmt.channels.max(1)),
        Some(u64::from(fmt.samples)),
    )
}

fn pcm_frames(declared: u32, data_len: usize, bytes_per_sample: usize, channels: usize) -> usize {
    (declared as usize).min(data_len / (bytes_per_sample * channels))
}

/// Decode a PCM16, PCM float or XMA2 stream. Opus streams are not decoded here (see [`crate::opus`] for the
/// Ogg repackage); other codecs are refused.
pub fn decode_stream(awc: &Awc<'_>, stream: &AwcStream) -> Result<Pcm> {
    let fmt = awc.format(stream)?;
    let data = awc.data(stream)?;
    let channels = usize::from(fmt.channels.max(1));
    let rate = u32::from(fmt.sample_rate);
    match fmt.codec {
        Codec::Pcm16 => {
            let frames = pcm_frames(fmt.samples, data.len(), 2, channels);
            let samples = data[..frames * channels * 2]
                .chunks_exact(2)
                .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
                .collect();
            Ok(Pcm {
                sample_rate: rate,
                channels,
                samples,
            })
        }
        Codec::PcmF32 => {
            let frames = pcm_frames(fmt.samples, data.len(), 4, channels);
            let samples = data[..frames * channels * 4]
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            Ok(Pcm {
                sample_rate: rate,
                channels,
                samples,
            })
        }
        Codec::Xma2 => Xma2Decoder::decode_all(xma2_config(&fmt), data),
        Codec::Opus => Err(Error::Unsupported(
            "decoding Opus to PCM (only the Ogg Opus repackage is implemented)".into(),
        )),
        Codec::Other(id) => Err(Error::Unsupported(format!("audio codec {id:#x}"))),
    }
}

/// A WAV file for a stream: 16-bit PCM for PCM16 and XMA2, IEEE float for codec 2 (values can exceed 1).
/// Marker offsets become cue points and region cues become loops.
pub fn stream_wav(awc: &Awc<'_>, stream: &AwcStream) -> Result<Vec<u8>> {
    let pcm = decode_stream(awc, stream)?;
    let fmt = awc.format(stream)?;
    let markers = awc.markers(stream)?;
    let extras = WavExtras {
        cues: markers.iter().map(|m| m.offset).collect(),
        loops: pair_regions(&markers)
            .into_iter()
            .map(|r| WavLoop {
                start: r.start,
                end: r.end.saturating_sub(1),
            })
            .collect(),
    };
    let channels = pcm.channels as u16;
    Ok(match fmt.codec {
        Codec::PcmF32 => f32_wav(pcm.sample_rate, channels, &pcm.samples, &extras),
        _ => pcm16_wav(pcm.sample_rate, channels, &pcm.to_i16(), &extras),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awc::build::*;
    use crate::awc::{TAG_DATA, TAG_FORMAT, TAG_MARKERS, joaat};
    use crate::wav::Wav;
    use crate::xma2::testenc::*;

    fn bank_of(
        codec: u8,
        channels: u8,
        samples: u32,
        data: Vec<u8>,
        extra: Vec<(u8, Vec<u8>)>,
    ) -> Vec<u8> {
        let mut tags = vec![
            (TAG_FORMAT, format_tag(samples, 32000, codec, channels)),
            (TAG_DATA, data),
        ];
        tags.extend(extra);
        bank(0, &[StreamSpec { id: 5, tags }])
    }

    #[test]
    fn pcm16_stream_decodes_and_exports() {
        let raw: Vec<i16> = vec![0, 16384, -16384, 32767, -32768, 100];
        let data: Vec<u8> = raw.iter().flat_map(|v| v.to_le_bytes()).collect();
        let bytes = bank_of(0, 2, 3, data, vec![]);
        let awc = Awc::parse(&bytes).unwrap();
        let pcm = decode_stream(&awc, &awc.streams[0]).unwrap();
        assert_eq!((pcm.channels, pcm.sample_rate, pcm.frames()), (2, 32000, 3));
        assert_eq!(pcm.samples[1], 0.5);
        assert_eq!(pcm.to_i16(), raw);
        let wav = Wav::parse(&stream_wav(&awc, &awc.streams[0]).unwrap()).unwrap();
        assert_eq!(wav.samples_i16().unwrap(), raw);
    }

    #[test]
    fn float_stream_keeps_values_above_one() {
        let raw = [0.25f32, 3.5, -2.0, 0.0];
        let data: Vec<u8> = raw.iter().flat_map(|v| v.to_le_bytes()).collect();
        let bytes = bank_of(2, 1, 4, data, vec![]);
        let awc = Awc::parse(&bytes).unwrap();
        let wav = Wav::parse(&stream_wav(&awc, &awc.streams[0]).unwrap()).unwrap();
        assert_eq!(wav.samples_f32().unwrap(), raw);
    }

    #[test]
    fn declared_length_limits_pcm_and_short_data_is_not_overread() {
        let data: Vec<u8> = (0..10i16).flat_map(|v| v.to_le_bytes()).collect();
        let bytes = bank_of(0, 1, 4, data.clone(), vec![]);
        let awc = Awc::parse(&bytes).unwrap();
        assert_eq!(decode_stream(&awc, &awc.streams[0]).unwrap().frames(), 4);
        let bytes = bank_of(0, 1, 400, data, vec![]);
        let awc = Awc::parse(&bytes).unwrap();
        assert_eq!(decode_stream(&awc, &awc.streams[0]).unwrap().frames(), 10);
    }

    #[test]
    fn xma2_stream_matches_direct_decoding_and_carries_cues() {
        let mut rng = Rng(77);
        let frames: Vec<Bits> = (0..5)
            .map(|_| {
                let t = random_tuples(&mut rng, 30);
                finish_frame(&encode_frame(&[&t], 0, false))
            })
            .collect();
        let packets = packetize(&frames);
        let data: Vec<u8> = packets.concat();
        let (start, end) = (joaat("start"), joaat("end"));
        let mut m = Vec::new();
        for v in [7u32, start, 100, 0, 7, end, 900, 0] {
            m.extend_from_slice(&v.to_le_bytes());
        }
        let bytes = bank_of(5, 1, 5 * 512, data.clone(), vec![(TAG_MARKERS, m)]);
        let awc = Awc::parse(&bytes).unwrap();
        let s = &awc.streams[0];
        let pcm = decode_stream(&awc, s).unwrap();
        assert_eq!(pcm.frames(), 2560);
        let direct = Xma2Decoder::decode_all(Xma2Config::new(32000, 1, Some(2560)), &data).unwrap();
        assert_eq!(pcm, direct);
        let wav = Wav::parse(&stream_wav(&awc, s).unwrap()).unwrap();
        assert_eq!(wav.samples_i16().unwrap(), pcm.to_i16());
        assert_eq!(wav.extras.cues, vec![100, 900]);
        assert_eq!(
            wav.extras.loops,
            vec![WavLoop {
                start: 100,
                end: 899
            }]
        );
    }

    #[test]
    fn opus_and_unknown_codecs_are_typed_errors() {
        for codec in [0x0Bu8, 9] {
            let bytes = bank_of(codec, 1, 10, vec![0; 16], vec![]);
            let awc = Awc::parse(&bytes).unwrap();
            assert!(matches!(
                decode_stream(&awc, &awc.streams[0]),
                Err(Error::Unsupported(_))
            ));
        }
    }
}
