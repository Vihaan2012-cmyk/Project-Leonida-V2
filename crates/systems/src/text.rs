//! Localised string tables: hash table, index array and UTF-8 blob inside a `0DRP` resource, read to
//! `Systems/Text/<language>.json` and patched back in place.
//!
//! Descriptor layout (all pointers are `0x5xxxxxxx` resource pointers, low 28 bits are offsets):
//! `ptr index | u32 | u32 0x10 | ptr | 01 01 00 00 | u32 hash_bytes | ptr hash | u32 0x10 | u32 blob_bytes | ptr blob`
//! followed by 16-byte `(u32 tag, u32 bytes, ptr)` entries; tag `0x10` entries are blob chunks.
//! A hash entry is 16 bytes: `u32 joaat(label)`, `u32 0`, `u8 1` at +8, `u24 index` at +11.
//! An index entry is `u64, u64` with the second word `offset << 20 | flags`; the chunk is `flags & 3`.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const FORMAT: &str = "leonida-text-table";

fn u32_at(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

fn pointer(buf: &[u8], at: usize) -> Option<usize> {
    let lo = u32_at(buf, at)?;
    (lo >> 28 == 5 && u32_at(buf, at + 4)? == 0).then_some((lo & 0x0FFF_FFFF) as usize)
}

struct Entry {
    hash: u32,
    start: usize,
    end: usize,
}

fn blob_chunks(buf: &[u8], mut at: usize) -> Vec<(usize, usize)> {
    let mut chunks = Vec::new();
    while at + 16 <= buf.len() {
        let (Some(tag), Some(size), Some(start)) =
            (u32_at(buf, at), u32_at(buf, at + 4), pointer(buf, at + 8))
        else {
            break;
        };
        if tag == 0x10 {
            if start + size as usize > buf.len() {
                break;
            }
            chunks.push((start, start + size as usize));
        }
        at += 16;
    }
    chunks
}

fn descriptor_at(buf: &[u8], p: usize) -> Option<Vec<Entry>> {
    let index = pointer(buf, p)?;
    (u32_at(buf, p + 12)? == 0x10).then_some(())?;
    pointer(buf, p + 16)?;
    (buf.get(p + 24..p + 28)? == [1, 1, 0, 0]).then_some(())?;
    let hash_bytes = u32_at(buf, p + 28)? as usize;
    let hashes = pointer(buf, p + 32)?;
    (u32_at(buf, p + 40)? == 0x10).then_some(())?;
    u32_at(buf, p + 44)?;
    pointer(buf, p + 48)?;
    let chunks = blob_chunks(buf, p + 40);
    if !(0 < hash_bytes
        && hash_bytes < buf.len()
        && hashes + hash_bytes <= buf.len()
        && !chunks.is_empty())
    {
        return None;
    }
    let mut entries = Vec::new();
    let mut o = hashes;
    while o + 16 <= hashes + hash_bytes {
        let h = u32_at(buf, o)?;
        if h != 0 && u32_at(buf, o + 4)? == 0 && buf[o + 8] == 1 {
            let i = usize::from(buf[o + 11])
                | usize::from(buf[o + 12]) << 8
                | usize::from(buf[o + 13]) << 16;
            let at = index + 16 * i + 8;
            if let Some(raw) = buf.get(at..at + 8) {
                let reference = u64::from_le_bytes(raw.try_into().unwrap());
                if let Some(&(lo, hi)) = chunks.get((reference & 3) as usize) {
                    let start = lo + (reference >> 20) as usize;
                    if start <= hi
                        && let Some(len) = buf[start..hi].iter().position(|&b| b == 0)
                    {
                        entries.push(Entry {
                            hash: h,
                            start,
                            end: start + len,
                        });
                    }
                }
            }
        }
        o += 16;
    }
    Some(entries)
}

fn find_tables(buf: &[u8]) -> Vec<Vec<Entry>> {
    let mut tables = Vec::new();
    let mut p = 0;
    while p + 56 <= buf.len() {
        if buf[p + 7] == 0
            && buf[p + 12] == 0x10
            && buf[p + 27] == 0
            && let Some(entries) = descriptor_at(buf, p).filter(|e| !e.is_empty())
        {
            tables.push(entries);
            p += 56;
            continue;
        }
        p += 1;
    }
    tables
}

fn text_of(buf: &[u8], e: &Entry) -> String {
    String::from_utf8_lossy(&buf[e.start..e.end]).into_owned()
}

/// Every string of a language, by label hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextFile {
    pub format: String,
    pub language: String,
    /// `"0x" + 8 hex digits` of joaat(lowercase label), to the text with its inline markup.
    pub strings: BTreeMap<String, String>,
}

