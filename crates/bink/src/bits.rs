//! LSB-first bit reader: Bink 2 reads its bit stream as little-endian words, low bit first.

#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, bit: usize) {
        self.pos = bit;
    }

    pub fn bit_len(&self) -> usize {
        self.data.len() * 8
    }

    pub fn overrun(&self) -> bool {
        self.pos > self.bit_len()
    }

    /// Up to 32 bits without consuming them; bytes past the end read as zero.
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        let byte = self.pos >> 3;
        let shift = (self.pos & 7) as u32;
        let word = if byte + 8 <= self.data.len() {
            u64::from_le_bytes(self.data[byte..byte + 8].try_into().unwrap())
        } else {
            let mut buf = [0u8; 8];
            if byte < self.data.len() {
                let tail = &self.data[byte..];
                buf[..tail.len()].copy_from_slice(tail);
            }
            u64::from_le_bytes(buf)
        };
        ((word >> shift) & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    #[inline]
    pub fn read(&mut self, n: u32) -> u32 {
        let value = self.peek(n);
        self.pos += n as usize;
        value
    }

    #[inline]
    pub fn read_bool(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Count of consecutive one bits before a zero, at most `max`. A run of `max` ones stops without a terminating zero.
    pub fn read_unary(&mut self, max: u32) -> u32 {
        let mut count = 0;
        while count < max {
            if !self.read_bool() {
                return count;
            }
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_low_bits_first() {
        let mut r = BitReader::new(&[0b1010_0110, 0xFF]);
        assert_eq!(r.read(3), 0b110);
        assert_eq!(r.read(5), 0b10100);
        assert_eq!(r.read(4), 0xF);
        assert_eq!(r.pos(), 12);
    }

    #[test]
    fn reads_across_words() {
        let data: Vec<u8> = (0..16).collect();
        let mut r = BitReader::new(&data);
        r.skip(60);
        assert_eq!(r.read(8), ((data[7] >> 4) | (data[8] << 4)) as u32);
        r.seek(0);
        assert_eq!(r.read(32), 0x0302_0100);
    }

    #[test]
    fn zero_fills_past_end() {
        let mut r = BitReader::new(&[0xFF]);
        assert_eq!(r.read(8), 0xFF);
        assert!(!r.overrun());
        r.skip(8);
        assert_eq!(r.read(4), 0);
        assert!(r.overrun());
    }

    #[test]
    fn unary_stops_at_max_or_zero() {
        let mut r = BitReader::new(&[0b0001_0111]);
        assert_eq!(r.read_unary(12), 3);
        assert_eq!(r.read_unary(1), 1);
        assert_eq!(r.read_unary(12), 0);
    }
}
