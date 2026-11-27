//! The blocklist: what the offline scan found, what to use instead, and the lookups the runtime needs.
//!
//! It is derived from game data (names and hashes of the player's own copy), so it is written to `Output/` and
//! never committed.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::hash::{ContentHash, hex64, hex64_opt, hex64_vec};
use crate::names::Category;

pub const FORMAT: u32 = 1;
pub const FILE_NAME: &str = "content-blocklist.json";

#[derive(Debug, thiserror::Error)]
pub enum BlocklistError {
    #[error("could not read or write {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is not a blocklist: {message}")]
    Format { path: String, message: String },
    #[error("{path} is blocklist format {found}; this build reads format {FORMAT}")]
    Version { path: String, found: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Drawable,
    Fragment,
    Texture,
    /// A ped appearance catalogue (`0x8dcb4cf5`). Never blocked; rewritten.
    Catalogue,
}

/// What to use instead of a blocked resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Replacement {
    /// Draw nothing: a drawable is skipped, a catalogue slot is emptied (drawable key 0).
    Hide,
    /// A texture is replaced by a neutral flat one.
    Neutral,
    /// A catalogue state takes a clothed drawable (an underwear layer) and its textures from the same catalogue.
    UseState {
        #[serde(with = "hex64")]
        drawable: u64,
        #[serde(with = "hex64")]
        albedo: u64,
        #[serde(with = "hex64")]
        normal: u64,
        #[serde(with = "hex64_opt", default)]
        cull_drawable: Option<u64>,
    },
}

/// One hashed payload of a resource: a whole inflated stream (page), as it is read from the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRecord {
    pub hash: ContentHash,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEntry {
    pub kind: ResourceKind,
    pub category: Category,
    pub name: Option<String>,
    pub name_hash32: Option<u32>,
    #[serde(with = "hex64_vec", default)]
    pub keys64: Vec<u64>,
    #[serde(default)]
    pub content: Vec<ContentRecord>,
    pub replacement: Replacement,
}

