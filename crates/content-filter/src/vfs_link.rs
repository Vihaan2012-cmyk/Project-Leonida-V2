//! What the `vfs` crate's content-filter layer calls.
//!
//! `vfs` is written in parallel and does not depend on this crate. It defines (or is given) a layer that sees every
//! resource it serves; the small trait here is the whole contract, so either crate can adapt the other without a type
//! from the other side.
//!
//! To connect: `vfs` builds the layer with `Box<dyn ResourceFilter>` (the `ContentFilter` below implements it) and,
//! for each resource it is about to serve, fills a `ResourceIdentity` from what it knows (name, name hashes, kind, the
//! catalogue's id) and calls `filter_resource(&identity, &mut bytes)`:
//! - `Unchanged`: serve the bytes as they are.
//! - `Rewritten`: serve the modified bytes (same length; a ped catalogue with blocked drawable keys replaced).
//! - `Hidden`: serve the resource as absent (a blocked drawable, fragment or texture).

use crate::blocklist::{BlockEntry, Replacement, ResourceKind};
use crate::filter::ContentFilter;
use crate::hash::ContentHash;

#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceIdentity<'a> {
    pub kind: Option<ResourceKind>,
    pub name: Option<&'a str>,
    pub name_hash32: Option<u32>,
    pub key64: Option<u64>,
    /// For a ped catalogue: the id the catalogue rules were derived under (the Murmur3-64 of its archetype name).
    pub catalogue: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    Unchanged,
    Rewritten,
    Hidden,
}

pub trait ResourceFilter: Send + Sync {
    fn filter_resource(
        &self,
        identity: &ResourceIdentity<'_>,
        bytes: &mut Vec<u8>,
    ) -> FilterOutcome;
}

/// How many words of a catalogue buffer were changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PatchStats {
    pub drawables_replaced: usize,
    pub drawables_emptied: usize,
    pub textures_replaced: usize,
}

fn word(bytes: &[u8], at: usize) -> Option<u64> {
    bytes
        .get(at..at + 8)
        .map(|raw| u64::from_le_bytes(raw.try_into().unwrap()))
}

