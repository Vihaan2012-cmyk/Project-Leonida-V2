//! Pixel formats and CPU-side images: the form in which the game's colour, depth and motion vectors arrive and in
//! which results leave.

use crate::UpscaleError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    R8Unorm,
    R16Unorm,
    R16Float,
    R32Float,
    Rg16Float,
    Rg16Snorm,
    Rg32Float,
    Rgba8Unorm,
    Rgba16Float,
    Rgba32Float,
    Rgb10A2Unorm,
    R11G11B10Float,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::R8Unorm => 1,
            PixelFormat::R16Unorm | PixelFormat::R16Float => 2,
            PixelFormat::R32Float
            | PixelFormat::Rg16Float
            | PixelFormat::Rg16Snorm
            | PixelFormat::Rgba8Unorm
            | PixelFormat::Rgb10A2Unorm
            | PixelFormat::R11G11B10Float => 4,
            PixelFormat::Rg32Float | PixelFormat::Rgba16Float => 8,
            PixelFormat::Rgba32Float => 16,
        }
    }

    pub fn channels(self) -> usize {
        match self {
            PixelFormat::R8Unorm
            | PixelFormat::R16Unorm
            | PixelFormat::R16Float
            | PixelFormat::R32Float => 1,
            PixelFormat::Rg16Float | PixelFormat::Rg16Snorm | PixelFormat::Rg32Float => 2,
            PixelFormat::R11G11B10Float => 3,
            _ => 4,
        }
    }

    /// Whether a result can be written in this format.
    pub fn is_color(self) -> bool {
        matches!(
            self,
            PixelFormat::Rgba8Unorm
                | PixelFormat::Rgba16Float
                | PixelFormat::Rgba32Float
                | PixelFormat::Rgb10A2Unorm
                | PixelFormat::R11G11B10Float
        )
    }
}

/// A borrowed image. Rows are `pitch` bytes apart (0 means tightly packed).
#[derive(Debug, Clone, Copy)]
pub struct ImageRef<'a> {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub pitch: usize,
    pub data: &'a [u8],
}

impl<'a> ImageRef<'a> {
    pub fn new(width: u32, height: u32, format: PixelFormat, data: &'a [u8]) -> Self {
        ImageRef {
            width,
            height,
            format,
            pitch: 0,
            data,
        }
    }

    pub fn with_pitch(mut self, pitch: usize) -> Self {
        self.pitch = pitch;
        self
    }

    fn row_bytes(&self) -> usize {
        if self.pitch == 0 {
            self.width as usize * self.format.bytes_per_pixel()
        } else {
            self.pitch
        }
    }

    fn check(&self, width: u32, height: u32, what: &'static str) -> Result<(), UpscaleError> {
        if width > self.width || height > self.height || width == 0 || height == 0 {
            return Err(UpscaleError::BadInput {
                what,
                reason: format!(
                    "needs {width}x{height} texels, the image is {}x{}",
                    self.width, self.height
                ),
            });
        }
        let bpp = self.format.bytes_per_pixel();
        let pitch = self.row_bytes();
        if pitch < self.width as usize * bpp {
            return Err(UpscaleError::BadInput {
                what,
                reason: format!("row pitch {pitch} is shorter than a row"),
            });
        }
        let need = pitch * (height as usize - 1) + width as usize * bpp;
        if self.data.len() < need {
            return Err(UpscaleError::BadInput {
                what,
                reason: format!("{} bytes where {need} are needed", self.data.len()),
            });
        }
        Ok(())
    }

    /// The top-left `width` x `height` texels as RGBA floats (missing channels are 0, alpha 1).
    pub fn rgba_f32(
        &self,
        width: u32,
        height: u32,
        what: &'static str,
    ) -> Result<Vec<f32>, UpscaleError> {
        self.check(width, height, what)?;
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        let bpp = self.format.bytes_per_pixel();
        let pitch = self.row_bytes();
        for y in 0..height as usize {
            let row = &self.data[y * pitch..];
            for x in 0..width as usize {
                out.extend_from_slice(&decode(self.format, &row[x * bpp..x * bpp + bpp]));
            }
        }
        Ok(out)
    }

    /// One channel of the top-left `width` x `height` texels.
    pub fn channel_f32(
        &self,
        channel: usize,
        width: u32,
        height: u32,
        what: &'static str,
    ) -> Result<Vec<f32>, UpscaleError> {
        Ok(self
            .rgba_f32(width, height, what)?
            .chunks_exact(4)
            .map(|p| p[channel])
            .collect())
    }

    /// The first two channels, interleaved.
    pub fn rg_f32(
        &self,
        width: u32,
        height: u32,
        what: &'static str,
    ) -> Result<Vec<f32>, UpscaleError> {
        Ok(self
            .rgba_f32(width, height, what)?
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1]])
            .collect())
    }
}

