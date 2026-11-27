//! Turning a pool descriptor into decoded texture data.

use super::format::{FormatInfo, format_info};
use super::layout::{
    ArrayLayout, CubemapLayout, LayoutError, SurfaceLayout, VolumeLayout, array_surface_layout,
    compact_surface_layout, cubemap_surface_layout, decode_chunk_pages, volume_address,
    volume_surface_layout,
};
use super::pool::{ChunkCache, Pool, ReadError, TextureDescriptor};
use super::tiling::{Region, TileError, TileSize, detile};
use super::{Shape, Texture, TextureInfo, TextureResult, Unsupported, placeholder};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TextureError {
    #[error("no texture {0} in the pool")]
    NoSuchTexture(u32),
    #[error("texture has no mapped allocation")]
    Unmapped,
    #[error("no element-size mapping for DXGI format {0}")]
    UnknownFormat(u32),
    #[error("unsupported descriptor: {0}")]
    Descriptor(String),
    #[error(transparent)]
    Layout(#[from] LayoutError),
    #[error("layout needs {layout} bytes but the allocation span is {span:?}")]
    SpanMismatch { layout: u64, span: Option<u64> },
    #[error(transparent)]
    Read(#[from] ReadError),
    #[error(transparent)]
    Tile(#[from] TileError),
}

enum Plan {
    Flat(SurfaceLayout),
    Array(ArrayLayout),
    Cube(CubemapLayout),
    Volume(VolumeLayout),
}

impl Plan {
    fn surface(&self) -> Option<&SurfaceLayout> {
        match self {
            Plan::Flat(s) => Some(s),
            Plan::Array(a) => Some(&a.surface),
            Plan::Cube(c) => Some(&c.surface),
            Plan::Volume(_) => None,
        }
    }

    fn slices(&self) -> u32 {
        match self {
            Plan::Flat(_) | Plan::Volume(_) => 1,
            Plan::Array(a) => a.depth,
            Plan::Cube(_) => 6,
        }
    }

    fn size(&self) -> u64 {
        match self {
            Plan::Flat(s) => s.size,
            Plan::Array(a) => a.size,
            Plan::Cube(c) => c.size,
            Plan::Volume(v) => v.size,
        }
    }

    /// Virtual range `(start, size)` of mip `m` of slice `s`.
    fn range(&self, base: u64, s: u32, m: usize) -> (u64, u64) {
        match self {
            Plan::Flat(l) => (base + l.mips[m].offset, l.mips[m].region),
            Plan::Array(a) => {
                let mip = &a.surface.mips[m];
                (
                    base + mip.offset + u64::from(s) * mip.slice_stride,
                    mip.region,
                )
            }
            Plan::Cube(c) => {
                let mip = &c.surface.mips[m];
                (
                    base + c.data_offset + u64::from(s) * c.face_size + mip.offset,
                    mip.region,
                )
            }
            Plan::Volume(v) => (base, v.size),
        }
    }
}

fn build_plan(d: &TextureDescriptor, fi: &FormatInfo) -> Result<(Plan, Shape), TextureError> {
    let tile = TileSize::from_descriptor_mode(d.mode);
    let bad = |s: String| Err(TextureError::Descriptor(s));
    match (d.dimension, d.depth) {
        (1, 1) => Ok((
            Plan::Flat(compact_surface_layout(
                d.width,
                d.height,
                d.mips,
                fi.element_bytes,
                fi.block,
                tile,
            )?),
            Shape::Texture2D,
        )),
        (1, _) => {
            if d.mode != 1 {
                return bad(format!("2D array with mode {}", d.mode));
            }
            Ok((
                Plan::Array(array_surface_layout(
                    d.width,
                    d.height,
                    d.depth,
                    d.mips,
                    fi.element_bytes,
                    fi.block,
                    tile,
                )?),
                Shape::Array2D,
            ))
        }
        (2, _) => {
            if d.depth != 6 || d.non_tail > 3 || d.mode != 18 + 2 * d.non_tail {
                return bad(format!(
                    "cubemap depth {} mode {} non_tail {}",
                    d.depth, d.mode, d.non_tail
                ));
            }
            let pages = if d.meta == 0 && d.extra == 0 {
                None
            } else {
                match decode_chunk_pages(d.meta, d.extra) {
                    Some(p) => Some(p),
                    None => return bad("cubemap chunk metadata".into()),
                }
            };
            Ok((
                Plan::Cube(cubemap_surface_layout(
                    d.width,
                    d.height,
                    d.mips,
                    fi.element_bytes,
                    fi.block,
                    d.non_tail,
                    pages.as_deref(),
                )?),
                Shape::Cube,
            ))
        }
        (3, _) => {
            if d.mode != 2 || d.mips != 1 || fi.is_block_compressed() {
                return bad(format!(
                    "volume mode {} mips {} format {}",
                    d.mode, d.mips, d.format
                ));
            }
            Ok((
                Plan::Volume(volume_surface_layout(
                    d.width,
                    d.height,
                    d.depth,
                    d.mips,
                    fi.element_bytes,
                )?),
                Shape::Volume,
            ))
        }
        (dim, _) => bad(format!("dimension {dim}")),
    }
}

/// Detile one mip and crop it to the visible blocks, rows tightly packed.
pub(super) fn extract_mip(
    surface: &SurfaceLayout,
    mip: &super::layout::MipLayout,
    tiled: &[u8],
) -> Result<Vec<u8>, TileError> {
    let log = surface.element_bytes.trailing_zeros();
    let region = Region {
        tile: surface.tile,
        log2_element: log,
        tiles_per_row: mip.padded_w / surface.tile_w,
        origin_x: mip.tile_x,
        origin_y: mip.tile_y,
        width: mip.blocks_w,
        height: mip.blocks_h,
    };
    let full = detile(&region, tiled)?;
    let (vw, vh) = mip.visible_blocks(surface.block);
    if (vw, vh) == (mip.blocks_w, mip.blocks_h) {
        return Ok(full);
    }
    let eb = surface.element_bytes as usize;
    let row = mip.blocks_w as usize * eb;
    let keep = vw as usize * eb;
    let mut out = Vec::with_capacity(keep * vh as usize);
    for y in 0..vh as usize {
        out.extend_from_slice(&full[y * row..y * row + keep]);
    }
    Ok(out)
}

fn extract_volume(layout: &VolumeLayout, tiled: &[u8]) -> Result<Vec<u8>, TileError> {
    if (tiled.len() as u64) < layout.size {
        return Err(TileError::TooShort {
            have: tiled.len(),
            need: layout.size,
        });
    }
    let eb = layout.element_bytes as usize;
    let mut out = Vec::with_capacity(
        layout.width as usize * layout.height as usize * layout.depth as usize * eb,
    );
    for z in 0..layout.depth {
        for y in 0..layout.height {
            for x in 0..layout.width {
                let at = volume_address(layout, x, y, z) as usize;
                out.extend_from_slice(&tiled[at..at + eb]);
            }
        }
    }
    Ok(out)
}

impl Pool<'_> {
    /// Decode one texture of the pool. Textures whose chunks need BCPack come back as
    /// [`TextureResult::Partial`] (2D only: the readable mips) or
    /// [`TextureResult::Placeholder`]; hard failures (layout, corrupt chunks) are errors, and
    /// callers can use [`placeholder`] for those too.
    pub fn decode_texture(
        &self,
        index: u32,
        cache: &mut ChunkCache,
    ) -> Result<TextureResult, TextureError> {
        let d = self
            .descriptor(index)
            .ok_or(TextureError::NoSuchTexture(index))?;
        if d.is_unmapped() {
            return Err(TextureError::Unmapped);
        }
        let fi = format_info(d.format).ok_or(TextureError::UnknownFormat(d.format))?;
        let (plan, shape) = build_plan(&d, &fi)?;
        let span = self.span(index, d.base);
        if span != Some(plan.size()) {
            return Err(TextureError::SpanMismatch {
                layout: plan.size(),
                span,
            });
        }
        let info = TextureInfo {
            width: d.width,
            height: d.height,
            depth: match shape {
                Shape::Texture2D => 1,
                _ => d.depth,
            },
            mips: d.mips,
            format: d.format,
            shape,
        };

        let mip_count = plan.surface().map_or(1, |s| s.mips.len());
        let mut avail = Vec::with_capacity(mip_count);
        let mut blocker = None;
        for m in 0..mip_count {
            let mut ok = true;
            for s in 0..plan.slices() {
                let (start, size) = plan.range(d.base, s, m);
                let set = self.codecs(start, size)?;
                if let Some(c) = set.undecodable_bcpack() {
                    blocker.get_or_insert(Unsupported::BcPack(c));
                    ok = false;
                }
            }
            avail.push(ok);
        }

        if avail.iter().all(|a| *a) {
            let data = self.read_all(&plan, d.base, 0, mip_count, cache)?;
            return Ok(TextureResult::Complete(Texture { info, data }));
        }
        let reason = blocker.expect("an unavailable mip has a blocker");
        let usable = avail.iter().position(|a| *a);
        if let (Plan::Flat(_), Some(first)) = (&plan, usable) {
            let run = avail[first..].iter().take_while(|a| **a).count();
            let data = self.read_all(&plan, d.base, first, first + run, cache)?;
            let texture = Texture {
                info: TextureInfo {
                    width: (d.width >> first).max(1),
                    height: (d.height >> first).max(1),
                    mips: run as u32,
                    ..info
                },
                data,
            };
            return Ok(TextureResult::Partial {
                texture,
                first_mip: first as u32,
                missing: reason,
            });
        }
        Ok(TextureResult::Placeholder {
            texture: placeholder(&info),
            original: info,
            reason,
        })
    }

    fn read_all(
        &self,
        plan: &Plan,
        base: u64,
        from: usize,
        to: usize,
        cache: &mut ChunkCache,
    ) -> Result<Vec<u8>, TextureError> {
        if let Plan::Volume(v) = plan {
            let tiled = self.read_range(base, v.size, cache)?;
            return Ok(extract_volume(v, &tiled)?);
        }
        let surface = plan.surface().expect("non-volume plans have a surface");
        let mut out = Vec::new();
        for s in 0..plan.slices() {
            for m in from..to {
                let (start, size) = plan.range(base, s, m);
                let tiled = self.read_range(start, size, cache)?;
                out.extend_from_slice(&extract_mip(surface, &surface.mips[m], &tiled)?);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::dds;
    use crate::texture::pool::builder::{Packed, first_page, pool, record};
    use crate::texture::tiling::{Region, tile_region};

    /// Linear pixels of a w x h RGBA8 image: a gradient plus noise.
    fn image(w: u32, h: u32, seed: u32) -> Vec<u8> {
        (0..w * h * 4)
            .map(|i| (i.wrapping_mul(2654435761).wrapping_add(seed) >> 13) as u8)
            .collect()
    }

    /// Write `linear` mips of `layout` into a tiled allocation.
    fn tile_chain(layout: &SurfaceLayout, mips: &[Vec<u8>], alloc: &mut [u8], base: usize) {
        for (m, lin) in layout.mips.iter().zip(mips) {
            let region = Region {
                tile: layout.tile,
                log2_element: layout.element_bytes.trailing_zeros(),
                tiles_per_row: m.padded_w / layout.tile_w,
                origin_x: m.tile_x,
                origin_y: m.tile_y,
                width: m.blocks_w,
                height: m.blocks_h,
            };
            let at = base + m.offset as usize;
            tile_region(&region, lin, &mut alloc[at..at + m.region as usize]);
        }
    }

    fn mip_chain(w: u32, h: u32, mips: u32) -> Vec<Vec<u8>> {
        (0..mips)
            .map(|i| image((w >> i).max(1), (h >> i).max(1), i))
            .collect()
    }

    #[test]
    fn rgba8_chain_with_a_mip_tail_round_trips_through_a_pool() {
        let (w, h, mips) = (256, 128, 8);
        let layout = compact_surface_layout(w, h, mips, 4, 1, TileSize::K4).unwrap();
        assert!(layout.first_tail < mips);
        let chain = mip_chain(w, h, mips);
        let mut alloc = vec![0u8; layout.size as usize];
        tile_chain(&layout, &chain, &mut alloc, 0);

        let first = first_page(1, 1);
        let rec = record(w, h, mips, 3, 28, 1, 1, 2, first, 0, 0);
        let file = pool(&[rec], &[9], &[Packed::Zlib(alloc)]);
        let p = Pool::parse(&file).unwrap();
        let TextureResult::Complete(t) = p.decode_texture(0, &mut ChunkCache::new()).unwrap()
        else {
            panic!("expected a complete texture");
        };
        assert_eq!(t.info.mips, 8);
        assert_eq!(t.data, chain.concat());
        let dds = dds::write(&t.info, &t.data).unwrap();
        assert_eq!(&dds[0..4], b"DDS ");
    }

    #[test]
    fn bc1_odd_sized_chain_is_cropped_to_visible_blocks() {
        // 17x13 BC1: mip 0 is 5x4 blocks, mip 1 is stored as 3x2 blocks but only 2x2 are visible.
        let (w, h, mips) = (17, 13, 5);
        let layout = compact_surface_layout(w, h, mips, 8, 4, TileSize::K4).unwrap();
        let mut chain = Vec::new();
        let mut visible = Vec::new();
        for (i, m) in layout.mips.iter().enumerate() {
            let lin: Vec<u8> = (0..m.blocks_w * m.blocks_h * 8)
                .map(|b| (b as usize * 5 + i * 11) as u8)
                .collect();
            let (vw, vh) = m.visible_blocks(4);
            for y in 0..vh {
                let a = (y * m.blocks_w * 8) as usize;
                visible.extend_from_slice(&lin[a..a + (vw * 8) as usize]);
            }
            chain.push(lin);
        }
        let mut alloc = vec![0u8; layout.size as usize];
        tile_chain(&layout, &chain, &mut alloc, 0);
        let first = first_page(1, 1);
        let rec = record(w, h, mips, 1, 71, 1, 1, 2, first, 0, 0);
        let file = pool(&[rec], &[1], &[Packed::Raw(alloc.clone())]);
        let p = Pool::parse(&file).unwrap();
        let t = match p.decode_texture(0, &mut ChunkCache::new()).unwrap() {
            TextureResult::Complete(t) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(t.data, visible);
        assert_eq!(t.data.len() as u64, dds::payload_size(&t.info).unwrap());
    }

    #[test]
    fn array_slices_round_trip() {
        let (w, h, mips, depth) = (512, 512, 10, 3);
        let arr = array_surface_layout(w, h, depth, mips, 4, 1, TileSize::K64).unwrap();
        let mut alloc = vec![0u8; arr.size as usize];
        let mut expect = Vec::new();
        for s in 0..depth {
            let chain = mip_chain(w, h, mips);
            let chain: Vec<Vec<u8>> = chain
                .into_iter()
                .map(|c| c.iter().map(|b| b.wrapping_add(s as u8 * 37)).collect())
                .collect();
            for (m, lin) in arr.surface.mips.iter().zip(&chain) {
                let region = Region {
                    tile: TileSize::K64,
                    log2_element: 2,
                    tiles_per_row: m.padded_w / arr.surface.tile_w,
                    origin_x: m.tile_x,
                    origin_y: m.tile_y,
                    width: m.blocks_w,
                    height: m.blocks_h,
                };
                let at = (m.offset + u64::from(s) * m.slice_stride) as usize;
                tile_region(&region, lin, &mut alloc[at..at + m.region as usize]);
            }
            expect.extend(chain.concat());
        }
        let first = first_page(1, 1);
        let rec = record(w, h, mips, 3, 28, depth, 1, 1, first, 0, 0);
        let file = pool(&[rec], &[1], &[Packed::Zlib(alloc)]);
        let p = Pool::parse(&file).unwrap();
        let t = match p.decode_texture(0, &mut ChunkCache::new()).unwrap() {
            TextureResult::Complete(t) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(t.info.shape, Shape::Array2D);
        assert_eq!(t.info.depth, 3);
        assert_eq!(t.data, expect);
    }

    #[test]
    fn cubemap_faces_round_trip() {
        let (w, h, mips) = (32, 32, 6);
        let layout = compact_surface_layout(w, h, mips, 4, 1, TileSize::K4).unwrap();
        let mut alloc = vec![0u8; 6 * layout.size as usize];
        let mut expect = Vec::new();
        for f in 0..6u32 {
            let chain: Vec<Vec<u8>> = mip_chain(w, h, mips)
                .into_iter()
                .map(|c| c.iter().map(|b| b.wrapping_add(f as u8 * 41)).collect())
                .collect();
            tile_chain(
                &layout,
                &chain,
                &mut alloc,
                f as usize * layout.size as usize,
            );
            expect.extend(chain.concat());
        }
        let first = first_page(1, 1);
        let rec = record(w, h, mips, 0, 28, 6, 2, 18, first, 0, 0);
        let file = pool(&[rec], &[1], &[Packed::Zlib(alloc)]);
        let p = Pool::parse(&file).unwrap();
        let t = match p.decode_texture(0, &mut ChunkCache::new()).unwrap() {
            TextureResult::Complete(t) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(t.info.shape, Shape::Cube);
        assert_eq!(t.data, expect);
    }

    #[test]
    fn volume_round_trips() {
        let (w, h, d) = (16, 8, 8);
        let vol = volume_surface_layout(w, h, d, 1, 2).unwrap();
        let linear: Vec<u8> = (0..w * h * d * 2).map(|i| (i * 13 + 5) as u8).collect();
        let mut alloc = vec![0u8; vol.size as usize];
        let mut i = 0;
        for z in 0..d {
            for y in 0..h {
                for x in 0..w {
                    let at = volume_address(&vol, x, y, z) as usize;
                    alloc[at..at + 2].copy_from_slice(&linear[i..i + 2]);
                    i += 2;
                }
            }
        }
        let first = first_page(1, 1);
        let rec = record(w, h, 1, 0, 54, d, 3, 2, first, 0, 0);
        let file = pool(&[rec], &[1], &[Packed::Zlib(alloc)]);
        let p = Pool::parse(&file).unwrap();
        let t = match p.decode_texture(0, &mut ChunkCache::new()).unwrap() {
            TextureResult::Complete(t) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(t.info.shape, Shape::Volume);
        assert_eq!(t.data, linear);
    }

    #[test]
    fn bcpack_mip0_gives_a_partial_chain_from_mip_one() {
        // 256x256 BC1, 4 mips: mips 2 and 3 share the tail tile, mip 1 and mip 0 are
        // standalone. Mip 0 lives in its own BCPack chunk, the rest is zlib.
        let (w, h, mips) = (256, 256, 4);
        let layout = compact_surface_layout(w, h, mips, 8, 4, TileSize::K4).unwrap();
        assert_eq!(layout.first_tail, 2);
        let chain: Vec<Vec<u8>> = layout
            .mips
            .iter()
            .enumerate()
            .map(|(i, m)| {
                (0..m.blocks_w * m.blocks_h * 8)
                    .map(|b| (b + i as u32 * 3) as u8)
                    .collect()
            })
            .collect();
        let mut alloc = vec![0u8; layout.size as usize];
        tile_chain(&layout, &chain, &mut alloc, 0);
        let split = layout.mips[0].offset as usize;
        assert_eq!(split, 12288);
        let (low, high) = alloc.split_at(split);
        let first = first_page(1, 1);
        let rec = record(w, h, mips, 1, 71, 1, 1, 2, first, 0, 0);
        let file = pool(
            &[rec],
            &[1],
            &[
                Packed::Zlib(low.to_vec()),
                Packed::BcPack(1, vec![0x1f; 300], high.len() as u64),
            ],
        );
        let p = Pool::parse(&file).unwrap();
        let r = p.decode_texture(0, &mut ChunkCache::new()).unwrap();
        let TextureResult::Partial {
            texture,
            first_mip,
            missing,
        } = r
        else {
            panic!("expected partial");
        };
        assert_eq!(first_mip, 1);
        assert_eq!(missing, Unsupported::BcPack(super::super::BcPackCodec::Bc1));
        assert_eq!(
            (texture.info.width, texture.info.height, texture.info.mips),
            (128, 128, 3)
        );
        assert_eq!(texture.data, chain[1..].concat());
    }

    #[test]
    fn all_bcpack_texture_gets_a_placeholder() {
        // 64x64 BC1, one mip: 16x16 blocks fill exactly one 4 KiB tile.
        let first = first_page(1, 1);
        let rec = record(64, 64, 1, 0, 71, 1, 1, 2, first, 0, 0);
        let file = pool(&[rec], &[1], &[Packed::BcPack(1, vec![0x1f; 300], 4096)]);
        let p = Pool::parse(&file).unwrap();
        match p.decode_texture(0, &mut ChunkCache::new()).unwrap() {
            TextureResult::Placeholder {
                texture,
                original,
                reason,
            } => {
                assert_eq!(reason, Unsupported::BcPack(super::super::BcPackCodec::Bc1));
                assert_eq!((original.width, original.format), (64, 71));
                assert_eq!(texture.info.format, 28);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unmapped_and_missing_textures_are_errors() {
        let rec = record(4, 4, 1, 0, 28, 1, 1, 2, 0, 0, 0);
        let file = pool(&[rec], &[1], &[Packed::Raw(vec![0; 4096])]);
        let p = Pool::parse(&file).unwrap();
        assert_eq!(
            p.decode_texture(0, &mut ChunkCache::new()),
            Err(TextureError::Unmapped)
        );
        assert_eq!(
            p.decode_texture(5, &mut ChunkCache::new()),
            Err(TextureError::NoSuchTexture(5))
        );
    }

    #[test]
    fn wrong_span_is_reported() {
        let first = first_page(1, 1);
        let rec = record(256, 256, 1, 0, 28, 1, 1, 2, first, 0, 0);
        let file = pool(&[rec], &[1], &[Packed::Zlib(vec![0; 8192])]);
        let p = Pool::parse(&file).unwrap();
        assert!(matches!(
            p.decode_texture(0, &mut ChunkCache::new()),
            Err(TextureError::SpanMismatch {
                layout: 262_144,
                span: Some(8192)
            })
        ));
    }
}
