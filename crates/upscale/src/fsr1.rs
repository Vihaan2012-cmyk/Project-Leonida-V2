//! AMD FidelityFX Super Resolution 1 (EASU upscale, RCAS sharpen), ported to WGSL (`shaders/easu.wgsl`,
//! `shaders/rcas.wgsl`) and to a CPU reference ([`mod@reference`]) that the tests compare the GPU against.
//!
//! The algorithm is AMD's `ffx_fsr1.h`, which is licensed under the MIT licence:
//!
//! > Copyright (c) 2021 Advanced Micro Devices, Inc. All rights reserved.
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! > documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//! > the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
//! > to permit persons to whom the Software is furnished to do so, subject to the following conditions: The above
//! > copyright notice and this permission notice shall be included in all copies or substantial portions of the
//! > Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT
//! > LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
//! > SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! > OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//! > DEALINGS IN THE SOFTWARE.
//!
//! RCAS's optional denoise is not ported. FSR 1 is purely spatial: jitter, depth and motion vectors are ignored, and
//! so is `exposure`.
//!
//! Pass layouts (storage buffers, then parameter words): EASU `[source rgba f32, destination rgba f32, params]` with
//! params `[in_w, in_h, out_w, out_h, in_linear, out_linear]`; RCAS `[source, destination, params]` with params
//! `[w, h, in_linear, out_linear, lobe scale as f32 bits]`.

use crate::compute::{SharedCompute, floats, groups_for, words};
use crate::{ColorSpace, Image, UpscaleError, UpscaleInputs, Upscaler, UpscalerKind, shaders};

/// RCAS's lobe scale for a sharpness in 0..1: AMD's "stops of attenuation" are `2 * (1 - sharpness)`.
pub fn rcas_lobe(sharpness: f32) -> f32 {
    (-2.0 * (1.0 - sharpness.clamp(0.0, 1.0))).exp2()
}

pub struct Fsr1Upscaler {
    compute: Option<SharedCompute>,
}

impl Fsr1Upscaler {
    pub fn new(compute: SharedCompute) -> Self {
        Fsr1Upscaler {
            compute: Some(compute),
        }
    }

    /// Runs the CPU reference instead of the GPU passes; slow, for machines and tests without a GPU.
    pub fn cpu() -> Self {
        Fsr1Upscaler { compute: None }
    }
}

impl Upscaler for Fsr1Upscaler {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Fsr1
    }

    fn name(&self) -> &'static str {
        "AMD FSR 1"
    }

    fn uses_jitter(&self) -> bool {
        false
    }

    fn upscale(&mut self, inputs: &UpscaleInputs<'_>) -> Result<Image, UpscaleError> {
        inputs.validate()?;
        let [rw, rh] = inputs.render_size;
        let [ow, oh] = inputs.output_size;
        let linear = inputs.color_space == ColorSpace::Linear;
        let sharpen = inputs.sharpness > 0.0;
        let source = inputs.color.rgba_f32(rw, rh, "colour")?;
        let lobe = rcas_lobe(inputs.sharpness);
        let result = match &self.compute {
            Some(compute) => {
                let mut src = words(&source);
                let mut mid = vec![0u32; (ow * oh * 4) as usize];
                let mut easu = [rw, rh, ow, oh, linear as u32, (linear && !sharpen) as u32];
                compute.dispatch(
                    shaders::EASU,
                    &mut [&mut src, &mut mid, &mut easu],
                    groups_for(ow, oh),
                )?;
                if sharpen {
                    let mut out = vec![0u32; (ow * oh * 4) as usize];
                    let mut rcas = [ow, oh, 0, linear as u32, lobe.to_bits()];
                    compute.dispatch(
                        shaders::RCAS,
                        &mut [&mut mid, &mut out, &mut rcas],
                        groups_for(ow, oh),
                    )?;
                    floats(&out)
                } else {
                    floats(&mid)
                }
            }
            None => {
                let up = reference::easu(&source, rw, rh, ow, oh, linear, linear && !sharpen);
                if sharpen {
                    reference::rcas(&up, ow, oh, false, linear, lobe)
                } else {
                    up
                }
            }
        };
        Image::from_rgba_f32(ow, oh, inputs.output_format, &result)
    }

    fn reset(&mut self) {}
}

