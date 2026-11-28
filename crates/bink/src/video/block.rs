//! Reading of the entropy-coded parts of a macroblock: flags, quantisers, coded-block patterns and coefficients.

use crate::bits::BitReader;
use crate::tables;
use crate::vlc::Codebook;

pub struct Codes {
    pub ac1: Codebook,
    pub ac2: Codebook,
    pub mv: Codebook,
}

impl Codes {
    pub fn new() -> Codes {
        Codes {
            ac1: Codebook::new(&tables::AC_CODES1, &tables::AC_BITS1),
            ac2: Codebook::new(&tables::AC_CODES2, &tables::AC_BITS2),
            mv: Codebook::new(&tables::MV_CODES, &tables::MV_BITS),
        }
    }
}

/// Run of ones ended by a zero; a run of 12 ones is complete without a terminating zero.
#[inline]
fn prefix(r: &mut BitReader) -> u32 {
    let ones = (!r.peek(12)).trailing_zeros().min(12);
    r.skip(if ones == 12 { 12 } else { ones + 1 });
    ones
}

#[inline]
fn magnitude(r: &mut BitReader, prefix: u32) -> i32 {
    if prefix >= 4 {
        let n = prefix - 3;
        ((1u32 << n) + r.read(n) + 2) as i32
    } else {
        prefix as i32
    }
}

/// An unsigned value that may be zero (DC differences).
pub fn read_unsigned(r: &mut BitReader) -> i32 {
    let p = prefix(r);
    magnitude(r, p)
}

/// A signed value that is never zero (AC levels).
pub fn read_level(r: &mut BitReader) -> i32 {
    let p = prefix(r) + 1;
    let v = magnitude(r, p);
    if r.read_bool() { -v } else { v }
}

/// Count of zero bits before a one, at most `max`; a run of `max` zeros stops without a terminating one.
pub fn read_zeros(r: &mut BitReader, max: u32) -> u32 {
    let mut count = 0;
    while count < max {
        if r.read_bool() {
            return count;
        }
        count += 1;
    }
    count
}

/// One motion vector component: symbols 0..=7 are themselves, 8..=14 are minus 7..=1, 15 escapes to a longer code
/// whose low bit is the sign.
fn read_mv_component(r: &mut BitReader, codes: &Codes) -> Option<i32> {
    let symbol = codes.mv.read(r)? as i32;
    Some(match symbol {
        0..=7 => symbol,
        8..=14 => symbol - 15,
        _ => {
            let bits = read_zeros(r, 12) + 4;
            let v = (r.read(bits) + (1 << bits) - 1) as i32;
            if v & 1 != 0 { -(v >> 1) - 1 } else { v >> 1 }
        }
    })
}

/// The motion vectors of a macroblock: a flag picks one vector for the whole block or one per 16x16 quarter. All
/// horizontal components come first, then all vertical ones. Returns the vector count and the `[x, y]` pairs.
pub fn read_mvs(r: &mut BitReader, codes: &Codes) -> Option<(usize, [[i32; 2]; 4])> {
    let count = if r.read_bool() { 1 } else { 4 };
    let mut mv = [[0i32; 2]; 4];
    for axis in 0..2 {
        for v in mv.iter_mut().take(count) {
            v[axis] = read_mv_component(r, codes)?;
        }
    }
    Some((count, mv))
}

/// The scaled DC differences of a block group: a presence flag, then per value a magnitude and a sign.
pub fn read_dc_diffs<const N: usize>(r: &mut BitReader, q: i32) -> [i32; N] {
    let mut d = [0i32; N];
    if r.read_bool() {
        let scale = tables::QUANT_DC[q.max(8) as usize] as i32;
        for v in d.iter_mut() {
            let mut x = read_unsigned(r);
            if x != 0 {
                x = (x * scale + 0x200) >> 10;
                if r.read_bool() {
                    x = -x;
                }
            }
            *v = x;
        }
    }
    d
}