fn put(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

impl ContentFilter {
    /// Rewrites the blocked drawable keys in the inflated bytes of a ped catalogue.
    ///
    /// A state stores its drawable, albedo and normal keys as three consecutive 64-bit words (state `+0x08..+0x20`). The
    /// buffer is searched at 8-byte steps for blocked drawable keys; 64-bit hashes make a false hit negligible.
    /// A word that begins a state recorded in the rules (same albedo and normal words after it) takes the replacement
    /// drawable and its textures; any other hit, such as the `cull_drawable` record that holds the key alone, takes the
    /// replacement drawable (or 0) only. Lengths never change.
    pub fn patch_catalogue(&self, catalogue: Option<u64>, bytes: &mut [u8]) -> PatchStats {
        let mut stats = PatchStats::default();
        let index = self.index();
        if !self.is_active() || !index.has_blocked_drawables() {
            return stats;
        }
        let mut at = 0;
        while let Some(key) = word(bytes, at) {
            if key != 0
                && let Some(default) = index.blocked_drawable(key)
            {
                let textures = word(bytes, at + 8).zip(word(bytes, at + 16));
                let exact = catalogue
                    .zip(textures)
                    .and_then(|(catalogue, (albedo, normal))| {
                        index.catalogue_rule(catalogue, key, albedo, normal)
                    });
                let chosen = exact
                    .or_else(|| {
                        catalogue
                            .and_then(|catalogue| index.catalogue_rule_for_drawable(catalogue, key))
                    })
                    .unwrap_or(default);
                match chosen {
                    Replacement::UseState {
                        drawable,
                        albedo,
                        normal,
                        ..
                    } => {
                        put(bytes, at, drawable);
                        stats.drawables_replaced += 1;
                        if exact.is_some() {
                            put(bytes, at + 8, albedo);
                            put(bytes, at + 16, normal);
                            stats.textures_replaced += 1;
                        }
                    }
                    _ => {
                        put(bytes, at, 0);
                        stats.drawables_emptied += 1;
                    }
                }
            }
            at += 8;
        }
        stats
    }

    fn blocked_entry(&self, identity: &ResourceIdentity<'_>, bytes: &[u8]) -> Option<&BlockEntry> {
        let index = self.index();
        identity
            .key64
            .and_then(|key| index.by_key(key))
            .or_else(|| {
                identity
                    .name_hash32
                    .and_then(|hash| index.by_name_hash(hash))
            })
            .or_else(|| {
                (index.has_content() && index.may_match_length(bytes.len() as u64))
                    .then(|| index.by_content(&ContentHash::of(bytes)))
                    .flatten()
            })
    }
}

impl ResourceFilter for ContentFilter {
    fn filter_resource(
        &self,
        identity: &ResourceIdentity<'_>,
        bytes: &mut Vec<u8>,
    ) -> FilterOutcome {
        if !self.is_active() {
            return FilterOutcome::Unchanged;
        }
        if identity.kind == Some(ResourceKind::Catalogue) {
            let stats = self.patch_catalogue(identity.catalogue, bytes);
            return if stats == PatchStats::default() {
                FilterOutcome::Unchanged
            } else {
                FilterOutcome::Rewritten
            };
        }
        if self.blocked_entry(identity, bytes).is_some() {
            FilterOutcome::Hidden
        } else {
            FilterOutcome::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::fixtures::drawable_entry;
    use crate::blocklist::{Blocklist, CatalogueRule};
    use crate::names::{Category, name_keys};
    use crate::settings::{ContentSettings, NudityMode};

    const GEN: &str = "bodygenitalsgen_fr1_008_regular_ws00_full";

    fn key(name: &str) -> u64 {
        name_keys(name).1[0]
    }

    fn filter(mode: NudityMode) -> ContentFilter {
        let mut list = Blocklist::new("1.0.2.1");
        list.entries
            .push(drawable_entry(GEN, Category::Genitals, Replacement::Hide));
        list.catalogue_rules.push(CatalogueRule {
            catalogue: 5,
            drawable: key(GEN),
            albedo: 0xA3,
            normal: 0xB3,
            replacement: Replacement::UseState {
                drawable: 0x77,
                albedo: 0xC4,
                normal: 0xD4,
                cull_drawable: None,
            },
        });
        ContentFilter::new(ContentSettings { nudity: mode }, &list)
    }

    fn words(values: &[u64]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn read(bytes: &[u8]) -> Vec<u64> {
        bytes
            .chunks_exact(8)
            .map(|raw| u64::from_le_bytes(raw.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn a_catalogue_state_with_a_recorded_rule_is_replaced_with_its_textures() {
        let filter = filter(NudityMode::Block);
        let mut bytes = words(&[1, key(GEN), 0xA3, 0xB3, 0x20, 9]);
        let stats = filter.patch_catalogue(Some(5), &mut bytes);
        assert_eq!(read(&bytes), vec![1, 0x77, 0xC4, 0xD4, 0x20, 9]);
        assert_eq!(stats.drawables_replaced, 1);
        assert_eq!(stats.textures_replaced, 1);
    }

    #[test]
    fn a_lone_key_such_as_a_cull_record_changes_only_the_key() {
        let filter = filter(NudityMode::Block);
        let mut bytes = words(&[0, key(GEN), 0x1111, 0x2222]);
        filter.patch_catalogue(Some(5), &mut bytes);
        assert_eq!(read(&bytes), vec![0, 0x77, 0x1111, 0x2222]);
    }

    #[test]
    fn an_unknown_catalogue_empties_the_slot() {
        let filter = filter(NudityMode::Block);
        let mut bytes = words(&[key(GEN), 0xA3, 0xB3]);
        let stats = filter.patch_catalogue(None, &mut bytes);
        assert_eq!(read(&bytes), vec![0, 0xA3, 0xB3]);
        assert_eq!(stats.drawables_emptied, 1);
    }

    #[test]
    fn allow_mode_leaves_everything_alone() {
        let filter = filter(NudityMode::Allow);
        let original = words(&[key(GEN), 0xA3, 0xB3]);
        let mut bytes = original.clone();
        let identity = ResourceIdentity {
            kind: Some(ResourceKind::Catalogue),
            catalogue: Some(5),
            ..Default::default()
        };
        assert_eq!(
            filter.filter_resource(&identity, &mut bytes),
            FilterOutcome::Unchanged
        );
        assert_eq!(bytes, original);
    }

    #[test]
    fn the_trait_rewrites_catalogues_and_hides_blocked_resources() {
        let filter: Box<dyn ResourceFilter> = Box::new(filter(NudityMode::Block));
        let mut bytes = words(&[key(GEN), 0xA3, 0xB3]);
        let catalogue = ResourceIdentity {
            kind: Some(ResourceKind::Catalogue),
            catalogue: Some(5),
            ..Default::default()
        };
        assert_eq!(
            filter.filter_resource(&catalogue, &mut bytes),
            FilterOutcome::Rewritten
        );
        let mut other = words(&[1, 2, 3]);
        assert_eq!(
            filter.filter_resource(&catalogue, &mut other),
            FilterOutcome::Unchanged
        );

        let drawable = ResourceIdentity {
            kind: Some(ResourceKind::Drawable),
            key64: Some(key(GEN)),
            ..Default::default()
        };
        assert_eq!(
            filter.filter_resource(&drawable, &mut Vec::new()),
            FilterOutcome::Hidden
        );
        let by_hash = ResourceIdentity {
            name_hash32: Some(name_keys(GEN).0),
            ..Default::default()
        };
        assert_eq!(
            filter.filter_resource(&by_hash, &mut Vec::new()),
            FilterOutcome::Hidden
        );
        let unrelated = ResourceIdentity {
            key64: Some(1),
            ..Default::default()
        };
        assert_eq!(
            filter.filter_resource(&unrelated, &mut Vec::new()),
            FilterOutcome::Unchanged
        );
    }
}
