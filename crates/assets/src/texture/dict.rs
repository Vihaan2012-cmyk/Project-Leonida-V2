//! `pgDictionary<grcTexture>`: a small system page of names, hashes and `grcTexture`
//! descriptors, paired with a graphics page that holds the texel data.
//!
//! The descriptor fields (name hashes, `0x980` descriptor type, size and format at `+0x38`,
//! mip count in the flags at `+0x40`) were measured on `textures.rpf`; the tiled mip layout of
//! embedded textures was not measured against known content, so [`DictTexture::decode`] applies
//! the same compact Gfx10 chain as the pools and takes the tile size from the caller.

use super::extract::{TextureError, extract_mip};
use super::format::format_info;
use super::layout::compact_surface_layout;
use super::tiling::TileSize;
use super::{Shape, Texture, TextureInfo};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DictError {
    #[error("texture dictionary page truncated: {0} bytes")]
    Truncated(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictTexture {
    pub name: String,
    /// Lower-case joaat hash of the name.
    pub hash: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub format: u32,
    pub mips: u32,
    pub flags: u64,
    /// Offset of the first byte in the graphics page.
    pub data_offset: u64,
    pub descriptor_type: u64,
}

/// Jenkins one-at-a-time hash of the lower-cased ASCII name.
pub fn joaat(name: &str) -> u32 {
    let mut h: u32 = 0;
    for c in name.bytes() {
        h = h.wrapping_add(u32::from(c.to_ascii_lowercase()));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(at..at + 8)?.try_into().ok()?))
}

/// A pointer into the system page: `0x5xxxxxxx` with a non-zero offset that fits.
fn system_offset(ptr: u64, len: usize) -> Option<usize> {
    if ptr >> 28 != 5 {
        return None;
    }
    let off = (ptr & 0x0FFF_FFFF) as usize;
    (off > 0 && off + 8 <= len).then_some(off)
}

/// Parse the system page of a texture dictionary.
pub fn parse_dictionary_page(page: &[u8]) -> Result<Vec<DictTexture>, DictError> {
    if page.len() < 400 {
        return Err(DictError::Truncated(page.len()));
    }
    let mut layout = None;
    if let (Some(h), Some(p), Some(n)) =
        (u64_at(page, 0x20), u64_at(page, 0x30), u16_at(page, 0x38))
    {
        let n = usize::from(n);
        if let (Some(ho), Some(po)) = (system_offset(h, page.len()), system_offset(p, page.len()))
            && (1..=64).contains(&n)
            && po + n * 8 <= page.len()
            && ho + n * 4 <= page.len()
        {
            layout = Some((po, ho, n));
        }
    }
    let (pointers, hashes, count) = layout.unwrap_or_else(|| {
        let at = |o: usize| u64_at(page, o).and_then(|v| system_offset(v, page.len()));
        if at(0x170).is_some() {
            (0x170, 0x180, 2)
        } else if at(0x178).is_some() && at(0x180).is_some() {
            (0x178, 0x188, 2)
        } else {
            (0x170, 0x180, 2)
        }
    });

    let names: Vec<(u32, String)> = page[0x140..pointers.max(0x140)]
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let n: String = s.iter().map(|b| char::from(*b)).collect();
            (joaat(&n), n)
        })
        .collect();

    let mut out = Vec::new();
    for i in 0..count {
        let Some(raw) = u64_at(page, pointers + i * 8) else {
            break;
        };
        let hash = u32_at(page, hashes + i * 4).unwrap_or(0);
        let Some(ptr) = system_offset(raw, page.len()) else {
            continue;
        };
        if ptr + 0x48 > page.len() {
            continue;
        }
        let (Some(dtype), Some(data), Some(w), Some(h), Some(d), Some(flags)) = (
            u64_at(page, ptr + 0x10),
            u64_at(page, ptr + 0x18),
            u16_at(page, ptr + 0x38),
            u16_at(page, ptr + 0x3A),
            u16_at(page, ptr + 0x3C),
            u64_at(page, ptr + 0x40),
        ) else {
            continue;
        };
        let name = names
            .iter()
            .find(|(nh, _)| *nh == hash)
            .map_or_else(|| format!("tex_{hash:08x}"), |(_, n)| n.clone());
        out.push(DictTexture {
            name,
            hash,
            width: u32::from(w),
            height: u32::from(h),
            depth: u32::from(d).max(1),
            format: u32::from(page[ptr + 0x3F]),
            mips: (((flags >> 16) & 0xFF) as u32).max(1),
            flags,
            data_offset: data & 0x0FFF_FFFF,
            descriptor_type: dtype,
        });
    }
    Ok(out)
}

impl DictTexture {
    /// Tiled textures carry `flags & 0xFFFF != 0` (5 in the measured dictionaries); the
    /// rest are linear with tightly packed rows.
    pub fn is_tiled(&self) -> bool {
        self.flags & 0xFFFF != 0
    }

