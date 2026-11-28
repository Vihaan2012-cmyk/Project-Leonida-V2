//! Motion vectors of inter macroblocks: prediction from neighbours and motion compensation.

use super::Plane;
use super::pred::median3;

/// Vectors of one macroblock: one `[x, y]` per 16x16 luma quarter, in half-pel luma units (quarter-pel in chroma).
pub type Mvs = [[i32; 2]; 4];

/// Motion vectors of the macroblock row being decoded and of the row above it.
pub struct MvLine {
    cur: Vec<Mvs>,
    prev: Vec<Mvs>,
}

fn med(a: [i32; 2], b: [i32; 2], c: [i32; 2]) -> [i32; 2] {
    [median3(a[0], b[0], c[0]), median3(a[1], b[1], c[1])]
}

fn add(a: [i32; 2], b: [i32; 2]) -> [i32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

impl MvLine {
    pub fn new(blocks: usize) -> Self {
        Self {
            cur: vec![[[0; 2]; 4]; blocks],
            prev: vec![[[0; 2]; 4]; blocks],
        }
    }

    pub fn next_row(&mut self) {
        std::mem::swap(&mut self.cur, &mut self.prev);
        self.cur.fill([[0; 2]; 4]);
    }

    /// Turns the decoded differences into vectors, stores them for macroblock `bx` and returns them. `count` is
    /// 1 when a single vector covers the macroblock; intra macroblocks pass 0 and all-zero differences.
    pub fn predict(&mut self, bx: usize, top: bool, left: bool, count: usize, d: &Mvs) -> Mvs {
        let l = self.cur[bx.saturating_sub(1)];
        let lt = self.prev[bx.saturating_sub(1)];
        let t = self.prev[bx];
        let out: Mvs = if count == 1 {
            let base = match (top, left) {
                (true, true) => [0, 0],
                (true, false) => med(l[0], l[1], l[3]),
                (false, false) => med(lt[3], t[2], l[1]),
                (false, true) => med(t[0], t[2], t[3]),
            };
            [add(d[0], base); 4]
        } else {
            let mut v = [[0i32; 2]; 4];
            match (top, left) {
                (false, true) => {
                    v[0] = add(d[0], med(t[0], t[2], t[3]));
                    v[1] = add(d[1], med(t[2], t[3], v[0]));
                    v[2] = add(d[2], med(t[2], v[0], v[1]));
                    v[3] = add(d[3], med(v[0], v[1], v[2]));
                }
                (false, false) => {
                    v[0] = add(d[0], med(t[2], lt[3], l[1]));
                    v[1] = add(d[1], med(t[2], t[3], v[0]));
                    v[2] = add(d[2], med(l[1], l[3], v[0]));
                    v[3] = add(d[3], med(v[0], v[1], v[2]));
                }
                (true, true) => {
                    v[0] = d[0];
                    v[1] = add(d[1], v[0]);
                    v[2] = add(d[2], v[0]);
                    v[3] = add(d[3], med(v[0], v[1], v[2]));
                }
                (true, false) => {
                    v[0] = add(d[0], med(l[0], l[1], l[3]));
                    v[2] = add(d[2], med(l[1], l[3], v[0]));
                    v[1] = add(d[1], med(l[1], v[0], v[2]));
                    v[3] = add(d[3], med(v[0], v[1], v[2]));
                }
            }
            v
        };
        self.cur[bx] = out;
        out
    }
}

/// Source samples with the nearest edge sample standing in for anything outside the plane.
struct Source<'a> {
    plane: &'a Plane,
    rows: usize,
}

impl<'a> Source<'a> {
    fn new(plane: &'a Plane) -> Self {
        Self {
            plane,
            rows: plane.data.len() / plane.stride,
        }
    }

    #[inline]
    fn at(&self, x: i32, y: i32) -> i32 {
        let (mx, my) = (self.plane.stride as i32 - 1, self.rows as i32 - 1);
        let x = x.clamp(0, mx) as usize;
        let y = y.clamp(0, my) as usize;
        i32::from(self.plane.data[y * self.plane.stride + x])
    }
}

#[inline]
fn clip(v: i32) -> i32 {
    v.clamp(0, 255)
}

/// Half-sample position of the six-tap luma filter between `s[1]` and `s[2]` of `s[0..6]`, not clipped.
#[inline]
fn luma_tap(s: [i32; 6]) -> i32 {
    ((((s[2] + s[3]) * 19) >> 1) - (s[1] + s[4]) * 2 + ((s[0] + s[5]) >> 1) + 8) >> 4
}

/// Chroma interpolation of `a` and `b` at quarter position `frac`, kept at sixteen times the sample scale.
#[inline]
fn chroma_tap(a: i32, b: i32, frac: i32) -> i32 {
    4 * ((4 - frac) * a + frac * b)
}