/// An owned image, tightly packed.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

impl Image {
    pub fn as_ref(&self) -> ImageRef<'_> {
        ImageRef::new(self.width, self.height, self.format, &self.data)
    }

    /// Encodes `rgba` (four floats per texel) into a colour format.
    pub fn from_rgba_f32(
        width: u32,
        height: u32,
        format: PixelFormat,
        rgba: &[f32],
    ) -> Result<Image, UpscaleError> {
        if !format.is_color() {
            return Err(UpscaleError::UnsupportedFormat {
                format,
                what: "an output colour image",
            });
        }
        let bpp = format.bytes_per_pixel();
        let mut data = vec![0u8; (width * height) as usize * bpp];
        for (pixel, out) in rgba.chunks_exact(4).zip(data.chunks_exact_mut(bpp)) {
            encode(format, [pixel[0], pixel[1], pixel[2], pixel[3]], out);
        }
        Ok(Image {
            width,
            height,
            format,
            data,
        })
    }

    /// The image as RGBA floats.
    pub fn to_rgba_f32(&self) -> Vec<f32> {
        self.as_ref()
            .rgba_f32(self.width, self.height, "image")
            .unwrap_or_default()
    }
}

pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as f32;
    match exp {
        0 => sign * mant * 2f32.powi(-24),
        31 if mant == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + mant / 1024.0) * 2f32.powi(exp - 15),
    }
}

pub fn f32_to_f16(f: f32) -> u16 {
    let b = f.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let e = ((b >> 23) & 0xff) as i32;
    let m = b & 0x7f_ffff;
    if e == 0xff {
        return sign | 0x7c00 | if m != 0 { 0x200 } else { 0 };
    }
    let he = e - 127 + 15;
    if he >= 0x1f {
        return sign | 0x7c00;
    }
    if he <= 0 {
        if he < -10 {
            return sign;
        }
        let m = m | 0x80_0000;
        let shift = (14 - he) as u32;
        let mut half = (m >> shift) as u16;
        let rem = m & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        if rem > halfway || (rem == halfway && half & 1 == 1) {
            half += 1;
        }
        return sign | half;
    }
    let mut half = (((he as u32) << 10) | (m >> 13)) as u16;
    let rem = m & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && half & 1 == 1) {
        half += 1;
    }
    sign | half
}

fn ufloat_to_f32(v: u32, mant_bits: u32) -> f32 {
    let mant = (v & ((1 << mant_bits) - 1)) as f32 / (1 << mant_bits) as f32;
    let exp = (v >> mant_bits) as i32;
    match exp {
        0 => mant * 2f32.powi(-14),
        31 if mant == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + mant) * 2f32.powi(exp - 15),
    }
}

fn f32_to_ufloat(v: f32, mant_bits: u32) -> u32 {
    let max = (30 << mant_bits) | ((1 << mant_bits) - 1);
    if v.is_nan() || v <= 0.0 {
        return 0;
    }
    let bits = v.to_bits();
    let e = (bits >> 23) as i32 - 127 + 15;
    if e >= 31 {
        return max;
    }
    if e <= 0 {
        let scaled = v * 2f32.powi(14 + mant_bits as i32);
        return (scaled.round_ties_even() as u32).min(max);
    }
    let m = bits & 0x7f_ffff;
    let shift = 23 - mant_bits;
    let mut r = ((e as u32) << mant_bits) | (m >> shift);
    let rem = m & ((1 << shift) - 1);
    let halfway = 1 << (shift - 1);
    if rem > halfway || (rem == halfway && r & 1 == 1) {
        r += 1;
    }
    r.min(max)
}

