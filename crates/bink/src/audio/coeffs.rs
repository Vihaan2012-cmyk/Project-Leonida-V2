//! One channel's spectrum of a Bink audio block: two floats, a quantiser per band, then the coefficients in runs
//! that each carry their own bit width.

use crate::bits::BitReader;
use crate::error::{Error, Result};

/// Upper edges of the 25 critical bands in Hz, the scale the encoder divides the spectrum by.
const CRITICAL_FREQS: [u32; 25] = [
    100, 200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320, 2700, 3150, 3700,
    4400, 5300, 6400, 7700, 9500, 12000, 15500, 24500,
];

/// Run lengths in units of 8 coefficients when a run is flagged long: indexed by the four bits after the flag.
const RUN_LENGTHS: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

/// The quantiser steps are `exp(index * QUANT_STEP)`: 0.0664 in decibel-like units, times `ln 10`.
const QUANT_STEP: f64 = 0.152_891_647_872_219_54;

/// The largest quantiser index; bigger stored values are clamped.
const QUANT_MAX: u32 = 95;

#[derive(Debug, Clone)]
pub struct Layout {
    pub frame_len: usize,
    pub overlap: usize,
    /// First coefficient of each band, then `frame_len`. Band 0 starts at 2: the first two coefficients are
    /// stored as floats.
    pub bands: Vec<usize>,
    /// Scale from the stored values to samples in `-1..1` for the unnormalised inverse DCT: `2 / sqrt(frame_len)`
    /// is the transform's gain, 1/32768 maps 16-bit full scale to 1.
    pub root: f32,
}

impl Layout {
    pub fn new(rate: u32) -> Layout {
        let bits = if rate < 22050 {
            9
        } else if rate < 44100 {
            10
        } else {
            11
        };
        let frame_len = 1usize << bits;
        let half = u64::from(rate).div_ceil(2).max(1);
        let mut count = 1;
        while count < 25 && half > u64::from(CRITICAL_FREQS[count - 1]) {
            count += 1;
        }
        let mut bands = vec![2];
        for freq in &CRITICAL_FREQS[..count - 1] {
            bands.push(((u64::from(*freq) * frame_len as u64 / half) & !1) as usize);
        }
        bands.push(frame_len);
        Layout {
            frame_len,
            overlap: frame_len / 16,
            bands,
            root: (2.0 / ((frame_len as f64).sqrt() * 32768.0)) as f32,
        }
    }

    pub fn band_count(&self) -> usize {
        self.bands.len() - 1
    }
}

/// A float stored as 5 bits of exponent, 23 bits of mantissa and a sign bit.
fn read_float(r: &mut BitReader) -> f32 {
    let power = r.read(5) as i32;
    let mantissa = r.read(23) as f32;
    let value = mantissa * 2f32.powi(power - 23);
    if r.read_bool() { -value } else { value }
}

