//! Predictors that carry values from block to block: quantisers, DC values and motion vectors.

#[inline]
pub fn median3(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

/// `2a - b`, kept between a and b.
#[inline]
fn pred2(a: i32, b: i32) -> i32 {
    (2 * a - b).clamp(a.min(b), a.max(b))
}

/// The gradient predictor `b - a + c`, kept inside the range of the three inputs.
#[inline]
fn pred3(a: i32, b: i32, c: i32) -> i32 {
    (b - a + c).clamp(a.min(b).min(c), a.max(b).max(c))
}

/// Quantiser of the macroblock above, left and above-left. Each macroblock has two slots, intra then inter.
pub struct QuantLine {
    slots: Vec<u8>,
    #[allow(dead_code, reason = "kept for inter frames, which are not decoded yet")]
    stride: usize,
    cur: usize,
    prev: usize,
}

impl QuantLine {
    pub fn new(blocks: usize) -> Self {
        let stride = blocks * 2;
        Self {
            slots: vec![0; stride * 2],
            stride,
            cur: 0,
            prev: stride,
        }
    }

    pub fn next_row(&mut self) {
        std::mem::swap(&mut self.cur, &mut self.prev);
    }

    pub fn predict(&self, slot: usize, top: bool) -> i32 {
        let left = slot < 2;
        i32::from(if top {
            if left {
                16
            } else {
                self.slots[self.cur + slot - 2]
            }
        } else if left {
            self.slots[self.prev + slot]
        } else {
            let above_left = i32::from(self.slots[self.prev + slot - 2]);
            let above = i32::from(self.slots[self.prev + slot]);
            let left = i32::from(self.slots[self.cur + slot - 2]);
            median3(above_left, above, left) as u8
        })
    }

    pub fn set(&mut self, slot: usize, q: i32) {
        self.slots[self.cur + slot] = q as u8;
    }
}

pub struct DcRange {
    pub lo: i32,
    pub hi: i32,
}

/// Luma (and alpha) DC state: 16 values per macroblock, a 4x4 grid of 8x8 blocks in quadrant order.
pub struct LumaDc {
    /// The last macroblock's values; slots 5, 7, 13 and 15 are its right column.
    pub left: [i32; 16],
    prev: Vec<i32>,
    next: Vec<i32>,
}

impl LumaDc {
    pub fn new(blocks: usize) -> Self {
        Self {
            left: [0; 16],
            prev: vec![0; blocks * 4 + 4],
            next: vec![0; blocks * 4 + 4],
        }
    }

    pub fn next_row(&mut self) {
        std::mem::swap(&mut self.prev, &mut self.next);
    }

    /// Intra prediction. `d` holds the decoded differences.
    pub fn predict_intra(
        &mut self,
        bx: usize,
        d: &[i32; 16],
        top: bool,
        left: bool,
        range: &DcRange,
    ) -> [i32; 16] {
        let c = |v: i32| v.clamp(range.lo, range.hi);
        let p = &self.prev[bx * 4..bx * 4 + 8];
        let s = &self.left;
        let mut a = [0i32; 16];
        match (top, left) {
            (true, true) => {
                a[0] = c(d[0] + 1024);
                a[1] = c(d[1] + a[0]);
                a[2] = c(d[2] + pred2(a[0], a[1]));
                a[3] = c(d[3] + pred3(a[0], a[1], a[2]));
                a[4] = c(d[4] + pred2(a[1], a[3]));
                a[5] = c(d[5] + a[4]);
                a[6] = c(d[6] + pred3(a[1], a[3], a[4]));
                a[7] = c(d[7] + pred3(a[4], a[5], a[6]));
                a[8] = c(d[8] + pred2(a[2], a[3]));
                a[9] = c(d[9] + pred3(a[2], a[3], a[8]));
                a[10] = c(d[10] + pred2(a[8], a[9]));
            }
            (false, true) => {
                a[0] = c(d[0] + pred2(p[4], p[5]));
                a[1] = c(d[1] + pred3(p[4], p[5], a[0]));
                a[2] = c(d[2] + pred2(a[0], a[1]));
                a[3] = c(d[3] + pred3(a[0], a[1], a[2]));
                a[4] = c(d[4] + pred3(p[5], p[6], a[1]));
                a[5] = c(d[5] + pred3(p[6], p[7], a[4]));
                a[6] = c(d[6] + pred3(a[1], a[3], a[4]));
                a[7] = c(d[7] + pred3(a[4], a[5], a[6]));
                a[8] = c(d[8] + pred2(a[2], a[3]));
                a[9] = c(d[9] + pred3(a[2], a[3], a[8]));
                a[10] = c(d[10] + pred2(a[8], a[9]));
            }
            (true, false) => {
                a[0] = c(d[0] + pred2(s[5], s[7]));
                a[1] = c(d[1] + a[0]);
                a[2] = c(d[2] + pred3(s[5], s[7], a[0]));
                a[3] = c(d[3] + pred3(a[0], a[1], a[2]));
                a[4] = c(d[4] + pred2(a[1], a[3]));
                a[5] = c(d[5] + a[4]);
                a[6] = c(d[6] + pred3(a[1], a[3], a[4]));
                a[7] = c(d[7] + pred3(a[4], a[5], a[6]));
                a[8] = c(d[8] + pred3(s[7], s[13], a[2]));
                a[9] = c(d[9] + pred3(a[2], a[3], a[8]));
                a[10] = c(d[10] + pred3(s[13], s[15], a[8]));
            }
            (false, false) => {
                a[0] = c(d[0] + pred3(p[3], p[4], s[5]));
                a[1] = c(d[1] + pred3(p[4], p[5], a[0]));
                a[2] = c(d[2] + pred3(s[5], s[7], a[0]));
                a[3] = c(d[3] + pred3(a[0], a[1], a[2]));
                a[4] = c(d[4] + pred3(p[5], p[6], a[1]));
                a[5] = c(d[5] + pred3(p[6], p[7], a[4]));
                a[6] = c(d[6] + pred3(a[1], a[3], a[4]));
                a[7] = c(d[7] + pred3(a[4], a[5], a[6]));
                a[8] = c(d[8] + pred3(s[7], s[13], a[2]));
                a[9] = c(d[9] + pred3(a[2], a[3], a[8]));
                a[10] = c(d[10] + pred3(s[13], s[15], a[8]));
            }
        }
        a[11] = c(d[11] + pred3(a[8], a[9], a[10]));
        a[12] = c(d[12] + pred3(a[3], a[6], a[9]));
        a[13] = c(d[13] + pred3(a[6], a[7], a[12]));
        a[14] = c(d[14] + pred3(a[9], a[11], a[12]));
        a[15] = c(d[15] + pred3(a[12], a[13], a[14]));
        self.left = a;
        self.next[bx * 4 + 4..bx * 4 + 8].copy_from_slice(&[a[10], a[11], a[14], a[15]]);
        a
    }

    /// Inter blocks predict from inside the block only.
    #[allow(dead_code, reason = "kept for inter frames, which are not decoded yet")]
    pub fn predict_inter(&mut self, d: &[i32; 16], range: &DcRange) -> [i32; 16] {
        let c = |v: i32| v.clamp(range.lo, range.hi);
        let mut a = [0i32; 16];
        a[0] = c(d[0]);
        a[1] = c(d[1] + a[0]);
        a[2] = c(d[2] + pred2(a[0], a[1]));
        a[3] = c(d[3] + pred3(a[0], a[1], a[2]));
        a[4] = c(d[4] + pred2(a[1], a[3]));
        a[5] = c(d[5] + a[4]);
        a[6] = c(d[6] + pred3(a[1], a[3], a[4]));
        a[7] = c(d[7] + pred3(a[4], a[5], a[6]));
        a[8] = c(d[8] + pred2(a[2], a[3]));
        a[9] = c(d[9] + pred3(a[2], a[3], a[8]));
        a[10] = c(d[10] + pred2(a[8], a[9]));
        a[11] = c(d[11] + pred3(a[8], a[9], a[10]));
        a[12] = c(d[12] + pred3(a[3], a[6], a[9]));
        a[13] = c(d[13] + pred3(a[6], a[7], a[12]));
        a[14] = c(d[14] + pred3(a[9], a[11], a[12]));
        a[15] = c(d[15] + pred3(a[12], a[13], a[14]));
        self.left = a;
        a
    }

    /// Values measured from the reconstructed pixels of a block that was not coded as intra.
    #[allow(
        dead_code,
        clippy::too_many_arguments,
        reason = "kept for inter frames, which are not decoded yet; one argument per measured DC value"
    )]
    pub fn set_measured(
        &mut self,
        bx: usize,
        dc5: i32,
        dc7: i32,
        dc13: i32,
        dc10: i32,
        dc11: i32,
        dc14: i32,
        dc15: i32,
    ) {
        self.left[5] = dc5;
        self.left[7] = dc7;
        self.left[13] = dc13;
        self.left[15] = dc15;
        self.next[bx * 4 + 4..bx * 4 + 8].copy_from_slice(&[dc10, dc11, dc14, dc15]);
    }
}