/// The same arithmetic as the WGSL passes, on the CPU.
pub mod reference {
    fn rcp_lo(a: f32) -> f32 {
        f32::from_bits(0x7ef0_7ebbu32.wrapping_sub(a.to_bits()))
    }

    fn rsq_lo(a: f32) -> f32 {
        f32::from_bits(0x5f34_7d74u32.wrapping_sub(a.to_bits() >> 1))
    }

    fn rcp_med(a: f32) -> f32 {
        let b = f32::from_bits(0x7ef1_9fffu32.wrapping_sub(a.to_bits()));
        b * (-b * a + 2.0)
    }

    fn sat(x: f32) -> f32 {
        x.clamp(0.0, 1.0)
    }

    type Rgb = [f32; 3];

    fn perceptual(c: Rgb) -> Rgb {
        c.map(|v| v.max(0.0).sqrt())
    }

    fn luma2(c: Rgb) -> f32 {
        c[2] * 0.5 + (c[0] * 0.5 + c[1])
    }

    fn load(src: &[f32], w: u32, h: u32, x: i32, y: i32, linear: bool) -> Rgb {
        let cx = x.clamp(0, w as i32 - 1) as usize;
        let cy = y.clamp(0, h as i32 - 1) as usize;
        let i = (cy * w as usize + cx) * 4;
        let c = [src[i], src[i + 1], src[i + 2]];
        if linear { perceptual(c) } else { c }
    }

    fn tap(off: [f32; 2], dir: [f32; 2], len: [f32; 2], lob: f32, clp: f32, c: Rgb) -> [f32; 4] {
        let v = [
            (off[0] * dir[0] + off[1] * dir[1]) * len[0],
            (off[0] * -dir[1] + off[1] * dir[0]) * len[1],
        ];
        let d2 = (v[0] * v[0] + v[1] * v[1]).min(clp);
        let mut wb = 0.4 * d2 - 1.0;
        let mut wa = lob * d2 - 1.0;
        wb *= wb;
        wa *= wa;
        wb = (25.0 / 16.0) * wb - (25.0 / 16.0 - 1.0);
        let w = wb * wa;
        [c[0] * w, c[1] * w, c[2] * w, w]
    }

    fn set_dir(w: f32, la: f32, lb: f32, lc: f32, ld: f32, le: f32) -> [f32; 3] {
        let mut len_x = (ld - lc).abs().max((lc - lb).abs());
        let dir_x = ld - lb;
        len_x = sat(dir_x.abs() * rcp_lo(len_x));
        len_x *= len_x;
        let mut len_y = (le - lc).abs().max((lc - la).abs());
        let dir_y = le - la;
        len_y = sat(dir_y.abs() * rcp_lo(len_y));
        len_y *= len_y;
        [dir_x * w, dir_y * w, (len_x + len_y) * w]
    }