impl TextFile {
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("a string map always serialises");
        s.push('\n');
        s
    }

    pub fn from_json(src: &str) -> Result<TextFile> {
        let file: TextFile = serde_json::from_str(src).map_err(|e| Error::Json(e.to_string()))?;
        if file.format != FORMAT {
            return Err(Error::Json(format!(
                "format is {:?}, expected {FORMAT:?}",
                file.format
            )));
        }
        Ok(file)
    }
}

fn key(hash: u32) -> String {
    format!("0x{hash:08x}")
}

fn parse_key(s: &str) -> Result<u32> {
    s.strip_prefix("0x")
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .ok_or_else(|| Error::Json(format!("bad key {s:?}")))
}

/// Read every string table in one logical `0DRP` resource. Later tables win when a hash repeats.
pub fn read_tables(buf: &[u8]) -> Result<TextFile> {
    let tables = find_tables(buf);
    if tables.is_empty() {
        return Err(Error::NotATable("no string table descriptor found"));
    }
    let mut strings = BTreeMap::new();
    for entry in tables.iter().flatten() {
        strings.insert(key(entry.hash), text_of(buf, entry));
    }
    let language = guess_language(strings.values().map(String::as_str));
    Ok(TextFile {
        format: FORMAT.into(),
        language,
        strings,
    })
}

/// Write edited texts back into the resource they were read from. A text may not grow: its slot in the blob is
/// fixed. Keys that were not in the resource are rejected.
pub fn patch_tables(buf: &[u8], edited: &TextFile) -> Result<Vec<u8>> {
    let original = read_tables(buf)?;
    let tables = find_tables(buf);
    let mut wanted: HashMap<u32, &str> = HashMap::new();
    for (k, text) in &edited.strings {
        let hash = parse_key(k)?;
        match original.strings.get(k) {
            None => {
                return Err(Error::Json(format!(
                    "{k} is not in the resource; new strings cannot be added"
                )));
            }
            Some(old) if old != text => {
                if text.contains('\0') {
                    return Err(Error::Json(format!("{k} contains a NUL")));
                }
                wanted.insert(hash, text);
            }
            Some(_) => {}
        }
    }
    let mut writes: BTreeMap<usize, (&Entry, &str)> = BTreeMap::new();
    for entry in tables.iter().flatten() {
        let Some(&text) = wanted.get(&entry.hash) else {
            continue;
        };
        let old = entry.end - entry.start;
        if text.len() > old {
            return Err(Error::TextDoesNotFit {
                key: key(entry.hash),
                new: text.len(),
                old,
            });
        }
        if let Some((_, other)) = writes.insert(entry.start, (entry, text))
            && other != text
        {
            return Err(Error::SharedText(key(entry.hash)));
        }
    }
    for entry in tables.iter().flatten() {
        if let Some(&(_, text)) = writes.get(&entry.start) {
            let final_text = edited
                .strings
                .get(&key(entry.hash))
                .map_or_else(|| text_of(buf, entry), Clone::clone);
            if final_text != text {
                return Err(Error::SharedText(key(entry.hash)));
            }
        }
    }
    let mut out = buf.to_vec();
    for (&start, &(entry, text)) in &writes {
        out[start..entry.end].fill(0);
        out[start..start + text.len()].copy_from_slice(text.as_bytes());
    }
    Ok(out)
}

