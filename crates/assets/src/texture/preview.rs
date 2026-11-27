//! PNG previews of decoded textures. Previews are for viewing only: HDR and signed data is
//! tone-mapped or remapped, sRGB data is left as stored.

use super::bcn;
use super::format::{FormatInfo, format_info, mip_bytes};
use super::{Shape, TextureInfo};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("format {0} has no preview decoder")]
    Format(u32),
    #[error("no such plane: slice {slice} mip {mip}")]
    Plane { slice: u32, mip: u32 },
    #[error("texture data is shorter than its description")]
    Truncated,
    #[error("PNG encoding failed: {0}")]
    Png(#[from] png::EncodingError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// True when [`plane_rgba8`] can decode `dxgi`.
pub fn can_preview(dxgi: u32) -> bool {
    matches!(
        dxgi,
        10 | 11
            | 28
            | 29
            | 49
            | 54
            | 56
            | 61
            | 65
            | 71
            | 72
            | 74
            | 75
            | 77
            | 78
            | 80
            | 81
            | 83
            | 84
            | 87
            | 88
            | 91
            | 93
            | 95
            | 96
            | 98
            | 99
    )
}

fn half(lo: u8, hi: u8) -> f32 {
    bcn::half_to_f32(u16::from_le_bytes([lo, hi]))
}

fn unit(v: f32) -> u8 {
    if v.is_nan() {
        0
    } else {
        (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    }
}

fn tonemap_srgb(v: f32) -> u8 {
    let v = if v.is_nan() { 0.0 } else { v.max(0.0) };
    let x = v / (1.0 + v);
    let s = if x <= 0.003_130_8 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    unit(s)
}

fn snorm(v: i8) -> u8 {
    ((i32::from(v.max(-127)) + 127) as f32 * 255.0 / 254.0 + 0.5) as u8
}

fn decode_blocks(f: &FormatInfo, w: u32, h: u32, data: &[u8]) -> Rgba8 {
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let eb = f.element_bytes as usize;
    let mut px = vec![0u8; w as usize * h as usize * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let at = (by * bw + bx) as usize * eb;
            let blk = &data[at..at + eb];
            let rgba: [[u8; 4]; 16] = match f.dxgi {
                71 | 72 => bcn::bc1_block(blk, false),
                74 | 75 => bcn::bc2_block(blk),
                77 | 78 => bcn::bc3_block(blk),
                80 => bcn::bc4_block(blk).map(|v| [v, v, v, 255]),
                81 => bcn::bc4_block_snorm(blk).map(|v| {
                    let g = snorm(v);
                    [g, g, g, 255]
                }),
                83 => bcn::bc5_block(blk).map(|[r, g]| [r, g, normal_z(r, g), 255]),
                84 => bcn::bc5_block_snorm(blk).map(|[r, g]| {
                    let (r, g) = (snorm(r), snorm(g));
                    [r, g, normal_z(r, g), 255]
                }),
                95 | 96 => bcn::bc6h_block(blk, f.dxgi == 96)
                    .map(|[r, g, b]| [tonemap_srgb(r), tonemap_srgb(g), tonemap_srgb(b), 255]),
                _ => bcn::bc7_block(blk),
            };
            for (i, p) in rgba.iter().enumerate() {
                let (x, y) = (bx * 4 + (i as u32 % 4), by * 4 + (i as u32 / 4));
                if x < w && y < h {
                    let o = (y * w + x) as usize * 4;
                    px[o..o + 4].copy_from_slice(p);
                }
            }
        }
    }
    Rgba8 {
        width: w,
        height: h,
        pixels: px,
    }
}

fn normal_z(r: u8, g: u8) -> u8 {
    let (x, y) = (f32::from(r) / 127.5 - 1.0, f32::from(g) / 127.5 - 1.0);
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    unit(z * 0.5 + 0.5)
}

fn decode_plain(f: &FormatInfo, w: u32, h: u32, data: &[u8]) -> Rgba8 {
    let eb = f.element_bytes as usize;
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    for e in data.chunks_exact(eb).take(w as usize * h as usize) {
        let p = match f.dxgi {
            28 | 29 => [e[0], e[1], e[2], e[3]],
            87 | 91 => [e[2], e[1], e[0], e[3]],
            88 | 93 => [e[2], e[1], e[0], 255],
            61 => [e[0], e[0], e[0], 255],
            65 => [255, 255, 255, e[0]],
            49 => [e[0], e[1], 0, 255],
            54 => {
                let v = unit(half(e[0], e[1]));
                [v, v, v, 255]
            }
            56 => {
                let v = (u16::from_le_bytes([e[0], e[1]]) >> 8) as u8;
                [v, v, v, 255]
            }
            10 => [
                unit(half(e[0], e[1])),
                unit(half(e[2], e[3])),
                unit(half(e[4], e[5])),
                unit(half(e[6], e[7])),
            ],
            _ => {
                let c = |i: usize| (u16::from_le_bytes([e[i], e[i + 1]]) >> 8) as u8;
                [c(0), c(2), c(4), c(6)]
            }
        };
        px.extend_from_slice(&p);
    }
    Rgba8 {
        width: w,
        height: h,
        pixels: px,
    }
}

/// Decode one plane (array slice or cube face `slice`, mip level `mip`) to RGBA8. For volumes
/// `slice` selects the Z slice.
pub fn plane_rgba8(
    info: &TextureInfo,
    data: &[u8],
    slice: u32,
    mip: u32,
) -> Result<Rgba8, PreviewError> {
    let f = format_info(info.format).ok_or(PreviewError::Format(info.format))?;
    if !can_preview(info.format) {
        return Err(PreviewError::Format(info.format));
    }
    let plane_err = PreviewError::Plane { slice, mip };
    let (w, h) = ((info.width >> mip).max(1), (info.height >> mip).max(1));
    let per_mip: Vec<u64> = (0..info.mips)
        .map(|m| mip_bytes(&f, (info.width >> m).max(1), (info.height >> m).max(1)))
        .collect();
    let offset = match info.shape {
        Shape::Volume => {
            if mip != 0 || slice >= info.depth {
                return Err(plane_err);
            }
            per_mip[0] * u64::from(slice)
        }
        Shape::Texture2D | Shape::Array2D | Shape::Cube => {
            let slices = if info.shape == Shape::Texture2D {
                1
            } else {
                info.depth
            };
            if slice >= slices || mip >= info.mips {
                return Err(plane_err);
            }
            let total: u64 = per_mip.iter().sum();
            total * u64::from(slice) + per_mip[..mip as usize].iter().sum::<u64>()
        }
    };
    let size = if info.shape == Shape::Volume {
        per_mip[0]
    } else {
        per_mip[mip as usize]
    };
    let bytes = data
        .get(offset as usize..(offset + size) as usize)
        .ok_or(PreviewError::Truncated)?;
    Ok(if f.is_block_compressed() {
        decode_blocks(&f, w, h, bytes)
    } else {
        decode_plain(&f, w, h, bytes)
    })
}

pub fn encode_png(img: &Rgba8) -> Result<Vec<u8>, PreviewError> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header()?;
        w.write_image_data(&img.pixels)?;
        w.finish()?;
    }
    Ok(out)
}

