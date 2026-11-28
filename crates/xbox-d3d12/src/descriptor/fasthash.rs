//! A multiply-and-rotate hasher for the small integer keys of the per-draw lookups: descriptor words, handles,
//! addresses. Not for keys an outside party picks adversarially.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x517c_c1b7_2722_0a95;

#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);

impl FastHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.add(u64::from_le_bytes(chunk.try_into().expect("eight bytes")));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(last));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FastBuild = BuildHasherDefault<FastHasher>;
pub type FastMap<K, V> = HashMap<K, V, FastBuild>;
pub type FastSet<K> = HashSet<K, FastBuild>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_over_the_fast_hasher_finds_what_it_holds() {
        let mut map: FastMap<[u32; 8], u32> = FastMap::default();
        for i in 0..1000u32 {
            map.insert([i, i * 3, 0, 7, 0, 0, i ^ 5, 1], i);
        }
        for i in 0..1000u32 {
            assert_eq!(map[&[i, i * 3, 0, 7, 0, 0, i ^ 5, 1]], i);
        }
        assert!(!map.contains_key(&[1; 8]));
    }

    #[test]
    fn words_that_differ_hash_differently_in_every_position() {
        use std::hash::BuildHasher;
        let build = FastBuild::default();
        let hash = |k: [u32; 8]| build.hash_one(k);
        let base = hash([0; 8]);
        for at in 0..8 {
            let mut k = [0u32; 8];
            k[at] = 1;
            assert_ne!(hash(k), base, "word {at}");
        }
    }
}
