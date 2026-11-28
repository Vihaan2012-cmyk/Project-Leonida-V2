//! Uniformly partitioned FFT convolution: the arithmetic behind the title's convolution reverb.

use std::f64::consts::PI;

/// A radix-2 complex FFT of a fixed power-of-two size.
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    reverse: Vec<u32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(
            n.is_power_of_two() && n >= 2,
            "FFT size must be a power of two"
        );
        let bits = n.trailing_zeros();
        let cos = (0..n / 2)
            .map(|k| (2.0 * PI * k as f64 / n as f64).cos() as f32)
            .collect();
        let sin = (0..n / 2)
            .map(|k| (2.0 * PI * k as f64 / n as f64).sin() as f32)
            .collect();
        let reverse = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        Fft {
            n,
            cos,
            sin,
            reverse,
        }
    }

    pub fn size(&self) -> usize {
        self.n
    }

    /// In-place transform; the inverse is scaled by `1 / n`.
    pub fn transform(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let n = self.n;
        assert!(re.len() == n && im.len() == n);
        for i in 0..n {
            let j = self.reverse[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inverse { 1.0 } else { -1.0 };
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], sign * self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
        if inverse {
            let scale = 1.0 / n as f32;
            for v in re.iter_mut().chain(im.iter_mut()) {
                *v *= scale;
            }
        }
    }
}

/// Convolves a running signal, `block` samples at a time, with one impulse response. The output of each call is
/// the same `block` samples a direct convolution would give, with no added delay.
pub struct Convolver {
    block: usize,
    fft: Fft,
    parts: usize,
    ir_re: Vec<f32>,
    ir_im: Vec<f32>,
    hist_re: Vec<f32>,
    hist_im: Vec<f32>,
    head: usize,
    previous: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    acc_re: Vec<f32>,
    acc_im: Vec<f32>,
}

impl Convolver {
    /// `block` is a power of two; an empty `ir` convolves to silence.
    pub fn new(ir: &[f32], block: usize) -> Convolver {
        let fft = Fft::new(block * 2);
        let bins = block + 1;
        let parts = ir.len().div_ceil(block).max(1);
        let mut ir_re = vec![0.0; parts * bins];
        let mut ir_im = vec![0.0; parts * bins];
        let (mut re, mut im) = (vec![0.0; block * 2], vec![0.0; block * 2]);
        for (p, chunk) in ir.chunks(block).enumerate() {
            re.fill(0.0);
            im.fill(0.0);
            re[..chunk.len()].copy_from_slice(chunk);
            fft.transform(&mut re, &mut im, false);
            ir_re[p * bins..(p + 1) * bins].copy_from_slice(&re[..bins]);
            ir_im[p * bins..(p + 1) * bins].copy_from_slice(&im[..bins]);
        }
        Convolver {
            block,
            fft,
            parts,
            ir_re,
            ir_im,
            hist_re: vec![0.0; parts * bins],
            hist_im: vec![0.0; parts * bins],
            head: 0,
            previous: vec![0.0; block],
            re,
            im,
            acc_re: vec![0.0; bins],
            acc_im: vec![0.0; bins],
        }
    }

    pub fn block(&self) -> usize {
        self.block
    }

    pub fn partitions(&self) -> usize {
        self.parts
    }