/// Reads one channel's coefficients into `out` (`layout.frame_len` long).
///
/// The first two coefficients are floats. Then come one 7-bit quantiser index per band and the rest of the
/// spectrum in runs: a flag, an optional 4-bit length code, a 4-bit width, then the run's magnitudes packed at that
/// width followed by one sign bit for each non-zero magnitude.
pub fn read_channel(r: &mut BitReader, layout: &Layout, out: &mut [f32]) -> Result<()> {
    debug_assert_eq!(out.len(), layout.frame_len);
    out[0] = read_float(r) * layout.root;
    out[1] = read_float(r) * layout.root;
    let bands = layout.band_count();
    let mut quant = [0f32; 25];
    for q in quant.iter_mut().take(bands) {
        let index = r.read(7).min(QUANT_MAX);
        *q = ((f64::from(index) * QUANT_STEP).exp() as f32) * layout.root;
    }
    if r.overrun() {
        return Err(Error::Audio("block ends inside the band quantisers"));
    }
    let mut band = 0;
    let mut q = 0.0f32;
    let mut i = 2;
    while i < layout.frame_len {
        let end = if r.read_bool() {
            i + RUN_LENGTHS[r.read(4) as usize] * 8
        } else {
            i + 8
        }
        .min(layout.frame_len);
        let width = r.read(4);
        let mut signs = r.clone();
        signs.seek(r.pos() + (end - i) * width as usize);
        for k in i..end {
            while band < bands && layout.bands[band] <= k {
                q = quant[band];
                band += 1;
            }
            let magnitude = if width == 0 { 0 } else { r.read(width) };
            out[k] = if magnitude == 0 {
                0.0
            } else if signs.read_bool() {
                -q * magnitude as f32
            } else {
                q * magnitude as f32
            };
        }
        if width != 0 {
            r.seek(signs.pos());
        }
        if r.overrun() {
            return Err(Error::Audio("block ends inside the coefficients"));
        }
        i = end;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// LSB-first bit writer, the mirror of `BitReader`.
    #[derive(Default)]
    pub struct BitWriter {
        pub bytes: Vec<u8>,
        bits: usize,
    }

    impl BitWriter {
        pub fn put(&mut self, value: u32, n: u32) {
            for b in 0..n {
                if self.bits % 8 == 0 {
                    self.bytes.push(0);
                }
                if (value >> b) & 1 != 0 {
                    *self.bytes.last_mut().unwrap() |= 1 << (self.bits % 8);
                }
                self.bits += 1;
            }
        }

        pub fn bits(&self) -> usize {
            self.bits
        }

        pub fn float(&mut self, power: u32, mantissa: u32, negative: bool) {
            self.put(power, 5);
            self.put(mantissa, 23);
            self.put(u32::from(negative), 1);
        }

        pub fn align32(&mut self) {
            while self.bits % 32 != 0 {
                self.put(0, 1);
            }
        }

        /// A run: header, magnitudes, then a sign bit for each non-zero one. 8 values use the short header.
        pub fn run(&mut self, values: &[i32], width: u32) {
            if values.len() == 8 {
                self.put(0, 1);
            } else {
                let code = RUN_LENGTHS
                    .iter()
                    .position(|l| l * 8 == values.len())
                    .unwrap();
                self.put(1, 1);
                self.put(code as u32, 4);
            }
            self.put(width, 4);
            for v in values {
                self.put(v.unsigned_abs(), width);
            }
            for v in values.iter().filter(|v| **v != 0) {
                self.put(u32::from(*v < 0), 1);
            }
        }

        /// Zero-width runs until `covered` coefficients reach `len`; the last may overshoot, which the reader clips.
        pub fn zero_tail(&mut self, mut covered: usize, len: usize) {
            while covered < len {
                let left = len - covered;
                let code = RUN_LENGTHS
                    .iter()
                    .position(|l| l * 8 >= left)
                    .unwrap_or(RUN_LENGTHS.len() - 1);
                self.put(1, 1);
                self.put(code as u32, 4);
                self.put(0, 4);
                covered += RUN_LENGTHS[code] * 8;
            }
        }

        /// A whole channel: two zero floats, band quantiser indices, a first run at coefficient 2 and zeros after.
        pub fn channel(&mut self, layout: &Layout, quant: &[u32], first: &[i32], width: u32) {
            self.float(0, 0, false);
            self.float(0, 0, false);
            for k in 0..layout.band_count() {
                self.put(quant.get(k).copied().unwrap_or(0), 7);
            }
            self.run(first, width);
            self.zero_tail(2 + first.len(), layout.frame_len);
        }
    }

    #[test]
    fn band_edges_for_48k_cover_the_spectrum_and_are_even() {
        let l = Layout::new(48000);
        assert_eq!(l.frame_len, 2048);
        assert_eq!(l.overlap, 128);
        assert_eq!(l.band_count(), 25);
        assert_eq!(l.bands[0], 2);
        assert_eq!(l.bands[1], 8);
        assert_eq!(l.bands[25], 2048);
        assert!(l.bands.windows(2).all(|w| w[0] < w[1]));
        assert!(l.bands[..25].iter().all(|b| b % 2 == 0));
    }

    #[test]
    fn lower_rates_use_shorter_frames_and_fewer_bands() {
        assert_eq!(Layout::new(44100).frame_len, 2048);
        assert_eq!(Layout::new(32000).frame_len, 1024);
        let low = Layout::new(11025);
        assert_eq!(low.frame_len, 512);
        assert!(low.band_count() < 25);
        assert_eq!(*low.bands.last().unwrap(), 512);
    }

    #[test]
    fn floats_read_exponent_mantissa_sign() {
        let mut w = BitWriter::default();
        w.float(23, 3, false);
        w.float(21, 8, true);
        let mut r = BitReader::new(&w.bytes);
        assert_eq!(read_float(&mut r), 3.0);
        assert_eq!(read_float(&mut r), -2.0);
    }

    fn read(layout: &Layout, w: &BitWriter) -> (Vec<f32>, usize) {
        let mut r = BitReader::new(&w.bytes);
        let mut out = vec![9.0; layout.frame_len];
        read_channel(&mut r, layout, &mut out).unwrap();
        (out, r.pos())
    }

    #[test]
    fn magnitudes_come_first_then_one_sign_per_non_zero_magnitude() {
        let layout = Layout::new(48000);
        let mut w = BitWriter::default();
        w.channel(&layout, &[], &[1, 0, -7, 2, 0, 0, -5, 3], 3);
        let (out, end) = read(&layout, &w);
        assert_eq!(end, w.bits());
        let want = [1.0, 0.0, -7.0, 2.0, 0.0, 0.0, -5.0, 3.0];
        for (k, v) in want.iter().enumerate() {
            assert!(
                (out[2 + k] - v * layout.root).abs() < 1e-9,
                "coefficient {}",
                2 + k
            );
        }
        assert!(out[10..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_sign_bits_of_one_run_do_not_leak_into_the_next() {
        let layout = Layout::new(48000);
        let mut w = BitWriter::default();
        w.float(0, 0, false);
        w.float(0, 0, false);
        for _ in 0..layout.band_count() {
            w.put(0, 7);
        }
        w.run(&[-1, -1, 1, 0, 0, 0, 0, 0], 1);
        w.run(&[0, 0, 0, 0, 0, 0, 0, 1], 1);
        w.zero_tail(18, layout.frame_len);
        let (out, end) = read(&layout, &w);
        assert_eq!(end, w.bits());
        let signs: Vec<i32> = out[2..18]
            .iter()
            .map(|v| {
                if *v == 0.0 {
                    0
                } else if *v < 0.0 {
                    -1
                } else {
                    1
                }
            })
            .collect();
        assert_eq!(signs, [-1, -1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn long_runs_use_the_length_table() {
        let layout = Layout::new(48000);
        let mut w = BitWriter::default();
        w.channel(&layout, &[], &[1; 16], 1);
        let (out, end) = read(&layout, &w);
        assert_eq!(end, w.bits());
        assert!(out[2..18].iter().all(|v| (v - layout.root).abs() < 1e-9));
        assert!(out[18..].iter().all(|v| *v == 0.0));
        let mut w = BitWriter::default();
        w.channel(&layout, &[], &[1; 64], 2);
        let (out, _) = read(&layout, &w);
        assert!(out[2..66].iter().all(|v| (v - layout.root).abs() < 1e-9));
        assert_eq!(out[66], 0.0);
    }

    #[test]
    fn each_band_uses_its_own_quantiser() {
        let layout = Layout::new(48000);
        let mut quant = [0u32; 25];
        quant[0] = 10;
        quant[1] = 40;
        let mut w = BitWriter::default();
        w.channel(&layout, &quant, &[1; 8], 1);
        let (out, _) = read(&layout, &w);
        let step = |i: u32| ((f64::from(i) * QUANT_STEP).exp() as f32) * layout.root;
        assert!((out[2] - step(10)).abs() < 1e-9);
        assert!((out[7] - step(10)).abs() < 1e-9);
        assert!((out[8] - step(40)).abs() < 1e-9);
        assert!((out[9] - step(40)).abs() < 1e-9);
    }

    #[test]
    fn a_quantiser_carries_across_a_zero_run_that_crosses_a_band_edge() {
        let layout = Layout::new(48000);
        let mut quant = [0u32; 25];
        quant[1] = 20;
        quant[2] = 30;
        let mut w = BitWriter::default();
        w.float(0, 0, false);
        w.float(0, 0, false);
        for q in &quant[..layout.band_count()] {
            w.put(*q, 7);
        }
        w.run(&[0; 8], 0);
        w.run(&[1; 8], 1);
        w.zero_tail(18, layout.frame_len);
        let (out, _) = read(&layout, &w);
        let step = |i: u32| ((f64::from(i) * QUANT_STEP).exp() as f32) * layout.root;
        assert!(layout.bands[1] < 10 && layout.bands[2] > 10);
        assert!((out[10] - step(20)).abs() < 1e-9);
    }

    #[test]
    fn the_leading_floats_are_scaled_and_quantiser_indices_clamp() {
        let layout = Layout::new(48000);
        let mut w = BitWriter::default();
        w.float(23, 1, false);
        w.float(23, 4, true);
        let mut quant = [0u32; 25];
        quant[0] = 127;
        for q in quant.iter().take(layout.band_count()) {
            w.put(*q, 7);
        }
        w.run(&[1, 0, 0, 0, 0, 0, 0, 0], 1);
        w.zero_tail(10, layout.frame_len);
        let (out, _) = read(&layout, &w);
        assert!((out[0] - layout.root).abs() < 1e-9);
        assert!((out[1] + 4.0 * layout.root).abs() < 1e-9);
        let clamped = ((95.0 * QUANT_STEP).exp() as f32) * layout.root;
        assert!((out[2] - clamped).abs() / clamped < 1e-6);
    }

    #[test]
    fn a_block_that_runs_out_of_bits_is_an_error() {
        let layout = Layout::new(48000);
        let mut out = vec![0.0; layout.frame_len];
        let mut w = BitWriter::default();
        w.float(0, 0, false);
        w.float(0, 0, false);
        for _ in 0..layout.band_count() {
            w.put(0, 7);
        }
        let mut r = BitReader::new(&w.bytes);
        assert!(read_channel(&mut r, &layout, &mut out).is_err());
        w.put(1, 1);
        w.put(15, 4);
        w.put(15, 4);
        let mut r = BitReader::new(&w.bytes);
        assert!(read_channel(&mut r, &layout, &mut out).is_err());
    }
}
