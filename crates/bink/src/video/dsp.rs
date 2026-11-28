//! Inverse transform and block output.

#[inline]
fn mul_a(v: i16) -> i16 {
    v.wrapping_add(v >> 2)
}
#[inline]
fn mul_b(v: i16) -> i16 {
    v >> 1
}
#[inline]
fn mul_c(v: i16) -> i16 {
    v.wrapping_sub(v >> 2).wrapping_sub(v >> 4)
}
#[inline]
fn mul_d(v: i16) -> i16 {
    v.wrapping_add(v >> 2).wrapping_sub(v >> 4)
}
#[inline]
fn mul_e(v: i16) -> i16 {
    v >> 2
}

/// One 8-point pass in the game's 16-bit lane arithmetic (RVA 0x1bae0).
#[inline]
fn pass(s: [i16; 8]) -> [i16; 8] {
    let add = i16::wrapping_add;
    let sub = i16::wrapping_sub;
    let t00 = add(s[3], s[5]);
    let t01 = sub(s[3], s[5]);
    let t02 = add(mul_a(s[2]), mul_b(s[6]));
    let t03 = sub(mul_b(s[2]), mul_a(s[6]));
    let e0 = add(s[0], s[4]);
    let t0 = add(e0, t02);
    let t1 = sub(e0, t02);
    let t2 = sub(s[0], s[4]);
    let t3 = add(s[1], t00);
    let t4 = sub(s[1], t00);
    let t5 = add(t01, s[7]);
    let t6 = sub(t01, s[7]);
    let t7 = add(t4, mul_c(t6));
    let t8 = sub(mul_c(t4), t6);
    let t9 = add(mul_d(t3), mul_e(t5));
    let t10 = sub(mul_e(t3), mul_d(t5));
    let t11 = add(t2, t03);
    let t12 = sub(t2, t03);
    [
        add(t0, t9),
        add(t11, t7),
        add(t12, t8),
        add(t1, t10),
        sub(t1, t10),
        sub(t12, t8),
        sub(t11, t7),
        sub(t0, t9),
    ]
}

/// Inverse transform of coefficients stored `[r * 8 + c]` as the scan order places them. The game transforms along
/// `r` first and then along `c`, and the result is `[y * 8 + x]` with `x` the position along `r`; every step is
/// 16 bits wide, as in its vector code.
pub fn idct(coefs: &mut [i32; 64]) {
    let mut first = [[0i16; 8]; 8];
    for c in 0..8 {
        let out = pass(std::array::from_fn(|r| coefs[r * 8 + c] as i16));
        for (x, v) in out.iter().enumerate() {
            first[x][c] = *v;
        }
    }
    for (x, line) in first.iter().enumerate() {
        let out = pass(*line);
        for (y, v) in out.iter().enumerate() {
            coefs[y * 8 + x] = i32::from(*v >> 6);
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
            dst[at] = clip(i32::from(dst[at]) + coefs[y * 8 + x]);
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
    fn the_scan_row_is_transformed_before_the_scan_column() {
        // Coefficients at scan positions (r, c) = (1, 0) and (0, 1) and a few more. Transforming along r first
        // rounds differently from transforming along c first, so the order shows in the samples.
        let mut c = [0i32; 64];
        for (pos, v) in [(8, 37), (1, -21), (9, 13), (16, 7), (3, 5), (24, -9)] {
            c[pos] = v * 8;
        }
        let mut a = c;
        idct(&mut a);
        // The same transform written out axis by axis.
        let line = |s: [i16; 8]| pass(s);
        let mut by_column = [[0i16; 8]; 8];
        for col in 0..8 {
            let out = line(std::array::from_fn(|r| c[r * 8 + col] as i16));
            by_column[col] = out;
        }
        for x in 0..8 {
            let out = line(std::array::from_fn(|col| by_column[col][x]));
            for y in 0..8 {
                assert_eq!(a[y * 8 + x], i32::from(out[y] >> 6), "{x},{y}");
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
