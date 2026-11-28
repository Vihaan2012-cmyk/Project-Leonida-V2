//! The inverse transform of Bink audio's DCT tracks: a DCT-III with the DC term at full weight, computed by a
//! complex FFT of twice the length (the cosines are the real part of a zero-padded inverse DFT).

use std::f64::consts::PI;

pub struct Dct3 {
    n: usize,
    /// `e^{i pi k / 2n}` for `k < n`.
    pre: Vec<(f64, f64)>,
    /// `e^{2 pi i k / 2n}` for `k < n`: the twiddles of the length-`2n` FFT.
    twiddle: Vec<(f64, f64)>,
    reverse: Vec<u32>,
    work: Vec<(f64, f64)>,
}

impl Dct3 {
    pub fn new(n: usize) -> Dct3 {
        assert!(n.is_power_of_two() && n >= 2);
        let m = 2 * n;
        let bits = m.trailing_zeros();
        Dct3 {
            n,
            pre: (0..n)
                .map(|k| {
                    let a = PI * k as f64 / m as f64;
                    (a.cos(), a.sin())
                })
                .collect(),
            twiddle: (0..n)
                .map(|k| {
                    let a = 2.0 * PI * k as f64 / m as f64;
                    (a.cos(), a.sin())
                })
                .collect(),
            reverse: (0..m as u32)
                .map(|i| i.reverse_bits() >> (32 - bits))
                .collect(),
            work: vec![(0.0, 0.0); m],
        }
    }

    /// `data[n] = sum_k data[k] cos(pi (2n + 1) k / 2N)`, in place.
    pub fn inverse(&mut self, data: &mut [f32]) {
        let n = self.n;
        assert_eq!(data.len(), n);
        let m = 2 * n;
        for i in 0..m {
            let source = self.reverse[i] as usize;
            self.work[i] = if source < n {
                let (c, s) = self.pre[source];
                let x = f64::from(data[source]);
                (x * c, x * s)
            } else {
                (0.0, 0.0)
            };
        }
        let mut half = 1;
        while half < m {
            let step = m / (2 * half);
            for start in (0..m).step_by(2 * half) {
                for j in 0..half {
                    let (wr, wi) = self.twiddle[j * step];
                    let (ar, ai) = self.work[start + j];
                    let (br, bi) = self.work[start + j + half];
                    let (tr, ti) = (br * wr - bi * wi, br * wi + bi * wr);
                    self.work[start + j] = (ar + tr, ai + ti);
                    self.work[start + j + half] = (ar - tr, ai - ti);
                }
            }
            half *= 2;
        }
        for (out, w) in data.iter_mut().zip(&self.work) {
            *out = w.0 as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(x: &[f32]) -> Vec<f64> {
        let n = x.len();
        (0..n)
            .map(|i| {
                x.iter()
                    .enumerate()
                    .map(|(k, &v)| {
                        f64::from(v) * (PI * (2 * i + 1) as f64 * k as f64 / (2 * n) as f64).cos()
                    })
                    .sum()
            })
            .collect()
    }

    fn signal(n: usize) -> Vec<f32> {
        (0..n)
            .map(|k| ((k * 37 + 11) % 101) as f32 / 50.0 - 1.0)
            .collect()
    }

    #[test]
    fn matches_the_direct_sum() {
        for n in [2, 8, 64, 256] {
            let x = signal(n);
            let want = reference(&x);
            let mut got = x.clone();
            Dct3::new(n).inverse(&mut got);
            for (g, w) in got.iter().zip(&want) {
                assert!((f64::from(*g) - w).abs() < 1e-3, "n={n}: {g} vs {w}");
            }
        }
    }

    #[test]
    fn a_lone_dc_term_is_a_constant_and_a_lone_cosine_is_that_cosine() {
        let mut t = Dct3::new(32);
        let mut dc = vec![0.0; 32];
        dc[0] = 0.5;
        t.inverse(&mut dc);
        assert!(dc.iter().all(|v| (v - 0.5).abs() < 1e-6));
        let mut one = vec![0.0; 32];
        one[3] = 1.0;
        t.inverse(&mut one);
        for (i, v) in one.iter().enumerate() {
            let want = (PI * (2 * i + 1) as f64 * 3.0 / 64.0).cos();
            assert!((f64::from(*v) - want).abs() < 1e-6);
        }
    }

    #[test]
    fn the_plan_can_be_reused() {
        let mut t = Dct3::new(16);
        let x = signal(16);
        let mut a = x.clone();
        let mut b = x.clone();
        t.inverse(&mut a);
        t.inverse(&mut b);
        assert_eq!(a, b);
    }
}