    /// EASU over RGBA f32 texels. Linear input is converted to a square-root perceptual encoding first, and back
    /// when `out_linear` is set.
    pub fn easu(
        src: &[f32],
        in_w: u32,
        in_h: u32,
        out_w: u32,
        out_h: u32,
        in_linear: bool,
        out_linear: bool,
    ) -> Vec<f32> {
        let mut out = vec![0.0; (out_w * out_h * 4) as usize];
        let scale = [in_w as f32 / out_w as f32, in_h as f32 / out_h as f32];
        for y in 0..out_h {
            for x in 0..out_w {
                let mut pp = [
                    x as f32 * scale[0] + (0.5 * scale[0] - 0.5),
                    y as f32 * scale[1] + (0.5 * scale[1] - 0.5),
                ];
                let fp = [pp[0].floor(), pp[1].floor()];
                pp = [pp[0] - fp[0], pp[1] - fp[1]];
                let (fx, fy) = (fp[0] as i32, fp[1] as i32);
                let ld = |dx: i32, dy: i32| load(src, in_w, in_h, fx + dx, fy + dy, in_linear);
                let (cb, cc, ce, cf, cg, ch) = (
                    ld(0, -1),
                    ld(1, -1),
                    ld(-1, 0),
                    ld(0, 0),
                    ld(1, 0),
                    ld(2, 0),
                );
                let (ci, cj, ck, cl, cn, co) =
                    (ld(-1, 1), ld(0, 1), ld(1, 1), ld(2, 1), ld(0, 2), ld(1, 2));
                let l = |c: Rgb| luma2(c);
                let (bl, cl_, el, fl, gl, hl) = (l(cb), l(cc), l(ce), l(cf), l(cg), l(ch));
                let (il, jl, kl, ll, nl, ol) = (l(ci), l(cj), l(ck), l(cl), l(cn), l(co));
                let mut acc = [0.0f32; 3];
                for s in [
                    set_dir((1.0 - pp[0]) * (1.0 - pp[1]), bl, el, fl, gl, jl),
                    set_dir(pp[0] * (1.0 - pp[1]), cl_, fl, gl, hl, kl),
                    set_dir((1.0 - pp[0]) * pp[1], fl, il, jl, kl, nl),
                    set_dir(pp[0] * pp[1], gl, jl, kl, ll, ol),
                ] {
                    for i in 0..3 {
                        acc[i] += s[i];
                    }
                }
                let mut dir = [acc[0], acc[1]];
                let mut len = acc[2];
                let mut dir_r = dir[0] * dir[0] + dir[1] * dir[1];
                let zro = dir_r < 1.0 / 32768.0;
                dir_r = rsq_lo(dir_r);
                if zro {
                    dir_r = 1.0;
                    dir[0] = 1.0;
                }
                dir = [dir[0] * dir_r, dir[1] * dir_r];
                len *= 0.5;
                len *= len;
                let stretch =
                    (dir[0] * dir[0] + dir[1] * dir[1]) * rcp_lo(dir[0].abs().max(dir[1].abs()));
                let len2 = [1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len];
                let lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
                let clp = rcp_lo(lob);
                let mut mn = [0.0f32; 3];
                let mut mx = [0.0f32; 3];
                for i in 0..3 {
                    mn[i] = cj[i].min(ck[i]).min(cf[i]).min(cg[i]);
                    mx[i] = cj[i].max(ck[i]).max(cf[i]).max(cg[i]);
                }
                let mut a = [0.0f32; 4];
                for (off, c) in [
                    ([0.0, -1.0], cb),
                    ([1.0, -1.0], cc),
                    ([-1.0, 1.0], ci),
                    ([0.0, 1.0], cj),
                    ([0.0, 0.0], cf),
                    ([-1.0, 0.0], ce),
                    ([1.0, 1.0], ck),
                    ([2.0, 1.0], cl),
                    ([2.0, 0.0], ch),
                    ([1.0, 0.0], cg),
                    ([1.0, 2.0], co),
                    ([0.0, 2.0], cn),
                ] {
                    let t = tap([off[0] - pp[0], off[1] - pp[1]], dir, len2, lob, clp, c);
                    for i in 0..4 {
                        a[i] += t[i];
                    }
                }
                let o = ((y * out_w + x) * 4) as usize;
                for i in 0..3 {
                    let v = mx[i].min(mn[i].max(a[i] * (1.0 / a[3])));
                    out[o + i] = if out_linear { v * v } else { v };
                }
                out[o + 3] = 1.0;
            }
        }
        out
    }