    /// Consumes `block` input samples and writes the `block` output samples.
    pub fn process(&mut self, input: &[f32], out: &mut [f32]) {
        let (b, bins) = (self.block, self.block + 1);
        assert!(input.len() == b && out.len() == b);
        self.re[..b].copy_from_slice(&self.previous);
        self.re[b..].copy_from_slice(input);
        self.im.fill(0.0);
        self.fft.transform(&mut self.re, &mut self.im, false);
        self.head = (self.head + self.parts - 1) % self.parts;
        let slot = self.head * bins;
        self.hist_re[slot..slot + bins].copy_from_slice(&self.re[..bins]);
        self.hist_im[slot..slot + bins].copy_from_slice(&self.im[..bins]);
        self.previous.copy_from_slice(input);

        self.acc_re.fill(0.0);
        self.acc_im.fill(0.0);
        for p in 0..self.parts {
            let at = ((self.head + p) % self.parts) * bins;
            let ir = p * bins;
            let (xr, xi) = (&self.hist_re[at..at + bins], &self.hist_im[at..at + bins]);
            let (hr, hi) = (&self.ir_re[ir..ir + bins], &self.ir_im[ir..ir + bins]);
            for k in 0..bins {
                self.acc_re[k] += xr[k] * hr[k] - xi[k] * hi[k];
                self.acc_im[k] += xr[k] * hi[k] + xi[k] * hr[k];
            }
        }
        self.re[..bins].copy_from_slice(&self.acc_re);
        self.im[..bins].copy_from_slice(&self.acc_im);
        for k in 1..b {
            self.re[2 * b - k] = self.acc_re[k];
            self.im[2 * b - k] = -self.acc_im[k];
        }
        self.fft.transform(&mut self.re, &mut self.im, true);
        out.copy_from_slice(&self.re[b..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            })
            .collect()
    }

    fn direct(signal: &[f32], ir: &[f32]) -> Vec<f32> {
        (0..signal.len())
            .map(|n| {
                ir.iter()
                    .enumerate()
                    .take(n + 1)
                    .map(|(m, h)| h * signal[n - m])
                    .sum()
            })
            .collect()
    }

    #[test]
    fn the_fft_round_trips_and_finds_a_tone() {
        let fft = Fft::new(64);
        let mut re: Vec<f32> = (0..64)
            .map(|i| (2.0 * std::f32::consts::PI * 4.0 * i as f32 / 64.0).cos())
            .collect();
        let mut im = vec![0.0; 64];
        let original = re.clone();
        fft.transform(&mut re, &mut im, false);
        assert!((re[4] - 32.0).abs() < 1e-3 && (re[60] - 32.0).abs() < 1e-3);
        assert!(re[5].abs() < 1e-3);
        fft.transform(&mut re, &mut im, true);
        for (a, b) in re.iter().zip(&original) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn partitioned_convolution_matches_direct_convolution_across_blocks() {
        let block = 64;
        let ir = noise(300, 7);
        let signal = noise(block * 9, 11);
        let want = direct(&signal, &ir);
        let mut conv = Convolver::new(&ir, block);
        assert_eq!(conv.partitions(), 5);
        let mut got = Vec::new();
        for chunk in signal.chunks(block) {
            let mut out = vec![0.0; block];
            conv.process(chunk, &mut out);
            got.extend(out);
        }
        for (i, (a, b)) in got.iter().zip(&want).enumerate() {
            assert!((a - b).abs() < 2e-3, "sample {i}: {a} against {b}");
        }
    }

    #[test]
    fn a_delayed_impulse_delays_the_signal_and_the_tail_rings_out_after_the_input_stops() {
        let block = 256;
        let mut ir = vec![0.0; 700];
        ir[600] = 0.5;
        let mut conv = Convolver::new(&ir, block);
        let mut impulse = vec![0.0; block];
        impulse[0] = 1.0;
        let silence = vec![0.0; block];
        let mut heard = Vec::new();
        for i in 0..4 {
            let mut out = vec![0.0; block];
            conv.process(if i == 0 { &impulse } else { &silence }, &mut out);
            heard.extend(out);
        }
        let peak = heard
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap();
        assert_eq!(peak.0, 600);
        assert!((peak.1 - 0.5).abs() < 1e-4);
        assert!(
            heard
                .iter()
                .enumerate()
                .all(|(i, v)| i == 600 || v.abs() < 1e-4)
        );
    }

    #[test]
    fn an_empty_response_is_silence() {
        let mut conv = Convolver::new(&[], 32);
        let mut out = vec![1.0; 32];
        conv.process(&noise(32, 3), &mut out);
        assert!(out.iter().all(|v| v.abs() < 1e-6));
    }
}
