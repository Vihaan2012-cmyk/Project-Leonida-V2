//! The three hashes the filter deals in: joaat32 and Murmur3-64 name hashes, and the content hash of a payload.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

pub use name_hash::joaat_lower;

/// Low 64 bits of MurmurHash3 x64 128 with seed 0 over the exact bytes: the 64-bit key of names and assets.
pub fn murmur64(data: &[u8]) -> u64 {
    const C1: u64 = 0x87c3_7b91_1142_53d5;
    const C2: u64 = 0x4cf5_ad43_2745_937f;
    let fmix = |mut k: u64| {
        k ^= k >> 33;
        k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
        k ^= k >> 33;
        k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        k ^ (k >> 33)
    };
    let (mut h1, mut h2) = (0u64, 0u64);
    let mut blocks = data.chunks_exact(16);
    for block in &mut blocks {
        let k1 = u64::from_le_bytes(block[..8].try_into().unwrap());
        let k2 = u64::from_le_bytes(block[8..].try_into().unwrap());
        h1 ^= k1.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
        h1 = h1
            .rotate_left(27)
            .wrapping_add(h2)
            .wrapping_mul(5)
            .wrapping_add(0x52dc_e729);
        h2 ^= k2.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
        h2 = h2
            .rotate_left(31)
            .wrapping_add(h1)
            .wrapping_mul(5)
            .wrapping_add(0x3849_5ab5);
    }
    let tail = blocks.remainder();
    let mut k1 = 0u64;
    let mut k2 = 0u64;
    for (index, byte) in tail.iter().enumerate() {
        if index < 8 {
            k1 |= u64::from(*byte) << (8 * index);
        } else {
            k2 |= u64::from(*byte) << (8 * (index - 8));
        }
    }
    if tail.len() > 8 {
        h2 ^= k2.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
    }
    if !tail.is_empty() {
        h1 ^= k1.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
    }
    h1 ^= data.len() as u64;
    h2 ^= data.len() as u64;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    h1 = fmix(h1);
    h2 = fmix(h2);
    h1.wrapping_add(h2)
}

/// First 128 bits of the SHA-256 of a payload.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(pub [u8; 16]);

impl ContentHash {
    pub fn of(data: &[u8]) -> Self {
        Self::from_digest(Sha256::digest(data).into())
    }

    pub fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest[..16].try_into().unwrap())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

impl FromStr for ContentHash {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        if text.len() != 32 || !text.is_ascii() {
            return Err(format!("`{text}` is not 32 hex digits"));
        }
        let mut out = [0u8; 16];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
                .map_err(|_| format!("`{text}` is not hex"))?;
        }
        Ok(Self(out))
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// serde adapters that write 64-bit keys as `0x` hex strings, as the game's documentation spells them.
pub mod hex64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn parse(text: &str) -> Result<u64, String> {
        let digits = text.strip_prefix("0x").unwrap_or(text);
        u64::from_str_radix(digits, 16).map_err(|_| format!("`{text}` is not a hex key"))
    }

    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("0x{value:016X}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

pub mod hex64_opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => super::hex64::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| super::hex64::parse(&text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

pub mod hex64_vec {
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u64], serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(value.len()))?;
        for key in value {
            seq.serialize_element(&format!("0x{key:016X}"))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u64>, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|text| super::hex64::parse(text).map_err(serde::de::Error::custom))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joaat_matches_known_values() {
        assert_eq!(joaat_lower("a"), 0xCA2E_9442);
        assert_eq!(joaat_lower("A"), 0xCA2E_9442);
        assert_eq!(joaat_lower(""), 0);
    }

    #[test]
    fn murmur64_matches_reference_vectors() {
        assert_eq!(murmur64(b""), 0);
        assert_eq!(murmur64(b"hello"), 0xcbd8_a7b3_41bd_9b02);
    }

    #[test]
    fn content_hash_round_trips_through_text() {
        let hash = ContentHash::of(b"payload");
        assert_eq!(hash.to_string().parse::<ContentHash>().unwrap(), hash);
        assert_ne!(hash, ContentHash::of(b"payloae"));
        assert!("zz".parse::<ContentHash>().is_err());
    }
}
