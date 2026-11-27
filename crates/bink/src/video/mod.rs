//! Bink 2 video: frame header, slices and macroblocks.

mod block;
mod dsp;
mod pred;

use crate::bits::BitReader;
use crate::container::Header;
use crate::error::{Error, Result};
use crate::tables;
use block::{
    Codes, read_acs, read_cbp_chroma, read_cbp_luma, read_flags, read_quant, read_unsigned,
};
use pred::{ChromaDc, DcRange, LumaDc, QuantLine};

#[derive(Debug, Clone)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub planes: Vec<Plane>,
}

const SLICE_COUNTS: [usize; 4] = [2, 3, 4, 8];

pub struct VideoDecoder {
    width: u32,
    height: u32,
    alpha: bool,
    slice_ends: Vec<usize>,
    codes: Codes,
}

struct SliceState {
    quant: QuantLine,
    y: LumaDc,
    u: ChromaDc,
    v: ChromaDc,
    a: LumaDc,
    cbp_y: u32,
    cbp_u: u32,
    cbp_v: u32,
    cbp_a: u32,
}

impl SliceState {
    fn new(blocks: usize) -> Self {
        Self {
            quant: QuantLine::new(blocks),
            y: LumaDc::new(blocks),
            u: ChromaDc::new(blocks),
            v: ChromaDc::new(blocks),
            a: LumaDc::new(blocks),
            cbp_y: 0,
            cbp_u: 0,
            cbp_v: 0,
            cbp_a: 0,
        }
    }
}

impl VideoDecoder {
    pub fn new(header: &Header) -> Result<VideoDecoder> {
        if header.version < b'f' {
            return Err(Error::Unsupported(header.version as char));
        }
        let aligned = ((header.height + 31) & !31) as usize;
        let count = SLICE_COUNTS[(header.flags & 3) as usize];
        let mut ends = Vec::with_capacity(count);
        let (mut start, mut end) = (0usize, aligned + 32 * count - 1);
        for i in 0..count - 1 {
            start += ((end - start) / (count - i)) & !31;
            end -= 32;
            ends.push(start >> 5);
        }
        ends.push(aligned >> 5);
        Ok(VideoDecoder {
            width: header.width,
            height: header.height,
            alpha: header.has_alpha,
            slice_ends: ends,
            codes: Codes::new(),
        })
    }

    fn blank(&self, index: u32) -> Frame {
        let aw = ((self.width + 31) & !31) as usize;
        let ah = ((self.height + 31) & !31) as usize;
        let mut planes = vec![
            Plane {
                data: vec![0; aw * ah],
                stride: aw,
                width: self.width,
                height: self.height,
            },
            Plane {
                data: vec![128; aw / 2 * ah / 2],
                stride: aw / 2,
                width: self.width.div_ceil(2),
                height: self.height.div_ceil(2),
            },
            Plane {
                data: vec![128; aw / 2 * ah / 2],
                stride: aw / 2,
                width: self.width.div_ceil(2),
                height: self.height.div_ceil(2),
            },
        ];
        if self.alpha {
            planes.push(Plane {
                data: vec![255; aw * ah],
                stride: aw,
                width: self.width,
                height: self.height,
            });
        }
        Frame {
            index,
            width: self.width,
            height: self.height,
            planes,
        }
    }

