//! MSB-first bit reader, the bit order of XMA/WMA Pro streams.

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    limit: usize,
    overrun: bool,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_bit_len(data, data.len() * 8)
    }

    /// Reads past `bit_len` still return the data bytes if any exist, but set the overrun flag.
    pub fn with_bit_len(data: &'a [u8], bit_len: usize) -> Self {
        Self {
            data,
            pos: 0,
            limit: bit_len.min(data.len() * 8),
            overrun: false,
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn bit_len(&self) -> usize {
        self.limit
    }

    pub fn bits_left(&self) -> isize {
        self.limit as isize - self.pos as isize
    }

    pub fn overrun(&self) -> bool {
        self.overrun
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
        if pos > self.limit {
            self.overrun = true;
        }
    }

    /// Up to 32 bits, zero-filled past the end of the data, without consuming them.
    pub fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        let byte = self.pos >> 3;
        let mut word = 0u64;
        for i in 0..8 {
            word = (word << 8) | u64::from(self.data.get(byte + i).copied().unwrap_or(0));
        }
        let shifted = word << (self.pos & 7);
        (shifted >> (64 - n)) as u32
    }

    pub fn skip(&mut self, n: usize) {
        self.pos += n;
        if self.pos > self.limit {
            self.overrun = true;
        }
    }

    pub fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.skip(n as usize);
        v
    }

    pub fn read_bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Two's complement value of `n` bits.
    pub fn read_signed(&mut self, n: u32) -> i32 {
        let v = self.read(n) as i32;
        let shift = 32 - n;
        (v << shift) >> shift
    }
}

#[cfg(test)]
pub struct BitWriter {
    pub bytes: Vec<u8>,
    pub bits: usize,
}

#[cfg(test)]
impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl BitWriter {
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            bits: 0,
        }
    }

    pub fn put(&mut self, n: u32, value: u32) {
        for i in (0..n).rev() {
            let bit = (value >> i) & 1;
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if bit != 0 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }

    pub fn put_bit(&mut self, b: bool) {
        self.put(1, u32::from(b));
    }

    pub fn append(&mut self, other: &BitWriter) {
        for i in 0..other.bits {
            self.put_bit(other.bytes[i / 8] & (0x80 >> (i % 8)) != 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_across_bytes() {
        let data = [0b1010_1100, 0b0101_0011, 0xFF];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(3), 0b101);
        assert_eq!(r.read(7), 0b011_0001);
        assert_eq!(r.read(6), 0b01_0011);
        assert_eq!(r.pos(), 16);
        assert_eq!(r.read(8), 0xFF);
        assert!(!r.overrun());
        assert_eq!(r.read(4), 0);
        assert!(r.overrun());
    }

    #[test]
    fn reads_32_bits_unaligned() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34];
        let mut r = BitReader::new(&data);
        r.skip(4);
        assert_eq!(r.read(32), 0xEADB_EEF1);
        assert_eq!(r.peek(4), 0x2);
    }

    #[test]
    fn signed_values() {
        let mut w = BitWriter::new();
        w.put(6, 0b111_110);
        w.put(6, 0b000_111);
        let mut r = BitReader::new(&w.bytes);
        assert_eq!(r.read_signed(6), -2);
        assert_eq!(r.read_signed(6), 7);
    }

    #[test]
    fn writer_round_trips() {
        let mut w = BitWriter::new();
        w.put(15, 0x1234);
        w.put(1, 1);
        w.put(11, 0x7FF);
        let mut r = BitReader::with_bit_len(&w.bytes, w.bits);
        assert_eq!(r.read(15), 0x1234);
        assert_eq!(r.read(1), 1);
        assert_eq!(r.read(11), 0x7FF);
        assert_eq!(r.bits_left(), 0);
    }
}