/// Reads `bits` run-coded booleans into `out`; `start` adds a leading false.
pub fn read_flags(r: &mut BitReader, out: &mut Vec<bool>, start: bool, bits: usize) {
    if start {
        out.push(false);
    }
    if !r.read_bool() {
        for _ in 0..bits {
            out.push(r.read_bool());
        }
        return;
    }
    let mut remaining = bits;
    let mut mode = 0;
    let mut last = false;
    while remaining > 0 {
        if !r.read_bool() {
            last = if mode == 3 { !last } else { r.read_bool() };
            out.push(last);
            remaining -= 1;
            let n = remaining.min(4);
            for _ in 0..n {
                out.push(r.read_bool());
            }
            remaining -= n;
            mode = 2;
        } else {
            let rb = if remaining < 4 {
                2
            } else if remaining < 16 {
                4
            } else {
                5
            };
            last = if mode == 3 { !last } else { r.read_bool() };
            let base = if mode == 3 { rb + 1 } else { rb + 2 };
            let mut run = remaining.min(base);
            if run != remaining {
                let extra = r.read(rb as u32) as usize;
                run += extra;
                mode = if extra == (1 << rb) - 1 { 1 } else { 3 };
                run = run.min(remaining);
            }
            for _ in 0..run {
                out.push(last);
            }
            remaining -= run;
        }
    }
}

/// The new quantiser index (0..=36) given the predicted one.
pub fn read_quant(r: &mut BitReader, predicted: i32) -> Option<i32> {
    let mut code = 0;
    while code < 4 && !r.read_bool() {
        code += 1;
    }
    if code == 0 {
        return Some(predicted);
    }
    if code == 3 {
        code += r.read(1) as i32;
    } else if code == 4 {
        code = r.read(5) as i32 + 5;
    }
    let q = if r.read_bool() {
        predicted - code
    } else {
        predicted + code
    };
    (0..tables::QUANT_DC.len() as i32).contains(&q).then_some(q)
}

pub fn read_cbp_luma(r: &mut BitReader, previous: u32, wide: bool) -> u32 {
    let count = (previous & 0xFFFF).count_ones();
    let (min_count, mask) = if count < 8 {
        (count, 0)
    } else {
        (16 - count, 0xFFFF)
    };
    let mut cbp = 0;
    if r.read_bool() {
        cbp = 0;
    } else if min_count >= 4 {
        cbp = r.read(16);
    } else {
        for i in 0..4 {
            if !r.read_bool() {
                cbp |= r.read(4) << (4 * i);
            }
        }
    }
    cbp ^= mask;
    if (!wide || cbp != 0) && r.read_bool() {
        cbp |= cbp << 16;
    }
    cbp
}

pub fn read_cbp_chroma(r: &mut BitReader, previous: u32) -> u32 {
    if r.read_bool() {
        return (previous & 0xF_0000) | u32::from(tables::CHROMA_CBPS[(previous & 0xF) as usize]);
    }
    let cbp = r.read(4);
    if r.read_bool() {
        cbp | (cbp << 16)
    } else {
        cbp
    }
}

