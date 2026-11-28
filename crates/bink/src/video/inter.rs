//! Macroblocks of inter frames: block types, motion-only and motion-plus-residue blocks.

use super::block::{
    read_acs, read_cbp_chroma, read_cbp_luma, read_dc_diffs, read_mvs, read_quant, read_zeros,
};
use super::deblock::Meta;
use super::motion::{self, Mvs};
use super::pred::{ChromaDc, DcRange, LumaDc};
use super::{Frame, Plane, Row, SliceState, VideoDecoder, dsp};
use crate::bits::BitReader;
use crate::tables;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Intra,
    Skip,
    Motion,
    Residue,
}

/// The four block types ordered by recency: each macroblock names its type by position in the list and the
/// chosen type moves up.
pub struct TypeLru([BlockKind; 4]);

impl TypeLru {
    pub fn new() -> Self {
        Self([
            BlockKind::Motion,
            BlockKind::Residue,
            BlockKind::Skip,
            BlockKind::Intra,
        ])
    }

    pub fn read(&mut self, r: &mut BitReader) -> BlockKind {
        let l = &mut self.0;
        match read_zeros(r, 3) {
            0 => l[0],
            1 => {
                l.swap(0, 1);
                l[0]
            }
            2 => {
                l.swap(2, 3);
                l[2]
            }
            _ => {
                l.swap(1, 2);
                l[1]
            }
        }
    }
}

/// Mean of an 8x8 block scaled by eight, from reconstructed samples: what intra prediction of the next
/// macroblocks sees for a neighbour that was not coded as intra.
fn block_sum(plane: &Plane, x: usize, y: usize) -> i32 {
    let mut sum = 0;
    for i in 0..8 {
        let at = |row: usize| i32::from(plane.data[(y + row) * plane.stride + x + i]);
        let avg = |a: usize, b: usize| (at(a) + at(b) + 1) >> 1;
        let e = (avg(0, 1) + avg(2, 3) + 1) >> 1;
        let f = (avg(4, 5) + avg(6, 7) + 1) >> 1;
        sum += (e + f + 1) >> 1;
    }
    sum
}

const LUMA_ORDER: [usize; 16] = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

fn measure_luma(plane: &Plane, bx: usize, by: usize) -> [i32; 16] {
    std::array::from_fn(|i| {
        let pos = LUMA_ORDER[i];
        block_sum(plane, bx * 32 + (pos & 3) * 8, by * 32 + (pos >> 2) * 8)
    })
}

fn measure_chroma(plane: &Plane, bx: usize, by: usize) -> [i32; 4] {
    std::array::from_fn(|i| block_sum(plane, bx * 16 + (i & 1) * 8, by * 16 + (i >> 1) * 8))
}

fn set_luma_measured(dc: &mut LumaDc, bx: usize, m: &[i32; 16]) {
    dc.set_measured(bx, m[5], m[7], m[13], m[10], m[11], m[14], m[15]);
}

fn set_chroma_measured(dc: &mut ChromaDc, bx: usize, m: &[i32; 4]) {
    dc.set_measured(bx, m[1], m[2], m[3]);
}

fn predict_luma(dst: &mut Plane, src: &Plane, bx: usize, by: usize, mvs: &Mvs) {
    for (k, mv) in mvs.iter().enumerate() {
        let (x, y) = (bx * 32 + (k & 1) * 16, by * 32 + (k >> 1) * 16);
        motion::luma_block(dst, src, x, y, *mv);
    }
}

fn predict_chroma(dst: &mut Plane, src: &Plane, bx: usize, by: usize, mvs: &Mvs) {
    for (k, mv) in mvs.iter().enumerate() {
        let (x, y) = (bx * 16 + (k & 1) * 8, by * 16 + (k >> 1) * 8);
        motion::chroma_block(dst, src, x, y, *mv);
    }
}

