//! Inverse MDCT (the "half" form WMA-family decoders use) through a DCT-IV on an N/2-point complex FFT.

use std::f64::consts::PI;

/// Produces the middle `n` samples of the `2n`-sample IMDCT of `n` coefficients,
/// `out[m] = scale * sum_k x[k] cos(pi/n * (m + n/2 + 1/2 + n/2) * (k + 1/2))`.
pub struct Imdct {
    n: usize,
    scale: f32,
    pre: Vec<(f32, f32)>,
    post: Vec<(f32, f32)>,
    fft_tw: Vec<(f32, f32)>,
    bitrev: Vec<u32>,
}

impl Imdct {
    pub fn new(n: usize, scale: f32) -> Self {
        assert!(n.is_power_of_two() && n >= 4);
        let m = n / 2;
        let tw = |i: usize| {
            let a = -PI * (i as f64 + 0.125) / n as f64;
            (a.cos() as f32, a.sin() as f32)
        };
        let fft_tw = (0..m / 2)
            .map(|i| {
                let a = -2.0 * PI * i as f64 / m as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let bits = m.trailing_zeros();
        let bitrev = (0..m as u32)
            .map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (32 - bits) })
            .collect();
        Self {
            n,
            scale,
            pre: (0..m).map(tw).collect(),
            post: (0..m).map(tw).collect(),
            fft_tw,
            bitrev,
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    fn fft(&self, re: &mut [f32], im: &mut [f32]) {
        let m = re.len();
        for i in 0..m {
            let j = self.bitrev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= m {
            let half = size / 2;
            let step = m / size;
            for start in (0..m).step_by(size) {
                for k in 0..half {
                    let (wr, wi) = self.fft_tw[k * step];
                    let a = start + k;
                    let b = a + half;
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }

    /// `coefs` and `out` both hold `n` values.
    pub fn process(&self, coefs: &[f32], out: &mut [f32]) {
        let n = self.n;
        let m = n / 2;
        debug_assert!(coefs.len() >= n && out.len() >= n);
        let mut re = vec![0f32; m];
        let mut im = vec![0f32; m];
        for i in 0..m {
            let a = coefs[2 * i];
            let b = coefs[n - 1 - 2 * i];
            let (wr, wi) = self.pre[i];
            re[i] = a * wr - b * wi;
            im[i] = a * wi + b * wr;
        }
        self.fft(&mut re, &mut im);
        for k in 0..m {
            let (wr, wi) = self.post[k];
            let yr = re[k] * wr - im[k] * wi;
            let yi = re[k] * wi + im[k] * wr;
            // DCT-IV outputs c[2k] = yr and c[n-1-2k] = -yi; the half IMDCT is the negated reversal.
            out[n - 1 - 2 * k] = -yr * self.scale;
            out[2 * k] = yi * self.scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_imdct(x: &[f32]) -> Vec<f64> {
        let n = x.len();
        (0..2 * n)
            .map(|i| {
                (0..n)
                    .map(|k| {
                        f64::from(x[k])
                            * (PI / n as f64 * (i as f64 + 0.5 + n as f64 / 2.0) * (k as f64 + 0.5))
                                .cos()
                    })
                    .sum()
            })
            .collect()
    }

    fn pseudo(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                ((s >> 8) as f32 / (1 << 24) as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn matches_direct_imdct_middle_half() {
        for n in [4usize, 8, 64, 128, 256, 512] {
            let x = pseudo(n, n as u32);
            let reference = full_imdct(&x);
            let imdct = Imdct::new(n, 0.5);
            let mut out = vec![0f32; n];
            imdct.process(&x, &mut out);
            for m in 0..n {
                let want = reference[n / 2 + m] * 0.5;
                assert!(
                    (f64::from(out[m]) - want).abs() < 2e-4 * (n as f64).sqrt(),
                    "n={n} m={m} got={} want={want}",
                    out[m]
                );
            }
        }
    }

    #[test]
    fn full_imdct_symmetries_hold() {
        let n = 64;
        let y = full_imdct(&pseudo(n, 9));
        for k in 0..n / 2 {
            assert!((y[k] + y[n - 1 - k]).abs() < 1e-9);
            assert!((y[2 * n - 1 - k] - y[n + k]).abs() < 1e-9);
        }
    }
}