/// Chroma DC state: 4 values per macroblock, a 2x2 grid.
pub struct ChromaDc {
    pub left: [i32; 4],
    prev: Vec<i32>,
    next: Vec<i32>,
}

impl ChromaDc {
    pub fn new(blocks: usize) -> Self {
        Self {
            left: [0; 4],
            prev: vec![0; blocks * 2 + 2],
            next: vec![0; blocks * 2 + 2],
        }
    }

    pub fn next_row(&mut self) {
        std::mem::swap(&mut self.prev, &mut self.next);
    }

    pub fn predict_intra(
        &mut self,
        bx: usize,
        d: &[i32; 4],
        top: bool,
        left: bool,
        range: &DcRange,
    ) -> [i32; 4] {
        let c = |v: i32| v.clamp(range.lo, range.hi);
        let p = &self.prev[bx * 2..bx * 2 + 4];
        let s = &self.left;
        let mut a = [0i32; 4];
        match (top, left) {
            (true, true) => {
                a[0] = c(d[0] + 1024);
                a[1] = c(d[1] + a[0]);
            }
            (false, true) => {
                a[0] = c(d[0] + pred2(p[2], p[3]));
                a[1] = c(d[1] + pred3(p[2], p[3], a[0]));
            }
            (true, false) => {
                a[0] = c(d[0] + pred2(s[1], s[3]));
                a[1] = c(d[1] + a[0]);
            }
            (false, false) => {
                a[0] = c(d[0] + pred3(p[1], p[2], s[1]));
                a[1] = c(d[1] + pred3(p[2], p[3], a[0]));
            }
        }
        a[2] = c(d[2]
            + if left {
                pred2(a[0], a[1])
            } else {
                pred3(s[1], s[3], a[0])
            });
        a[3] = c(d[3] + pred3(a[0], a[1], a[2]));
        self.left = a;
        self.next[bx * 2 + 2..bx * 2 + 4].copy_from_slice(&[a[2], a[3]]);
        a
    }

