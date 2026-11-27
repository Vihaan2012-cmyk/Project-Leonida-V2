//! Deriving replacement rules from ped appearance catalogue data.
//!
//! A catalogue (`0x8dcb4cf5`, see the upstream `ped_catalogues.md`) is a list of presets, each with states that name a
//! drawable, an albedo and a normal texture by 64-bit key. Another crate decodes the container; this module takes the
//! decoded states.
//!
//! Rules, per catalogue:
//! - a state whose drawable is a blocked nude patch (genitals, nipples, bare bodies, mannequins) is rewritten to an
//!   underwear-level drawable of the same gender from the **same catalogue**: pelvic patch to underwear, chest patch to
//!   a bra or bikini top. The modular drawables share one standing skeleton space, so a garment of another slot lands in
//!   place; whether the game's slot table accepts it was not measured.
//! - with no such garment in the catalogue, the state is emptied (drawable key 0, the value a disabled slot carries in
//!   559,139 of 896,115 measured states).
//! - base skin layers (`hand_*_nude_cull`, `feet_*_nude_cull`) are kept. They are reported with whether their
//!   catalogue has cover, because an outfit with no garment over them is not fixable from a catalogue alone.
//! - a texture is blocked only when every state that uses it carries a blocked drawable. The female nipple patch shares
//!   all four texture keys with the hands, so those are never blocked.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::blocklist::{BlockEntry, CatalogueRule, Replacement, ResourceKind};
use crate::hash::{hex64, hex64_opt};
use crate::names::{BodySlot, Category, Gender, NameClass, classify, name_keys, stem};

/// A decoded catalogue state (`0x37e52277`): its drawable, textures and cull variant, by 64-bit key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub preset_id: u32,
    #[serde(with = "hex64")]
    pub drawable: u64,
    #[serde(with = "hex64")]
    pub albedo: u64,
    #[serde(with = "hex64")]
    pub normal: u64,
    #[serde(with = "hex64_opt", default)]
    pub cull_drawable: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalogue {
    /// Murmur3-64 of the archetype name, or any other stable id for the catalogue.
    #[serde(with = "hex64")]
    pub id: u64,
    pub states: Vec<State>,
}

/// The file `leonida content scan --catalogues` reads: catalogues as a catalogue decoder exports them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogueFile {
    pub catalogues: Vec<Catalogue>,
}

/// 64-bit key to drawable name, for the names the scan recovered.
#[derive(Debug, Clone, Default)]
pub struct NameDictionary {
    names: HashMap<u64, String>,
}

impl NameDictionary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a name under every key spelling the game's data might use for it.
    pub fn insert(&mut self, name: &str) {
        let stem = stem(name);
        for key in name_keys(&stem).1 {
            self.names.insert(key, stem.clone());
        }
    }

    pub fn get(&self, key: u64) -> Option<&str> {
        self.names.get(&key).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// A base skin layer found in a catalogue and whether the catalogue has underwear to cover it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseBodyReport {
    pub catalogue: u64,
    pub slot: BodySlot,
    pub has_cover: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DerivedRules {
    pub entries: Vec<BlockEntry>,
    pub catalogue_rules: Vec<CatalogueRule>,
    pub base_bodies: Vec<BaseBodyReport>,
    /// Blocked states whose catalogue had no underwear to use, so they were emptied.
    pub emptied: usize,
    /// States whose drawable key is not in the name dictionary.
    pub unnamed_states: usize,
}

struct Blocked {
    name: String,
    category: Category,
    gender: Option<Gender>,
}

fn cover_candidate<'a>(
    covers: &'a [(String, Gender, &'a State)],
    slot_wanted: BodySlot,
    gender: Option<Gender>,
    names: &HashMap<u64, NameClass>,
) -> Option<&'a State> {
    covers
        .iter()
        .filter(|(_, g, state)| {
            names.get(&state.drawable) == Some(&NameClass::Cover(slot_wanted))
                && gender.is_none_or(|wanted| wanted == *g)
                && state.albedo != 0
        })
        .min_by_key(|(name, _, state)| (!name.contains("full"), name.clone(), state.preset_id))
        .map(|(_, _, state)| *state)
}

