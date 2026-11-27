//! Block decoders for BC1-BC7 (D3D11 / Khronos Data Format Specification layouts).

use std::sync::OnceLock;

const W2: [u32; 4] = [0, 21, 43, 64];
const W3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const W4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn weights(bits: u32) -> &'static [u32] {
    match bits {
        2 => &W2,
        3 => &W3,
        _ => &W4,
    }
}

/// Two-subset partitions: bit `p` of the mask is the subset of pixel `p`.
pub const PART2: [u16; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80, 0xc800, 0xffec, 0xfe80, 0xe800,
    0xffe8, 0xff00, 0xfff0, 0xf000, 0xf710, 0x008e, 0x7100, 0x08ce, 0x008c, 0x7310, 0x3100, 0x8cce,
    0x088c, 0x3110, 0x6666, 0x366c, 0x17e8, 0x0ff0, 0x718e, 0x399c, 0xaaaa, 0xf0f0, 0x5a5a, 0x33cc,
    0x3c3c, 0x55aa, 0x9696, 0xa55a, 0x73ce, 0x13c8, 0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660,
    0x0272, 0x04e4, 0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c, 0x9336, 0x9cc6, 0x817e, 0xe718,
    0xccf0, 0x0fcc, 0x7744, 0xee22,
];

/// Three-subset partitions: bits `2p..2p+2` of the mask are the subset of pixel `p`.
pub const PART3: [u32; 64] = [
    0xaa685050, 0x6a5a5040, 0x5a5a4200, 0x5450a0a8, 0xa5a50000, 0xa0a05050, 0x5555a0a0, 0x5a5a5050,
    0xaa550000, 0xaa555500, 0xaaaa5500, 0x90909090, 0x94949494, 0xa4a4a4a4, 0xa9a59450, 0x2a0a4250,
    0xa5945040, 0x0a425054, 0xa5a5a500, 0x55a0a0a0, 0xa8a85454, 0x6a6a4040, 0xa4a45000, 0x1a1a0500,
    0x0050a4a4, 0xaaa59090, 0x14696914, 0x69691400, 0xa08585a0, 0xaa821414, 0x50a4a450, 0x6a5a0200,
    0xa9a58000, 0x5090a0a8, 0xa8a09050, 0x24242424, 0x00aa5500, 0x24924924, 0x24499224, 0x50a50a50,
    0x500aa550, 0xaaaa4444, 0x66660000, 0xa5a0a5a0, 0x50a050a0, 0x69286928, 0x44aaaa44, 0x66666600,
    0xaa444444, 0x54a854a8, 0x95809580, 0x96969600, 0xa85454a8, 0x80959580, 0xaa141414, 0x96960000,
    0xaaaa1414, 0xa05050a0, 0xa0a5a5a0, 0x96000000, 0x40804080, 0xa9a8a9a8, 0xaaaaaa44, 0x2a4a5254,
];

/// Anchor pixel of subset 1 in two-subset partitions (subset 0 is always pixel 0).
pub const ANCHOR2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8,
    2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2,
    2, 15, 15, 15, 15, 15, 2, 2, 15,
];

/// Anchor pixels of subsets 1 and 2 in three-subset partitions.
pub const ANCHOR3A: [u8; 64] = [
    3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3, 3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5,
    15, 15, 8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15, 3, 15, 5, 5, 5, 8, 5, 10, 5,
    10, 8, 13, 15, 12, 3, 3,
];
pub const ANCHOR3B: [u8; 64] = [
    15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8, 15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6,
    10, 15, 15, 10, 8, 15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8, 15, 3, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
];

fn expand565(c: u16) -> [u8; 3] {
    let (r, g, b) = (
        u32::from(c >> 11) & 31,
        u32::from(c >> 5) & 63,
        u32::from(c) & 31,
    );
    [
        ((r << 3) | (r >> 2)) as u8,
        ((g << 2) | (g >> 4)) as u8,
        ((b << 3) | (b >> 2)) as u8,
    ]
}

fn bc1_palette(block: &[u8], four_color_only: bool) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (expand565(c0), expand565(c1));
    let mix = |wa: u32, wb: u32, div: u32| -> [u8; 4] {
        let f = |i: usize| ((wa * u32::from(a[i]) + wb * u32::from(b[i])) / div) as u8;
        [f(0), f(1), f(2), 255]
    };
    let mut p = [
        [a[0], a[1], a[2], 255],
        [b[0], b[1], b[2], 255],
        [0; 4],
        [0; 4],
    ];
    if c0 > c1 || four_color_only {
        p[2] = mix(2, 1, 3);
        p[3] = mix(1, 2, 3);
    } else {
        p[2] = mix(1, 1, 2);
        p[3] = [0, 0, 0, 0];
    }
    p
}

