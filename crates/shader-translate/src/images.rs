//! The contract between translated shaders and the runtime for textures and samplers (spec section 4.2).

use rdna::format::Number as Format;
use shader_ir::ImageDim;

pub const SET: u32 = 1;
pub const TEXTURE_TABLE: u32 = 0;
pub const SAMPLER_TABLE: u32 = 1;
pub const SAMPLERS: u32 = 2;
pub const ENTRY_WORDS: usize = 10;
pub const PROBES: u32 = 16;
pub const EMPTY: u32 = u32::MAX;
pub const SLOT_MASK: u32 = (1 << 30) - 1;
const LARGEST: usize = 1 << 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    D2,
    D2Array,
    D3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Number {
    Float,
    Uint,
    Sint,
}

pub const VIEWS: [View; 3] = [View::D2, View::D2Array, View::D3];
pub const NUMBERS: [Number; 3] = [Number::Float, Number::Uint, Number::Sint];

impl View {
    /// 1D textures are one texel high 2D views; cube maps are six-layer 2D array views.
    pub fn of(dim: ImageDim) -> View {
        match dim {
            ImageDim::D1 | ImageDim::D2 => View::D2,
            ImageDim::D1Array | ImageDim::D2Array | ImageDim::Cube => View::D2Array,
            ImageDim::D3 => View::D3,
        }
    }
}

pub fn binding(view: View, number: Number) -> u32 {
    3 + 3 * view as u32 + number as u32
}

/// Which array a texture with this T# belongs in, from the FORMAT field (word 1, bits 20-28).
pub fn number(descriptor: &[u32; 8]) -> Number {
    match rdna::format::number((descriptor[1] >> 20) & 0x1FF) {
        Some(Format::Uint) => Number::Uint,
        Some(Format::Sint) => Number::Sint,
        _ => Number::Float,
    }
}

/// A table entry's slot word: the bindless index, with the number type in the top two bits.
pub fn slot(index: u32, number: Number) -> u32 {
    (index & SLOT_MASK) | (number as u32) << 30
}

pub fn sampler_key(sampler: &[u32; 4]) -> [u32; 8] {
    [sampler[0], sampler[1], sampler[2], sampler[3], 0, 0, 0, 0]
}

/// FNV-1a over the key's words.
pub fn hash(key: &[u32; 8]) -> u32 {
    key.iter()
        .fold(0x811C_9DC5, |h, word| (h ^ word).wrapping_mul(0x0100_0193))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    #[error("descriptor {0:08x?} is registered with two different slots")]
    Conflict([u32; 8]),
    #[error("too many descriptors for one table")]
    TooLarge,
}

/// A table holding every `(descriptor, slot word)`, each within `PROBES` entries of its hash.
pub fn table(entries: &[([u32; 8], u32)]) -> Result<Vec<u32>, TableError> {
    let mut size = (entries.len() * 2).next_power_of_two().max(16);
    while size <= LARGEST {
        if let Some(words) = fill(entries, size)? {
            return Ok(words);
        }
        size *= 2;
    }
    Err(TableError::TooLarge)
}

fn fill(entries: &[([u32; 8], u32)], size: usize) -> Result<Option<Vec<u32>>, TableError> {
    let mask = size as u32 - 1;
    let mut words = vec![0; 1 + size * ENTRY_WORDS];
    words[0] = mask;
    for entry in 0..size {
        words[1 + entry * ENTRY_WORDS + 8] = EMPTY;
    }
    'entries: for (key, slot) in entries {
        let start = hash(key);
        for probe in 0..PROBES {
            let base = 1 + (start.wrapping_add(probe) & mask) as usize * ENTRY_WORDS;
            let stored = words[base + 8];
            if stored == EMPTY {
                words[base..base + 8].copy_from_slice(key);
                words[base + 8] = *slot;
                continue 'entries;
            }
            if words[base..base + 8] == key[..] {
                if stored != *slot {
                    return Err(TableError::Conflict(*key));
                }
                continue 'entries;
            }
        }
        return Ok(None);
    }
    Ok(Some(words))
}

/// What a shader finds for `key`: its slot word, or 0 when the descriptor is not registered.
pub fn lookup(table: &[u32], key: &[u32; 8]) -> u32 {
    let mask = table[0];
    let start = hash(key);
    for probe in 0..PROBES {
        let base = 1 + (start.wrapping_add(probe) & mask) as usize * ENTRY_WORDS;
        let slot = table[base + 8];
        if slot == EMPTY {
            return 0;
        }
        if table[base..base + 8] == key[..] {
            return slot;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u32) -> [u32; 8] {
        std::array::from_fn(|i| seed.wrapping_mul(0x9E37_79B9) ^ i as u32)
    }

    #[test]
    fn every_registered_descriptor_is_found_and_others_are_not() {
        let entries: Vec<([u32; 8], u32)> = (0..1000)
            .map(|i| (key(i), slot(i + 1, NUMBERS[i as usize % 3])))
            .collect();
        let words = table(&entries).unwrap();
        for (key, slot) in &entries {
            assert_eq!(lookup(&words, key), *slot);
        }
        assert_eq!(lookup(&words, &key(5000)), 0);
    }

    #[test]
    fn colliding_descriptors_still_fit() {
        let entries: Vec<([u32; 8], u32)> = (0..40u32)
            .map(|i| {
                let mut k = [0; 8];
                k[7] = i << 20;
                (k, i + 1)
            })
            .collect();
        let words = table(&entries).unwrap();
        for (key, slot) in &entries {
            assert_eq!(lookup(&words, key), *slot);
        }
    }

    #[test]
    fn duplicates_must_agree() {
        assert!(table(&[(key(1), 4), (key(1), 4)]).is_ok());
        assert_eq!(
            table(&[(key(1), 4), (key(1), 5)]),
            Err(TableError::Conflict(key(1)))
        );
    }

    #[test]
    fn number_type_comes_from_the_descriptor_format() {
        let with_format = |format: u32| {
            let mut t = [0; 8];
            t[1] = format << 20 | 0x3F;
            t
        };
        assert_eq!(number(&with_format(20)), Number::Uint);
        assert_eq!(number(&with_format(21)), Number::Sint);
        assert_eq!(number(&with_format(22)), Number::Float);
        assert_eq!(number(&with_format(56)), Number::Float);
        assert_eq!(slot(7, Number::Sint) >> 30, 2);
        assert_eq!(slot(7, Number::Sint) & SLOT_MASK, 7);
    }

    #[test]
    fn bindings_follow_the_spec() {
        assert_eq!(binding(View::D2, Number::Float), 3);
        assert_eq!(binding(View::D2Array, Number::Uint), 7);
        assert_eq!(binding(View::D3, Number::Sint), 11);
        assert_eq!(View::of(ImageDim::Cube), View::D2Array);
        assert_eq!(View::of(ImageDim::D1), View::D2);
    }
}
