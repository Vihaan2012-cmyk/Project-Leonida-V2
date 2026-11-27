//! Prefix-code decoding for the WMA Pro tables.
//!
//! The tables list `(symbol, code length)` in code order: each entry's code is the previous code plus one at
//! its own length (left-aligned in 32 bits), starting from zero.

use crate::bits::BitReader;

const FAST_BITS: u32 = 10;

pub struct Vlc {
    codes: Vec<u32>,
    lens: Vec<u8>,
    syms: Vec<i32>,
    fast: Vec<(i32, u8)>,
}

impl Vlc {
    pub fn from_entries(entries: impl Iterator<Item = (i32, u8)>) -> Result<Vlc, &'static str> {
        let mut codes = Vec::new();
        let mut lens = Vec::new();
        let mut syms = Vec::new();
        let mut code: u64 = 0;
        for (sym, len) in entries {
            if len == 0 || len > 32 {
                return Err("bad code length");
            }
            if code >= 1 << 32 {
                return Err("over-subscribed code");
            }
            if code & ((1u64 << (32 - len)) - 1) != 0 {
                return Err("code not aligned to its length");
            }
            codes.push(code as u32);
            lens.push(len);
            syms.push(sym);
            code += 1u64 << (32 - len);
        }
        let mut fast = vec![(0i32, 0u8); 1 << FAST_BITS];
        for i in 0..codes.len() {
            let len = u32::from(lens[i]);
            if len <= FAST_BITS {
                let first = (codes[i] >> (32 - FAST_BITS)) as usize;
                let count = 1usize << (FAST_BITS - len);
                for slot in &mut fast[first..first + count] {
                    *slot = (syms[i], lens[i]);
                }
            }
        }
        Ok(Vlc {
            codes,
            lens,
            syms,
            fast,
        })
    }

    /// `None` when the bits do not form a code of this table.
    pub fn decode(&self, br: &mut BitReader<'_>) -> Option<i32> {
        let v = br.peek(32);
        let (sym, len) = self.fast[(v >> (32 - FAST_BITS)) as usize];
        if len != 0 {
            br.skip(len as usize);
            return Some(sym);
        }
        let idx = self.codes.partition_point(|&c| c <= v);
        if idx == 0 {
            return None;
        }
        let i = idx - 1;
        let len = u32::from(self.lens[i]);
        if (v ^ self.codes[i]) >> (32 - len) != 0 {
            return None;
        }
        br.skip(len as usize);
        Some(self.syms[i])
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.codes.len()
    }

    /// `(code, length)` of entry `i`, right-aligned; used by tests to build bitstreams.
    #[cfg(test)]
    pub fn code_of(&self, i: usize) -> (u32, u32) {
        let len = u32::from(self.lens[i]);
        (self.codes[i] >> (32 - len), len)
    }

    #[cfg(test)]
    pub fn symbol_of(&self, i: usize) -> i32 {
        self.syms[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;

    #[test]
    fn decodes_small_table() {
        // lengths 1,2,3,3 -> codes 0, 10, 110, 111
        let v = Vlc::from_entries([(10, 1), (11, 2), (12, 3), (13, 3)].into_iter()).unwrap();
        let mut w = BitWriter::new();
        for bits in [(1, 0b0), (2, 0b10), (3, 0b111), (3, 0b110), (1, 0)] {
            w.put(bits.0, bits.1);
        }
        let mut r = BitReader::new(&w.bytes);
        let got: Vec<_> = (0..5).map(|_| v.decode(&mut r).unwrap()).collect();
        assert_eq!(got, [10, 11, 13, 12, 10]);
    }

    #[test]
    fn rejects_misaligned_and_oversubscribed() {
        assert!(Vlc::from_entries([(0, 2), (1, 1)].into_iter()).is_err());
        assert!(Vlc::from_entries([(0, 1), (1, 1), (2, 1)].into_iter()).is_err());
    }

    #[test]
    fn long_codes_use_the_slow_path_and_incomplete_codes_fail() {
        let v = Vlc::from_entries([(1, 1), (2, 12), (3, 12)].into_iter()).unwrap();
        let mut w = BitWriter::new();
        w.put(1, 0);
        w.put(12, 0b1000_0000_0001);
        let mut r = BitReader::new(&w.bytes);
        assert_eq!(v.decode(&mut r), Some(1));
        assert_eq!(v.decode(&mut r), Some(3));
        let mut r = BitReader::new(&[0xFF, 0xFF, 0xFF]);
        assert_eq!(v.decode(&mut r), None);
    }
}
