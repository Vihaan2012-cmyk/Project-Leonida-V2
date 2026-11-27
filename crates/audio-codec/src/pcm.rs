//! Decoded PCM held in memory.

/// Decoded interleaved PCM.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn to_i16(&self) -> Vec<i16> {
        self.samples.iter().map(|&v| f32_to_i16(v)).collect()
    }
}

pub fn f32_to_i16(v: f32) -> i16 {
    (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_rounds_and_clips() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(-1.0), -32768);
        assert_eq!(f32_to_i16(7.5), 32767);
        assert_eq!(f32_to_i16(-7.5), -32768);
        assert_eq!(f32_to_i16(0.5), 16384);
        assert_eq!(f32_to_i16(1.0 / 65536.0), 1);
    }

    #[test]
    fn frames_count_per_channel() {
        let p = Pcm {
            sample_rate: 8000,
            channels: 2,
            samples: vec![0.0; 10],
        };
        assert_eq!(p.frames(), 5);
        assert_eq!(p.to_i16().len(), 10);
    }
}