/// Where a block's source lies, clamped the way the game does (RVA 0x1cf40) before it predicts anything: the vector
/// is added to the block's own position in half samples, and a source that would reach outside the picture (taps
/// of the luma filter included) moves back in as a whole. `size` is the picture's extent in half samples along the
/// axis, `snug` the frame flag 0x20000 that lets a block with a fractional position sit one half sample closer to
/// the far edge.
pub fn clamp_source(position: i32, size: i32, snug: bool) -> i32 {
    let frac = (position & 1) as usize;
    let reach = [32, 42][frac];
    let (low, high) = if snug {
        let low = [0, 5][frac];
        (low, low + size - reach)
    } else {
        let low = [0, 4][frac];
        (low, size - reach)
    };
    if position < low {
        low
    } else {
        position.min(high)
    }
}

/// Predicts the 16x16 luma block at (`bx`, `by`) of `dst` from `src` at the half-sample position `pos`. A
/// two-dimensional position filters the rows first and keeps those results unclipped for the column pass.
pub fn luma_block(dst: &mut Plane, src: &Plane, bx: usize, by: usize, pos: [i32; 2]) {
    let s = Source::new(src);
    let (x0, y0) = (pos[0] >> 1, pos[1] >> 1);
    let (fx, fy) = (pos[0] & 1 != 0, pos[1] & 1 != 0);
    let mut tmp = [[0i32; 16]; 21];
    for (r, row) in tmp.iter_mut().enumerate() {
        let y = y0 + r as i32 - 2;
        for (c, v) in row.iter_mut().enumerate() {
            let x = x0 + c as i32;
            *v = if fx {
                luma_tap(std::array::from_fn(|k| s.at(x + k as i32 - 2, y)))
            } else {
                s.at(x, y)
            };
        }
    }
    for j in 0..16 {
        let out = &mut dst.data[(by + j) * dst.stride + bx..][..16];
        for (i, o) in out.iter_mut().enumerate() {
            let v = if fy {
                luma_tap(std::array::from_fn(|k| tmp[j + k][i]))
            } else {
                tmp[j + 2][i]
            };
            *o = clip(v) as u8;
        }
    }
}

/// Predicts the 8x8 chroma block at (`bx`, `by`) of `dst` from `src` at the quarter-sample position `pos` (the
/// luma half-sample position of the block is the chroma quarter-sample position). The rows are interpolated at
/// sixteen times the sample scale and rounded once after the columns.
pub fn chroma_block(dst: &mut Plane, src: &Plane, bx: usize, by: usize, pos: [i32; 2]) {
    let s = Source::new(src);
    let (x0, y0) = (pos[0] >> 2, pos[1] >> 2);
    let (fx, fy) = (pos[0] & 3, pos[1] & 3);
    let mut tmp = [[0i32; 8]; 9];
    for (r, row) in tmp.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            let (x, y) = (x0 + c as i32, y0 + r as i32);
            *v = chroma_tap(s.at(x, y), s.at(x + 1, y), fx);
        }
    }
    for j in 0..8 {
        let out = &mut dst.data[(by + j) * dst.stride + bx..][..8];
        for (i, o) in out.iter_mut().enumerate() {
            let v = (chroma_vertical(tmp[j][i], tmp[j + 1][i], fy) + 8) >> 4;
            *o = clip(v) as u8;
        }
    }
}

/// Column step of the chroma filter on rows already at sixteen times the sample scale.
#[inline]
fn chroma_vertical(a: i32, b: i32, frac: i32) -> i32 {
    ((4 - frac) * a + frac * b) / 4
}