    #[allow(dead_code, reason = "kept for inter frames, which are not decoded yet")]
    pub fn predict_inter(&mut self, d: &[i32; 4], range: &DcRange) -> [i32; 4] {
        let c = |v: i32| v.clamp(range.lo, range.hi);
        let mut a = [0i32; 4];
        a[0] = c(d[0]);
        a[1] = c(d[1] + a[0]);
        a[2] = c(d[2] + pred2(a[0], a[1]));
        a[3] = c(d[3] + pred3(a[0], a[1], a[2]));
        self.left = a;
        a
    }

    #[allow(dead_code, reason = "kept for inter frames, which are not decoded yet")]
    pub fn set_measured(&mut self, bx: usize, dc1: i32, dc2: i32, dc3: i32) {
        self.left[1] = dc1;
        self.left[2] = dc2;
        self.left[3] = dc3;
        self.next[bx * 2 + 2..bx * 2 + 4].copy_from_slice(&[dc2, dc3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predictors_stay_inside_their_inputs() {
        assert_eq!(pred2(10, 4), 10);
        assert_eq!(pred2(4, 10), 4);
        assert_eq!(pred2(5, 4), 5);
        assert_eq!(pred3(10, 20, 15), 20);
        assert_eq!(pred3(10, 12, 20), 20);
        assert_eq!(pred3(1, 5, 3), 5);
        assert_eq!(pred3(5, 1, 3), 1);
    }

    #[test]
    fn median_of_three() {
        assert_eq!(median3(1, 2, 3), 2);
        assert_eq!(median3(3, 1, 2), 2);
        assert_eq!(median3(2, 3, 1), 2);
        assert_eq!(median3(5, 5, 1), 5);
    }

    #[test]
    fn first_intra_block_of_a_slice_starts_at_mid_grey() {
        let mut dc = LumaDc::new(2);
        let range = DcRange { lo: 0, hi: 0x7FF };
        let a = dc.predict_intra(0, &[0; 16], true, true, &range);
        assert!(a.iter().all(|&v| v == 1024));
        let b = dc.predict_intra(1, &[0; 16], true, false, &range);
        assert!(b.iter().all(|&v| v == 1024));
    }

    #[test]
    fn quantiser_prediction_uses_left_above_and_median() {
        let mut q = QuantLine::new(3);
        assert_eq!(q.predict(0, true), 16);
        q.set(0, 20);
        assert_eq!(q.predict(2, true), 20);
        q.set(2, 10);
        q.set(4, 30);
        q.next_row();
        assert_eq!(q.predict(0, false), 20);
        assert_eq!(q.predict(2, false), 10);
        q.set(0, 12);
        assert_eq!(q.predict(2, false), median3(20, 10, 12));
    }

    #[test]
    fn chroma_prediction_chains_through_the_grid() {
        let mut dc = ChromaDc::new(1);
        let range = DcRange { lo: 0, hi: 0x7FF };
        let a = dc.predict_intra(0, &[5, 1, 2, 3], true, true, &range);
        assert_eq!(a[0], 1029);
        assert_eq!(a[1], 1030);
        assert_eq!(a[2], 1031);
        assert_eq!(a[3], 1034);
    }
}