fn decode(format: PixelFormat, b: &[u8]) -> [f32; 4] {
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let f32_at = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let u32_at = || u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let snorm = |v: u16| (v as i16 as f32 / 32767.0).max(-1.0);
    match format {
        PixelFormat::R8Unorm => [b[0] as f32 / 255.0, 0.0, 0.0, 1.0],
        PixelFormat::R16Unorm => [u16_at(0) as f32 / 65535.0, 0.0, 0.0, 1.0],
        PixelFormat::R16Float => [f16_to_f32(u16_at(0)), 0.0, 0.0, 1.0],
        PixelFormat::R32Float => [f32_at(0), 0.0, 0.0, 1.0],
        PixelFormat::Rg16Float => [f16_to_f32(u16_at(0)), f16_to_f32(u16_at(2)), 0.0, 1.0],
        PixelFormat::Rg16Snorm => [snorm(u16_at(0)), snorm(u16_at(2)), 0.0, 1.0],
        PixelFormat::Rg32Float => [f32_at(0), f32_at(4), 0.0, 1.0],
        PixelFormat::Rgba8Unorm => [
            b[0] as f32 / 255.0,
            b[1] as f32 / 255.0,
            b[2] as f32 / 255.0,
            b[3] as f32 / 255.0,
        ],
        PixelFormat::Rgba16Float => [
            f16_to_f32(u16_at(0)),
            f16_to_f32(u16_at(2)),
            f16_to_f32(u16_at(4)),
            f16_to_f32(u16_at(6)),
        ],
        PixelFormat::Rgba32Float => [f32_at(0), f32_at(4), f32_at(8), f32_at(12)],
        PixelFormat::Rgb10A2Unorm => {
            let v = u32_at();
            [
                (v & 0x3ff) as f32 / 1023.0,
                ((v >> 10) & 0x3ff) as f32 / 1023.0,
                ((v >> 20) & 0x3ff) as f32 / 1023.0,
                (v >> 30) as f32 / 3.0,
            ]
        }
        PixelFormat::R11G11B10Float => {
            let v = u32_at();
            [
                ufloat_to_f32(v & 0x7ff, 6),
                ufloat_to_f32((v >> 11) & 0x7ff, 6),
                ufloat_to_f32(v >> 22, 5),
                1.0,
            ]
        }
    }
}

fn encode(format: PixelFormat, p: [f32; 4], out: &mut [u8]) {
    let unorm8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match format {
        PixelFormat::Rgba8Unorm => out.copy_from_slice(&p.map(unorm8)),
        PixelFormat::Rgba16Float => {
            for (i, v) in p.iter().enumerate() {
                out[i * 2..i * 2 + 2].copy_from_slice(&f32_to_f16(*v).to_le_bytes());
            }
        }
        PixelFormat::Rgba32Float => {
            for (i, v) in p.iter().enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        PixelFormat::Rgb10A2Unorm => {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 1023.0).round() as u32;
            let a = (p[3].clamp(0.0, 1.0) * 3.0).round() as u32;
            let v = q(p[0]) | (q(p[1]) << 10) | (q(p[2]) << 20) | (a << 30);
            out.copy_from_slice(&v.to_le_bytes());
        }
        PixelFormat::R11G11B10Float => {
            let v = f32_to_ufloat(p[0], 6)
                | (f32_to_ufloat(p[1], 6) << 11)
                | (f32_to_ufloat(p[2], 5) << 22);
            out.copy_from_slice(&v.to_le_bytes());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_trip_and_round_to_even() {
        for v in [0.0, 1.0, -2.5, 65504.0, 6.1035156e-5, 5.9604645e-8] {
            assert_eq!(f16_to_f32(f32_to_f16(v)), v, "{v}");
        }
        assert_eq!(f32_to_f16(1.0 + 2f32.powi(-11)), 0x3c00);
        assert_eq!(f32_to_f16(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        assert_eq!(f32_to_f16(1.0e6), 0x7c00);
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
    }

    #[test]
    fn packed_formats_round_trip() {
        let mut bytes = [0u8; 4];
        encode(
            PixelFormat::R11G11B10Float,
            [1.0, 0.5, 3.0, 1.0],
            &mut bytes,
        );
        let back = decode(PixelFormat::R11G11B10Float, &bytes);
        assert_eq!(&back[..3], &[1.0, 0.5, 3.0]);
        encode(
            PixelFormat::R11G11B10Float,
            [1.0e9, -1.0, 0.0, 1.0],
            &mut bytes,
        );
        let back = decode(PixelFormat::R11G11B10Float, &bytes);
        assert_eq!(back[0], 65024.0);
        assert_eq!(back[1], 0.0);
        encode(
            PixelFormat::Rgb10A2Unorm,
            [1.0, 0.0, 512.0 / 1023.0, 1.0],
            &mut bytes,
        );
        let back = decode(PixelFormat::Rgb10A2Unorm, &bytes);
        assert_eq!(back, [1.0, 0.0, 512.0 / 1023.0, 1.0]);
    }

    #[test]
    fn pitched_and_cropped_reads() {
        let mut data = vec![0u8; 8 * 3];
        data[8] = 255;
        data[8 + 4 + 1] = 51;
        let image = ImageRef::new(2, 3, PixelFormat::Rgba8Unorm, &data).with_pitch(8);
        let rgba = image.rgba_f32(2, 2, "colour").unwrap();
        assert_eq!(rgba[8], 1.0);
        assert_eq!(rgba[13], 0.2);
        assert!(image.rgba_f32(3, 2, "colour").is_err());
        let short = ImageRef::new(2, 3, PixelFormat::Rgba8Unorm, &data[..20]).with_pitch(8);
        assert!(short.rgba_f32(2, 3, "colour").is_err());
    }
}