/// One BC1 block to 16 RGBA pixels (row-major). Without `four_color_only`, a block with
/// `color0 <= color1` has a transparent fourth entry.
pub fn bc1_block(block: &[u8], four_color_only: bool) -> [[u8; 4]; 16] {
    let pal = bc1_palette(block, four_color_only);
    let idx = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|p| pal[((idx >> (2 * p)) & 3) as usize])
}

pub fn bc2_block(block: &[u8]) -> [[u8; 4]; 16] {
    let mut px = bc1_block(&block[8..16], true);
    for (p, v) in px.iter_mut().enumerate() {
        let n = (block[p / 2] >> (4 * (p % 2))) & 15;
        v[3] = n * 17;
    }
    px
}

fn bc4_unorm_values(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (u32::from(block[0]), u32::from(block[1]));
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7u32 {
            pal[i as usize + 1] = ((7 - i) * a0 + i * a1 + 3) / 7;
        }
    } else {
        for i in 1..5u32 {
            pal[i as usize + 1] = ((5 - i) * a0 + i * a1 + 2) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let bits = u64::from_le_bytes([
        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
    ]);
    std::array::from_fn(|p| pal[((bits >> (3 * p)) & 7) as usize] as u8)
}

/// BC4 unsigned: 16 values, row-major.
pub fn bc4_block(block: &[u8]) -> [u8; 16] {
    bc4_unorm_values(block)
}

/// BC4 signed: 16 values in -127..=127, row-major.
pub fn bc4_block_snorm(block: &[u8]) -> [i8; 16] {
    let clamp = |v: i8| i32::from(v).max(-127);
    let (a0, a1) = (clamp(block[0] as i8), clamp(block[1] as i8));
    let mut pal = [0i32; 8];
    pal[0] = a0;
    pal[1] = a1;
    let round_div = |n: i32, d: i32| -> i32 { (n as f32 / d as f32).round() as i32 };
    if a0 > a1 {
        for i in 1..7 {
            pal[i as usize + 1] = round_div((7 - i) * a0 + i * a1, 7);
        }
    } else {
        for i in 1..5 {
            pal[i as usize + 1] = round_div((5 - i) * a0 + i * a1, 5);
        }
        pal[6] = -127;
        pal[7] = 127;
    }
    let bits = u64::from_le_bytes([
        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
    ]);
    std::array::from_fn(|p| pal[((bits >> (3 * p)) & 7) as usize] as i8)
}

pub fn bc3_block(block: &[u8]) -> [[u8; 4]; 16] {
    let alpha = bc4_unorm_values(&block[0..8]);
    let mut px = bc1_block(&block[8..16], true);
    for (v, a) in px.iter_mut().zip(alpha) {
        v[3] = a;
    }
    px
}

/// BC5: red and green planes, each a BC4 block.
pub fn bc5_block(block: &[u8]) -> [[u8; 2]; 16] {
    let r = bc4_unorm_values(&block[0..8]);
    let g = bc4_unorm_values(&block[8..16]);
    std::array::from_fn(|p| [r[p], g[p]])
}

pub fn bc5_block_snorm(block: &[u8]) -> [[i8; 2]; 16] {
    let r = bc4_block_snorm(&block[0..8]);
    let g = bc4_block_snorm(&block[8..16]);
    std::array::from_fn(|p| [r[p], g[p]])
}

// ---------------------------------------------------------------- BC7

/// `(subsets, partition bits, rotation bits, index-select bits, colour bits, alpha bits,
/// per-endpoint p-bits, shared p-bits, index bits, second index bits)`.
type Bc7Mode = (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32);

const BC7_MODES: [Bc7Mode; 8] = [
    (3, 4, 0, 0, 4, 0, 1, 0, 3, 0),
    (2, 6, 0, 0, 6, 0, 0, 1, 3, 0),
    (3, 6, 0, 0, 5, 0, 0, 0, 2, 0),
    (2, 6, 0, 0, 7, 0, 1, 0, 2, 0),
    (1, 0, 2, 1, 5, 6, 0, 0, 2, 3),
    (1, 0, 2, 0, 7, 8, 0, 0, 2, 2),
    (1, 0, 0, 0, 7, 7, 1, 0, 4, 0),
    (2, 6, 0, 0, 5, 5, 1, 0, 2, 0),
];

fn bits(blk: u128, pos: u32, n: u32) -> u32 {
    if n == 0 {
        0
    } else {
        ((blk >> pos) & ((1u128 << n) - 1)) as u32
    }
}

fn subset_of(subsets: u32, part: usize, p: usize) -> usize {
    match subsets {
        1 => 0,
        2 => ((PART2[part] >> p) & 1) as usize,
        _ => ((PART3[part] >> (2 * p)) & 3) as usize,
    }
}

fn anchor_of(subsets: u32, part: usize, subset: usize) -> usize {
    match (subsets, subset) {
        (_, 0) => 0,
        (2, _) => ANCHOR2[part] as usize,
        (_, 1) => ANCHOR3A[part] as usize,
        _ => ANCHOR3B[part] as usize,
    }
}

/// One BC7 block to 16 RGBA pixels. A block with no mode bit set is invalid and decodes to
/// transparent black, as the specification requires.
pub fn bc7_block(block: &[u8]) -> [[u8; 4]; 16] {
    if block[0] == 0 {
        return [[0; 4]; 16];
    }
    let blk = u128::from_le_bytes(block[..16].try_into().expect("16-byte block"));
    let mode = block[0].trailing_zeros() as usize;
    let (ns, pb, rb, isb, cb, ab, epb, spb, ib, ib2) = BC7_MODES[mode];
    let mut pos = mode as u32 + 1;
    let part = bits(blk, pos, pb) as usize;
    pos += pb;
    let rot = bits(blk, pos, rb);
    pos += rb;
    let isel = bits(blk, pos, isb) == 1;
    pos += isb;

    let n_ep = 2 * ns as usize;
    let mut raw = [[0u32; 6]; 4];
    for ch in raw.iter_mut().take(3) {
        for v in ch.iter_mut().take(n_ep) {
            *v = bits(blk, pos, cb);
            pos += cb;
        }
    }
    if ab > 0 {
        for v in raw[3].iter_mut().take(n_ep) {
            *v = bits(blk, pos, ab);
            pos += ab;
        }
    }
    let mut pbit = [None::<u32>; 6];
    let (mut c_bits, mut a_bits) = (cb, ab);
    if epb > 0 {
        for (k, p) in pbit.iter_mut().enumerate().take(n_ep) {
            *p = Some(bits(blk, pos + k as u32, 1));
        }
        pos += n_ep as u32;
        c_bits += 1;
        if ab > 0 {
            a_bits += 1;
        }
    } else if spb > 0 {
        let shared = [bits(blk, pos, 1), bits(blk, pos + 1, 1)];
        pos += ns;
        for (k, p) in pbit.iter_mut().enumerate().take(n_ep) {
            *p = Some(shared[k / 2]);
        }
        c_bits += 1;
    }
    let expand = |v: u32, nbits: u32| -> u32 {
        let v = v << (8 - nbits);
        v | (v >> nbits)
    };
    let mut ep = [[255u32; 6]; 4];
    for ch in 0..3 {
        for k in 0..n_ep {
            let v = match pbit[k] {
                Some(p) => (raw[ch][k] << 1) | p,
                None => raw[ch][k],
            };
            ep[ch][k] = expand(v, c_bits);
        }
    }
    if ab > 0 {
        for k in 0..n_ep {
            let v = match (epb > 0, pbit[k]) {
                (true, Some(p)) => (raw[3][k] << 1) | p,
                _ => raw[3][k],
            };
            ep[3][k] = expand(v, a_bits);
        }
    }

    let mut idx1 = [0usize; 16];
    for (p, v) in idx1.iter_mut().enumerate() {
        let sub = subset_of(ns, part, p);
        let n = if p == anchor_of(ns, part, sub) {
            ib - 1
        } else {
            ib
        };
        *v = bits(blk, pos, n) as usize;
        pos += n;
    }
    let mut idx2 = [0usize; 16];
    if ib2 > 0 {
        for (p, v) in idx2.iter_mut().enumerate() {
            let n = if p == 0 { ib2 - 1 } else { ib2 };
            *v = bits(blk, pos, n) as usize;
            pos += n;
        }
    }
    let (c_idx, c_w, a_idx, a_w) = if ib2 > 0 {
        if isel {
            (&idx2, weights(ib2), &idx1, weights(ib))
        } else {
            (&idx1, weights(ib), &idx2, weights(ib2))
        }
    } else {
        (&idx1, weights(ib), &idx1, weights(ib))
    };

    std::array::from_fn(|p| {
        let s = subset_of(ns, part, p) * 2;
        let lerp = |ch: usize, w: u32| (((64 - w) * ep[ch][s] + w * ep[ch][s + 1] + 32) >> 6) as u8;
        let wc = c_w[c_idx[p]];
        let mut px = [lerp(0, wc), lerp(1, wc), lerp(2, wc), 0];
        let wa = a_w[a_idx[p]];
        let a = lerp(3, wa);
        px[3] = a;
        match rot {
            1 => px.swap(0, 3),
            2 => px.swap(1, 3),
            3 => px.swap(2, 3),
            _ => {}
        }
        px
    })
}

// ---------------------------------------------------------------- BC6H

struct Bc6hMode {
    code: u8,
    transformed: bool,
    base_bits: u32,
    delta_bits: [u32; 3],
    regions: u32,
    layout: &'static str,
}

/// Header bit layouts: `rw9-0` is bits 0..=9 of the red `w` endpoint in stream order, `rw11~10`
/// is bit 11 then bit 10, `gy4` is the single bit 4, `d` is the 5-bit partition. Endpoint
/// letters are the four endpoint slots `w x y z`.
const BC6H_MODES: [Bc6hMode; 14] = [
    Bc6hMode {
        code: 0x00,
        transformed: true,
        base_bits: 10,
        delta_bits: [5, 5, 5],
        regions: 2,
        layout: "gy4 by4 bz4 rw9-0 gw9-0 bw9-0 rx4-0 gz4 gy3-0 gx4-0 bz0 gz3-0 bx4-0 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Bc6hMode {
        code: 0x01,
        transformed: true,
        base_bits: 7,
        delta_bits: [6, 6, 6],
        regions: 2,
        layout: "gy5 gz4 gz5 rw6-0 bz0 bz1 by4 gw6-0 by5 bz2 gy4 bw6-0 bz3 bz5 bz4 rx5-0 gy3-0 gx5-0 gz3-0 bx5-0 by3-0 ry5-0 rz5-0 d",
    },
    Bc6hMode {
        code: 0x02,
        transformed: true,
        base_bits: 11,
        delta_bits: [5, 4, 4],
        regions: 2,
        layout: "rw9-0 gw9-0 bw9-0 rx4-0 rw10 gy3-0 gx3-0 gw10 bz0 gz3-0 bx3-0 bw10 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Bc6hMode {
        code: 0x06,
        transformed: true,
        base_bits: 11,
        delta_bits: [4, 5, 4],
        regions: 2,
        layout: "rw9-0 gw9-0 bw9-0 rx3-0 rw10 gz4 gy3-0 gx4-0 gw10 gz3-0 bx3-0 bw10 bz1 by3-0 ry3-0 bz0 bz2 rz3-0 gy4 bz3 d",
    },
    Bc6hMode {
        code: 0x0A,
        transformed: true,
        base_bits: 11,
        delta_bits: [4, 4, 5],
        regions: 2,
        layout: "rw9-0 gw9-0 bw9-0 rx3-0 rw10 by4 gy3-0 gx3-0 gw10 bz0 gz3-0 bx4-0 bw10 by3-0 ry3-0 bz1 bz2 rz3-0 bz4 bz3 d",
    },
    Bc6hMode {
        code: 0x0E,
        transformed: true,
        base_bits: 9,
        delta_bits: [5, 5, 5],
        regions: 2,
        layout: "rw8-0 by4 gw8-0 gy4 bw8-0 bz4 rx4-0 gz4 gy3-0 gx4-0 bz0 gz3-0 bx4-0 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Bc6hMode {
        code: 0x12,
        transformed: true,
        base_bits: 8,
        delta_bits: [6, 5, 5],
        regions: 2,
        layout: "rw7-0 gz4 by4 gw7-0 bz2 gy4 bw7-0 bz3 bz4 rx5-0 gy3-0 gx4-0 bz0 gz3-0 bx4-0 bz1 by3-0 ry5-0 rz5-0 d",
    },
    Bc6hMode {
        code: 0x16,
        transformed: true,
        base_bits: 8,
        delta_bits: [5, 6, 5],
        regions: 2,
        layout: "rw7-0 bz0 by4 gw7-0 gy5 gy4 bw7-0 gz5 bz4 rx4-0 gz4 gy3-0 gx5-0 gz3-0 bx4-0 bz1 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Bc6hMode {
        code: 0x1A,
        transformed: true,
        base_bits: 8,
        delta_bits: [5, 5, 6],
        regions: 2,
        layout: "rw7-0 bz1 by4 gw7-0 by5 gy4 bw7-0 bz5 bz4 rx4-0 gz4 gy3-0 gx4-0 bz0 gz3-0 bx5-0 by3-0 ry4-0 bz2 rz4-0 bz3 d",
    },
    Bc6hMode {
        code: 0x1E,
        transformed: false,
        base_bits: 6,
        delta_bits: [6, 6, 6],
        regions: 2,
        layout: "rw5-0 gz4 bz0 bz1 by4 gw5-0 gy5 by5 bz2 gy4 bw5-0 gz5 bz3 bz5 bz4 rx5-0 gy3-0 gx5-0 gz3-0 bx5-0 by3-0 ry5-0 rz5-0 d",
    },
    Bc6hMode {
        code: 0x03,
        transformed: false,
        base_bits: 10,
        delta_bits: [10, 10, 10],
        regions: 1,
        layout: "rw9-0 gw9-0 bw9-0 rx9-0 gx9-0 bx9-0",
    },
    Bc6hMode {
        code: 0x07,
        transformed: true,
        base_bits: 11,
        delta_bits: [9, 9, 9],
        regions: 1,
        layout: "rw9-0 gw9-0 bw9-0 rx8-0 rw10 gx8-0 gw10 bx8-0 bw10",
    },
    Bc6hMode {
        code: 0x0B,
        transformed: true,
        base_bits: 12,
        delta_bits: [8, 8, 8],
        regions: 1,
        layout: "rw9-0 gw9-0 bw9-0 rx7-0 rw11~10 gx7-0 gw11~10 bx7-0 bw11~10",
    },
    Bc6hMode {
        code: 0x0F,
        transformed: true,
        base_bits: 16,
        delta_bits: [4, 4, 4],
        regions: 1,
        layout: "rw9-0 gw9-0 bw9-0 rx3-0 rw15~10 gx3-0 gw15~10 bx3-0 bw15~10",
    },
];

/// `(channel, slot, field bit, stream position)`; channel 3 is the partition field.
type PlanBit = (u8, u8, u8, u8);

fn bc6h_plans() -> &'static [Vec<PlanBit>; 14] {
    static PLANS: OnceLock<[Vec<PlanBit>; 14]> = OnceLock::new();
    PLANS.get_or_init(|| {
        std::array::from_fn(|m| {
            let mode = &BC6H_MODES[m];
            let mut pos: u32 = if mode.code < 2 { 2 } else { 5 };
            let mut plan = Vec::new();
            for tok in mode.layout.split(' ') {
                if tok == "d" {
                    for b in 0..5 {
                        plan.push((3, 0, b, pos as u8));
                        pos += 1;
                    }
                    continue;
                }
                let name = tok.as_bytes();
                let chan = match name[0] {
                    b'r' => 0,
                    b'g' => 1,
                    _ => 2,
                };
                let slot = match name[1] {
                    b'w' => 0,
                    b'x' => 1,
                    b'y' => 2,
                    _ => 3,
                };
                let body = &tok[2..];
                let bits: Vec<u8> = if let Some((hi, lo)) = body.split_once('~') {
                    let (hi, lo): (u8, u8) = (hi.parse().unwrap(), lo.parse().unwrap());
                    (lo..=hi).rev().collect()
                } else if let Some((hi, lo)) = body.split_once('-') {
                    let (hi, lo): (u8, u8) = (hi.parse().unwrap(), lo.parse().unwrap());
                    (lo..=hi).collect()
                } else {
                    vec![body.parse().unwrap()]
                };
                for b in bits {
                    plan.push((chan, slot, b, pos as u8));
                    pos += 1;
                }
            }
            plan
        })
    })
}