    /// Read the texture out of the dictionary's graphics page (2D only).
    pub fn decode(&self, graphics: &[u8], tile: TileSize) -> Result<Texture, TextureError> {
        let fi = format_info(self.format).ok_or(TextureError::UnknownFormat(self.format))?;
        if self.depth != 1 {
            return Err(TextureError::Descriptor(format!(
                "dictionary texture depth {}",
                self.depth
            )));
        }
        let info = TextureInfo {
            width: self.width,
            height: self.height,
            depth: 1,
            mips: self.mips,
            format: self.format,
            shape: Shape::Texture2D,
        };
        let base = self.data_offset as usize;
        let short = |need: u64| {
            TextureError::Descriptor(format!(
                "graphics page has {} bytes after the data offset, the texture needs {need}",
                graphics.len().saturating_sub(base)
            ))
        };
        if !self.is_tiled() {
            let need = super::dds::payload_size(&info)
                .map_err(|_| TextureError::UnknownFormat(self.format))?;
            let data = graphics
                .get(base..base + need as usize)
                .ok_or_else(|| short(need))?
                .to_vec();
            return Ok(Texture { info, data });
        }
        let layout = compact_surface_layout(
            self.width,
            self.height,
            self.mips,
            fi.element_bytes,
            fi.block,
            tile,
        )?;
        let alloc = graphics
            .get(base..base + layout.size as usize)
            .ok_or_else(|| short(layout.size))?;
        let mut data = Vec::new();
        for m in &layout.mips {
            let tiled = &alloc[m.offset as usize..(m.offset + m.region) as usize];
            data.extend_from_slice(&extract_mip(&layout, m, tiled)?);
        }
        Ok(Texture { info, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::tiling::{Region, tile_region};

    #[allow(clippy::too_many_arguments)]
    fn put_texture(
        page: &mut [u8],
        at: usize,
        w: u16,
        h: u16,
        fmt: u8,
        mips: u8,
        flags: u16,
        data: u32,
    ) {
        page[at + 0x10..at + 0x18].copy_from_slice(&0x980u64.to_le_bytes());
        page[at + 0x18..at + 0x20]
            .copy_from_slice(&(0x6000_0000u64 + u64::from(data)).to_le_bytes());
        page[at + 0x38..at + 0x3A].copy_from_slice(&w.to_le_bytes());
        page[at + 0x3A..at + 0x3C].copy_from_slice(&h.to_le_bytes());
        page[at + 0x3C..at + 0x3E].copy_from_slice(&1u16.to_le_bytes());
        page[at + 0x3F] = fmt;
        let flags = u64::from(flags) | (u64::from(mips) << 16);
        page[at + 0x40..at + 0x48].copy_from_slice(&flags.to_le_bytes());
    }

    fn page() -> Vec<u8> {
        let mut p = vec![0u8; 0x300];
        p[0x20..0x28].copy_from_slice(&(0x5000_0180u64).to_le_bytes());
        p[0x30..0x38].copy_from_slice(&(0x5000_0170u64).to_le_bytes());
        p[0x38..0x3A].copy_from_slice(&2u16.to_le_bytes());
        let names = b"waterfog2\0waterfog\0";
        p[0x140..0x140 + names.len()].copy_from_slice(names);
        p[0x170..0x178].copy_from_slice(&0x5000_0200u64.to_le_bytes());
        p[0x178..0x180].copy_from_slice(&0x5000_0260u64.to_le_bytes());
        p[0x180..0x184].copy_from_slice(&joaat("waterfog2").to_le_bytes());
        p[0x184..0x188].copy_from_slice(&joaat("waterfog").to_le_bytes());
        put_texture(&mut p, 0x200, 128, 128, 95, 1, 5, 0);
        put_texture(&mut p, 0x260, 128, 128, 95, 1, 5, 16384);
        p
    }

    #[test]
    fn joaat_matches_the_measured_hashes() {
        assert_eq!(joaat("waterfog2"), 0x9b9b31df);
        assert_eq!(joaat("waterfog"), 0xf1389e16);
        assert_eq!(joaat("WaterFog"), joaat("waterfog"));
    }

    #[test]
    fn parses_names_formats_and_offsets() {
        let t = parse_dictionary_page(&page()).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].name, "waterfog2");
        assert_eq!(t[1].name, "waterfog");
        assert_eq!(
            (t[0].width, t[0].height, t[0].format, t[0].mips),
            (128, 128, 95, 1)
        );
        assert_eq!(t[0].data_offset, 0);
        assert_eq!(t[1].data_offset, 16384);
        assert!(t[0].is_tiled());
        assert_eq!(t[0].descriptor_type, 0x980);
    }

    #[test]
    fn rejects_short_pages() {
        assert_eq!(
            parse_dictionary_page(&[0; 100]),
            Err(DictError::Truncated(100))
        );
    }

    #[test]
    fn tiled_bc6h_texture_round_trips() {
        let t = parse_dictionary_page(&page()).unwrap();
        let linear: Vec<u8> = (0..32 * 32 * 16).map(|i| (i * 5 + 1) as u8).collect();
        let region = Region {
            tile: TileSize::K4,
            log2_element: 4,
            tiles_per_row: 2,
            origin_x: 0,
            origin_y: 0,
            width: 32,
            height: 32,
        };
        let mut gfx = vec![0u8; 32768];
        tile_region(&region, &linear, &mut gfx[16384..]);
        let tex = t[1].decode(&gfx, TileSize::K4).unwrap();
        assert_eq!(tex.info.format, 95);
        assert_eq!(tex.data, linear);
        assert!(t[1].decode(&gfx[..20000], TileSize::K4).is_err());
    }

    #[test]
    fn linear_texture_is_read_as_packed_rows() {
        let mut p = page();
        put_texture(&mut p, 0x200, 4, 2, 10, 1, 0, 8);
        let t = parse_dictionary_page(&p).unwrap();
        assert!(!t[0].is_tiled());
        let gfx: Vec<u8> = (0..80).collect();
        let tex = t[0].decode(&gfx, TileSize::K4).unwrap();
        assert_eq!(tex.data, gfx[8..72].to_vec());
    }
}