/// Reads the coefficients of up to four blocks. `cbp` has one bit per block in its low bits; a non-zero high half
/// selects the second code table.
pub fn read_acs(
    r: &mut BitReader,
    codes: &Codes,
    cbp: u32,
    quant: &[[u16; 64]; 4],
    q: i32,
    blocks: &mut [[i32; 64]],
) -> bool {
    let book = if cbp >> 16 != 0 {
        &codes.ac1
    } else {
        &codes.ac2
    };
    let matrix = &quant[(q & 3) as usize];
    let shift = q >> 2;
    for (n, block) in blocks.iter_mut().enumerate() {
        if (cbp >> n) & 1 == 0 {
            continue;
        }
        let mut index = 1usize;
        let mut escape = 0;
        while index < 64 {
            if escape > 0 {
                escape -= 1;
            } else {
                let Some(symbol) = book.read(r) else {
                    return false;
                };
                let run = if symbol == 11 {
                    r.read(6) as usize
                } else {
                    usize::from(tables::AC_RUNS[symbol])
                };
                index += run;
                if index > 63 {
                    break;
                }
                escape = if symbol == 13 { 6 } else { 0 };
            }
            let level = read_level(r);
            let pos = usize::from(tables::ZIGZAG[index]);
            block[pos] = (((level * i32::from(matrix[pos])) << shift) + 0x40) >> 7;
            index += 1;
        }
    }
    !r.overrun()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packs (value, bit count) pairs LSB first.
    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut bits = Vec::new();
        for &(v, n) in fields {
            for i in 0..n {
                bits.push((v >> i) & 1);
            }
        }
        let mut out = vec![0u8; bits.len().div_ceil(8) + 8];
        for (i, b) in bits.iter().enumerate() {
            out[i / 8] |= (*b as u8) << (i % 8);
        }
        out
    }

    #[test]
    fn unsigned_values_use_escape_lengths() {
        // prefix 0 -> 0; prefix 3 -> 3; prefix 4 -> extra 1 bit: (1<<1)+bit+2
        let data = pack(&[(0b0, 1), (0b0111, 4), (0b01111, 5), (1, 1)]);
        let mut r = BitReader::new(&data);
        assert_eq!(read_unsigned(&mut r), 0);
        assert_eq!(read_unsigned(&mut r), 3);
        assert_eq!(read_unsigned(&mut r), 5);
    }

    #[test]
    fn levels_are_signed_and_never_zero() {
        let data = pack(&[(0, 1), (0, 1), (0b01, 2), (1, 1)]);
        let mut r = BitReader::new(&data);
        assert_eq!(read_level(&mut r), 1);
        assert_eq!(read_level(&mut r), -2);
    }

    #[test]
    fn quant_delta_adds_or_subtracts() {
        let data = pack(&[(1, 1), (0b10, 2), (0, 1), (0b10, 2), (1, 1)]);
        let mut r = BitReader::new(&data);
        assert_eq!(read_quant(&mut r, 20), Some(20));
        assert_eq!(read_quant(&mut r, 20), Some(21));
        assert_eq!(read_quant(&mut r, 20), Some(19));
    }

    #[test]
    fn flags_literal_and_run_coded() {
        let data = pack(&[(0, 1), (0b101, 3)]);
        let mut out = Vec::new();
        read_flags(&mut BitReader::new(&data), &mut out, true, 3);
        assert_eq!(out, [false, true, false, true]);
    }

    #[test]
    fn cbp_chroma_repeats_through_the_table() {
        let data = pack(&[(1, 1)]);
        let mut r = BitReader::new(&data);
        assert_eq!(read_cbp_chroma(&mut r, 0b0101), 0xF);
        let data = pack(&[(0, 1), (0b1010, 4), (0, 1)]);
        let mut r = BitReader::new(&data);
        assert_eq!(read_cbp_chroma(&mut r, 0), 0b1010);
    }

    #[test]
    fn cbp_luma_empty_and_full_nibbles() {
        let data = pack(&[(1, 1), (0, 1)]);
        let mut r = BitReader::new(&data);
        assert_eq!(read_cbp_luma(&mut r, 0, false), 0);
        let data = pack(&[(0, 1), (0xBEEF, 16), (0, 1)]);
        let mut r = BitReader::new(&data);
        assert_eq!(read_cbp_luma(&mut r, 0b1111, false), 0xBEEF);
    }

    #[test]
    fn coefficients_are_placed_by_zigzag_and_scaled() {
        let codes = Codes::new();
        // ac2 symbol 1 (code 0b000 over 3 bits) = run 1, then level +1 at scan index 2 -> raster 1.
        let data = pack(&[(0b000, 3), (0, 1), (0, 1), (0b10, 2)]);
        let mut r = BitReader::new(&data);
        let mut block = [[0i32; 64]];
        assert!(read_acs(
            &mut r,
            &codes,
            1,
            &tables::QUANT_INTRA_LUMA,
            0,
            &mut block
        ));
        let expect = ((i32::from(tables::QUANT_INTRA_LUMA[0][1])) + 0x40) >> 7;
        assert_eq!(block[0][1], expect);
        assert_eq!(block[0].iter().filter(|&&v| v != 0).count(), 1);
    }
}
