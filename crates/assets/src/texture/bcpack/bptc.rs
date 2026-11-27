//! BC7 codec constants: the public BPTC partition and anchor tables and the lane-15 signature table.
//!
//! Provenance: the partition masks and anchor indices are the standard D3D11 / Khronos BPTC tables. The signature
//! table (`config`) was recovered clean-room by the upstream Leonida project (`tools/bcpack_tables_bptc.py`):
//! lane 15 carries two symbols per block that name the block's mode, partition, rotation and index selection.
//! The built-in endpoint and index histories of BC7 are not recovered, so none are embedded here.

use std::sync::OnceLock;

/// Two-subset partition masks, one bit per pixel.
pub static PART2: [u16; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80, 0xc800, 0xffec, 0xfe80, 0xe800,
    0xffe8, 0xff00, 0xfff0, 0xf000, 0xf710, 0x008e, 0x7100, 0x08ce, 0x008c, 0x7310, 0x3100, 0x8cce,
    0x088c, 0x3110, 0x6666, 0x366c, 0x17e8, 0x0ff0, 0x718e, 0x399c, 0xaaaa, 0xf0f0, 0x5a5a, 0x33cc,
    0x3c3c, 0x55aa, 0x9696, 0xa55a, 0x73ce, 0x13c8, 0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660,
    0x0272, 0x04e4, 0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c, 0x9336, 0x9cc6, 0x817e, 0xe718,
    0xccf0, 0x0fcc, 0x7744, 0xee22,
];

/// Three-subset partition masks, two bits per pixel.
pub static PART3: [u32; 64] = [
    0xaa685050, 0x6a5a5040, 0x5a5a4200, 0x5450a0a8, 0xa5a50000, 0xa0a05050, 0x5555a0a0, 0x5a5a5050,
    0xaa550000, 0xaa555500, 0xaaaa5500, 0x90909090, 0x94949494, 0xa4a4a4a4, 0xa9a59450, 0x2a0a4250,
    0xa5945040, 0x0a425054, 0xa5a5a500, 0x55a0a0a0, 0xa8a85454, 0x6a6a4040, 0xa4a45000, 0x1a1a0500,
    0x0050a4a4, 0xaaa59090, 0x14696914, 0x69691400, 0xa08585a0, 0xaa821414, 0x50a4a450, 0x6a5a0200,
    0xa9a58000, 0x5090a0a8, 0xa8a09050, 0x24242424, 0x00aa5500, 0x24924924, 0x24499224, 0x50a50a50,
    0x500aa550, 0xaaaa4444, 0x66660000, 0xa5a0a5a0, 0x50a050a0, 0x69286928, 0x44aaaa44, 0x66666600,
    0xaa444444, 0x54a854a8, 0x95809580, 0x96969600, 0xa85454a8, 0x80959580, 0xaa141414, 0x96960000,
    0xaaaa1414, 0xa05050a0, 0xa0a5a5a0, 0x96000000, 0x40804080, 0xa9a8a9a8, 0xaaaaaa44, 0x2a4a5254,
];

/// Anchor pixel of the second subset of each two-subset partition.
pub static ANCHOR2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8,
    2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2,
    2, 15, 15, 15, 15, 15, 2, 2, 15,
];
/// Anchor pixel of the second subset of each three-subset partition.
pub static ANCHOR3A: [u8; 64] = [
    3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3, 3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5,
    15, 15, 8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15, 3, 15, 5, 5, 5, 8, 5, 10, 5,
    10, 8, 13, 15, 12, 3, 3,
];
/// Anchor pixel of the third subset of each three-subset partition.
pub static ANCHOR3B: [u8; 64] = [
    15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8, 15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6,
    10, 15, 15, 10, 8, 15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8, 15, 3, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
];

/// What a lane-15 signature names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub mode: u8,
    pub part: u8,
    pub rot: u8,
    pub isel: u8,
    /// Upstream inferred this entry from anchor positions and order only.
    pub inferred: bool,
}

/// Signature ranks whose mode-2 partition is inferred only.
const INFERRED_RANKS: [usize; 10] = [149, 151, 152, 153, 246, 248, 251, 252, 253, 254];

const MODE2_HEAD: [u8; 22] = [
    63, 59, 58, 57, 54, 53, 52, 47, 43, 42, 40, 38, 37, 35, 34, 32, 23, 21, 18, 17, 16, 11,
];

pub struct Tables {
    config: Vec<Signature>,
    rank_of: [i16; 512],
}

impl Tables {
    fn build() -> Tables {
        let sig = |mode, part, rot, isel| Signature {
            mode,
            part,
            rot,
            isel,
            inferred: false,
        };
        let mut config = Vec::with_capacity(286);
        config.extend((0..64).map(|r| sig(7, 63 - r, 0, 0)));
        config.push(sig(6, 0, 0, 0));
        config.extend((0..4).map(|r| sig(5, 0, 3 - r, 0)));
        config.extend((0..8).map(|r| sig(4, 0, (7 - r) >> 1, (7 - r) & 1)));
        config.extend((0..64).map(|r| sig(3, 63 - r, 0, 0)));
        config.extend(MODE2_HEAD.iter().map(|&p| sig(2, p, 0, 0)));
        config.extend((0..64).map(|r| sig(1, 63 - r, 0, 0)));
        config.extend((8..=15).rev().chain((0..=5).rev()).map(|p| sig(0, p, 0, 0)));
        config.extend(
            (0..64)
                .rev()
                .filter(|p| !MODE2_HEAD.contains(p))
                .map(|p| sig(2, p, 0, 0)),
        );
        config.push(sig(0, 7, 0, 0));
        config.push(sig(0, 6, 0, 0));
        for &r in &INFERRED_RANKS {
            config[r].inferred = true;
        }
        let mut cells: Vec<(u32, u32)> =
            (0..32).flat_map(|a| (0..16).map(move |b| (a, b))).collect();
        cells.sort_by_key(|&(a, b)| ((a + 1) * (b + 1), a));
        let mut rank_of = [-1i16; 512];
        for (rank, &(a, b)) in cells.iter().enumerate().take(config.len()) {
            rank_of[(a * 16 + b) as usize] = rank as i16;
        }
        Tables { config, rank_of }
    }

    /// The signature of a lane-15 symbol pair, `None` outside the table or for the end marker.
    pub fn signature(&self, a: u8, b: u8) -> Option<Signature> {
        if a >= 32 || b >= 16 {
            return None;
        }
        let rank = usize::try_from(self.rank_of[usize::from(a) * 16 + usize::from(b)]).ok()?;
        self.config.get(rank).copied().filter(|s| s.mode < 8)
    }

    /// The lane-15 symbol pair that names a signature.
    #[cfg(test)]
    pub fn cell_of(&self, mode: u8, part: u8, rot: u8, isel: u8) -> (u8, u8) {
        let rank = self
            .config
            .iter()
            .position(|s| (s.mode, s.part, s.rot, s.isel) == (mode, part, rot, isel))
            .expect("signature exists");
        let at = self
            .rank_of
            .iter()
            .position(|&r| r as usize == rank)
            .unwrap();
        ((at / 16) as u8, (at % 16) as u8)
    }
}

pub fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(Tables::build)
}
