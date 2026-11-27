//! DDS writing and header parsing. Payload bytes are written exactly as stored: BCn blocks are
//! never re-encoded.

use super::format::{format_info, mip_bytes};
use super::{Shape, TextureInfo};
use thiserror::Error;

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PITCH: u32 = 0x8;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x20000;
const DDSD_LINEARSIZE: u32 = 0x80000;
const DDSD_DEPTH: u32 = 0x800000;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_MIPMAP: u32 = 0x400000;
const DDSCAPS2_CUBEMAP_ALLFACES: u32 = 0xFE00;
const DDSCAPS2_VOLUME: u32 = 0x200000;
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const RESOURCE_DIMENSION_TEXTURE2D: u32 = 3;
const RESOURCE_DIMENSION_TEXTURE3D: u32 = 4;
const MISC_TEXTURECUBE: u32 = 0x4;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DdsError {
    #[error("format {0} has no DDS mapping")]
    UnknownFormat(u32),
    #[error("DDS data is truncated")]
    Truncated,
    #[error("not a DDS file")]
    Magic,
    #[error("payload is {have} bytes, the header describes {want}")]
    PayloadSize { have: u64, want: u64 },
}

enum Legacy {
    FourCc(&'static [u8; 4]),
    Masks {
        bits: u32,
        r: u32,
        g: u32,
        b: u32,
        a: u32,
    },
}

/// Formats a pre-DX10 reader can open: BC1-3 UNORM and 8-bit RGBA/BGRA. Everything else
/// (sRGB, BC4-7, float, arrays, cubemaps, volumes) gets the DX10 extension header.
fn legacy_format(dxgi: u32) -> Option<Legacy> {
    Some(match dxgi {
        71 => Legacy::FourCc(b"DXT1"),
        74 => Legacy::FourCc(b"DXT3"),
        77 => Legacy::FourCc(b"DXT5"),
        28 => Legacy::Masks {
            bits: 32,
            r: 0xff,
            g: 0xff00,
            b: 0xff_0000,
            a: 0xff00_0000,
        },
        87 => Legacy::Masks {
            bits: 32,
            r: 0xff_0000,
            g: 0xff00,
            b: 0xff,
            a: 0xff00_0000,
        },
        _ => return None,
    })
}

fn put(h: &mut [u8], at: usize, v: u32) {
    h[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Bytes the payload of `info` must have: every slice, each with its mips tightly packed.
pub fn payload_size(info: &TextureInfo) -> Result<u64, DdsError> {
    let f = format_info(info.format).ok_or(DdsError::UnknownFormat(info.format))?;
    let mut per_slice = 0u64;
    for m in 0..info.mips {
        let (w, h) = ((info.width >> m).max(1), (info.height >> m).max(1));
        per_slice += mip_bytes(&f, w, h);
    }
    Ok(match info.shape {
        Shape::Volume => mip_bytes(&f, info.width, info.height) * u64::from(info.depth),
        Shape::Texture2D => per_slice,
        Shape::Array2D | Shape::Cube => per_slice * u64::from(info.depth),
    })
}

pub fn header_bytes(info: &TextureInfo) -> Result<Vec<u8>, DdsError> {
    let f = format_info(info.format).ok_or(DdsError::UnknownFormat(info.format))?;
    let legacy = if info.shape == Shape::Texture2D {
        legacy_format(info.format)
    } else {
        None
    };
    let volume = info.shape == Shape::Volume;
    let cube = info.shape == Shape::Cube;

    let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT;
    if info.mips > 1 {
        flags |= DDSD_MIPMAPCOUNT;
    }
    if volume {
        flags |= DDSD_DEPTH;
    }
    let pitch_or_size = if f.is_block_compressed() {
        flags |= DDSD_LINEARSIZE;
        mip_bytes(&f, info.width, info.height) as u32
    } else {
        flags |= DDSD_PITCH;
        info.width * f.element_bytes
    };
    let mut caps = DDSCAPS_TEXTURE;
    if info.mips > 1 {
        caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
    }
    if info.shape != Shape::Texture2D {
        caps |= DDSCAPS_COMPLEX;
    }
    let caps2 = if cube {
        DDSCAPS2_CUBEMAP_ALLFACES
    } else if volume {
        DDSCAPS2_VOLUME
    } else {
        0
    };

    let mut h = vec![0u8; 128];
    h[0..4].copy_from_slice(b"DDS ");
    put(&mut h, 4, 124);
    put(&mut h, 8, flags);
    put(&mut h, 12, info.height);
    put(&mut h, 16, info.width);
    put(&mut h, 20, pitch_or_size);
    put(&mut h, 24, if volume { info.depth } else { 0 });
    put(&mut h, 28, info.mips.max(1));
    put(&mut h, 76, 32);
    match legacy {
        Some(Legacy::FourCc(cc)) => {
            put(&mut h, 80, DDPF_FOURCC);
            h[84..88].copy_from_slice(cc);
        }
        Some(Legacy::Masks { bits, r, g, b, a }) => {
            put(&mut h, 80, DDPF_RGB | DDPF_ALPHAPIXELS);
            put(&mut h, 88, bits);
            put(&mut h, 92, r);
            put(&mut h, 96, g);
            put(&mut h, 100, b);
            put(&mut h, 104, a);
        }
        None => {
            put(&mut h, 80, DDPF_FOURCC);
            h[84..88].copy_from_slice(b"DX10");
        }
    }
    put(&mut h, 108, caps);
    put(&mut h, 112, caps2);
    if legacy.is_none() {
        let array_size = match info.shape {
            Shape::Array2D => info.depth,
            Shape::Cube => info.depth / 6,
            _ => 1,
        };
        let mut ext = [0u32; 5];
        ext[0] = info.format;
        ext[1] = if volume {
            RESOURCE_DIMENSION_TEXTURE3D
        } else {
            RESOURCE_DIMENSION_TEXTURE2D
        };
        ext[2] = if cube { MISC_TEXTURECUBE } else { 0 };
        ext[3] = array_size;
        for v in ext {
            h.extend_from_slice(&v.to_le_bytes());
        }
    }
    Ok(h)
}

/// Header plus payload. The payload must be exactly what [`payload_size`] says.
pub fn write(info: &TextureInfo, payload: &[u8]) -> Result<Vec<u8>, DdsError> {
    let want = payload_size(info)?;
    if payload.len() as u64 != want {
        return Err(DdsError::PayloadSize {
            have: payload.len() as u64,
            want,
        });
    }
    let mut out = header_bytes(info)?;
    out.extend_from_slice(payload);
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdsHeader {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mips: u32,
    pub flags: u32,
    pub caps2: u32,
    pub fourcc: [u8; 4],
    /// DX10 extension fields.
    pub dxgi: Option<u32>,
    pub array_size: u32,
    pub is_cube: bool,
    pub is_volume: bool,
    pub data_offset: usize,
}

pub fn parse_header(data: &[u8]) -> Result<DdsHeader, DdsError> {
    if data.len() < 128 {
        return Err(DdsError::Truncated);
    }
    if &data[0..4] != b"DDS " {
        return Err(DdsError::Magic);
    }
    let u = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().expect("4 bytes"));
    let pf_flags = u(80);
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&data[84..88]);
    let mut hdr = DdsHeader {
        width: u(16),
        height: u(12),
        depth: u(24).max(1),
        mips: u(28).max(1),
        flags: u(8),
        caps2: u(112),
        fourcc: if pf_flags & DDPF_FOURCC != 0 {
            fourcc
        } else {
            [0; 4]
        },
        dxgi: None,
        array_size: 1,
        is_cube: u(112) & 0x200 != 0,
        is_volume: u(112) & DDSCAPS2_VOLUME != 0,
        data_offset: 128,
    };
    if hdr.fourcc == *b"DX10" {
        if data.len() < 148 {
            return Err(DdsError::Truncated);
        }
        hdr.dxgi = Some(u(128));
        hdr.array_size = u(140).max(1);
        hdr.is_cube |= u(136) & MISC_TEXTURECUBE != 0;
        hdr.is_volume |= u(132) == RESOURCE_DIMENSION_TEXTURE3D;
        hdr.data_offset = 148;
    }
    Ok(hdr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(format: u32, shape: Shape, w: u32, h: u32, depth: u32, mips: u32) -> TextureInfo {
        TextureInfo {
            width: w,
            height: h,
            depth,
            mips,
            format,
            shape,
        }
    }

    fn le(h: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(h[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn bc1_unorm_uses_a_legacy_header() {
        let h = header_bytes(&info(71, Shape::Texture2D, 512, 256, 1, 7)).unwrap();
        assert_eq!(h.len(), 128);
        assert_eq!(&h[0..4], b"DDS ");
        assert_eq!(le(&h, 4), 124);
        assert_eq!(le(&h, 8), 0x1007 | 0x20000 | 0x80000);
        assert_eq!(le(&h, 12), 256);
        assert_eq!(le(&h, 16), 512);
        assert_eq!(le(&h, 20), 128 * 64 * 8);
        assert_eq!(le(&h, 28), 7);
        assert_eq!(le(&h, 76), 32);
        assert_eq!(le(&h, 80), 4);
        assert_eq!(&h[84..88], b"DXT1");
        assert_eq!(le(&h, 108), 0x1000 | 0x8 | 0x400000);
        assert_eq!(le(&h, 112), 0);
    }

    #[test]
    fn bc7_srgb_uses_the_dx10_header() {
        let h = header_bytes(&info(99, Shape::Texture2D, 64, 64, 1, 1)).unwrap();
        assert_eq!(h.len(), 148);
        assert_eq!(&h[84..88], b"DX10");
        assert_eq!(le(&h, 128), 99);
        assert_eq!(le(&h, 132), 3);
        assert_eq!(le(&h, 136), 0);
        assert_eq!(le(&h, 140), 1);
        assert_eq!(le(&h, 8), 0x1007 | 0x80000);
        assert_eq!(le(&h, 108), 0x1000);
    }

    #[test]
    fn rgba8_legacy_masks_and_pitch() {
        let h = header_bytes(&info(28, Shape::Texture2D, 10, 4, 1, 1)).unwrap();
        assert_eq!(le(&h, 8), 0x1007 | 0x8);
        assert_eq!(le(&h, 20), 40);
        assert_eq!(le(&h, 80), 0x41);
        assert_eq!(le(&h, 88), 32);
        assert_eq!(le(&h, 92), 0xff);
        assert_eq!(le(&h, 104), 0xff00_0000);
    }

    #[test]
    fn cubemap_header() {
        let h = header_bytes(&info(98, Shape::Cube, 128, 128, 6, 8)).unwrap();
        assert_eq!(le(&h, 112), 0xFE00);
        assert_eq!(le(&h, 132), 3);
        assert_eq!(le(&h, 136), 4);
        assert_eq!(le(&h, 140), 1);
        assert_eq!(le(&h, 108), 0x1000 | 0x8 | 0x400000);
        let parsed = parse_header(&h).unwrap();
        assert!(parsed.is_cube);
        assert_eq!(parsed.dxgi, Some(98));
    }

    #[test]
    fn array_and_volume_headers() {
        let h = header_bytes(&info(83, Shape::Array2D, 256, 256, 8, 9)).unwrap();
        assert_eq!(le(&h, 140), 8);
        assert_eq!(le(&h, 108) & 0x8, 0x8);
        let v = header_bytes(&info(10, Shape::Volume, 32, 32, 32, 1)).unwrap();
        assert_eq!(le(&v, 8) & 0x800000, 0x800000);
        assert_eq!(le(&v, 24), 32);
        assert_eq!(le(&v, 112), 0x200000);
        assert_eq!(le(&v, 132), 4);
        let p = parse_header(&v).unwrap();
        assert!(p.is_volume);
        assert_eq!(p.depth, 32);
    }

    #[test]
    fn write_checks_payload_size_and_keeps_bytes() {
        let i = info(71, Shape::Texture2D, 8, 8, 1, 2);
        // 2x2 blocks + 1x1 block
        assert_eq!(payload_size(&i).unwrap(), 5 * 8);
        let payload: Vec<u8> = (0..40).collect();
        let dds = write(&i, &payload).unwrap();
        assert_eq!(&dds[128..], &payload[..]);
        assert!(matches!(
            write(&i, &payload[..39]),
            Err(DdsError::PayloadSize { have: 39, want: 40 })
        ));
        let parsed = parse_header(&dds).unwrap();
        assert_eq!((parsed.width, parsed.height, parsed.mips), (8, 8, 2));
        assert_eq!(parsed.fourcc, *b"DXT1");
        assert_eq!(parsed.data_offset, 128);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_header(&[0; 10]), Err(DdsError::Truncated));
        assert_eq!(parse_header(&[0; 200]), Err(DdsError::Magic));
        assert!(matches!(
            header_bytes(&info(1, Shape::Texture2D, 4, 4, 1, 1)),
            Err(DdsError::UnknownFormat(1))
        ));
    }
}