    pub fn decode(
        &self,
        index: u32,
        data: &[u8],
        key: bool,
        _reference: Option<&Frame>,
    ) -> Result<Frame> {
        let fail = |reason| Error::Frame {
            frame: index,
            reason,
        };
        if data.len() < 4 {
            return Err(Error::Truncated);
        }
        let mut reader = BitReader::new(data);
        let flags = reader.read(32);
        let slices = self.slice_ends.len();
        let mut offsets = Vec::with_capacity(slices);
        for _ in 1..slices {
            offsets.push(reader.read(32) as usize);
        }
        if flags & 0x2000 == 0 {
            return Err(Error::Unsupported('?'));
        }
        let mut frame = self.blank(index);
        if flags & 0x10000 != 0 {
            let mut scratch = Vec::new();
            if flags & 0x8000 == 0 {
                read_flags(
                    &mut reader,
                    &mut scratch,
                    true,
                    (self.height as usize).div_ceil(16) * 2 - 1,
                );
            }
            if flags & 0x4000 == 0 {
                read_flags(
                    &mut reader,
                    &mut scratch,
                    true,
                    (self.width as usize).div_ceil(16) * 2 - 1,
                );
            }
        }
        let skip_alpha = self.alpha && flags & 0x8_0000 != 0;
        if skip_alpha {
            frame.planes[3].data.fill((flags >> 24) as u8);
        }
        if !key {
            return Err(Error::Unsupported('P'));
        }
        let bw = (self.width as usize).div_ceil(32);
        let mut first = 0;
        for (i, &end) in self.slice_ends.iter().enumerate() {
            let mut r = reader.clone();
            if i > 0 {
                r.seek(offsets[i - 1] * 8);
            }
            let mut state = SliceState::new(bw);
            for by in first..end {
                self.decode_row(
                    &mut r,
                    &mut state,
                    &mut frame,
                    flags,
                    by,
                    by == first,
                    bw,
                    !skip_alpha,
                )
                .ok_or_else(|| fail("bad intra macroblock data"))?;
            }
            first = end;
        }
        Ok(frame)
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_row(
        &self,
        r: &mut BitReader,
        st: &mut SliceState,
        frame: &mut Frame,
        flags: u32,
        by: usize,
        top: bool,
        bw: usize,
        with_alpha: bool,
    ) -> Option<()> {
        st.cbp_y = 0;
        st.cbp_u = 0;
        st.cbp_v = 0;
        st.cbp_a = 0;
        let wide = flags & 0x4_0000 != 0;
        let range = DcRange { lo: 0, hi: 0x7FF };
        for bx in 0..bw {
            let left = bx == 0;
            let predicted = st.quant.predict(bx * 2, top);
            let q = read_quant(r, predicted)?;
            st.quant.set(bx * 2, q);
            let luma_at = |plane: &Plane| by * 32 * plane.stride + bx * 32;
            let at = luma_at(&frame.planes[0]);
            self.intra_luma(
                r,
                &mut st.y,
                &mut st.cbp_y,
                q,
                bx,
                top,
                left,
                wide,
                &range,
                &mut frame.planes[0],
                at,
            )?;
            for p in [2usize, 1] {
                let (dc, cbp) = if p == 2 {
                    (&mut st.v, &mut st.cbp_v)
                } else {
                    (&mut st.u, &mut st.cbp_u)
                };
                let plane = &mut frame.planes[p];
                let at = by * 16 * plane.stride + bx * 16;
                self.intra_chroma(r, dc, cbp, q, bx, top, left, &range, plane, at)?;
            }
            if self.alpha && with_alpha {
                let at = luma_at(&frame.planes[3]);
                self.intra_luma(
                    r,
                    &mut st.a,
                    &mut st.cbp_a,
                    q,
                    bx,
                    top,
                    left,
                    wide,
                    &range,
                    &mut frame.planes[3],
                    at,
                )?;
            }
        }
        st.quant.next_row();
        st.y.next_row();
        st.u.next_row();
        st.v.next_row();
        st.a.next_row();
        Some(())
    }

    #[allow(clippy::too_many_arguments)]
    fn intra_luma(
        &self,
        r: &mut BitReader,
        dc: &mut LumaDc,
        cbp_state: &mut u32,
        q: i32,
        bx: usize,
        top: bool,
        left: bool,
        wide: bool,
        range: &DcRange,
        plane: &mut Plane,
        at: usize,
    ) -> Option<()> {
        let cbp = read_cbp_luma(r, *cbp_state, wide);
        *cbp_state = cbp;
        let mut d = [0i32; 16];
        if r.read_bool() {
            let scale = tables::QUANT_DC[q.max(8) as usize] as i32;
            for v in d.iter_mut() {
                let mut x = read_unsigned(r);
                if x != 0 {
                    x = (x * scale + 0x200) >> 10;
                    if r.read_bool() {
                        x = -x;
                    }
                }
                *v = x;
            }
        }
        let dcs = dc.predict_intra(bx, &d, top, left, range);
        let mut blocks = [[0i32; 64]; 16];
        for quad in 0..4 {
            if !read_acs(
                r,
                &self.codes,
                cbp >> (4 * quad),
                &tables::QUANT_INTRA_LUMA,
                q,
                &mut blocks[quad * 4..quad * 4 + 4],
            ) {
                return None;
            }
        }
        let stride = plane.stride;
        for (k, block) in blocks.iter_mut().enumerate() {
            block[0] = dcs[k] * 8 + 32;
            let (qx, qy) = ((k / 4) % 2, (k / 4) / 2);
            let (jx, jy) = ((k % 4) % 2, (k % 4) / 2);
            let off = at + (qy * 16 + jy * 8) * stride + qx * 16 + jx * 8;
            dsp::put_block(block, &mut plane.data[off..], stride);
        }
        Some(())
    }

    #[allow(clippy::too_many_arguments)]
    fn intra_chroma(
        &self,
        r: &mut BitReader,
        dc: &mut ChromaDc,
        cbp_state: &mut u32,
        q: i32,
        bx: usize,
        top: bool,
        left: bool,
        range: &DcRange,
        plane: &mut Plane,
        at: usize,
    ) -> Option<()> {
        let cbp = read_cbp_chroma(r, *cbp_state);
        *cbp_state = cbp;
        let mut d = [0i32; 4];
        if r.read_bool() {
            let scale = tables::QUANT_DC[q.max(8) as usize] as i32;
            for v in d.iter_mut() {
                let mut x = read_unsigned(r);
                if x != 0 {
                    x = (x * scale + 0x200) >> 10;
                    if r.read_bool() {
                        x = -x;
                    }
                }
                *v = x;
            }
        }
        let dcs = dc.predict_intra(bx, &d, top, left, range);
        let mut blocks = [[0i32; 64]; 4];
        if !read_acs(
            r,
            &self.codes,
            cbp,
            &tables::QUANT_INTRA_CHROMA,
            q,
            &mut blocks,
        ) {
            return None;
        }
        let stride = plane.stride;
        for (k, block) in blocks.iter_mut().enumerate() {
            block[0] = dcs[k] * 8 + 32;
            let off = at + (k / 2) * 8 * stride + (k % 2) * 8;
            dsp::put_block(block, &mut plane.data[off..], stride);
        }
        Some(())
    }
}