    /// RCAS over RGBA f32 texels with the lobe scale from [`super::rcas_lobe`].
    pub fn rcas(
        src: &[f32],
        w: u32,
        h: u32,
        in_linear: bool,
        out_linear: bool,
        lobe_scale: f32,
    ) -> Vec<f32> {
        let mut out = vec![0.0; (w * h * 4) as usize];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let ld = |dx: i32, dy: i32| load(src, w, h, x + dx, y + dy, in_linear);
                let (b, d, e, f, hh) = (ld(0, -1), ld(-1, 0), ld(0, 0), ld(1, 0), ld(0, 1));
                let mut lobe_v = [0.0f32; 3];
                for i in 0..3 {
                    let mn4 = b[i].min(d[i]).min(f[i]).min(hh[i]);
                    let mx4 = b[i].max(d[i]).max(f[i]).max(hh[i]);
                    let hit_min = mn4.min(e[i]) * (1.0 / (4.0 * mx4).max(1.0e-6));
                    let hit_max = (1.0 - mx4.max(e[i])) * (1.0 / (4.0 * mn4 - 4.0).min(-1.0e-6));
                    lobe_v[i] = (-hit_min).max(hit_max);
                }
                let limit: f32 = 0.25 - 1.0 / 16.0;
                let lobe =
                    (-limit).max(lobe_v[0].max(lobe_v[1]).max(lobe_v[2]).min(0.0)) * lobe_scale;
                let rcp_l = rcp_med(4.0 * lobe + 1.0);
                let o = ((y as u32 * w + x as u32) * 4) as usize;
                for i in 0..3 {
                    let v = (lobe * b[i] + lobe * d[i] + lobe * hh[i] + lobe * f[i] + e[i]) * rcp_l;
                    out[o + i] = if out_linear { v * v } else { v };
                }
                out[o + 3] = src[o + 3];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageRef, PixelFormat};

    fn gradient(w: u32, h: u32) -> Vec<f32> {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let edge = if x > w / 2 { 0.9 } else { 0.1 };
                data.extend_from_slice(&[
                    edge,
                    y as f32 / h as f32,
                    ((x + y) % 3) as f32 * 0.3,
                    1.0,
                ]);
            }
        }
        data
    }

    #[test]
    fn the_cpu_path_keeps_a_flat_image_flat_and_sharpens_an_edge() {
        let flat = vec![0.5f32; 8 * 8 * 4];
        let up = reference::easu(&flat, 8, 8, 16, 16, false, false);
        assert!(
            up.chunks(4)
                .all(|p| (p[0] - 0.5).abs() < 1e-3 && p[3] == 1.0)
        );
        let src = gradient(8, 8);
        let up = reference::easu(&src, 8, 8, 16, 16, false, false);
        let sharp = reference::rcas(&up, 16, 16, false, false, rcas_lobe(1.0));
        let soft = reference::rcas(&up, 16, 16, false, false, rcas_lobe(0.0));
        let diff = |a: &[f32]| a.iter().zip(&up).map(|(a, b)| (a - b).abs()).sum::<f32>();
        assert!(diff(&sharp) > diff(&soft) && diff(&soft) > 0.0);
    }

    #[test]
    fn the_cpu_upscaler_returns_the_requested_size_and_format() {
        let src = gradient(6, 4);
        let bytes: Vec<u8> = src.iter().flat_map(|v| v.to_le_bytes()).collect();
        let color = ImageRef::new(6, 4, PixelFormat::Rgba32Float, &bytes);
        let mut inputs = UpscaleInputs::new(color, [6, 4], [12, 8]);
        inputs.output_format = PixelFormat::Rgba8Unorm;
        let image = Fsr1Upscaler::cpu().upscale(&inputs).unwrap();
        assert_eq!(
            (image.width, image.height, image.data.len()),
            (12, 8, 12 * 8 * 4)
        );
        inputs.output_format = PixelFormat::R32Float;
        assert!(Fsr1Upscaler::cpu().upscale(&inputs).is_err());
    }
}