fn sign_extend(v: i32, nbits: u32) -> i32 {
    let s = 1 << (nbits - 1);
    (v ^ s) - s
}

fn bc6h_unquantize(comp: i32, nbits: u32, signed: bool) -> i32 {
    if !signed {
        if nbits >= 15 {
            return comp;
        }
        if comp == 0 {
            return 0;
        }
        if comp == (1 << nbits) - 1 {
            return 0xFFFF;
        }
        return ((comp << 16) + 0x8000) >> nbits;
    }
    if nbits >= 16 {
        return comp.max(-32767);
    }
    let neg = comp < 0;
    let c = comp.abs();
    let unq = if c == 0 {
        0
    } else if c >= (1 << (nbits - 1)) - 1 {
        0x7FFF
    } else {
        ((c << 15) + 0x4000) >> (nbits - 1)
    };
    if neg { -unq } else { unq }
}

/// Half-float bit pattern (finite, non-negative magnitude) to f32.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 31);
    let man = f32::from(h & 0x3FF);
    if exp == 0 {
        sign * man * 2f32.powi(-24)
    } else if exp == 31 {
        if man == 0.0 {
            sign * f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        sign * (1.0 + man / 1024.0) * 2f32.powi(exp - 15)
    }
}

fn bc6h_finish(val: i32, signed: bool) -> f32 {
    if !signed {
        let h = ((val.max(0) * 31) >> 6).min(0x7BFF);
        return half_to_f32(h as u16);
    }
    let v = val.abs().min(0x7FFF);
    let h = ((v * 31) >> 5).min(0x7BFF) as u16;
    half_to_f32(if val < 0 { h | 0x8000 } else { h })
}

