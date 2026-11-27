//! Gfx10 (RDNA2) swizzle equations and the CPU reference every GPU pass is checked against.
//!
//! Inside a tile, each address bit comes from one bit of x, y or z (a permutation, no XOR); the tile index is
//! row-major, then slab-major, above the in-tile bits. The first `log2(element bytes)` address bits are the byte
//! within the element. Measured tables: `SW_4KB_S` and `SW_64KB_D` for 1/2/4/8/16-byte elements, and the 3D
//! `SW_4KB_S` for volumes (docs/textures.md, docs/textures_arrays.md).

use crate::LayoutError;

/// Where one address bit of an element comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bit {
    Byte,
    X(u8),
    Y(u8),
    Z(u8),
}

const fn x(n: u8) -> Bit {
    Bit::X(n)
}
const fn y(n: u8) -> Bit {
    Bit::Y(n)
}
const fn z(n: u8) -> Bit {
    Bit::Z(n)
}
const B: Bit = Bit::Byte;

#[rustfmt::skip]
const S4K: [[Bit; 12]; 5] = [
    [x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4), x(4), y(5), x(5)],
    [B, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4), x(5)],
    [B, B, x(0), x(1), y(0), y(1), y(2), x(2), y(3), x(3), y(4), x(4)],
    [B, B, B, x(0), y(0), y(1), x(1), x(2), y(2), x(3), y(3), x(4)],
    [B, B, B, B, y(0), y(1), x(0), x(1), y(2), x(2), y(3), x(3)],
];

#[rustfmt::skip]
const D64K: [[Bit; 16]; 5] = [
    [x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4), x(4), y(5), x(5), y(6), x(6), y(7), x(7)],
    [B, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4), x(5), y(5), x(6), y(6), x(7)],
    [B, B, x(0), x(1), y(0), y(1), x(2), y(2), y(3), x(3), y(4), x(4), y(5), x(5), y(6), x(6)],
    [B, B, B, x(0), y(0), x(1), x(2), y(1), y(2), x(3), y(3), x(4), y(4), x(5), y(5), x(6)],
    [B, B, B, B, x(0), y(0), x(1), y(1), y(2), x(2), y(3), x(3), y(4), x(4), y(5), x(5)],
];

#[rustfmt::skip]
const S4K_3D: [[Bit; 12]; 5] = [
    [x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2), x(3), z(3), y(3)],
    [B, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2), x(2), z(3), y(3)],
    [B, B, x(0), y(0), z(0), y(1), x(1), z(1), y(2), x(2), z(2), y(3)],
    [B, B, B, x(0), z(0), y(0), x(1), z(1), y(1), x(2), z(2), y(2)],
    [B, B, B, B, z(0), y(0), x(0), z(1), y(1), x(1), z(2), y(2)],
];

/// The swizzle modes with measured equations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swizzle {
    /// `SW_64KB_D`: descriptor tile mode 0 (2D, 2D arrays).
    Sw64kbD,
    /// `SW_4KB_S`: descriptor tile modes 1-3 and 9-12 (2D, cube faces).
    Sw4kbS,
    /// 3D `SW_4KB_S`: volumes.
    Sw4kbS3d,
}

impl Swizzle {
    pub fn tile_bits(self) -> u32 {
        match self {
            Swizzle::Sw64kbD => 16,
            _ => 12,
        }
    }

    pub fn tile_bytes(self) -> u32 {
        1 << self.tile_bits()
    }

    /// The swizzle of a TTPF descriptor: `mode` is the 9-bit field (`b >> 23`), `volume` a 3D texture. Tile
    /// modes 4-8 and 13-15 never occur in the measured corpus.
    pub fn from_descriptor_mode(mode: u32, volume: bool) -> Result<Swizzle, LayoutError> {
        match (mode >> 1) & 0xF {
            0 if !volume => Ok(Swizzle::Sw64kbD),
            1..=3 | 9..=12 => Ok(if volume {
                Swizzle::Sw4kbS3d
            } else {
                Swizzle::Sw4kbS
            }),
            _ => Err(LayoutError::UnmeasuredTileMode(mode)),
        }
    }
}

/// The address equation of one swizzle for one element size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Equation {
    pub swizzle: Swizzle,
    pub elem_log2: u32,
    /// One entry per address bit, lowest first.
    pub bits: Vec<Bit>,
    /// log2 of the tile's extent in elements along x, y and z.
    pub tile_log2: [u32; 3],
}

impl Equation {
    pub fn new(swizzle: Swizzle, elem_bytes: u32) -> Result<Equation, LayoutError> {
        if !matches!(elem_bytes, 1 | 2 | 4 | 8 | 16) {
            return Err(LayoutError::ElementSize(elem_bytes));
        }
        let log = elem_bytes.trailing_zeros() as usize;
        let bits: Vec<Bit> = match swizzle {
            Swizzle::Sw64kbD => D64K[log].to_vec(),
            Swizzle::Sw4kbS => S4K[log].to_vec(),
            Swizzle::Sw4kbS3d => S4K_3D[log].to_vec(),
        };
        let mut tile_log2 = [0u32; 3];
        for bit in &bits {
            match bit {
                Bit::X(_) => tile_log2[0] += 1,
                Bit::Y(_) => tile_log2[1] += 1,
                Bit::Z(_) => tile_log2[2] += 1,
                Bit::Byte => {}
            }
        }
        Ok(Equation {
            swizzle,
            elem_log2: log as u32,
            bits,
            tile_log2,
        })
    }

    /// Tile extent in elements.
    pub fn tile_extent(&self) -> [u32; 3] {
        self.tile_log2.map(|l| 1 << l)
    }

    /// Six bits per address bit (kind in bits 4-5: 1 x, 2 y, 3 z; index in bits 0-3), five bits to a word.
    pub fn pack(&self) -> [u32; 4] {
        let mut words = [0u32; 4];
        for (b, bit) in self.bits.iter().enumerate() {
            let code = match *bit {
                Bit::Byte => 0,
                Bit::X(n) => 16 | n as u32,
                Bit::Y(n) => 32 | n as u32,
                Bit::Z(n) => 48 | n as u32,
            };
            words[b / 5] |= code << (6 * (b % 5));
        }
        words
    }

    /// The `shape` word of the GPU parameters.
    pub fn shape(&self) -> u32 {
        self.elem_log2
            | (self.swizzle.tile_bits() << 4)
            | (self.tile_log2[0] << 9)
            | (self.tile_log2[1] << 13)
            | (self.tile_log2[2] << 17)
    }
}
