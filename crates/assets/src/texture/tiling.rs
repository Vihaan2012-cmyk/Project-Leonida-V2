//! Console (AMD Gfx10 / RDNA2 RbPlus) tiling: `SW_4KB_S` and `SW_64KB_D` micro-tiles.
//!
//! A surface is a grid of tiles in row-major order. Inside a tile the byte offset is a
//! bit-interleave of the element's x and y coordinates; the bit order depends on the element
//! size. [`PATTERNS_4K`] and [`PATTERNS_64K`] hold that bit order, indexed by
//! `log2(element_bytes)`; entry `i` of a pattern describes bit `i` of the byte offset inside the
//! tile:
//!
//! * `s > 0`: the bit is bit `s - 1` of x
//! * `s < 0`: the bit is bit `-s - 1` of y
//! * `s == 0`: a bit inside the element itself (always zero for whole-element addressing)
//!
//! The full address of element `(x, y)` is
//! `(y / tile_h) * tiles_per_row * tile_bytes + (x / tile_w) * tile_bytes + intra(x, y)`.

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileSize {
    K4,
    K64,
}

impl TileSize {
    pub const fn bytes(self) -> u32 {
        match self {
            TileSize::K4 => 4096,
            TileSize::K64 => 65536,
        }
    }

    pub fn from_bytes(bytes: u32) -> Option<TileSize> {
        match bytes {
            4096 => Some(TileSize::K4),
            65536 => Some(TileSize::K64),
            _ => None,
        }
    }

    /// Descriptor mode to tile size: only Gfx10 tile mode 0 is 64 KiB.
    pub const fn from_descriptor_mode(mode: u32) -> TileSize {
        if (mode >> 1) & 0xF == 0 {
            TileSize::K64
        } else {
            TileSize::K4
        }
    }

    const fn address_bits(self) -> u32 {
        match self {
            TileSize::K4 => 12,
            TileSize::K64 => 16,
        }
    }
}

pub const PATTERNS_4K: [[i8; 12]; 5] = [
    [1, 2, 3, 4, -1, -2, -3, -4, -5, 5, -6, 6],
    [0, 1, 2, 3, -1, -2, -3, 4, -4, 5, -5, 6],
    [0, 0, 1, 2, -1, -2, -3, 3, -4, 4, -5, 5],
    [0, 0, 0, 1, -1, -2, 2, 3, -3, 4, -4, 5],
    [0, 0, 0, 0, -1, -2, 1, 2, -3, 3, -4, 4],
];

/// RDNA2/RbPlus `SW_64KB_D`. For 4-byte elements bits 6 and 7 are swapped relative to Navi 1x.
pub const PATTERNS_64K: [[i8; 16]; 5] = [
    [1, 2, 3, -2, -1, -3, 4, -4, -5, 5, -6, 6, -7, 7, -8, 8],
    [0, 1, 2, 3, -1, -2, -3, 4, -4, 5, -5, 6, -6, 7, -7, 8],
    [0, 0, 1, 2, -1, -2, 3, -3, -4, 4, -5, 5, -6, 6, -7, 7],
    [0, 0, 0, 1, -1, 2, 3, -2, -3, 4, -4, 5, -5, 6, -6, 7],
    [0, 0, 0, 0, 1, -1, 2, -2, -3, 3, -4, 4, -5, 5, -6, 6],
];

/// Bit order of one tile for an element size of `1 << log2_element` bytes (0..=4).
pub fn pattern(tile: TileSize, log2_element: u32) -> &'static [i8] {
    match tile {
        TileSize::K4 => &PATTERNS_4K[log2_element as usize],
        TileSize::K64 => &PATTERNS_64K[log2_element as usize],
    }
}

/// Width and height of one tile in elements.
pub const fn tile_dims(tile: TileSize, log2_element: u32) -> (u32, u32) {
    let bits = tile.address_bits() - log2_element;
    (1 << (bits - bits / 2), 1 << (bits / 2))
}

/// Byte offset of element `(x, y)` inside its tile. Coordinates beyond the tile are ignored.
pub fn intra_tile_offset(tile: TileSize, log2_element: u32, x: u32, y: u32) -> u32 {
    let mut off = 0;
    for (bit, &s) in pattern(tile, log2_element).iter().enumerate() {
        let v = match s {
            1.. => (x >> (s - 1)) & 1,
            0 => 0,
            _ => (y >> (-s - 1)) & 1,
        };
        off |= v << bit;
    }
    off
}