/// One BC6H block to 16 linear RGB float pixels. Reserved modes decode to black.
pub fn bc6h_block(block: &[u8], signed: bool) -> [[f32; 3]; 16] {
    let b0 = block[0];
    let code = if b0 & 3 < 2 { b0 & 3 } else { b0 & 31 };
    let Some(mi) = BC6H_MODES.iter().position(|m| m.code == code) else {
        return [[0.0; 3]; 16];
    };
    let mode = &BC6H_MODES[mi];
    let plan = &bc6h_plans()[mi];
    let blk = u128::from_le_bytes(block[..16].try_into().expect("16-byte block"));
    let mut f = [[0i32; 4]; 3];
    let mut part = 0usize;
    let mut header_end = 0u32;
    for &(chan, slot, bit, pos) in plan {
        header_end = header_end.max(u32::from(pos) + 1);
        if (blk >> pos) & 1 == 1 {
            if chan == 3 {
                part |= 1 << bit;
            } else {
                f[chan as usize][slot as usize] |= 1 << bit;
            }
        }
    }
    let n_ep = if mode.regions == 2 { 4 } else { 2 };
    let mask = (1i32 << mode.base_bits) - 1;
    let mut eps = [[0i32; 4]; 3];
    for c in 0..3 {
        let dc = mode.delta_bits[c];
        let e0 = if signed {
            sign_extend(f[c][0], mode.base_bits)
        } else {
            f[c][0]
        };
        eps[c][0] = bc6h_unquantize(e0, mode.base_bits, signed);
        for q in 1..n_ep {
            let mut e = if signed || mode.transformed {
                sign_extend(f[c][q], dc)
            } else {
                f[c][q]
            };
            if mode.transformed {
                e = (e0 + e) & mask;
                if signed {
                    e = sign_extend(e, mode.base_bits);
                }
            }
            eps[c][q] = bc6h_unquantize(e, mode.base_bits, signed);
        }
    }

    let mut pos = header_end;
    let mut out = [[0f32; 3]; 16];
    let (wt, anchor1): (&[u32], usize) = if mode.regions == 1 {
        (&W4, 0)
    } else {
        (&W3, ANCHOR2[part] as usize)
    };
    let full = if mode.regions == 1 { 4 } else { 3 };
    for (p, px) in out.iter_mut().enumerate() {
        let n = if p == 0 || (mode.regions == 2 && p == anchor1) {
            full - 1
        } else {
            full
        };
        let w = wt[bits(blk, pos, n) as usize] as i32;
        pos += n;
        let s = if mode.regions == 2 {
            ((PART2[part] >> p) & 1) as usize * 2
        } else {
            0
        };
        for c in 0..3 {
            let v = ((64 - w) * eps[c][s] + w * eps[c][s + 1] + 32) >> 6;
            px[c] = bc6h_finish(v, signed);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_tables_are_consistent_with_their_anchors() {
        for p in 0..64 {
            assert_eq!((PART2[p] >> ANCHOR2[p]) & 1, 1, "2-subset partition {p}");
            assert_eq!(PART2[p] & 1, 0, "pixel 0 is subset 0");
            let sub = |i: usize| (PART3[p] >> (2 * i)) & 3;
            assert_eq!(sub(0), 0);
            assert_eq!(sub(ANCHOR3A[p] as usize), 1, "3-subset partition {p}");
            assert_eq!(sub(ANCHOR3B[p] as usize), 2, "3-subset partition {p}");
            assert!((0..16).all(|i| sub(i) < 3));
            assert!((0..3).all(|s| (0..16).any(|i| sub(i) == s)));
        }
    }

    #[test]
    fn bc6h_headers_have_the_documented_bit_counts() {
        for (m, plan) in bc6h_plans().iter().enumerate() {
            let total = plan.len() as u32 + if BC6H_MODES[m].code < 2 { 2 } else { 5 };
            let want = if BC6H_MODES[m].regions == 2 { 82 } else { 65 };
            assert_eq!(total, want, "mode {:#x}", BC6H_MODES[m].code);
            let mut positions: Vec<u8> = plan.iter().map(|p| p.3).collect();
            positions.sort();
            positions.dedup();
            assert_eq!(positions.len(), plan.len());
        }
    }

    #[test]
    fn bc6h_endpoint_fields_cover_every_bit_exactly_once() {
        for (m, plan) in bc6h_plans().iter().enumerate() {
            let mode = &BC6H_MODES[m];
            if mode.regions == 1 && mode.code != 0x03 {
                continue;
            }
            let n_ep = if mode.regions == 2 { 4 } else { 2 };
            for c in 0..3u8 {
                for q in 0..n_ep as u8 {
                    let mut got: Vec<u8> = plan
                        .iter()
                        .filter(|p| p.0 == c && p.1 == q)
                        .map(|p| p.2)
                        .collect();
                    got.sort();
                    let width = if q == 0 {
                        mode.base_bits
                    } else {
                        mode.delta_bits[c as usize]
                    };
                    assert_eq!(
                        got,
                        (0..width as u8).collect::<Vec<_>>(),
                        "mode {:#x} c{c} q{q}",
                        mode.code
                    );
                }
            }
        }
    }

    #[test]
    fn bc1_four_colour_block() {
        // red 31,0,0 and blue 0,0,31
        let c0: u16 = 31 << 11;
        let c1: u16 = 31;
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&c0.to_le_bytes());
        b[2..4].copy_from_slice(&c1.to_le_bytes());
        b[4] = 0b11_10_01_00;
        let px = bc1_block(&b, false);
        assert_eq!(px[0], [255, 0, 0, 255]);
        assert_eq!(px[1], [0, 0, 255, 255]);
        assert_eq!(px[2], [170, 0, 85, 255]);
        assert_eq!(px[3], [85, 0, 170, 255]);
    }

    #[test]
    fn bc1_three_colour_block_has_transparent_black() {
        let c0: u16 = 31;
        let c1: u16 = 31 << 11;
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&c0.to_le_bytes());
        b[2..4].copy_from_slice(&c1.to_le_bytes());
        b[4] = 0b11_10_01_00;
        let px = bc1_block(&b, false);
        assert_eq!(px[2], [127, 0, 127, 255]);
        assert_eq!(px[3], [0, 0, 0, 0]);
        assert_eq!(bc1_block(&b, true)[3][3], 255);
    }

    #[test]
    fn bc2_and_bc3_alpha() {
        let mut b = [0u8; 16];
        b[0] = 0xF1;
        let px = bc2_block(&b);
        assert_eq!(px[0][3], 17);
        assert_eq!(px[1][3], 255);

        let mut b3 = [0u8; 16];
        b3[0] = 200;
        b3[1] = 100;
        // indices: pixel0 = 0 (a0), pixel1 = 1 (a1), pixel2 = 2 (6/7 a0 + 1/7 a1)
        b3[2] = 0x88;
        let px = bc3_block(&b3);
        assert_eq!(px[0][3], 200);
        assert_eq!(px[1][3], 100);
        assert_eq!(px[2][3], 186);
    }

    #[test]
    fn bc4_six_value_mode_has_zero_and_full_endpoints() {
        let mut b = [0u8; 8];
        b[0] = 10;
        b[1] = 250;
        // pixel 0 -> index 6, pixel 1 -> index 7
        b[2] = 0b00_111_110;
        let v = bc4_block(&b);
        assert_eq!(v[0], 0);
        assert_eq!(v[1], 255);
        let mut b = [0u8; 8];
        b[0] = 10;
        b[1] = 250;
        b[2] = 0b00_000_010; // index 2 = (4*10 + 250)/5 = 58
        assert_eq!(bc4_block(&b)[0], 58);
    }

    #[test]
    fn bc5_reads_two_planes() {
        let mut b = [0u8; 16];
        b[0] = 9;
        b[1] = 9;
        b[8] = 77;
        b[9] = 77;
        assert_eq!(bc5_block(&b)[5], [9, 77]);
    }

    #[test]
    fn bc4_snorm_endpoints() {
        let mut b = [0u8; 8];
        b[0] = 127;
        b[1] = 0x81; // -127
        assert_eq!(bc4_block_snorm(&b)[0], 127);
        b[2] = 1;
        assert_eq!(bc4_block_snorm(&b)[0], -127);
    }

    /// Writes bit fields into a 128-bit block, LSB first.
    struct BitWriter(u128, u32);
    impl BitWriter {
        fn put(&mut self, v: u32, n: u32) {
            self.0 |= u128::from(v & ((1u64 << n) - 1) as u32) << self.1;
            self.1 += n;
        }
    }

    #[test]
    fn bc7_mode6_endpoints_and_indices() {
        let mut w = BitWriter(0, 0);
        w.put(0b100_0000, 7); // mode 6
        for v in [127, 0, 0, 127, 64, 64] {
            w.put(v, 7); // r0 r1 g0 g1 b0 b1
        }
        w.put(127, 7);
        w.put(127, 7); // a0 a1
        w.put(1, 1);
        w.put(1, 1); // p-bits
        // 16 indices, 4 bits (3 for the anchor pixel 0): pixel 0 = 0, pixel 1 = 15, others 0
        w.put(0, 3);
        w.put(15, 4);
        let bytes = w.0.to_le_bytes();
        let px = bc7_block(&bytes);
        assert_eq!(px[0], [255, 1, 129, 255]);
        assert_eq!(px[1], [1, 255, 129, 255]);
    }

    #[test]
    fn bc7_mode4_swaps_rotated_channel() {
        let mut w = BitWriter(0, 0);
        w.put(0b1_0000, 5); // mode 4
        w.put(1, 2); // rotation 1: swap alpha and red
        w.put(0, 1); // index selection 0
        for v in [31, 31, 0, 0, 0, 0] {
            w.put(v, 5); // r0 r1 g0 g1 b0 b1
        }
        w.put(10, 6);
        w.put(10, 6);
        let px = bc7_block(&w.0.to_le_bytes());
        // red channel 255 swapped with alpha 40 (10 << 2)
        assert_eq!(px[0][0], 40);
        assert_eq!(px[0][3], 255);
    }

    #[test]
    fn bc7_zero_block_is_transparent_black() {
        assert_eq!(bc7_block(&[0; 16]), [[0; 4]; 16]);
    }

    #[test]
    fn bc6h_mode11_unsigned_endpoints() {
        // mode 11 (code 0x03): 10-bit endpoints, no transform, 1 region.
        let mut w = BitWriter(0, 0);
        w.put(0x03, 5);
        for v in [0u32, 0, 0, 1023, 1023, 1023] {
            w.put(v, 10); // rw gw bw then rx gx bx
        }
        // indices: pixel 0 -> 0 (3 bits), pixel 1 -> 15 (4 bits)
        w.put(0, 3);
        w.put(15, 4);
        let px = bc6h_block(&w.0.to_le_bytes(), false);
        assert_eq!(px[0], [0.0, 0.0, 0.0]);
        // full-scale unsigned endpoint 0xFFFF maps to half 0x7BFF
        assert_eq!(px[1][0], half_to_f32(0x7BFF));
        assert_eq!(half_to_f32(0x7BFF), 65504.0);
    }

    #[test]
    fn bc6h_reserved_mode_is_black() {
        let mut b = [0u8; 16];
        b[0] = 0b10011; // mode code 0x13 is reserved
        assert_eq!(bc6h_block(&b, false), [[0.0; 3]; 16]);
    }

    #[test]
    fn half_conversion() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0), 0.0);
    }
}
