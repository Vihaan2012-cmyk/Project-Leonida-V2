//! Prefix-code tables read with the LSB-first bit reader.

use crate::bits::BitReader;

pub struct Codebook {
    lut: Vec<u16>,
    bits: u32,
}

impl Codebook {
    /// `codes[i]` holds the first `lengths[i]` bits read, the first bit in bit 0. The decoded symbol is `i`.
    pub fn new(codes: &[u16], lengths: &[u8]) -> Codebook {
        let bits = u32::from(*lengths.iter().max().unwrap());
        let mut lut = vec![u16::MAX; 1 << bits];
        for (symbol, (&code, &len)) in codes.iter().zip(lengths).enumerate() {
            let len = u32::from(len);
            let mut index = u32::from(code);
            while index < 1 << bits {
                debug_assert_eq!(lut[index as usize], u16::MAX, "codes overlap");
                lut[index as usize] = (symbol as u16) | ((len as u16) << 8);
                index += 1 << len;
            }
        }
        Codebook { lut, bits }
    }

    /// The symbol, or `None` when the bits match no code.
    #[inline]
    pub fn read(&self, reader: &mut BitReader) -> Option<usize> {
        let entry = self.lut[reader.peek(self.bits) as usize];
        if entry == u16::MAX {
            return None;
        }
        reader.skip(u32::from(entry >> 8));
        Some(usize::from(entry & 0xFF))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_codes_first_bit_in_the_low_bit() {
        let book = Codebook::new(&[0b1, 0b00, 0b10], &[1, 2, 2]);
        let mut reader = BitReader::new(&[0b0010_0101]);
        assert_eq!(book.read(&mut reader), Some(0));
        assert_eq!(book.read(&mut reader), Some(2));
        assert_eq!(book.read(&mut reader), Some(1));
        assert_eq!(book.read(&mut reader), Some(0));
        assert_eq!(reader.pos(), 6);
    }

    #[test]
    fn incomplete_codes_report_no_match() {
        let book = Codebook::new(&[0b0, 0b01], &[1, 2]);
        let mut reader = BitReader::new(&[0b11]);
        assert_eq!(book.read(&mut reader), None);
    }
}