/// Derives the blocked drawables, their per-catalogue replacements and the textures only they use.
pub fn derive(
    catalogues: &[Catalogue],
    dict: &NameDictionary,
    extra_names: &[String],
) -> DerivedRules {
    let mut out = DerivedRules::default();
    let mut blocked: BTreeMap<u64, Blocked> = BTreeMap::new();
    let mut classes: HashMap<u64, NameClass> = HashMap::new();
    let mut texture_users: HashMap<u64, (usize, usize)> = HashMap::new();
    let mut texture_category: HashMap<u64, Category> = HashMap::new();

    fn consider(
        classes: &mut HashMap<u64, NameClass>,
        blocked: &mut BTreeMap<u64, Blocked>,
        key: u64,
        name: &str,
    ) {
        let info = classify(name);
        classes.insert(key, info.class);
        if let NameClass::Blocked(category) = info.class {
            blocked.insert(
                key,
                Blocked {
                    name: stem(name),
                    category,
                    gender: info.gender,
                },
            );
        }
    }
    for catalogue in catalogues {
        for state in &catalogue.states {
            if state.drawable == 0 || classes.contains_key(&state.drawable) {
                continue;
            }
            match dict.get(state.drawable) {
                Some(name) => consider(&mut classes, &mut blocked, state.drawable, name),
                None => {
                    classes.insert(state.drawable, NameClass::Other);
                }
            }
        }
    }
    let extra: Vec<(String, Vec<u64>)> = extra_names
        .iter()
        .map(|name| (name.clone(), name_keys(name).1))
        .collect();
    for (name, keys) in &extra {
        for key in keys {
            if !blocked.contains_key(key) && matches!(classify(name).class, NameClass::Blocked(_)) {
                consider(&mut classes, &mut blocked, *key, name);
            }
        }
    }

    for catalogue in catalogues {
        for state in &catalogue.states {
            if state.drawable != 0 && !dict.names.contains_key(&state.drawable) {
                out.unnamed_states += 1;
            }
            let is_blocked = blocked.contains_key(&state.drawable);
            for texture in [state.albedo, state.normal] {
                if texture != 0 {
                    let users = texture_users.entry(texture).or_default();
                    users.0 += 1;
                    if is_blocked {
                        users.1 += 1;
                        texture_category.insert(texture, blocked[&state.drawable].category);
                    }
                }
            }
        }
    }

    for catalogue in catalogues {
        let covers: Vec<(String, Gender, &State)> = catalogue
            .states
            .iter()
            .filter_map(|state| {
                let name = dict.get(state.drawable)?;
                let info = classify(name);
                matches!(info.class, NameClass::Cover(_)).then(|| {
                    (
                        name.to_string(),
                        info.gender.unwrap_or(Gender::Female),
                        state,
                    )
                })
            })
            .collect();
        for slot in [BodySlot::Upper, BodySlot::Lower] {
            let has_body = catalogue
                .states
                .iter()
                .any(|state| classes.get(&state.drawable) == Some(&NameClass::BaseBody(slot)));
            if has_body {
                out.base_bodies.push(BaseBodyReport {
                    catalogue: catalogue.id,
                    slot,
                    has_cover: covers.iter().any(|(_, _, state)| {
                        classes.get(&state.drawable) == Some(&NameClass::Cover(slot))
                    }),
                });
            }
        }
        let mut seen: HashSet<(u64, u64, u64)> = HashSet::new();
        for state in &catalogue.states {
            let Some(block) = blocked.get(&state.drawable) else {
                continue;
            };
            if !seen.insert((state.drawable, state.albedo, state.normal)) {
                continue;
            }
            let wanted = match block.category {
                Category::Genitals => Some(BodySlot::Lower),
                Category::Nipples => Some(BodySlot::Upper),
                Category::BareBody | Category::Mannequin => None,
            };
            let replacement = wanted
                .and_then(|slot| cover_candidate(&covers, slot, block.gender, &classes))
                .map(|cover| Replacement::UseState {
                    drawable: cover.drawable,
                    albedo: cover.albedo,
                    normal: cover.normal,
                    cull_drawable: cover.cull_drawable,
                })
                .unwrap_or(Replacement::Hide);
            if replacement == Replacement::Hide {
                out.emptied += 1;
            }
            out.catalogue_rules.push(CatalogueRule {
                catalogue: catalogue.id,
                drawable: state.drawable,
                albedo: state.albedo,
                normal: state.normal,
                replacement,
            });
        }
    }

    let mut by_name: BTreeMap<&str, (&Blocked, Vec<u64>)> = BTreeMap::new();
    for (key, block) in &blocked {
        by_name
            .entry(&block.name)
            .or_insert((block, Vec::new()))
            .1
            .push(*key);
    }
    for (block, extra_keys) in by_name.values() {
        let (hash32, mut keys) = name_keys(&block.name);
        for key in extra_keys {
            if !keys.contains(key) {
                keys.push(*key);
            }
        }
        out.entries.push(BlockEntry {
            kind: ResourceKind::Drawable,
            category: block.category,
            name: Some(block.name.clone()),
            name_hash32: Some(hash32),
            keys64: keys,
            content: Vec::new(),
            replacement: Replacement::Hide,
        });
    }
    let mut exclusive: Vec<(u64, Category)> = texture_users
        .iter()
        .filter(|(_, (all, blocked))| all == blocked)
        .map(|(key, _)| (*key, texture_category[key]))
        .collect();
    exclusive.sort_unstable();
    for (key, category) in exclusive {
        out.entries.push(BlockEntry {
            kind: ResourceKind::Texture,
            category,
            name: None,
            name_hash32: None,
            keys64: vec![key],
            content: Vec::new(),
            replacement: Replacement::Neutral,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(names: &[&str]) -> NameDictionary {
        let mut dict = NameDictionary::new();
        for name in names {
            dict.insert(name);
        }
        dict
    }

    fn key(name: &str) -> u64 {
        name_keys(name).1[0]
    }

    fn state(preset: u32, name: &str, albedo: u64, normal: u64) -> State {
        State {
            preset_id: preset,
            drawable: key(name),
            albedo,
            normal,
            cull_drawable: None,
        }
    }

    const GEN_F: &str = "bodygenitalsgen_fr1_008_regular_ws00_full";
    const NIP_F: &str = "bodynipplesgen_fr1_000_regular_ws00_full";
    const UNDIES_F: &str = "undiesgstring_fr1_002_slim_hw_full";
    const UNDIES_M: &str = "undiesboxers_mr1_000_regular_full";
    const BRA_F: &str = "brabikini_fr1_012_regular_ws00_full";
    const HAND_F: &str = "hand_fr1_000_nude_cull";
    const JEANS: &str = "pantsjeans_fr1_002_regular_hw_mid_full";

    fn female_catalogue() -> Catalogue {
        Catalogue {
            id: 1,
            states: vec![
                state(1, HAND_F, 0xA1, 0xB1),
                state(2, NIP_F, 0xA1, 0xB1),
                state(3, GEN_F, 0xA3, 0xB3),
                state(4, UNDIES_F, 0xC4, 0xD4),
                state(5, BRA_F, 0xC5, 0xD5),
                state(6, JEANS, 0xC6, 0xD6),
            ],
        }
    }

    #[test]
    fn genital_and_nipple_patches_are_swapped_for_underwear_of_the_same_catalogue() {
        let names = [GEN_F, NIP_F, UNDIES_F, UNDIES_M, BRA_F, HAND_F, JEANS];
        let rules = derive(&[female_catalogue()], &dict(&names), &[]);
        let by_drawable = |name: &str| {
            rules
                .catalogue_rules
                .iter()
                .find(|rule| rule.drawable == key(name))
                .unwrap()
        };
        let genitals = by_drawable(GEN_F);
        assert_eq!(
            genitals.replacement,
            Replacement::UseState {
                drawable: key(UNDIES_F),
                albedo: 0xC4,
                normal: 0xD4,
                cull_drawable: None
            }
        );
        let nipples = by_drawable(NIP_F);
        assert!(matches!(
            nipples.replacement,
            Replacement::UseState { drawable, albedo: 0xC5, .. } if drawable == key(BRA_F)
        ));
        assert_eq!(rules.catalogue_rules.len(), 2);
        assert_eq!(rules.emptied, 0);
    }

    #[test]
    fn gender_of_the_replacement_follows_the_blocked_patch() {
        let male_gen = "bodygenitalsgen_mr1_000_regular_ws00_full";
        let catalogue = Catalogue {
            id: 2,
            states: vec![
                state(1, male_gen, 0x11, 0x12),
                state(2, UNDIES_F, 0x21, 0x22),
                state(3, UNDIES_M, 0x31, 0x32),
            ],
        };
        let rules = derive(&[catalogue], &dict(&[male_gen, UNDIES_F, UNDIES_M]), &[]);
        assert!(matches!(
            rules.catalogue_rules[0].replacement,
            Replacement::UseState { drawable, .. } if drawable == key(UNDIES_M)
        ));
    }

    #[test]
    fn no_cover_in_the_catalogue_means_the_slot_is_emptied() {
        let catalogue = Catalogue {
            id: 3,
            states: vec![state(1, GEN_F, 0x1, 0x2), state(2, JEANS, 0x3, 0x4)],
        };
        let rules = derive(&[catalogue], &dict(&[GEN_F, JEANS]), &[]);
        assert_eq!(rules.catalogue_rules.len(), 1);
        assert_eq!(rules.catalogue_rules[0].replacement, Replacement::Hide);
        assert_eq!(rules.emptied, 1);
    }

    #[test]
    fn scene_monsters_and_mannequins_are_hidden_not_dressed() {
        let monster = "sc_fast_demon_bodylowergen_fr1_000_m_full";
        let catalogue = Catalogue {
            id: 4,
            states: vec![state(1, monster, 0x1, 0x2), state(2, UNDIES_F, 0x3, 0x4)],
        };
        let rules = derive(&[catalogue], &dict(&[monster, UNDIES_F]), &[]);
        assert_eq!(rules.catalogue_rules[0].replacement, Replacement::Hide);
    }

    #[test]
    fn base_bodies_are_kept_and_reported_with_their_cover() {
        let rules = derive(
            &[female_catalogue()],
            &dict(&[GEN_F, NIP_F, UNDIES_F, BRA_F, HAND_F, JEANS]),
            &[],
        );
        assert!(
            rules
                .entries
                .iter()
                .all(|entry| entry.name.as_deref() != Some(HAND_F))
        );
        assert_eq!(
            rules.base_bodies,
            vec![BaseBodyReport {
                catalogue: 1,
                slot: BodySlot::Upper,
                has_cover: true
            }]
        );
        let bare = Catalogue {
            id: 5,
            states: vec![state(1, HAND_F, 1, 2)],
        };
        let rules = derive(&[bare], &dict(&[HAND_F]), &[]);
        assert!(!rules.base_bodies[0].has_cover);
    }

    #[test]
    fn only_textures_used_by_blocked_drawables_alone_are_blocked() {
        let names = [GEN_F, NIP_F, UNDIES_F, BRA_F, HAND_F, JEANS];
        let rules = derive(&[female_catalogue()], &dict(&names), &[]);
        let blocked_textures: Vec<u64> = rules
            .entries
            .iter()
            .filter(|entry| entry.kind == ResourceKind::Texture)
            .flat_map(|entry| entry.keys64.clone())
            .collect();
        assert_eq!(blocked_textures, vec![0xA3, 0xB3]);
    }

    #[test]
    fn drawables_in_the_dictionary_but_not_in_a_catalogue_are_still_listed() {
        let rules = derive(
            &[],
            &NameDictionary::new(),
            &[
                "gmt_mannequin_proxy_f_01a.#dr".to_string(),
                "hand_fr1_000_nude_cull.#dr".to_string(),
            ],
        );
        let names: Vec<_> = rules
            .entries
            .iter()
            .filter_map(|entry| entry.name.clone())
            .collect();
        assert_eq!(names, vec!["gmt_mannequin_proxy_f_01a"]);
    }

    #[test]
    fn the_catalogue_file_round_trips_through_json() {
        let file = CatalogueFile {
            catalogues: vec![female_catalogue()],
        };
        let text = serde_json::to_string(&file).unwrap();
        assert!(text.contains("0x"));
        let back: CatalogueFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.catalogues[0].states, file.catalogues[0].states);
    }

    #[test]
    fn unnamed_states_are_counted_and_left_alone() {
        let catalogue = Catalogue {
            id: 6,
            states: vec![State {
                preset_id: 1,
                drawable: 0xDEAD,
                albedo: 1,
                normal: 2,
                cull_drawable: None,
            }],
        };
        let rules = derive(&[catalogue], &NameDictionary::new(), &[]);
        assert_eq!(rules.unnamed_states, 1);
        assert!(rules.entries.is_empty() && rules.catalogue_rules.is_empty());
    }
}