/// Copies the `size` x `size` block at (`bx`, `by`) unchanged.
pub fn copy_block(dst: &mut Plane, src: &Plane, bx: usize, by: usize, size: usize) {
    for j in 0..size {
        let at = (by + j) * dst.stride + bx;
        dst.data[at..at + size].copy_from_slice(&src.data[at..at + size]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Plane {
        let mut data = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = f(x, y);
            }
        }
        Plane {
            data,
            stride: w,
            width: w as u32,
            height: h as u32,
        }
    }

    #[test]
    fn single_vector_adds_the_median_of_the_neighbours() {
        let mut line = MvLine::new(3);
        let d = |x, y| [[x, y], [0, 0], [0, 0], [0, 0]];
        assert_eq!(line.predict(0, true, true, 1, &d(2, -1)), [[2, -1]; 4]);
        assert_eq!(line.predict(1, true, false, 1, &d(1, 1)), [[3, 0]; 4]);
        line.predict(2, true, false, 1, &d(0, 0));
        line.next_row();
        assert_eq!(line.predict(0, false, true, 1, &d(0, 0)), [[2, -1]; 4]);
    }

    #[test]
    fn four_vectors_chain_through_their_quarters() {
        let mut line = MvLine::new(1);
        let d = [[1, 0], [0, 2], [3, 0], [0, 0]];
        let v = line.predict(0, true, true, 4, &d);
        assert_eq!(v, [[1, 0], [1, 2], [4, 0], [1, 0]]);
    }

    #[test]
    fn whole_pel_vector_copies_with_edge_replication() {
        let src = plane(32, 32, |x, y| (x * 3 + y) as u8);
        let mut dst = plane(32, 32, |_, _| 0);
        luma_block(&mut dst, &src, 0, 0, [-6, 4]);
        assert_eq!(dst.data[0], src.data[2 * 32]);
        assert_eq!(dst.data[5 * 32 + 10], src.data[7 * 32 + 7]);
        luma_block(&mut dst, &src, 16, 16, [32, 32]);
        assert_eq!(dst.data[16 * 32 + 16], src.data[16 * 32 + 16]);
    }

    #[test]
    fn half_pel_luma_of_a_ramp_lands_between_samples() {
        let src = plane(48, 48, |x, _| (x * 4) as u8);
        let mut dst = plane(48, 48, |_, _| 0);
        luma_block(&mut dst, &src, 16, 16, [33, 32]);
        // The filter taps sum to one, so a ramp of slope 4 is sampled half a step along.
        assert_eq!(dst.data[16 * 48 + 20], 82);
        luma_block(&mut dst, &src, 16, 16, [32, 33]);
        assert_eq!(dst.data[16 * 48 + 20], 80);
    }

    #[test]
    fn quarter_pel_chroma_blends_neighbours() {
        let src = plane(16, 16, |x, _| (x * 8) as u8);
        let mut dst = plane(16, 16, |_, _| 0);
        for (frac, wa, wb) in [(1, 6, 2), (2, 4, 4), (3, 2, 6)] {
            chroma_block(&mut dst, &src, 0, 0, [frac, 0]);
            let (a, b) = (4 * 8, 5 * 8);
            assert_eq!(i32::from(dst.data[4]), (wa * a + wb * b + 4) >> 3);
        }
    }

    #[test]
    fn sources_are_moved_back_inside_the_picture() {
        // A 64 sample wide picture is 128 half samples across.
        for snug in [false, true] {
            assert_eq!(clamp_source(-20, 128, snug), 0, "far left, whole sample");
            assert_eq!(clamp_source(200, 128, snug), 96, "far right, whole sample");
            assert_eq!(clamp_source(40, 128, snug), 40);
        }
        // A fractional position needs the filter taps inside too: 5 half samples from the left edge, and with
        // the snug flag the last allowed position leaves the taps exactly at the right edge.
        assert_eq!(clamp_source(-3, 128, true), 5);
        assert_eq!(clamp_source(201, 128, true), 91);
        assert_eq!(clamp_source(-3, 128, false), 4);
        assert_eq!(clamp_source(201, 128, false), 86);
        assert_eq!(clamp_source(91, 128, true), 91);
        assert_eq!(clamp_source(93, 128, true), 91);
    }

    #[test]
    fn chroma_rounds_once_after_both_directions() {
        // Values chosen so that rounding the rows first would give 21 where the single rounding gives 20.
        let rows = [[10u8, 27], [11, 30]];
        let src = plane(16, 16, |x, y| if x < 2 && y < 2 { rows[y][x] } else { 0 });
        let mut dst = plane(16, 16, |_, _| 0);
        chroma_block(&mut dst, &src, 0, 0, [1, 1]);
        let h = |r: usize| 4 * (3 * i32::from(rows[r][0]) + i32::from(rows[r][1]));
        let exact = ((3 * h(0) + h(1)) / 4 + 8) >> 4;
        assert_eq!(i32::from(dst.data[0]), exact);
    }

    #[test]
    fn luma_columns_see_unclipped_row_results() {
        let hash = |x: usize, y: usize| ((x * 7919 + y * 104_729 + x * y * 31) % 5 / 4 * 255) as u8;
        let src = plane(48, 48, hash);
        let mut dst = plane(48, 48, |_, _| 0);
        luma_block(&mut dst, &src, 16, 16, [33, 33]);
        let tap = |s: [i32; 6]| {
            ((((s[2] + s[3]) * 19) >> 1) - (s[1] + s[4]) * 2 + ((s[0] + s[5]) >> 1) + 8) >> 4
        };
        let mut overshoot = false;
        for j in 0..16 {
            for i in 0..16 {
                let rows: [i32; 6] = std::array::from_fn(|k| {
                    let y = 16 + j + k - 2;
                    tap(std::array::from_fn(|t| {
                        i32::from(src.data[y * 48 + 16 + i + t - 2])
                    }))
                });
                overshoot |= rows.iter().any(|&r| !(0..=255).contains(&r));
                assert_eq!(
                    i32::from(dst.data[(16 + j) * 48 + 16 + i]),
                    tap(rows).clamp(0, 255)
                );
            }
        }
        assert!(
            overshoot,
            "the pattern never made a row result leave the sample range"
        );
    }
}