/// PNG of mip 0 of the first slice.
pub fn png_preview(info: &TextureInfo, data: &[u8]) -> Result<Vec<u8>, PreviewError> {
    encode_png(&plane_rgba8(info, data, 0, 0)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(format: u32, w: u32, h: u32, mips: u32, shape: Shape, depth: u32) -> TextureInfo {
        TextureInfo {
            width: w,
            height: h,
            depth,
            mips,
            format,
            shape,
        }
    }

    #[test]
    fn bc1_plane_decodes_and_crops_to_the_visible_size() {
        // 6x5 BC1: 2x2 blocks. First block solid red, the rest solid blue.
        let red = {
            let mut b = [0u8; 8];
            b[0..2].copy_from_slice(&(31u16 << 11).to_le_bytes());
            b
        };
        let blue = {
            let mut b = [0u8; 8];
            b[0..2].copy_from_slice(&31u16.to_le_bytes());
            b
        };
        let mut data = Vec::new();
        for blk in [red, blue, blue, blue] {
            data.extend_from_slice(&blk);
        }
        let i = info(71, 6, 5, 1, Shape::Texture2D, 1);
        let img = plane_rgba8(&i, &data, 0, 0).unwrap();
        assert_eq!((img.width, img.height), (6, 5));
        assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(
            &img.pixels[(4 * 6 + 5) * 4..(4 * 6 + 5) * 4 + 4],
            &[0, 0, 255, 255]
        );
        let png = encode_png(&img).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn png_round_trips_through_the_decoder() {
        let img = Rgba8 {
            width: 3,
            height: 2,
            pixels: (0..24).collect(),
        };
        let bytes = encode_png(&img).unwrap();
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let fr = reader.next_frame(&mut buf).unwrap();
        assert_eq!((fr.width, fr.height), (3, 2));
        assert_eq!(&buf[..24], &img.pixels[..]);
    }

    #[test]
    fn plane_selection_walks_slices_and_mips() {
        // 2-slice 2x2 RGBA8 array with 2 mips: slice 0 = 16 + 4 bytes, slice 1 likewise.
        let mut data = Vec::new();
        for s in 0..2u8 {
            data.extend(std::iter::repeat_n(10 + s, 16));
            data.extend(std::iter::repeat_n(100 + s, 4));
        }
        let i = info(28, 2, 2, 2, Shape::Array2D, 2);
        assert_eq!(plane_rgba8(&i, &data, 1, 0).unwrap().pixels[0], 11);
        assert_eq!(plane_rgba8(&i, &data, 1, 1).unwrap().pixels, vec![101; 4]);
        assert!(matches!(
            plane_rgba8(&i, &data, 2, 0),
            Err(PreviewError::Plane { .. })
        ));
        assert!(matches!(
            plane_rgba8(&i, &data[..30], 1, 1),
            Err(PreviewError::Truncated)
        ));
    }

    #[test]
    fn bgra_is_swizzled_and_a8_becomes_alpha() {
        let i = info(87, 1, 1, 1, Shape::Texture2D, 1);
        assert_eq!(
            plane_rgba8(&i, &[1, 2, 3, 4], 0, 0).unwrap().pixels,
            vec![3, 2, 1, 4]
        );
        let i = info(65, 1, 1, 1, Shape::Texture2D, 1);
        assert_eq!(
            plane_rgba8(&i, &[9], 0, 0).unwrap().pixels,
            vec![255, 255, 255, 9]
        );
    }

    #[test]
    fn volume_slice_selection() {
        let data: Vec<u8> = (0..2 * 2 * 3).collect();
        let i = info(61, 2, 2, 1, Shape::Volume, 3);
        assert_eq!(plane_rgba8(&i, &data, 2, 0).unwrap().pixels[0], 8);
    }

    #[test]
    fn unsupported_formats_say_so() {
        let i = info(2, 1, 1, 1, Shape::Texture2D, 1);
        assert!(matches!(
            plane_rgba8(&i, &[0; 16], 0, 0),
            Err(PreviewError::Format(2))
        ));
    }
}
