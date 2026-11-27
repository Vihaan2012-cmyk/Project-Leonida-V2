//! The runtime hook the console D3D12 layer calls.
//!
//! Descriptors (T#, V#, S# words) carry only addresses, never a name or a hash, so a draw's textures and buffers
//! cannot be identified when they are bound. The only moments where identity is available are resource creation (when
//! the loader can tag the resource with the asset key it is streaming) and upload (when the bytes are in hand). The
//! filter therefore decides at those moments, remembers the decision against the resource's GPU address range, and
//! enforces it at bind and draw time by address, which also covers descriptor copies and re-created descriptors.
//!
//! Matching routes, in order of reliability:
//! 1. `ResourceTag`: the loader names the asset (64-bit key or 32-bit name hash). Exact, needs the loader to say.
//! 2. Content hash at upload: the payload of one upload, or of sequential uploads that together fill a resource
//!    whose size equals a blocked payload's length. Reliable only if the bytes the game uploads equal the inflated
//!    stream bytes the scan hashed; that was not measured (the game is never run).
//! 3. Catalogue rewriting (`crate::vfs_link`): the game never asks for the blocked drawable at all. Independent of 1 and 2.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::blocklist::{BlockEntry, BlockIndex, Blocklist, ResourceKind};
use crate::hash::ContentHash;
use crate::settings::ContentSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    Buffer,
    Texture,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceDesc {
    pub id: ResourceId,
    pub kind: GpuKind,
    pub size: u64,
    /// The console GPU address the resource is mapped at, when it has one.
    pub gpu_address: Option<u64>,
}