impl VideoDecoder {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn inter_mb(
        &self,
        r: &mut BitReader,
        st: &mut SliceState,
        frame: &mut Frame,
        meta: &mut Meta,
        refr: &Frame,
        row: &Row,
        bx: usize,
        kind: BlockKind,
    ) -> Option<()> {
        let (by, top) = (row.by, row.top);
        let left = bx == 0;
        let intra_q = st.quant.predict(bx * 2, top);
        let inter_q = st.quant.predict(bx * 2 + 1, top);
        st.quant.set(bx * 2, intra_q);
        st.quant.set(bx * 2 + 1, inter_q);
        let planes = if row.alpha { 4 } else { 3 };
        if kind == BlockKind::Skip {
            for p in 0..planes {
                let size = if p == 1 || p == 2 { 16 } else { 32 };
                let (dst, src) = (&mut frame.planes[p], &refr.planes[p]);
                motion::copy_block(dst, src, bx * size, by * size, size);
            }
        } else {
            let (count, diffs) = read_mvs(r, &self.codes)?;
            let mvs = st.mvs.predict(bx, top, left, count, &diffs);
            meta.mvs[by * meta.mb_width + bx] = mvs;
            let q = if kind == BlockKind::Residue {
                let q = read_quant(r, inter_q)?;
                st.quant.set(bx * 2 + 1, q);
                q
            } else {
                inter_q
            };
            for p in 0..planes {
                let (dst, src) = (&mut frame.planes[p], &refr.planes[p]);
                if p == 1 || p == 2 {
                    predict_chroma(dst, src, bx, by, &mvs);
                } else {
                    predict_luma(dst, src, bx, by, &mvs);
                }
            }
            if kind == BlockKind::Residue {
                self.residue(r, st, frame, row, bx, q)?;
            }
        }
        let m = measure_luma(&frame.planes[0], bx, by);
        set_luma_measured(&mut st.y, bx, &m);
        for p in [2usize, 1] {
            let m = measure_chroma(&frame.planes[p], bx, by);
            set_chroma_measured(if p == 2 { &mut st.v } else { &mut st.u }, bx, &m);
        }
        if row.alpha {
            let m = measure_luma(&frame.planes[3], bx, by);
            set_luma_measured(&mut st.a, bx, &m);
        }
        Some(())
    }

    fn residue(
        &self,
        r: &mut BitReader,
        st: &mut SliceState,
        frame: &mut Frame,
        row: &Row,
        bx: usize,
        q: i32,
    ) -> Option<()> {
        let by = row.by;
        let at = |plane: &Plane, size: usize| by * size * plane.stride + bx * size;
        let a = at(&frame.planes[0], 32);
        let (cbp, wide) = (&mut st.cbp_inter[0], row.wide);
        self.residue_luma(r, &mut st.y, cbp, q, wide, &mut frame.planes[0], a)?;
        if r.read_bool() {
            for p in [2usize, 1] {
                let (dc, cbp) = if p == 2 {
                    (&mut st.v, &mut st.cbp_inter[2])
                } else {
                    (&mut st.u, &mut st.cbp_inter[1])
                };
                let a = at(&frame.planes[p], 16);
                self.residue_chroma(r, dc, cbp, q, &mut frame.planes[p], a)?;
            }
        } else {
            st.cbp_inter[1] = 0;
            st.cbp_inter[2] = 0;
        }
        if row.alpha {
            let a = at(&frame.planes[3], 32);
            let cbp = &mut st.cbp_inter[3];
            self.residue_luma(r, &mut st.a, cbp, q, wide, &mut frame.planes[3], a)?;
        }
        Some(())
    }

    #[allow(clippy::too_many_arguments)]
    fn residue_luma(
        &self,
        r: &mut BitReader,
        dc: &mut LumaDc,
        cbp_state: &mut u32,
        q: i32,
        wide: bool,
        plane: &mut Plane,
        at: usize,
    ) -> Option<()> {
        let cbp = read_cbp_luma(r, *cbp_state, wide);
        *cbp_state = cbp;
        let d: [i32; 16] = read_dc_diffs(r, q);
        let dcs = dc.predict_inter(
            &d,
            &DcRange {
                lo: -1023,
                hi: 1023,
            },
        );
        let mut blocks = [[0i32; 64]; 16];
        for quad in 0..4 {
            let group = &mut blocks[quad * 4..quad * 4 + 4];
            read_acs(
                r,
                &self.codes,
                cbp >> (4 * quad),
                &tables::QUANT_INTER,
                q,
                group,
            )?;
        }
        let stride = plane.stride;
        for (k, block) in blocks.iter_mut().enumerate() {
            block[0] = dcs[k] * 8 + 32;
            let (qx, qy) = ((k / 4) % 2, (k / 4) / 2);
            let (jx, jy) = ((k % 4) % 2, (k % 4) / 2);
            let off = at + (qy * 16 + jy * 8) * stride + qx * 16 + jx * 8;
            dsp::add_block(block, &mut plane.data[off..], stride);
        }
        Some(())
    }

    fn residue_chroma(
        &self,
        r: &mut BitReader,
        dc: &mut ChromaDc,
        cbp_state: &mut u32,
        q: i32,
        plane: &mut Plane,
        at: usize,
    ) -> Option<()> {
        let cbp = read_cbp_chroma(r, *cbp_state);
        *cbp_state = cbp;
        let d: [i32; 4] = read_dc_diffs(r, q);
        let dcs = dc.predict_inter(
            &d,
            &DcRange {
                lo: -1023,
                hi: 1023,
            },
        );
        let mut blocks = [[0i32; 64]; 4];
        read_acs(r, &self.codes, cbp, &tables::QUANT_INTER, q, &mut blocks)?;
        let stride = plane.stride;
        for (k, block) in blocks.iter_mut().enumerate() {
            block[0] = dcs[k] * 8 + 32;
            let off = at + (k / 2) * 8 * stride + (k % 2) * 8;
            dsp::add_block(block, &mut plane.data[off..], stride);
        }
        Some(())
    }
}
