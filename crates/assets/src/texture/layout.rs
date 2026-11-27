//! Allocation layouts of TTPF surfaces: the Gfx10 compact mip chain, 2D arrays, cubemaps and
//! single-mip volumes. Each layout was measured byte-exact against the allocation spans of every
//! descriptor in the retail pools.

use super::tiling::{TileSize, tile_dims};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LayoutError {
    #[error("invalid surface dimensions {0}x{1}")]
    Dimensions(u32, u32),
    #[error("unsupported element size {0}")]
    ElementSize(u32),
    #[error("unsupported block dimension {0}")]
    Block(u32),
    #[error("invalid mip count {mips} for {width}x{height}")]
    MipCount { mips: u32, width: u32, height: u32 },
    #[error("invalid array depth {0}")]
    Depth(u32),
    #[error("unsupported cubemap non_tail {0}")]
    CubemapSnapshots(u32),
    #[error("cubemap chunk pages {0:?} do not describe {1} snapshots")]
    CubemapPages(Vec<u32>, u32),
    #[error("only single-mip volumes are measured, got {0} mips")]
    VolumeMips(u32),
}

/// One mip of a surface. Offsets are relative to the start of the allocation (for cubemaps, to
/// the start of one face).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MipLayout {
    pub index: u32,
    pub blocks_w: u32,
    pub blocks_h: u32,
    pub padded_w: u32,
    pub padded_h: u32,
    pub pixels_w: u32,
    pub pixels_h: u32,
    pub offset: u64,
    pub region: u64,
    /// Element origin inside the shared tile for mip-tail mips.
    pub tile_x: u32,
    pub tile_y: u32,
    pub in_tail: bool,
    /// Distance between slices of an array (`region` for standalone mips, one tile for the tail).
    pub slice_stride: u64,
}

