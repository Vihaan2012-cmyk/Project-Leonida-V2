//! The console's tiled surface addressing: GFX10 (RDNA2, with the RB+ pipe layout) `SW_4KB_S` and `SW_64KB_D`.
//!
//! A swizzle mode names, for each address bit of a tile from bit 0 up, which bit of x or y it is. The tables
//! below hold that for one element size each (1, 2, 4, 8 and 16 bytes: an entry of 0 is a byte inside the
//! element, a positive `n` is bit `n - 1` of x, a negative `-n` bit `n - 1` of y), as the public addrlib pattern
//! tables for these two modes give them, and as validated against the textures of the game package
//! (`docs/specs/textures.md`). Neither mode here uses the pipe/bank XOR, so a tile is a pure bit permutation.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileMode {
    Sw4kS,
    Sw64kD,
}

const PATTERNS_4K: [[i8; 12]; 5] = [
    [1, 2, 3, 4, -1, -2, -3, -4, -5, 5, -6, 6],
    [0, 1, 2, 3, -1, -2, -3, 4, -4, 5, -5, 6],
    [0, 0, 1, 2, -1, -2, -3, 3, -4, 4, -5, 5],
    [0, 0, 0, 1, -1, -2, 2, 3, -3, 4, -4, 5],
    [0, 0, 0, 0, -1, -2, 1, 2, -3, 3, -4, 4],
];

const PATTERNS_64K: [[i8; 16]; 5] = [
    [1, 2, 3, -2, -1, -3, 4, -4, -5, 5, -6, 6, -7, 7, -8, 8],
    [0, 1, 2, 3, -1, -2, -3, 4, -4, 5, -5, 6, -6, 7, -7, 8],
    [0, 0, 1, 2, -1, -2, 3, -3, -4, 4, -5, 5, -6, 6, -7, 7],
    [0, 0, 0, 1, -1, 2, 3, -2, -3, 4, -4, 5, -5, 6, -6, 7],
    [0, 0, 0, 0, 1, -1, 2, -2, -3, 3, -4, 4, -5, 5, -6, 6],
];

impl TileMode {
    pub fn tile_bytes(self) -> u64 {
        match self {
            TileMode::Sw4kS => 4096,
            TileMode::Sw64kD => 65536,
        }
    }

    /// How many mips at the end of a chain can share one tile.
    pub fn max_tail_mips(self) -> u32 {
        match self {
            TileMode::Sw4kS => 8,
            TileMode::Sw64kD => 12,
        }
    }

    fn pattern(self, element_bytes: u32) -> Option<&'static [i8]> {
        let row = match element_bytes {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            16 => 4,
            _ => return None,
        };
        Some(match self {
            TileMode::Sw4kS => &PATTERNS_4K[row],
            TileMode::Sw64kD => &PATTERNS_64K[row],
        })
    }
}

/// One surface level stored tile after tile, rows of tiles left to right.
#[derive(Debug, Clone, Copy)]
pub struct Tiled2D {
    pattern: &'static [i8],
    mode: TileMode,
    element_bytes: u32,
    width: u32,
    height: u32,
    tile_width: u32,
    tile_height: u32,
    tiles_x: u64,
    size: u64,
}

impl Tiled2D {
    /// `width` and `height` count elements (blocks, for compressed formats).
    pub fn new(mode: TileMode, element_bytes: u32, width: u32, height: u32) -> Option<Tiled2D> {
        if width == 0 || height == 0 {
            return None;
        }
        let pattern = mode.pattern(element_bytes)?;
        let x_bits = pattern.iter().filter(|p| **p > 0).count() as u32;
        let y_bits = pattern.iter().filter(|p| **p < 0).count() as u32;
        let (tile_width, tile_height) = (1u32 << x_bits, 1u32 << y_bits);
        let tiles_x = u64::from(width.div_ceil(tile_width));
        let tiles_y = u64::from(height.div_ceil(tile_height));
        Some(Tiled2D {
            pattern,
            mode,
            element_bytes,
            width,
            height,
            tile_width,
            tile_height,
            tiles_x,
            size: tiles_x * tiles_y * mode.tile_bytes(),
        })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Elements across and down one tile.
    pub fn tile_extent(&self) -> (u32, u32) {
        (self.tile_width, self.tile_height)
    }

    /// The byte offset of the element inside its own tile.
    pub fn intra_offset(&self, x: u32, y: u32) -> u64 {
        let mut offset = 0u64;
        for (bit, source) in self.pattern.iter().enumerate() {
            let value = match *source {
                0 => 0,
                s if s > 0 => (x >> (s - 1)) & 1,
                s => (y >> (-s - 1)) & 1,
            };
            offset |= u64::from(value) << bit;
        }
        offset
    }

    /// The byte offset of the element at (`x`, `y`) in the surface.
    pub fn offset(&self, x: u32, y: u32) -> Option<u64> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let tile = u64::from(y / self.tile_height) * self.tiles_x + u64::from(x / self.tile_width);
        Some(tile * self.mode.tile_bytes() + self.intra_offset(x, y))
    }

    fn copy(&self, from: &[u8], to: &mut [u8], pitch: usize, to_tiled: bool) -> bool {
        let element = self.element_bytes as usize;
        let row = self.width as usize * element;
        if pitch < row {
            return false;
        }
        let linear_len = (self.height as usize - 1) * pitch + row;
        let (linear_ok, tiled_ok) = if to_tiled {
            (from.len() >= linear_len, to.len() as u64 >= self.size)
        } else {
            (to.len() >= linear_len, from.len() as u64 >= self.size)
        };
        if !linear_ok || !tiled_ok {
            return false;
        }
        for y in 0..self.height {
            for x in 0..self.width {
                let tiled = self.offset(x, y).expect("inside") as usize;
                let linear = y as usize * pitch + x as usize * element;
                if to_tiled {
                    to[tiled..tiled + element].copy_from_slice(&from[linear..linear + element]);
                } else {
                    to[linear..linear + element].copy_from_slice(&from[tiled..tiled + element]);
                }
            }
        }
        true
    }

    /// Lays linear rows `pitch` bytes apart out into `tiled`. False, and nothing written, if a buffer is short.
    pub fn tile(&self, linear: &[u8], tiled: &mut [u8], pitch: usize) -> bool {
        self.copy(linear, tiled, pitch, true)
    }

    pub fn detile(&self, tiled: &[u8], linear: &mut [u8], pitch: usize) -> bool {
        self.copy(tiled, linear, pitch, false)
    }
}