/// Byte address of element `(x, y)` in a surface that is `tiles_per_row` tiles wide.
pub fn element_address(
    tile: TileSize,
    log2_element: u32,
    tiles_per_row: u32,
    x: u32,
    y: u32,
) -> u64 {
    let (tw, th) = tile_dims(tile, log2_element);
    let tile_index = u64::from(y / th) * u64::from(tiles_per_row) + u64::from(x / tw);
    tile_index * u64::from(tile.bytes()) + u64::from(intra_tile_offset(tile, log2_element, x, y))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TileError {
    #[error("tiled data is {have} bytes, the region needs {need}")]
    TooShort { have: usize, need: u64 },
}

/// A rectangle of elements inside a tiled surface.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub tile: TileSize,
    pub log2_element: u32,
    pub tiles_per_row: u32,
    /// Element offset added to the in-tile coordinates (mip-tail mips share one tile).
    pub origin_x: u32,
    pub origin_y: u32,
    pub width: u32,
    pub height: u32,
}

/// Copy `region` out of `tiled` into row-major order.
pub fn detile(region: &Region, tiled: &[u8]) -> Result<Vec<u8>, TileError> {
    let eb = 1usize << region.log2_element;
    let (tw, th) = tile_dims(region.tile, region.log2_element);
    let tile_bytes = u64::from(region.tile.bytes());
    let x_offs: Vec<u64> = (0..region.width)
        .map(|x| {
            u64::from(x / tw) * tile_bytes
                + u64::from(intra_tile_offset(
                    region.tile,
                    region.log2_element,
                    x + region.origin_x,
                    0,
                ))
        })
        .collect();
    let y_offs: Vec<u64> = (0..region.height)
        .map(|y| {
            u64::from(y / th) * u64::from(region.tiles_per_row) * tile_bytes
                + u64::from(intra_tile_offset(
                    region.tile,
                    region.log2_element,
                    0,
                    y + region.origin_y,
                ))
        })
        .collect();
    let need = match (x_offs.iter().max(), y_offs.iter().max()) {
        (Some(x), Some(y)) => x + y + eb as u64,
        _ => 0,
    };
    if (tiled.len() as u64) < need {
        return Err(TileError::TooShort {
            have: tiled.len(),
            need,
        });
    }
    let mut out = Vec::with_capacity(region.width as usize * region.height as usize * eb);
    for &yo in &y_offs {
        for &xo in &x_offs {
            let at = (yo + xo) as usize;
            out.extend_from_slice(&tiled[at..at + eb]);
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn tile_region(region: &Region, linear: &[u8], tiled: &mut [u8]) {
    let eb = 1usize << region.log2_element;
    let (tw, th) = tile_dims(region.tile, region.log2_element);
    for y in 0..region.height {
        for x in 0..region.width {
            let off = u64::from(y / th)
                * u64::from(region.tiles_per_row)
                * u64::from(region.tile.bytes())
                + u64::from(x / tw) * u64::from(region.tile.bytes())
                + u64::from(intra_tile_offset(
                    region.tile,
                    region.log2_element,
                    x + region.origin_x,
                    y + region.origin_y,
                ));
            let src = (y as usize * region.width as usize + x as usize) * eb;
            tiled[off as usize..off as usize + eb].copy_from_slice(&linear[src..src + eb]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> u8 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 33) as u8
    }

    #[test]
    fn tile_dims_match_the_documented_shapes() {
        assert_eq!(tile_dims(TileSize::K4, 0), (64, 64));
        assert_eq!(tile_dims(TileSize::K4, 1), (64, 32));
        assert_eq!(tile_dims(TileSize::K4, 2), (32, 32));
        assert_eq!(tile_dims(TileSize::K4, 3), (32, 16));
        assert_eq!(tile_dims(TileSize::K4, 4), (16, 16));
        assert_eq!(tile_dims(TileSize::K64, 0), (256, 256));
        assert_eq!(tile_dims(TileSize::K64, 2), (128, 128));
        assert_eq!(tile_dims(TileSize::K64, 4), (64, 64));
    }

    #[test]
    fn patterns_use_every_coordinate_bit_of_the_tile_exactly_once() {
        for tile in [TileSize::K4, TileSize::K64] {
            for log in 0..5 {
                let p = pattern(tile, log);
                let (tw, th) = tile_dims(tile, log);
                let mut xs: Vec<i8> = p.iter().copied().filter(|s| *s > 0).collect();
                let mut ys: Vec<i8> = p.iter().filter(|s| **s < 0).map(|s| -s).collect();
                xs.sort();
                ys.sort();
                assert_eq!(xs, (1..=tw.trailing_zeros() as i8).collect::<Vec<_>>());
                assert_eq!(ys, (1..=th.trailing_zeros() as i8).collect::<Vec<_>>());
                assert_eq!(p.iter().filter(|s| **s == 0).count() as u32, log);
            }
        }
    }

    #[test]
    fn intra_tile_offsets_are_a_bijection() {
        for tile in [TileSize::K4, TileSize::K64] {
            for log in 0..5 {
                let (tw, th) = tile_dims(tile, log);
                let mut seen = vec![false; (tile.bytes() >> log) as usize];
                for y in 0..th {
                    for x in 0..tw {
                        let off = intra_tile_offset(tile, log, x, y);
                        assert_eq!(off & ((1 << log) - 1), 0);
                        let slot = (off >> log) as usize;
                        assert!(!seen[slot]);
                        seen[slot] = true;
                    }
                }
            }
        }
    }

    #[test]
    fn one_byte_4k_tile_interleaves_low_bits() {
        assert_eq!(intra_tile_offset(TileSize::K4, 0, 1, 0), 1);
        assert_eq!(intra_tile_offset(TileSize::K4, 0, 0, 1), 1 << 4);
        assert_eq!(intra_tile_offset(TileSize::K4, 0, 16, 0), 1 << 9);
        assert_eq!(element_address(TileSize::K4, 0, 3, 64, 64), 4 * 4096);
    }

    #[test]
    fn rbplus_64k_swaps_bits_six_and_seven_for_four_byte_elements() {
        assert_eq!(intra_tile_offset(TileSize::K64, 2, 4, 0), 1 << 6);
        assert_eq!(intra_tile_offset(TileSize::K64, 2, 0, 4), 1 << 7);
    }

    #[test]
    fn detile_inverts_tiling_for_every_element_size_and_tile() {
        let mut seed = 7;
        for tile in [TileSize::K4, TileSize::K64] {
            for log in 0..5u32 {
                let eb = 1usize << log;
                let (tw, th) = tile_dims(tile, log);
                let (w, h) = (tw + tw / 2 + 1, th * 2 + 3);
                let tiles_per_row = w.div_ceil(tw);
                let tiles_y = h.div_ceil(th);
                let region = Region {
                    tile,
                    log2_element: log,
                    tiles_per_row,
                    origin_x: 0,
                    origin_y: 0,
                    width: w,
                    height: h,
                };
                let linear: Vec<u8> = (0..w as usize * h as usize * eb)
                    .map(|_| rng(&mut seed))
                    .collect();
                let mut tiled =
                    vec![0xEE; (tiles_per_row * tiles_y) as usize * tile.bytes() as usize];
                tile_region(&region, &linear, &mut tiled);
                assert_eq!(
                    detile(&region, &tiled).unwrap(),
                    linear,
                    "{tile:?} log {log}"
                );
            }
        }
    }

    #[test]
    fn detile_applies_the_tail_origin() {
        let region = Region {
            tile: TileSize::K4,
            log2_element: 3,
            tiles_per_row: 1,
            origin_x: 8,
            origin_y: 4,
            width: 8,
            height: 4,
        };
        let linear: Vec<u8> = (0..8 * 4 * 8).map(|i| (i * 7 + 3) as u8).collect();
        let mut tiled = vec![0; 4096];
        tile_region(&region, &linear, &mut tiled);
        assert_eq!(detile(&region, &tiled).unwrap(), linear);
        let at = intra_tile_offset(TileSize::K4, 3, 8, 4) as usize;
        assert_eq!(&tiled[at..at + 8], &linear[..8]);
    }

    #[test]
    fn short_input_is_an_error() {
        let region = Region {
            tile: TileSize::K4,
            log2_element: 2,
            tiles_per_row: 1,
            origin_x: 0,
            origin_y: 0,
            width: 32,
            height: 32,
        };
        assert!(matches!(
            detile(&region, &[0; 100]),
            Err(TileError::TooShort { .. })
        ));
    }
}