impl MipLayout {
    /// Visible block grid: the blocks that hold texels of this mip.
    pub fn visible_blocks(&self, block: u32) -> (u32, u32) {
        (
            self.pixels_w.div_ceil(block).max(1),
            self.pixels_h.div_ceil(block).max(1),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceLayout {
    /// Allocation size of one slice (or of the whole surface for 2D).
    pub size: u64,
    pub tile: TileSize,
    pub first_tail: u32,
    /// Tile size in elements.
    pub tile_w: u32,
    pub tile_h: u32,
    pub element_bytes: u32,
    pub block: u32,
    pub levels: u32,
    pub mips: Vec<MipLayout>,
}

const fn max_tail_mips(tile: TileSize) -> u32 {
    match tile {
        TileSize::K4 => 8,
        TileSize::K64 => 12,
    }
}

const fn tile_origin_unit(log2: u32) -> (u32, u32) {
    match log2 {
        0 => (16, 16),
        1 => (16, 8),
        2 => (8, 8),
        3 => (8, 4),
        _ => (4, 4),
    }
}

/// Mips are stored smallest first. The last few mips (those whose element grid fits half a tile
/// width and a tile height) share one tile at offset 0, each at a fixed entry offset inside it;
/// the larger mips follow as padded standalone regions.
pub fn compact_surface_layout(
    width: u32,
    height: u32,
    mips: u32,
    element_bytes: u32,
    block: u32,
    tile: TileSize,
) -> Result<SurfaceLayout, LayoutError> {
    if width == 0 || height == 0 {
        return Err(LayoutError::Dimensions(width, height));
    }
    if !matches!(element_bytes, 1 | 2 | 4 | 8 | 16) {
        return Err(LayoutError::ElementSize(element_bytes));
    }
    if block != 1 && block != 4 {
        return Err(LayoutError::Block(block));
    }
    let levels = 32 - width.max(height).leading_zeros();
    if mips < 1 || mips > levels {
        return Err(LayoutError::MipCount {
            mips,
            width,
            height,
        });
    }
    let log = element_bytes.trailing_zeros();
    let (bw, bh) = tile_dims(tile, log);
    let max_tail = max_tail_mips(tile);
    let (mw, mh) = tile_origin_unit(log);
    let ew = width.div_ceil(block);
    let eh = height.div_ceil(block);
    let grid = |i: u32| {
        (
            ((ew + (1 << i) - 1) >> i).max(1),
            ((eh + (1 << i) - 1) >> i).max(1),
        )
    };
    let pixels = |i: u32| ((width >> i).max(1), (height >> i).max(1));

    let mut tail = mips;
    let mut out: Vec<MipLayout> = Vec::with_capacity(mips as usize);
    for i in 0..mips {
        let (wc, hc) = grid(i);
        if mips > 1 && wc <= bw / 2 && hc <= bh && mips - i <= max_tail {
            tail = i;
            break;
        }
        let pw = wc.div_ceil(bw) * bw;
        let ph = hc.div_ceil(bh) * bh;
        let (pxw, pxh) = pixels(i);
        let region = u64::from(pw) * u64::from(ph) * u64::from(element_bytes);
        out.push(MipLayout {
            index: i,
            blocks_w: wc,
            blocks_h: hc,
            padded_w: pw,
            padded_h: ph,
            pixels_w: pxw,
            pixels_h: pxh,
            offset: 0,
            region,
            tile_x: 0,
            tile_y: 0,
            in_tail: false,
            slice_stride: region,
        });
    }

    let mut off = if tail < mips {
        u64::from(tile.bytes())
    } else {
        0
    };
    for m in out.iter_mut().rev() {
        m.offset = off;
        off += m.region;
    }
    for i in tail..mips {
        let entry = max_tail - 1 - (i - tail);
        let moff: u32 = if entry > 6 { 16 << entry } else { entry << 8 };
        let (mut x, mut y) = (0u32, 0u32);
        for bit in 0..6 {
            x |= ((moff >> (9 + 2 * bit)) & 1) << bit;
            y |= ((moff >> (8 + 2 * bit)) & 1) << bit;
        }
        let (wc, hc) = grid(i);
        let (pxw, pxh) = pixels(i);
        out.push(MipLayout {
            index: i,
            blocks_w: wc,
            blocks_h: hc,
            padded_w: bw,
            padded_h: bh,
            pixels_w: pxw,
            pixels_h: pxh,
            offset: 0,
            region: u64::from(tile.bytes()),
            tile_x: x * mw,
            tile_y: y * mh,
            in_tail: true,
            slice_stride: u64::from(tile.bytes()),
        });
    }
    Ok(SurfaceLayout {
        size: off,
        tile,
        first_tail: tail,
        tile_w: bw,
        tile_h: bh,
        element_bytes,
        block,
        levels,
        mips: out,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrayLayout {
    pub depth: u32,
    pub slice_size: u64,
    /// Total allocation: `depth * slice_size` for arrays without a tail, otherwise the same
    /// measured span (`size` includes the per-slice tail tiles).
    pub size: u64,
    pub surface: SurfaceLayout,
}

/// Mip-major 2D array: the tail tiles of all slices come first (slice `s` at `s * tile`), then
/// each standalone mip for all slices, from the smallest standalone mip to mip 0.
pub fn array_surface_layout(
    width: u32,
    height: u32,
    depth: u32,
    mips: u32,
    element_bytes: u32,
    block: u32,
    tile: TileSize,
) -> Result<ArrayLayout, LayoutError> {
    if depth < 1 {
        return Err(LayoutError::Depth(depth));
    }
    let mut surface = compact_surface_layout(width, height, mips, element_bytes, block, tile)?;
    let slice_size = surface.size;
    let d = u64::from(depth);
    let mut off = if surface.first_tail < mips {
        d * u64::from(tile.bytes())
    } else {
        0
    };
    for m in surface.mips.iter_mut().rev() {
        if m.in_tail {
            m.offset = 0;
            m.slice_stride = u64::from(tile.bytes());
        } else {
            m.offset = off;
            m.slice_stride = m.region;
            off += d * m.region;
        }
    }
    surface.size = off;
    Ok(ArrayLayout {
        depth,
        slice_size,
        size: off,
        surface,
    })
}

/// Decode the 43-bit `(extra << 11) | meta` list of up to four chunk sizes in 4 KiB pages.
pub fn decode_chunk_pages(meta: u32, extra: u32) -> Option<Vec<u32>> {
    if meta > 0x7FF || (extra >> 9) & 0x3FF != 0 {
        return None;
    }
    let val = (u64::from(extra) << 11) | u64::from(meta);
    let mut pages = Vec::new();
    let mut ended = false;
    for slot in 0..4 {
        let code = ((val >> (5 * slot)) & 0x1F) as u32;
        if code == 0 {
            ended = true;
            continue;
        }
        if ended {
            return None;
        }
        pages.push(match code {
            1..=8 => code,
            10 => 12,
            11 => 16,
            12 => 24,
            13 => 32,
            14 => 48,
            15 => 64,
            16 => 128,
            21 => 18,
            22 => 80,
            23 => 96,
            24 => 144,
            26 => 272,
            27 => 384,
            28 => 528,
            _ => return None,
        });
    }
    Some(pages)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CubemapLayout {
    pub size: u64,
    /// Start of the full-resolution snapshot inside the allocation.
    pub data_offset: u64,
    pub face_size: u64,
    pub snapshots: u32,
    pub surface: SurfaceLayout,
}

/// A streaming cubemap stores `non_tail + 1` snapshots back to back; the last holds the full
/// resolution, six faces in slice-major `SW_4KB_S` order.
pub fn cubemap_surface_layout(
    width: u32,
    height: u32,
    mips: u32,
    element_bytes: u32,
    block: u32,
    non_tail: u32,
    chunk_pages: Option<&[u32]>,
) -> Result<CubemapLayout, LayoutError> {
    if non_tail > 3 {
        return Err(LayoutError::CubemapSnapshots(non_tail));
    }
    let surface = compact_surface_layout(width, height, mips, element_bytes, block, TileSize::K4)?;
    let face_size = surface.size;
    let full = 6 * face_size;
    let (data_offset, size) = if non_tail == 0 {
        (0, full)
    } else {
        let padded = if full >= 36 * 4096 {
            full.div_ceil(65536) * 65536
        } else {
            full
        };
        match chunk_pages {
            Some(p) => {
                if p.len() != non_tail as usize + 1
                    || u64::from(*p.last().unwrap_or(&0)) * 4096 != padded
                {
                    return Err(LayoutError::CubemapPages(p.to_vec(), non_tail + 1));
                }
                let all: u64 = p.iter().map(|&x| u64::from(x)).sum();
                let before: u64 = p[..p.len() - 1].iter().map(|&x| u64::from(x)).sum();
                (before * 4096, all * 4096)
            }
            None => (0, padded),
        }
    };
    Ok(CubemapLayout {
        size,
        data_offset,
        face_size,
        snapshots: non_tail + 1,
        surface,
    })
}

/// 3D `SW_4KB_S` tile extents `(x, y, z)` in elements by `log2(element_bytes)`.
pub const fn volume_tile_dims(log2: u32) -> (u32, u32, u32) {
    match log2 {
        0 => (16, 16, 16),
        1 => (8, 16, 16),
        2 => (8, 16, 8),
        3 => (8, 8, 8),
        _ => (4, 8, 8),
    }
}

/// 3D `SW_4KB_S` intra-tile bit order: `1..=99` x bit `s-1`, negative y bit `-s-1`,
/// `>= 101` z bit `s-101`, `0` an element byte bit.
pub const VOLUME_PATTERNS_4K: [[i16; 12]; 5] = [
    [1, 2, 101, -1, 102, -2, 3, 103, -3, 4, 104, -4],
    [0, 1, 101, -1, 102, -2, 2, 103, -3, 3, 104, -4],
    [0, 0, 1, -1, 101, -2, 2, 102, -3, 3, 103, -4],
    [0, 0, 0, 1, 101, -1, 2, 102, -2, 3, 103, -3],
    [0, 0, 0, 0, 101, -1, 1, 102, -2, 2, 103, -3],
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeLayout {
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub element_bytes: u32,
    pub tile_w: u32,
    pub tile_h: u32,
    pub tile_d: u32,
    pub padded_w: u32,
    pub padded_h: u32,
    pub padded_d: u32,
    pub z_tile_stride: u64,
}

pub fn volume_surface_layout(
    width: u32,
    height: u32,
    depth: u32,
    mips: u32,
    element_bytes: u32,
) -> Result<VolumeLayout, LayoutError> {
    if width == 0 || height == 0 || depth == 0 {
        return Err(LayoutError::Dimensions(width, height));
    }
    if mips != 1 {
        return Err(LayoutError::VolumeMips(mips));
    }
    if !matches!(element_bytes, 1 | 2 | 4 | 8 | 16) {
        return Err(LayoutError::ElementSize(element_bytes));
    }
    let (bw, bh, bd) = volume_tile_dims(element_bytes.trailing_zeros());
    let pw = width.div_ceil(bw) * bw;
    let ph = height.div_ceil(bh) * bh;
    let pd = depth.div_ceil(bd) * bd;
    let z_tile_stride = u64::from(pw / bw) * u64::from(ph / bh) * 4096;
    Ok(VolumeLayout {
        size: u64::from(pd / bd) * z_tile_stride,
        width,
        height,
        depth,
        element_bytes,
        tile_w: bw,
        tile_h: bh,
        tile_d: bd,
        padded_w: pw,
        padded_h: ph,
        padded_d: pd,
        z_tile_stride,
    })
}

/// Byte address of voxel `(x, y, z)` in a tiled volume.
pub fn volume_address(layout: &VolumeLayout, x: u32, y: u32, z: u32) -> u64 {
    let log = layout.element_bytes.trailing_zeros() as usize;
    let tiles_x = u64::from(layout.padded_w / layout.tile_w);
    let tile = u64::from(z / layout.tile_d) * layout.z_tile_stride
        + (u64::from(y / layout.tile_h) * tiles_x + u64::from(x / layout.tile_w)) * 4096;
    let mut intra = 0u64;
    for (bit, &s) in VOLUME_PATTERNS_4K[log].iter().enumerate() {
        let v = match s {
            101.. => (z >> (s - 101)) & 1,
            1..=99 => (x >> (s - 1)) & 1,
            0 => 0,
            _ => (y >> (-s - 1)) & 1,
        };
        intra |= u64::from(v) << bit;
    }
    tile + intra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_bc1_chain_512x256() {
        // 7 mips, 4 KiB tiles: mip 0 at 24576, mip 1 at 8192, mip 2 at 4096, tail tile at 0
        // holding mips 3..6 at origins (16,0) (8,8) (0,12) (0,8) in 8x8-block units.
        let l = compact_surface_layout(512, 256, 7, 8, 4, TileSize::K4).unwrap();
        assert_eq!(l.first_tail, 3);
        assert_eq!(l.mips[0].offset, 24576);
        assert_eq!(l.mips[1].offset, 8192);
        assert_eq!(l.mips[2].offset, 4096);
        assert_eq!(l.size, 24576 + l.mips[0].region);
        let origins: Vec<_> = l.mips[3..].iter().map(|m| (m.tile_x, m.tile_y)).collect();
        assert_eq!(origins, vec![(16, 0), (8, 8), (0, 12), (0, 8)]);
        assert!(l.mips[3..].iter().all(|m| m.in_tail && m.offset == 0));
    }

    #[test]
    fn single_mip_has_no_tail() {
        let l = compact_surface_layout(256, 256, 1, 4, 1, TileSize::K4).unwrap();
        assert_eq!(l.first_tail, 1);
        assert_eq!(l.mips[0].offset, 0);
        assert_eq!(l.size, 256 * 256 * 4);
    }

    #[test]
    fn bc6h_128x128_one_mip_fills_four_tiles() {
        let l = compact_surface_layout(128, 128, 1, 16, 4, TileSize::K4).unwrap();
        assert_eq!(l.size, 16384);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(matches!(
            compact_surface_layout(0, 4, 1, 4, 1, TileSize::K4),
            Err(LayoutError::Dimensions(..))
        ));
        assert!(matches!(
            compact_surface_layout(16, 16, 6, 4, 1, TileSize::K4),
            Err(LayoutError::MipCount { .. })
        ));
        assert!(matches!(
            compact_surface_layout(16, 16, 1, 12, 1, TileSize::K4),
            Err(LayoutError::ElementSize(12))
        ));
    }

    #[test]
    fn chain_regions_are_disjoint_and_cover_the_allocation() {
        for (w, h, mips, eb, blk, tile) in [
            (1024, 512, 11, 8, 4, TileSize::K4),
            (512, 512, 10, 16, 4, TileSize::K4),
            (1024, 1024, 11, 4, 1, TileSize::K64),
            (100, 60, 7, 2, 1, TileSize::K4),
        ] {
            let l = compact_surface_layout(w, h, mips, eb, blk, tile).unwrap();
            let mut spans: Vec<(u64, u64)> = l
                .mips
                .iter()
                .filter(|m| !m.in_tail)
                .map(|m| (m.offset, m.offset + m.region))
                .collect();
            if l.first_tail < mips {
                spans.push((0, u64::from(tile.bytes())));
            }
            spans.sort();
            assert_eq!(spans.first().unwrap().0, 0);
            for pair in spans.windows(2) {
                assert_eq!(pair[0].1, pair[1].0);
            }
            assert_eq!(spans.last().unwrap().1, l.size);
        }
    }

    #[test]
    fn array_layout_is_mip_major() {
        let a = array_surface_layout(256, 256, 3, 9, 16, 4, TileSize::K64).unwrap();
        let tail_mips = a.surface.mips.iter().filter(|m| m.in_tail).count();
        assert!(tail_mips > 0);
        let tail_end = 3 * 65536;
        let smallest_standalone = a.surface.mips.iter().rev().find(|m| !m.in_tail).unwrap();
        assert_eq!(smallest_standalone.offset, tail_end);
        let m0 = &a.surface.mips[0];
        assert_eq!(a.size, m0.offset + 3 * m0.region);
        let standalone: u64 = a
            .surface
            .mips
            .iter()
            .filter(|m| !m.in_tail)
            .map(|m| m.region)
            .sum();
        assert_eq!(a.slice_size, standalone + 65536);
    }

    #[test]
    fn chunk_pages_decode() {
        // codes 11 (16 pages), 28 (528 pages), then end
        let val: u64 = 11 | (28 << 5);
        let pages = decode_chunk_pages((val & 0x7FF) as u32, (val >> 11) as u32).unwrap();
        assert_eq!(pages, vec![16, 528]);
        assert_eq!(decode_chunk_pages(0, 0), Some(vec![]));
        // code after a terminator is invalid
        let bad: u64 = 3 << 5;
        assert_eq!(decode_chunk_pages((bad & 0x7FF) as u32, 0), None);
        assert_eq!(decode_chunk_pages(9, 0), None);
    }

    #[test]
    fn cubemap_512_bc7_four_snapshots_is_byte_exact() {
        let face = compact_surface_layout(512, 512, 10, 16, 4, TileSize::K4)
            .unwrap()
            .size;
        assert_eq!(6 * face, 2_113_536);
        let c = cubemap_surface_layout(512, 512, 10, 16, 4, 3, Some(&[12, 64, 128, 528])).unwrap();
        assert_eq!(c.data_offset, 835_584);
        assert_eq!(c.size, 2_998_272);
        assert!(cubemap_surface_layout(512, 512, 10, 16, 4, 3, Some(&[12, 64, 128, 500])).is_err());
    }

    #[test]
    fn volume_layout_matches_the_measured_spans() {
        assert_eq!(
            volume_surface_layout(256, 32, 8, 1, 2).unwrap().size,
            262_144
        );
        assert_eq!(
            volume_surface_layout(32, 32, 32, 1, 8).unwrap().size,
            262_144
        );
        assert_eq!(
            volume_surface_layout(32, 64, 32, 1, 8).unwrap().size,
            524_288
        );
    }

    #[test]
    fn volume_addresses_are_unique_and_inside_the_span() {
        for eb in [1u32, 2, 4, 8, 16] {
            let l = volume_surface_layout(20, 18, 9, 1, eb).unwrap();
            let mut seen = std::collections::HashSet::new();
            for z in 0..l.depth {
                for y in 0..l.height {
                    for x in 0..l.width {
                        let a = volume_address(&l, x, y, z);
                        assert!(a + u64::from(eb) <= l.size);
                        assert!(seen.insert(a));
                    }
                }
            }
        }
    }
}
