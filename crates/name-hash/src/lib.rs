//! The 32-bit name hash the game's data uses: Jenkins one-at-a-time, over the bytes as given or over the
//! ASCII-lower-cased bytes of a name.

/// Jenkins one-at-a-time hash over the bytes as given.
pub fn joaat(bytes: &[u8]) -> u32 {
    let mut hash = 0u32;
    for &byte in bytes {
        hash = hash.wrapping_add(u32::from(byte));
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash.wrapping_add(hash << 15)
}

/// `joaat` of the ASCII-lower-cased text.
pub fn joaat_lower(text: &str) -> u32 {
    let mut hash = 0u32;
    for byte in text.bytes() {
        hash = hash.wrapping_add(u32::from(byte.to_ascii_lowercase()));
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash.wrapping_add(hash << 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joaat_matches_the_known_values() {
        assert_eq!(joaat(b""), 0);
        assert_eq!(joaat(b"a"), 0xCA2E_9442);
        assert_eq!(
            joaat(b"The quick brown fox jumps over the lazy dog"),
            0x519E_91F5
        );
        assert_eq!(joaat(b"CMapData"), 0xd359_3fa6);
    }

    #[test]
    fn lower_hash_ignores_ascii_case_and_matches_the_measured_hashes() {
        assert_eq!(joaat_lower("A"), joaat(b"a"));
        assert_eq!(joaat_lower("WaterFog"), 0xf138_9e16);
        assert_eq!(joaat_lower("TrackId"), 0xa6d9_3246);
        assert_eq!(joaat_lower("waterfog2"), 0x9b9b_31df);
    }
}