/// A catalogue state that carries a blocked drawable, and what it becomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueRule {
    #[serde(with = "hex64")]
    pub catalogue: u64,
    #[serde(with = "hex64")]
    pub drawable: u64,
    #[serde(with = "hex64")]
    pub albedo: u64,
    #[serde(with = "hex64")]
    pub normal: u64,
    pub replacement: Replacement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocklist {
    pub format: u32,
    pub game_version: String,
    pub entries: Vec<BlockEntry>,
    #[serde(default)]
    pub catalogue_rules: Vec<CatalogueRule>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Blocklist {
    pub fn new(game_version: &str) -> Self {
        Self {
            format: FORMAT,
            game_version: game_version.to_string(),
            entries: Vec::new(),
            catalogue_rules: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), BlocklistError> {
        let io = |source| BlocklistError::Io {
            path: path.display().to_string(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| BlocklistError::Format {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        std::fs::write(path, text).map_err(io)
    }

    pub fn load(path: &Path) -> Result<Self, BlocklistError> {
        let shown = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|source| BlocklistError::Io {
            path: shown.clone(),
            source,
        })?;
        Self::from_json(&text, &shown)
    }

    pub fn from_json(text: &str, shown: &str) -> Result<Self, BlocklistError> {
        let list: Blocklist =
            serde_json::from_str(text).map_err(|error| BlocklistError::Format {
                path: shown.to_string(),
                message: error.to_string(),
            })?;
        if list.format != FORMAT {
            return Err(BlocklistError::Version {
                path: shown.to_string(),
                found: list.format,
            });
        }
        Ok(list)
    }

    pub fn index(&self) -> BlockIndex {
        BlockIndex::new(self)
    }
}

/// Hash-table form of a blocklist for the lookups made while the game runs.
#[derive(Debug, Default)]
pub struct BlockIndex {
    entries: Vec<BlockEntry>,
    by_name32: HashMap<u32, usize>,
    by_key64: HashMap<u64, usize>,
    by_content: HashMap<ContentHash, usize>,
    content_lengths: HashSet<u64>,
    blocked_drawables: HashMap<u64, Replacement>,
    rules: HashMap<(u64, u64, u64, u64), Replacement>,
    rules_by_drawable: HashMap<(u64, u64), Replacement>,
}

impl BlockIndex {
    fn new(list: &Blocklist) -> Self {
        let mut index = BlockIndex {
            entries: list.entries.clone(),
            ..Self::default()
        };
        for (at, entry) in list.entries.iter().enumerate() {
            if let Some(hash) = entry.name_hash32 {
                index.by_name32.entry(hash).or_insert(at);
            }
            for key in &entry.keys64 {
                index.by_key64.entry(*key).or_insert(at);
                if matches!(entry.kind, ResourceKind::Drawable | ResourceKind::Fragment) {
                    index
                        .blocked_drawables
                        .entry(*key)
                        .or_insert(entry.replacement);
                }
            }
            for record in &entry.content {
                index.by_content.entry(record.hash).or_insert(at);
                index.content_lengths.insert(record.len);
            }
        }
        for rule in &list.catalogue_rules {
            index.rules.insert(
                (rule.catalogue, rule.drawable, rule.albedo, rule.normal),
                rule.replacement,
            );
            index
                .rules_by_drawable
                .entry((rule.catalogue, rule.drawable))
                .or_insert(rule.replacement);
        }
        index
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn by_name_hash(&self, hash: u32) -> Option<&BlockEntry> {
        self.by_name32.get(&hash).map(|at| &self.entries[*at])
    }

    pub fn by_key(&self, key: u64) -> Option<&BlockEntry> {
        self.by_key64.get(&key).map(|at| &self.entries[*at])
    }

    pub fn by_content(&self, hash: &ContentHash) -> Option<&BlockEntry> {
        self.by_content.get(hash).map(|at| &self.entries[*at])
    }

    /// Whether any blocked payload has this length: the cheap test before hashing an upload.
    pub fn may_match_length(&self, len: u64) -> bool {
        self.content_lengths.contains(&len)
    }

    pub fn has_content(&self) -> bool {
        !self.content_lengths.is_empty()
    }

    /// The replacement for a drawable key that appears in a catalogue, if the key is blocked.
    pub fn blocked_drawable(&self, key: u64) -> Option<Replacement> {
        self.blocked_drawables.get(&key).copied()
    }

    pub fn has_blocked_drawables(&self) -> bool {
        !self.blocked_drawables.is_empty()
    }

    /// The rule for a state with exactly these textures.
    pub fn catalogue_rule(
        &self,
        catalogue: u64,
        drawable: u64,
        albedo: u64,
        normal: u64,
    ) -> Option<Replacement> {
        self.rules
            .get(&(catalogue, drawable, albedo, normal))
            .copied()
    }

    /// Some rule of the catalogue for this drawable, whatever its textures.
    pub fn catalogue_rule_for_drawable(
        &self,
        catalogue: u64,
        drawable: u64,
    ) -> Option<Replacement> {
        self.rules_by_drawable.get(&(catalogue, drawable)).copied()
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::names::name_keys;

    pub fn drawable_entry(name: &str, category: Category, replacement: Replacement) -> BlockEntry {
        let (hash32, keys) = name_keys(name);
        BlockEntry {
            kind: ResourceKind::Drawable,
            category,
            name: Some(name.to_string()),
            name_hash32: Some(hash32),
            keys64: keys,
            content: Vec::new(),
            replacement,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::drawable_entry;
    use super::*;

    fn sample() -> Blocklist {
        let mut list = Blocklist::new("1.0.2.1");
        let mut entry = drawable_entry(
            "bodygenitalsgen_fr1_008_regular_ws00_full",
            Category::Genitals,
            Replacement::Hide,
        );
        entry.content.push(ContentRecord {
            hash: ContentHash::of(b"page"),
            len: 4,
        });
        list.entries.push(entry);
        list.entries.push(BlockEntry {
            kind: ResourceKind::Texture,
            category: Category::Genitals,
            name: None,
            name_hash32: None,
            keys64: vec![0x5B68_29CF_EC0F_41A1],
            content: Vec::new(),
            replacement: Replacement::Neutral,
        });
        list.catalogue_rules.push(CatalogueRule {
            catalogue: 7,
            drawable: 11,
            albedo: 12,
            normal: 13,
            replacement: Replacement::UseState {
                drawable: 21,
                albedo: 22,
                normal: 23,
                cull_drawable: Some(24),
            },
        });
        list
    }

    #[test]
    fn lookups_find_entries_by_name_key_and_content() {
        let list = sample();
        let index = list.index();
        let entry = &list.entries[0];
        assert_eq!(
            index.by_name_hash(entry.name_hash32.unwrap()).unwrap().name,
            entry.name
        );
        assert!(index.by_key(entry.keys64[1]).is_some());
        assert!(index.by_key(0x5B68_29CF_EC0F_41A1).is_some());
        assert!(index.by_key(1).is_none());
        assert!(index.by_content(&ContentHash::of(b"page")).is_some());
        assert!(index.by_content(&ContentHash::of(b"other")).is_none());
        assert!(index.may_match_length(4));
        assert!(!index.may_match_length(5));
    }

    #[test]
    fn catalogue_rules_prefer_the_exact_state() {
        let index = sample().index();
        assert!(matches!(
            index.catalogue_rule(7, 11, 12, 13),
            Some(Replacement::UseState { drawable: 21, .. })
        ));
        assert!(index.catalogue_rule(7, 11, 99, 99).is_none());
        assert!(index.catalogue_rule_for_drawable(7, 11).is_some());
        assert!(index.catalogue_rule(8, 11, 12, 13).is_none());
        assert!(index.catalogue_rule_for_drawable(8, 11).is_none());
    }

    #[test]
    fn blocked_drawables_cover_every_key_spelling_but_not_textures() {
        let index = sample().index();
        let list = sample();
        assert_eq!(
            index.blocked_drawable(list.entries[0].keys64[0]),
            Some(Replacement::Hide)
        );
        assert_eq!(index.blocked_drawable(0x5B68_29CF_EC0F_41A1), None);
    }

    #[test]
    fn json_round_trip_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Output").join(FILE_NAME);
        let list = sample();
        list.save(&path).unwrap();
        assert_eq!(Blocklist::load(&path).unwrap(), list);
    }

    #[test]
    fn other_formats_and_garbage_are_refused() {
        let mut list = sample();
        list.format = 99;
        let text = serde_json::to_string(&list).unwrap();
        assert!(matches!(
            Blocklist::from_json(&text, "x"),
            Err(BlocklistError::Version { found: 99, .. })
        ));
        assert!(matches!(
            Blocklist::from_json("{}", "x"),
            Err(BlocklistError::Format { .. })
        ));
    }
}