/// Identity the loader knows about a resource it is about to fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceTag {
    Key64(u64),
    NameHash32(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Skip every draw that reads this buffer.
    Hide,
    /// Bind a neutral flat texture in its place.
    NeutralTexture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawDecision {
    Draw,
    Skip,
}

#[derive(Debug, Default)]
struct Pending {
    hasher: Option<Sha256>,
    next_offset: u64,
}

#[derive(Debug)]
struct Tracked {
    desc: ResourceDesc,
    pending: Pending,
    verdict: Verdict,
    ranges: Vec<u64>,
}

#[derive(Debug)]
struct Range {
    end: u64,
    verdict: Verdict,
}

#[derive(Debug, Default)]
struct State {
    resources: HashMap<ResourceId, Tracked>,
    ranges: BTreeMap<u64, Range>,
}

pub struct ContentFilter {
    active: bool,
    index: BlockIndex,
    state: Mutex<State>,
}

fn verdict_of(entry: &BlockEntry) -> Verdict {
    match entry.kind {
        ResourceKind::Texture => Verdict::NeutralTexture,
        _ => Verdict::Hide,
    }
}

impl ContentFilter {
    /// A filter that does nothing: nudity set to `allow`, or no blocklist.
    pub fn disabled() -> Self {
        Self {
            active: false,
            index: BlockIndex::default(),
            state: Mutex::default(),
        }
    }

    pub fn new(settings: ContentSettings, blocklist: &Blocklist) -> Self {
        if !settings.blocks_nudity() || blocklist.entries.is_empty() {
            return Self::disabled();
        }
        Self {
            active: true,
            index: blocklist.index(),
            state: Mutex::default(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn index(&self) -> &BlockIndex {
        &self.index
    }

    /// The game created a resource. `tag` is what the loader knows about it, if anything.
    pub fn on_create(&self, desc: ResourceDesc, tag: Option<ResourceTag>) -> Verdict {
        if !self.active {
            return Verdict::Allow;
        }
        let verdict = tag
            .and_then(|tag| match tag {
                ResourceTag::Key64(key) => self.index.by_key(key),
                ResourceTag::NameHash32(hash) => self.index.by_name_hash(hash),
            })
            .map_or(Verdict::Allow, verdict_of);
        let mut state = self.state.lock().unwrap();
        let mut tracked = Tracked {
            desc,
            pending: Pending::default(),
            verdict,
            ranges: Vec::new(),
        };
        if verdict != Verdict::Allow {
            if let Some(start) = desc.gpu_address {
                state.ranges.insert(
                    start,
                    Range {
                        end: start + desc.size.max(1),
                        verdict,
                    },
                );
                tracked.ranges.push(start);
            }
        } else if self.index.has_content() && self.index.may_match_length(desc.size) {
            tracked.pending.hasher = Some(Sha256::new());
        }
        state.resources.insert(desc.id, tracked);
        verdict
    }

    /// The game wrote `data` at `offset` into a resource, by copy or upload. Returns the resource's verdict.
    pub fn on_upload(&self, id: ResourceId, offset: u64, data: &[u8]) -> Verdict {
        if !self.active {
            return Verdict::Allow;
        }
        let mut state = self.state.lock().unwrap();
        let State { resources, ranges } = &mut *state;
        let Some(tracked) = resources.get_mut(&id) else {
            return Verdict::Allow;
        };
        if tracked.verdict != Verdict::Allow {
            return tracked.verdict;
        }
        let len = data.len() as u64;
        let mut found = None;
        if self.index.may_match_length(len) {
            found = self
                .index
                .by_content(&ContentHash::of(data))
                .map(|entry| (offset, len, verdict_of(entry)));
        }
        if found.is_none()
            && let Some(hasher) = tracked.pending.hasher.as_mut()
        {
            if offset == tracked.pending.next_offset {
                hasher.update(data);
                tracked.pending.next_offset += len;
                if tracked.pending.next_offset == tracked.desc.size {
                    let digest = tracked.pending.hasher.take().unwrap().finalize();
                    found = self
                        .index
                        .by_content(&ContentHash::from_digest(digest.into()))
                        .map(|entry| (0, tracked.desc.size, verdict_of(entry)));
                }
            } else {
                tracked.pending.hasher = None;
            }
        }
        let Some((from, length, verdict)) = found else {
            return Verdict::Allow;
        };
        if from == 0 && length == tracked.desc.size {
            tracked.verdict = verdict;
        }
        if let Some(base) = tracked.desc.gpu_address {
            ranges.insert(
                base + from,
                Range {
                    end: base + from + length.max(1),
                    verdict,
                },
            );
            tracked.ranges.push(base + from);
        }
        verdict
    }

    /// The game freed a resource.
    pub fn on_release(&self, id: ResourceId) {
        if !self.active {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if let Some(tracked) = state.resources.remove(&id) {
            for start in tracked.ranges {
                state.ranges.remove(&start);
            }
        }
    }

    /// The verdict for a whole resource, by id.
    pub fn resource_verdict(&self, id: ResourceId) -> Verdict {
        if !self.active {
            return Verdict::Allow;
        }
        let state = self.state.lock().unwrap();
        state
            .resources
            .get(&id)
            .map_or(Verdict::Allow, |tracked| tracked.verdict)
    }

    /// The verdict for whatever a descriptor or buffer address points into.
    pub fn address_verdict(&self, address: u64) -> Verdict {
        if !self.active {
            return Verdict::Allow;
        }
        let state = self.state.lock().unwrap();
        state
            .ranges
            .range(..=address)
            .next_back()
            .filter(|(_, range)| address < range.end)
            .map_or(Verdict::Allow, |(_, range)| range.verdict)
    }

    /// A texture is bound for a draw, through a descriptor holding `address`.
    pub fn on_bind_texture(&self, address: u64) -> Verdict {
        self.address_verdict(address)
    }

    /// A draw is about to be recorded with these vertex, index and mesh buffer addresses. A draw reading any hidden
    /// buffer is skipped.
    pub fn on_draw(&self, buffer_addresses: &[u64]) -> DrawDecision {
        if buffer_addresses
            .iter()
            .any(|address| self.address_verdict(*address) == Verdict::Hide)
        {
            DrawDecision::Skip
        } else {
            DrawDecision::Draw
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::fixtures::drawable_entry;
    use crate::blocklist::{ContentRecord, Replacement};
    use crate::hash::murmur64;
    use crate::names::{Category, name_keys};
    use crate::settings::NudityMode;

    const PAGE: &[u8] = b"synthetic vertex page of a blocked drawable, 0123456789abcdef";

    fn blocklist() -> Blocklist {
        let mut list = Blocklist::new("1.0.2.1");
        let mut drawable = drawable_entry(
            "bodygenitalsgen_fr1_008_regular_ws00_full",
            Category::Genitals,
            Replacement::Hide,
        );
        drawable.content.push(ContentRecord {
            hash: ContentHash::of(PAGE),
            len: PAGE.len() as u64,
        });
        list.entries.push(drawable);
        list.entries.push(BlockEntry {
            kind: ResourceKind::Texture,
            category: Category::Genitals,
            name: None,
            name_hash32: None,
            keys64: vec![0x5B68_29CF_EC0F_41A1],
            content: Vec::new(),
            replacement: Replacement::Neutral,
        });
        list
    }

    fn filter() -> ContentFilter {
        ContentFilter::new(ContentSettings::default(), &blocklist())
    }

    fn buffer(id: u64, size: u64, address: u64) -> ResourceDesc {
        ResourceDesc {
            id: ResourceId(id),
            kind: GpuKind::Buffer,
            size,
            gpu_address: Some(address),
        }
    }

    #[test]
    fn a_tagged_drawable_is_hidden_at_creation_and_its_draws_are_skipped() {
        let filter = filter();
        let key = name_keys("bodygenitalsgen_fr1_008_regular_ws00_full").1[0];
        let verdict = filter.on_create(buffer(1, 4096, 0x1000), Some(ResourceTag::Key64(key)));
        assert_eq!(verdict, Verdict::Hide);
        assert_eq!(filter.on_draw(&[0x1000]), DrawDecision::Skip);
        assert_eq!(filter.on_draw(&[0x1000 + 4095]), DrawDecision::Skip);
        assert_eq!(filter.on_draw(&[0x1000 + 4096]), DrawDecision::Draw);
        assert_eq!(filter.on_draw(&[0x0FFF, 0x9000]), DrawDecision::Draw);
        filter.on_release(ResourceId(1));
        assert_eq!(filter.on_draw(&[0x1000]), DrawDecision::Draw);
    }

    #[test]
    fn a_tagged_texture_gets_the_neutral_texture_at_bind() {
        let filter = filter();
        let desc = ResourceDesc {
            id: ResourceId(2),
            kind: GpuKind::Texture,
            size: 1 << 16,
            gpu_address: Some(0x40_0000),
        };
        let verdict = filter.on_create(desc, Some(ResourceTag::Key64(0x5B68_29CF_EC0F_41A1)));
        assert_eq!(verdict, Verdict::NeutralTexture);
        assert_eq!(filter.on_bind_texture(0x40_0100), Verdict::NeutralTexture);
        assert_eq!(filter.on_draw(&[0x40_0100]), DrawDecision::Draw);
    }

    #[test]
    fn an_untagged_resource_is_matched_by_the_content_of_one_upload() {
        let filter = filter();
        assert_eq!(
            filter.on_create(buffer(3, PAGE.len() as u64, 0x8000), None),
            Verdict::Allow
        );
        assert_eq!(filter.on_upload(ResourceId(3), 0, PAGE), Verdict::Hide);
        assert_eq!(filter.resource_verdict(ResourceId(3)), Verdict::Hide);
        assert_eq!(filter.on_draw(&[0x8000 + 10]), DrawDecision::Skip);
    }

    #[test]
    fn chunked_sequential_uploads_are_hashed_as_one_payload() {
        let filter = filter();
        filter.on_create(buffer(4, PAGE.len() as u64, 0x8000), None);
        let (first, second) = PAGE.split_at(20);
        assert_eq!(filter.on_upload(ResourceId(4), 0, first), Verdict::Allow);
        assert_eq!(filter.on_upload(ResourceId(4), 20, second), Verdict::Hide);
        assert_eq!(filter.address_verdict(0x8000), Verdict::Hide);
    }

    #[test]
    fn out_of_order_chunks_do_not_match_and_other_payloads_are_allowed() {
        let filter = filter();
        filter.on_create(buffer(5, PAGE.len() as u64, 0x8000), None);
        let (first, second) = PAGE.split_at(20);
        assert_eq!(filter.on_upload(ResourceId(5), 20, second), Verdict::Allow);
        assert_eq!(filter.on_upload(ResourceId(5), 0, first), Verdict::Allow);
        filter.on_create(buffer(6, 64, 0xA000), None);
        assert_eq!(
            filter.on_upload(ResourceId(6), 0, &[7u8; 64]),
            Verdict::Allow
        );
        assert_eq!(filter.on_draw(&[0x8000, 0xA000]), DrawDecision::Draw);
    }

    #[test]
    fn a_blocked_page_inside_a_larger_resource_only_blocks_its_own_range() {
        let filter = filter();
        filter.on_create(buffer(7, 1 << 20, 0x10_0000), None);
        assert_eq!(filter.on_upload(ResourceId(7), 4096, PAGE), Verdict::Hide);
        assert_eq!(filter.resource_verdict(ResourceId(7)), Verdict::Allow);
        assert_eq!(filter.on_draw(&[0x10_0000 + 4096 + 3]), DrawDecision::Skip);
        assert_eq!(filter.on_draw(&[0x10_0000 + 100]), DrawDecision::Draw);
    }

    #[test]
    fn allow_mode_and_an_empty_blocklist_do_nothing() {
        let settings = ContentSettings {
            nudity: NudityMode::Allow,
        };
        let filter = ContentFilter::new(settings, &blocklist());
        assert!(!filter.is_active());
        let key = murmur64(b"bodygenitalsgen_fr1_008_regular_ws00_full");
        assert_eq!(
            filter.on_create(buffer(1, 16, 0x1000), Some(ResourceTag::Key64(key))),
            Verdict::Allow
        );
        assert_eq!(filter.on_draw(&[0x1000]), DrawDecision::Draw);
        assert!(!ContentFilter::new(ContentSettings::default(), &Blocklist::new("x")).is_active());
    }

    #[test]
    fn unknown_resources_and_ids_are_allowed() {
        let filter = filter();
        assert_eq!(filter.on_upload(ResourceId(99), 0, b"x"), Verdict::Allow);
        assert_eq!(filter.resource_verdict(ResourceId(99)), Verdict::Allow);
        assert_eq!(
            filter.on_create(buffer(8, 16, 0x2000), Some(ResourceTag::Key64(1))),
            Verdict::Allow
        );
        assert_eq!(
            filter.on_create(buffer(9, 16, 0x3000), Some(ResourceTag::NameHash32(5))),
            Verdict::Allow
        );
    }

    #[test]
    fn name_hash_tags_work_too() {
        let filter = filter();
        let hash = name_keys("bodygenitalsgen_fr1_008_regular_ws00_full").0;
        assert_eq!(
            filter.on_create(buffer(10, 16, 0x5000), Some(ResourceTag::NameHash32(hash))),
            Verdict::Hide
        );
    }
}
