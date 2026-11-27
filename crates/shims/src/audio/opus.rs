//! The Opus decoder installed for the coprocessor worker: `opus-pure`, 48 kHz, interleaved `f32`.

use opus_pure::OpusDecoder;

use super::xapu::OpusStream;

const SAMPLE_RATE: i32 = 48_000;

struct PureOpus {
    decoder: OpusDecoder,
    channels: usize,
}

impl OpusStream for PureOpus {
    fn decode(&mut self, packet: &[u8], frames: usize, out: &mut [f32]) -> Result<usize, String> {
        let frames = frames.min(out.len() / self.channels);
        self.decoder
            .decode(packet, frames, &mut out[..frames * self.channels])
            .map_err(|e| e.to_string())
    }

    fn reset(&mut self) {
        let _ = self.decoder.reset_state();
    }
}

/// The [`OpusFactory`](super::OpusFactory) for `set_opus_factory`: a mono or stereo decoder at 48 kHz.
pub fn open(channels: u8) -> Option<Box<dyn OpusStream>> {
    let channels = usize::from(channels.clamp(1, 2));
    let decoder = OpusDecoder::new(SAMPLE_RATE, channels).ok()?;
    Some(Box::new(PureOpus { decoder, channels }))
}

#[cfg(test)]
mod tests {
    use opus_pure::{Application, OpusEncoder};

    use super::*;

    fn tone(frames: usize, channels: usize, offset: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = (i + offset) as f32 / 48_000.0;
                let v = 0.4 * (t * 440.0 * std::f32::consts::TAU).sin();
                (0..channels).map(move |c| if c == 0 { v } else { -v })
            })
            .collect()
    }

    fn encode(channels: usize, packets: usize) -> Vec<Vec<u8>> {
        let mut encoder = OpusEncoder::new(48_000, channels, Application::Audio).unwrap();
        (0..packets)
            .map(|p| {
                let mut buf = vec![0u8; 4000];
                let n = encoder
                    .encode(&tone(960, channels, p * 960), 960, &mut buf)
                    .unwrap();
                buf.truncate(n);
                buf
            })
            .collect()
    }

    fn energy(pcm: &[f32]) -> f32 {
        pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32
    }

    #[test]
    fn stereo_packets_decode_to_960_frames_with_signal() {
        let mut stream = open(2).unwrap();
        let mut last = vec![0.0f32; 1920];
        for packet in encode(2, 8) {
            let mut out = vec![0.0f32; 1920];
            assert_eq!(stream.decode(&packet, 960, &mut out).unwrap(), 960);
            last = out;
        }
        assert!(energy(&last) > 0.01, "decoded audio is silent");
        assert!(last.chunks(2).any(|f| f[0] * f[1] < 0.0));
    }

    #[test]
    fn mono_packets_decode_to_960_frames_with_signal() {
        let mut stream = open(1).unwrap();
        let mut last = vec![0.0f32; 960];
        for packet in encode(1, 8) {
            assert_eq!(stream.decode(&packet, 960, &mut last).unwrap(), 960);
        }
        assert!(energy(&last) > 0.01);
    }

    #[test]
    fn toc_only_celt_packet_is_silence() {
        let mut stream = open(1).unwrap();
        let mut out = vec![1.0f32; 960];
        assert_eq!(stream.decode(&[0xF8], 960, &mut out).unwrap(), 960);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn garbage_is_an_error_and_reset_recovers() {
        let mut stream = open(2).unwrap();
        let mut out = vec![0.0f32; 1920];
        assert!(stream.decode(&[0xFB, 0x00], 960, &mut out).is_err());
        stream.reset();
        let packet = &encode(2, 1)[0];
        assert_eq!(stream.decode(packet, 960, &mut out).unwrap(), 960);
    }

    #[test]
    fn a_buffer_shorter_than_the_packet_is_an_error() {
        let mut stream = open(1).unwrap();
        let mut out = vec![0.0f32; 480];
        let packet = &encode(1, 1)[0];
        assert!(stream.decode(packet, 960, &mut out).is_err());
    }
}
