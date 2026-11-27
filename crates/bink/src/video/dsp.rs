//! Inverse transform and block output.

#[inline]
fn mul_a(v: i32) -> i32 {
    v + (v >> 2)
}
#[inline]
fn mul_b(v: i32) -> i32 {
    v >> 1
}
#[inline]
fn mul_c(v: i32) -> i32 {
    v - (v >> 2) - (v >> 4)
}
#[inline]
fn mul_d(v: i32) -> i32 {
    v + (v >> 2) - (v >> 4)
}
#[inline]
fn mul_e(v: i32) -> i32 {
    v >> 2
}

/// One 8-point pass. `src` is read with `step` between elements, the result is returned in order.
#[inline]
fn pass(s: [i32; 8]) -> [i32; 8] {
    let t00 = s[3] + s[5];
    let t01 = s[3] - s[5];
    let t02 = mul_a(s[2]) + mul_b(s[6]);
    let t03 = mul_b(s[2]) - mul_a(s[6]);
    let e0 = s[0] + s[4];
    let t0 = e0 + t02;
    let t1 = e0 - t02;
    let t2 = s[0] - s[4];
    let t3 = s[1] + t00;
    let t4 = s[1] - t00;
    let t5 = t01 + s[7];
    let t6 = t01 - s[7];
    let t7 = t4 + mul_c(t6);
    let t8 = mul_c(t4) - t6;
    let t9 = mul_d(t3) + mul_e(t5);
    let t10 = mul_e(t3) - mul_d(t5);
    let t11 = t2 + t03;
    let t12 = t2 - t03;
    [
        t0 + t9,
        t11 + t7,
        t12 + t8,
        t1 + t10,
        t1 - t10,
        t12 - t8,
        t11 - t7,
        t0 - t9,
    ]
}

/// Inverse transform of coefficients stored `[horizontal frequency * 8 + vertical frequency]`; the result is
/// `[x * 8 + y]` scaled so that a DC value of `8 * v` gives the sample `v / 8`... see the callers for rounding.
pub fn idct(coefs: &mut [i32; 64]) {
    let mut tmp = [0i32; 64];
    for i in 0..8 {
        let mut row = [0i32; 8];
        row.copy_from_slice(&coefs[i * 8..i * 8 + 8]);
        let out = pass(row);
        for (j, v) in out.iter().enumerate() {
            tmp[j * 8 + i] = *v;
        }
    }
    for i in 0..8 {
        let mut row = [0i32; 8];
        row.copy_from_slice(&tmp[i * 8..i * 8 + 8]);
        let out = pass(row);
        for (j, v) in out.iter().enumerate() {
            coefs[i * 8 + j] = *v >> 6;
        }
    }
}

#[inline]
pub fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Writes the transformed block at `dst` (row stride `stride`).
pub fn put_block(coefs: &mut [i32; 64], dst: &mut [u8], stride: usize) {
    idct(coefs);
    for y in 0..8 {
        for x in 0..8 {
            dst[y * stride + x] = clip(coefs[y * 8 + x]);
        }
    }
}

/// Adds the transformed block to the prediction already at `dst`.
pub fn add_block(coefs: &mut [i32; 64], dst: &mut [u8], stride: usize) {
    idct(coefs);
    for y in 0..8 {
        for x in 0..8 {
            let at = y * stride + x;
            dst[at] = clip(i32::from(dst[at]) + coefs[x * 8 + y]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_block_is_flat() {
        let mut c = [0i32; 64];
        c[0] = 800 * 8 + 32;
        let mut out = [0u8; 8 * 8];
        put_block(&mut c, &mut out, 8);
        assert!(out.iter().all(|&v| v == 100), "{out:?}");
    }

    #[test]
    fn transform_is_separable_and_symmetric_in_sign() {
        let mut a = [0i32; 64];
        a[1] = 400;
        let mut b = [0i32; 64];
        b[1] = -400;
        idct(&mut a);
        idct(&mut b);
        for i in 0..64 {
            assert!((a[i] + b[i]).abs() <= 1, "{i}");
        }
        // A single horizontal-frequency coefficient varies along one axis only.
        let column: Vec<i32> = (0..8).map(|col| a[col * 8]).collect();
        assert!(column.iter().any(|&v| v != column[0]));
        for col in 0..8 {
            for row in 1..8 {
                assert_eq!(a[col * 8 + row], a[col * 8]);
            }
        }
    }

    #[test]
    fn add_block_clips() {
        let mut c = [0i32; 64];
        c[0] = 2000 * 8;
        let mut out = [200u8; 64];
        add_block(&mut c, &mut out, 8);
        assert!(out.iter().all(|&v| v == 255));
    }
}
