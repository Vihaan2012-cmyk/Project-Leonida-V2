//! The offline scan: archives and catalogue data in, blocklist out.
//!
//! What is read: resource names from the plaintext inside inflated streams (`archive`), their content hashes, and, when
//! a catalogue decoder supplies them, the decoded ped catalogues. Nothing is decrypted and nothing is run.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::archive::{Stream, WalkOptions, WalkSummary, walk};
use crate::blocklist::{Blocklist, ContentRecord, ResourceKind};
use crate::hash::ContentHash;
use crate::names::{Category, NameClass, classify, stem};
use crate::rules::{BaseBodyReport, Catalogue, NameDictionary, derive};

/// Streams after a named stream that are counted as its pages, at most.
const MAX_PAGES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    names: Vec<String>,
    streams: Vec<(ContentHash, u64)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
}

/// What a scan found, for the report.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub archives: Vec<(String, WalkSummary)>,
    pub names_seen: usize,
    pub blocked_by_category: BTreeMap<Category, usize>,
    pub textures_blocked: usize,
    pub catalogue_rules: usize,
    pub emptied: usize,
    pub unnamed_states: usize,
    pub base_bodies: Vec<BaseBodyReport>,
}

/// Collects streams from any number of archives, then builds the blocklist.
#[derive(Default)]
pub struct Scanner {
    groups: Vec<Group>,
    hash_counts: HashMap<ContentHash, u32>,
    names: BTreeMap<String, ResourceKind>,
    archives: Vec<(String, WalkSummary)>,
}

