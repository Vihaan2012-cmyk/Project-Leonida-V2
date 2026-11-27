//! Static probability models of the BCPack rANS lanes.

use super::tables::{BASE_TABLE, FREQ_TABLE};
use std::sync::OnceLock;

/// rANS lower bound: a state at or below it ends a fragment.
pub const L: u32 = 1 << 24;

/// Widest alphabet any lane uses (8 bits), so the models the decoder needs are `eab` 0..=7.
pub const MAX_EAB: usize = 7;

/// One model: symbol width `eab + 1` bits and probability level `q`.
pub struct Model {
    /// Frequency and cumulative frequency of each symbol, packed `f << 16 | c`.
    fc: [u32; 256],
    /// Symbol owning each of the 65536 low state values.
    lut: Box<[u8; 65536]>,
}

impl Model {
    fn build(eab: usize, q: usize) -> Model {
        let base = &BASE_TABLE[eab][q];
        let freq = &FREQ_TABLE[eab][q];
        let mut fc = [0u32; 256];
        let mut lut = Box::new([0u8; 65536]);
        for (s, slot) in fc.iter_mut().enumerate().take(1 << (eab + 1)) {
            let seg = (usize::BITS - s.leading_zeros()) as usize;
            let seg_start = if seg == 0 { 0 } else { 1 << (seg - 1) };
            let f = u32::from(freq[seg]);
            let c = (u32::from(base[seg]) + f * (s - seg_start) as u32) & 0xFFFF;
            // Slot 0 is the end marker, so symbol 0 gives up its first slot.
            let (f, c) = if c == 0 { (f - 1, 1) } else { (f, c) };
            *slot = f << 16 | c;
            lut[c as usize..(c + f) as usize].fill(s as u8);
        }
        Model { fc, lut }
    }

    /// The symbol a state decodes to.
    #[inline(always)]
    pub fn symbol(&self, state: u32) -> u8 {
        self.lut[(state & 0xFFFF) as usize]
    }

    /// State after decoding `sym` from `x`.
    #[inline(always)]
    pub fn advance(&self, x: u32, sym: u8) -> u32 {
        let fc = self.fc[sym as usize];
        (fc >> 16)
            .wrapping_mul(x >> 16)
            .wrapping_add(x & 0xFFFF)
            .wrapping_sub(fc & 0xFFFF)
    }

    /// `(frequency, cumulative frequency)` of a symbol.
    #[cfg(test)]
    pub fn interval(&self, sym: u8) -> (u32, u32) {
        let fc = self.fc[sym as usize];
        (fc >> 16, fc & 0xFFFF)
    }
}

static MODELS: [OnceLock<Model>; (MAX_EAB + 1) * 16] =
    [const { OnceLock::new() }; (MAX_EAB + 1) * 16];

/// The model for an alphabet of `eab + 1` bits and level `q`, built on first use.
pub fn model(eab: usize, q: usize) -> &'static Model {
    MODELS[eab * 16 + q].get_or_init(|| Model::build(eab, q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_row_covers_the_whole_probability_space() {
        for eab in 0..=8 {
            for q in 0..16 {
                let (base, freq) = (&BASE_TABLE[eab][q], &FREQ_TABLE[eab][q]);
                let mut at = 0u32;
                for seg in 0..eab + 2 {
                    assert_eq!(u32::from(base[seg]), at, "eab {eab} q {q} seg {seg}");
                    let symbols = if seg == 0 { 1 } else { 1u32 << (seg - 1) };
                    at += u32::from(freq[seg]) * symbols;
                }
                assert_eq!(at, 65536, "eab {eab} q {q}");
                assert!(freq[..eab + 2].iter().all(|f| *f >= 1));
                assert!(freq[eab + 2..].iter().all(|f| *f == 0));
            }
        }
    }

    #[test]
    fn q_zero_is_uniform_and_q_fifteen_favours_symbol_zero() {
        for eab in 0..=MAX_EAB {
            let n = 1u32 << (eab + 1);
            let (f, _) = model(eab, 0).interval(1);
            assert_eq!(f * n, 65536);
            let (f0, c0) = model(eab, 15).interval(0);
            assert_eq!(c0, 1);
            assert!(f0 > 65536 - 2 * n);
        }
    }

    #[test]
    fn the_symbol_lookup_inverts_the_intervals() {
        for eab in [0usize, 2, 5, 7] {
            for q in [0usize, 1, 7, 15] {
                let m = model(eab, q);
                let mut covered = 0u32;
                for s in 0..(1u32 << (eab + 1)) {
                    let (f, c) = m.interval(s as u8);
                    covered += f;
                    for cf in [c, c + f / 2, c + f - 1] {
                        assert_eq!(u32::from(m.symbol(cf)), s, "eab {eab} q {q} cf {cf}");
                    }
                }
                assert_eq!(covered, 65535, "slot 0 is reserved");
            }
        }
    }

    #[test]
    fn advance_undoes_an_encode_step() {
        let m = model(7, 5);
        for s in [0u8, 17, 200] {
            let (f, c) = m.interval(s);
            let x = f * 300 + 5;
            let enc = (x / f) << 16 | (x % f + c);
            assert_eq!(m.symbol(enc), s);
            assert_eq!(m.advance(enc, s), x);
        }
    }

    #[test]
    fn the_symbol_lookup_matches_the_segment_arithmetic_for_every_state_of_every_model() {
        for eab in 0..=MAX_EAB {
            for q in 0..16 {
                let (base, freq) = (&BASE_TABLE[eab][q], &FREQ_TABLE[eab][q]);
                let m = model(eab, q);
                for cf in 1..65536u32 {
                    let seg = (0..=eab + 1)
                        .rev()
                        .find(|&k| cf >= u32::from(base[k]))
                        .unwrap();
                    let steps = (cf - u32::from(base[seg])) / u32::from(freq[seg]);
                    let expect = (if seg == 0 { 0 } else { 1 << (seg - 1) }) + steps;
                    assert_eq!(u32::from(m.symbol(cf)), expect, "eab {eab} q {q} cf {cf}");
                }
            }
        }
    }
}
