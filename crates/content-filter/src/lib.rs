//! Hides nudity in the running game and nothing else: gore, violence and language are not touched.
//!
//! Pieces:
//! - `settings`: `[content] nudity = "block" | "allow"` in Config/leonida.toml (block by default).
//! - `names`, `rules`: what counts as nudity, derived from drawable names and ped catalogue data, and what to use
//!   instead (an underwear layer from the same catalogue, or hiding the drawable).
//! - `archive`, `scan`: the offline scan that finds those resources in the player's archives and writes the blocklist.
//! - `blocklist`: the blocklist file (names, name hashes, content hashes) and its lookups. Derived from game data:
//!   it lives in `Output/` and is never committed.
//! - `filter`: `ContentFilter`, the hook the console D3D12 layer calls on resource creation, upload, bind and draw.
//! - `vfs_link`: the `ResourceFilter` trait the `vfs` content-filter layer calls, and catalogue rewriting.
//!
//! Measured (upstream documentation): the modular body parts and their names (`bodygenitalsgen_*`,
//! `bodynipplesgen_*`, `hand_*_nude_cull`, `feet_*_nude_cull`, `undies*`, `brabikini*`), their shared standing skeleton
//! space, the catalogue state layout (drawable, albedo, normal as consecutive 64-bit keys), and that the nipple patch
//! shares all its textures with the hands.
//!
//! Assumed (nothing about the running game was measured; it is never run): that a catalogue state with drawable key 0
//! is an empty slot, that a garment of another slot can fill a patch's slot, that the 64-bit keys hash the lower-cased
//! name (with or without its suffix), that the bytes the game uploads equal the inflated stream bytes the scan hashed,
//! that the unnamed streams after a named one are its pages, and that unclothed female mannequins and scene-monster
//! bodies count as nudity. Male chests are not treated as nudity. Which scenes, if any, show nude characters is not in
//! the documentation, so there are no scene rules.

pub mod archive;
pub mod blocklist;
pub mod hash;
pub mod names;
pub mod rules;
pub mod settings;