fn kind_of(name: &str) -> ResourceKind {
    if name.ends_with(".#ft") {
        ResourceKind::Fragment
    } else {
        ResourceKind::Drawable
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next stream of an archive, in archive order.
    pub fn add_stream(&mut self, stream: Stream) {
        *self.hash_counts.entry(stream.hash).or_default() += 1;
        let record = (stream.hash, stream.inflated);
        if !stream.names.is_empty() {
            for name in &stream.names {
                let kind = kind_of(name);
                let entry = self.names.entry(stem(name)).or_insert(kind);
                if kind == ResourceKind::Drawable {
                    *entry = kind;
                }
            }
            self.groups.push(Group {
                names: stream.names,
                streams: vec![record],
            });
        } else if let Some(group) = self.groups.last_mut()
            && group.streams.len() <= MAX_PAGES
        {
            group.streams.push(record);
        }
    }

    pub fn add_archive_bytes(&mut self, label: &str, data: &[u8], options: WalkOptions) {
        let summary = walk(data, options, |stream| self.add_stream(stream));
        self.archives.push((label.to_string(), summary));
    }

    pub fn add_archive_file(&mut self, path: &Path, options: WalkOptions) -> Result<(), ScanError> {
        let read = |source| ScanError::Read {
            path: path.display().to_string(),
            source,
        };
        let file = std::fs::File::open(path).map_err(read)?;
        let map = game_package::map_file(&file).map_err(read)?;
        let label = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        self.add_archive_bytes(&label, &map, options);
        Ok(())
    }

    /// Builds the blocklist. `catalogues` are decoded ped catalogues; without them only name rules apply.
    pub fn finish(&self, game_version: &str, catalogues: &[Catalogue]) -> (Blocklist, ScanReport) {
        let mut dict = NameDictionary::new();
        for name in self.names.keys() {
            dict.insert(name);
        }
        let blocked_names: Vec<String> = self
            .names
            .keys()
            .filter(|name| matches!(classify(name).class, NameClass::Blocked(_)))
            .cloned()
            .collect();
        let derived = derive(catalogues, &dict, &blocked_names);

        let mut pages: HashMap<String, Vec<ContentRecord>> = HashMap::new();
        let mut seen: HashSet<(String, ContentHash)> = HashSet::new();
        for group in &self.groups {
            for name in &group.names {
                let name = stem(name);
                if !blocked_names.contains(&name) {
                    continue;
                }
                for (hash, len) in &group.streams {
                    if self.hash_counts[hash] == 1 && seen.insert((name.clone(), *hash)) {
                        pages.entry(name.clone()).or_default().push(ContentRecord {
                            hash: *hash,
                            len: *len,
                        });
                    }
                }
            }
        }

        let mut list = Blocklist::new(game_version);
        let mut report = ScanReport {
            archives: self.archives.clone(),
            names_seen: self.names.len(),
            catalogue_rules: derived.catalogue_rules.len(),
            emptied: derived.emptied,
            unnamed_states: derived.unnamed_states,
            base_bodies: derived.base_bodies.clone(),
            ..ScanReport::default()
        };
        for mut entry in derived.entries {
            match entry.kind {
                ResourceKind::Texture => report.textures_blocked += 1,
                _ => {
                    *report
                        .blocked_by_category
                        .entry(entry.category)
                        .or_default() += 1
                }
            }
            if let Some(name) = entry.name.as_deref() {
                if let Some(kind) = self.names.get(name) {
                    entry.kind = *kind;
                }
                if let Some(content) = pages.remove(name) {
                    entry.content = content;
                }
            }
            list.entries.push(entry);
        }
        list.catalogue_rules = derived.catalogue_rules;
        list.notes = notes(!catalogues.is_empty());
        (list, report)
    }
}

fn notes(with_catalogues: bool) -> Vec<String> {
    let mut notes = vec![
        "Drawables are matched by name rules (see names.rs); names come from the plaintext inside inflated streams, not from the table of contents.".to_string(),
        "Content hashes are of whole inflated streams, as read from the archive; a runtime match needs the game to upload those exact bytes (not measured).".to_string(),
        "Pages are the unnamed streams that follow a named stream, up to 16, by file order (assumption).".to_string(),
    ];
    if !with_catalogues {
        notes.push("No catalogue data was given: no replacement rules and no texture blocklist were derived.".to_string());
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::testutil::{archive, named_payload, stream};
    use crate::names::name_keys;
    use crate::rules::State;

    fn sample_archive() -> Vec<u8> {
        let genitals = named_payload("bodygenitalsgen_fr1_008_regular_ws00_full.#dr", 64);
        let page = vec![0x42u8; 9000];
        let jeans = named_payload("pantsjeans_fr1_002_regular_hw_mid_full.#dr", 64);
        let jeans_page = vec![0x24u8; 7000];
        let shared_page = vec![0x99u8; 3000];
        let mannequin = named_payload("gmt_mannequin_proxy_f_01a.#dr", 16);
        let hand = named_payload("hand_fr1_000_nude_cull.#dr", 16);
        archive(
            &[
                stream(&genitals),
                stream(&page),
                stream(&shared_page),
                stream(&jeans),
                stream(&jeans_page),
                stream(&shared_page),
                stream(&mannequin),
                stream(&[1u8; 5000]),
                stream(&hand),
            ],
            0,
        )
    }

    fn scanner() -> Scanner {
        let mut scanner = Scanner::new();
        scanner.add_archive_bytes("packs.rpf", &sample_archive(), WalkOptions::default());
        scanner
    }

    #[test]
    fn name_rules_produce_entries_with_exclusive_page_hashes() {
        let (list, report) = scanner().finish("1.0.2.1", &[]);
        let names: Vec<_> = list
            .entries
            .iter()
            .filter_map(|entry| entry.name.as_deref())
            .collect();
        assert_eq!(
            names,
            vec![
                "bodygenitalsgen_fr1_008_regular_ws00_full",
                "gmt_mannequin_proxy_f_01a"
            ]
        );
        let genitals = &list.entries[0];
        assert_eq!(genitals.category, Category::Genitals);
        assert!(
            genitals
                .content
                .iter()
                .any(|record| record.hash == ContentHash::of(&[0x42u8; 9000]))
        );
        assert!(
            genitals
                .content
                .iter()
                .all(|record| record.hash != ContentHash::of(&[0x99u8; 3000])),
            "a page shared with a clothed drawable must not be blocked"
        );
        assert!(
            genitals
                .content
                .iter()
                .all(|record| record.hash != ContentHash::of(&[0x24u8; 7000])),
            "pages of the next resource are not attributed"
        );
        assert_eq!(report.names_seen, 4);
        assert_eq!(report.blocked_by_category[&Category::Genitals], 1);
        assert!(
            list.notes
                .iter()
                .any(|note| note.contains("No catalogue data"))
        );
    }

    #[test]
    fn catalogue_data_adds_rules_and_exclusive_textures() {
        let state = |preset_id: u32, name: &str, albedo: u64, normal: u64| State {
            preset_id,
            drawable: name_keys(name).1[0],
            albedo,
            normal,
            cull_drawable: None,
        };
        let catalogue = Catalogue {
            id: 9,
            states: vec![
                state(1, "bodygenitalsgen_fr1_008_regular_ws00_full", 0xA3, 0xB3),
                state(2, "undiesgstring_fr1_002_slim_hw_full", 0xC4, 0xD4),
                state(3, "hand_fr1_000_nude_cull", 0xA1, 0xB1),
            ],
        };
        let mut scanner = scanner();
        scanner.add_archive_bytes(
            "extra.rpf",
            &archive(
                &[stream(&named_payload(
                    "undiesgstring_fr1_002_slim_hw_full.#dr",
                    8,
                ))],
                0,
            ),
            WalkOptions::default(),
        );
        let (list, report) = scanner.finish("1.0.2.1", &[catalogue]);
        assert_eq!(report.catalogue_rules, 1);
        assert_eq!(report.textures_blocked, 2);
        assert_eq!(report.base_bodies.len(), 1);
        assert!(
            !list
                .notes
                .iter()
                .any(|note| note.contains("No catalogue data"))
        );
        assert!(
            list.entries
                .iter()
                .any(|entry| entry.kind == ResourceKind::Texture && entry.keys64 == vec![0xA3])
        );
        let index = list.index();
        assert!(
            index
                .catalogue_rule(
                    9,
                    name_keys("bodygenitalsgen_fr1_008_regular_ws00_full").1[0],
                    0xA3,
                    0xB3
                )
                .is_some()
        );
    }

    #[test]
    fn fragments_are_listed_as_fragments() {
        let archive = archive(&[stream(&named_payload("gmt_mannequin_f_01b.#ft", 8))], 0);
        let mut scanner = Scanner::new();
        scanner.add_archive_bytes("levels.rpf", &archive, WalkOptions::default());
        let (list, _) = scanner.finish("1.0.2.1", &[]);
        assert_eq!(list.entries[0].kind, ResourceKind::Fragment);
    }

    #[test]
    fn nothing_blocked_means_an_empty_list() {
        let archive = archive(
            &[stream(&named_payload(
                "pantsjeans_fr1_002_regular_hw_mid_full.#dr",
                8,
            ))],
            0,
        );
        let mut scanner = Scanner::new();
        scanner.add_archive_bytes("packs.rpf", &archive, WalkOptions::default());
        let (list, _) = scanner.finish("1.0.2.1", &[]);
        assert!(list.entries.is_empty());
    }

    #[test]
    fn scanning_a_file_reads_it_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("packs.rpf");
        std::fs::write(&path, sample_archive()).unwrap();
        let mut scanner = Scanner::new();
        scanner
            .add_archive_file(&path, WalkOptions::default())
            .unwrap();
        assert!(
            scanner
                .add_archive_file(&dir.path().join("missing.rpf"), WalkOptions::default())
                .is_err()
        );
        assert_eq!(scanner.finish("x", &[]).1.names_seen, 4);
    }
}