const STOP_WORDS: [(&str, &str); 7] = [
    (
        "en",
        "the you and to is what it this that i me my your we not",
    ),
    ("es", "que el la de no y es lo por un una se mi te"),
    ("fr", "le les est pas je vous que et un une ce tu"),
    ("de", "der die und ist nicht ich du das ein es zu"),
    ("it", "che il non di sono un la ho per mi ti"),
    ("pt", "que não você é o de um uma eu isso"),
    ("pl", "nie to jest się co ja ty na tak"),
];
const LETTERS: [(&str, &str); 6] = [
    ("pl", "ąćęłńśźż"),
    ("de", "äöüß"),
    ("pt", "ãõ"),
    ("es", "ñ¿¡"),
    ("fr", "œêëîïûù"),
    ("it", "ìò"),
];
const ZH_HANT: &str = "這們說個來時會為過還沒對嗎讓後裡開關動發現樣點車號裝經話見長門問麼";
const ZH_HANS: &str = "这们说个来时会为过还没对吗让后里开关动发现样点车号装经话见长门问么";
const ES_ES: &str = "vosotros vosotras os vuestro vuestra vuestros vuestras habéis hacéis vais sois tenéis queréis estáis \
                     podéis sabéis decís id venid mirad móvil ordenador guay joder hostia";
const ES_419: &str = "ustedes carro celular ahorita chamba güey wey platicar chido neta pinche cuate computadora manejar \
                      órale acá chingar chingada pendejo mande güera";

/// A table carries no language field; this guesses one from the first 400 strings (script first, then letters and
/// common words). `xx` when nothing decides it. The nested archive a table sits in is a better source when known.
pub fn guess_language<'a>(strings: impl IntoIterator<Item = &'a str>) -> String {
    let sample = strings.into_iter().take(400).collect::<Vec<_>>().join(" ");
    let in_range = |lo: char, hi: char| sample.chars().any(|c| (lo..=hi).contains(&c));
    if in_range('\u{3040}', '\u{30ff}') {
        return "ja".into();
    }
    if in_range('\u{ac00}', '\u{d7af}') {
        return "ko".into();
    }
    if in_range('\u{4e00}', '\u{9fff}') {
        let hant = sample.chars().filter(|&c| ZH_HANT.contains(c)).count();
        let hans = sample.chars().filter(|&c| ZH_HANS.contains(c)).count();
        return match hant.cmp(&hans) {
            std::cmp::Ordering::Greater => "zh-Hant",
            std::cmp::Ordering::Less => "zh-Hans",
            std::cmp::Ordering::Equal => "zh",
        }
        .into();
    }
    if in_range('\u{0400}', '\u{04ff}') {
        return "ru".into();
    }
    let low = sample.to_lowercase();
    let words: Vec<&str> = low
        .split(|c: char| {
            !(c.is_ascii_lowercase()
                || ('\u{e0}'..='\u{ff}').contains(&c)
                || "ąćęłńóśźż".contains(c))
        })
        .filter(|w| !w.is_empty())
        .collect();
    let mut score: Vec<(&str, usize)> = STOP_WORDS
        .iter()
        .map(|&(lang, stop)| {
            let stop: Vec<&str> = stop.split(' ').collect();
            let mut n = words.iter().filter(|w| stop.contains(w)).count();
            if let Some(&(_, letters)) = LETTERS.iter().find(|(l, _)| *l == lang) {
                n += 2 * low.chars().filter(|&c| letters.contains(c)).count();
            }
            (lang, n)
        })
        .collect();
    score.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let (best, top) = score[0];
    if top == 0 || score[1].1 == top {
        return "xx".into();
    }
    if best == "es" {
        let count = |list: &str| {
            words
                .iter()
                .filter(|w| list.split(' ').any(|x| x == **w))
                .count()
        };
        return match count(ES_ES).cmp(&count(ES_419)) {
            std::cmp::Ordering::Greater => "es-ES",
            std::cmp::Ordering::Less => "es-419",
            std::cmp::Ordering::Equal => "es",
        }
        .into();
    }
    best.into()
}
