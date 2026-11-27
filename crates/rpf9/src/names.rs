//! Names that can be read out of the plaintext of a resource, and the hash the game stores them under.

const MAX_STEM: usize = 128;
const MAX_NAMES: usize = 32;

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

/// The hash resources are referred to by: `joaat` of the lower-cased name, without a `.#xx` suffix.
pub fn name_hash(name: &str) -> u32 {
    let stem = stem(name);
    let lower: Vec<u8> = stem.bytes().map(|byte| byte.to_ascii_lowercase()).collect();
    joaat(&lower)
}

/// `name` without a trailing `.#xx` resource suffix.
pub fn stem(name: &str) -> &str {
    match name.rfind(".#") {
        Some(at) if name.len() - at == 4 => &name[..at],
        _ => name,
    }
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')
}

/// Every `name.#dr`, `name.#ft`, `name.#dd` (any two-letter lowercase suffix) spelled in `data`, in order of
/// appearance, without repeats.
pub fn find_names(data: &[u8]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(found) = memchr::memmem::find(&data[from..], b".#") {
        let dot = from + found;
        from = dot + 2;
        let suffix = &data[dot + 2..data.len().min(dot + 4)];
        if suffix.len() != 2 || !suffix.iter().all(u8::is_ascii_lowercase) {
            continue;
        }
        if data
            .get(dot + 4)
            .is_some_and(|&next| next.is_ascii_alphanumeric())
        {
            continue;
        }
        let mut start = dot;
        while start > 0 && dot - start < MAX_STEM && is_name_byte(data[start - 1]) {
            start -= 1;
        }
        if start == dot {
            continue;
        }
        let name = String::from_utf8_lossy(&data[start..dot + 4]).into_owned();
        if !names.contains(&name) {
            names.push(name);
            if names.len() == MAX_NAMES {
                break;
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joaat_matches_the_measured_class_hashes() {
        assert_eq!(joaat(b"CMapData"), 0xd359_3fa6);
        assert_eq!(joaat(b"CEntityDef"), 0xce50_1483);
        assert_eq!(joaat(b""), 0);
    }

    #[test]
    fn name_hash_lowercases_and_drops_the_suffix() {
        assert_eq!(name_hash("Prop_Crate.#dr"), joaat(b"prop_crate"));
        assert_eq!(name_hash("prop_crate"), joaat(b"prop_crate"));
        assert_eq!(name_hash("a.b.#ft"), joaat(b"a.b"));
    }

    #[test]
    fn finds_names_between_other_bytes() {
        let mut data = vec![0u8; 8];
        data.extend_from_slice(b"first_one.#dr\0\x01second-two.#ft\0third.#dd");
        data.extend_from_slice(b"\0first_one.#dr\0notaname.#drx\0.#dr\0x.#Dr");
        assert_eq!(
            find_names(&data),
            ["first_one.#dr", "second-two.#ft", "third.#dd"]
        );
    }

    #[test]
    fn a_name_is_cut_at_the_first_other_byte() {
        assert_eq!(find_names(b"dir/sub name_x.#dr"), ["name_x.#dr"]);
    }
}
