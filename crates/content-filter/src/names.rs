//! Reading what a drawable is from its name.
//!
//! Names follow `<item>_<rig>_<variant>_..._<lod>`, with an optional character prefix (`player_story000_00_`,
//! `cs_lucia_`, `sc_`). The item tokens used here are the ones the upstream character documentation names; the game's
//! own component table is not decoded, so this is a naming rule, not the game's.

use serde::{Deserialize, Serialize};

use crate::hash::{joaat_lower, murmur64};

/// What kind of thing is hidden, for the blocklist and the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// `bodygenitalsgen_*`: the modular pelvic patches.
    Genitals,
    /// `bodynipplesgen_*`: the modular chest patch.
    Nipples,
    /// `bodyuppergen_*` / `bodylowergen_*` bare bodies of scene characters.
    BareBody,
    /// `gmt_mannequin*_f_*`: the unclothed female mannequin forms.
    Mannequin,
}

/// How firmly the category rests on what the upstream documentation measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// The documentation names these drawables and measures their geometry as the nude parts.
    Measured,
    /// Chosen by a naming rule; the geometry was measured as bare but how the game shows it was not.
    Assumed,
}

impl Category {
    pub fn evidence(self) -> Evidence {
        match self {
            Category::Genitals | Category::Nipples => Evidence::Measured,
            Category::BareBody | Category::Mannequin => Evidence::Assumed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Female,
    Male,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodySlot {
    Upper,
    Lower,
}

/// What a name says about a drawable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameClass {
    /// Shows nudity by itself.
    Blocked(Category),
    /// The base skin layer (`hand_<rig>_000_nude_cull` upper, `feet_<rig>_000_nude_cull` lower). It sits under every
    /// outfit and is not hidden: that would remove arms and legs from every character.
    BaseBody(BodySlot),
    /// Underwear-level clothing, usable as a replacement for the slot.
    Cover(BodySlot),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameInfo {
    pub class: NameClass,
    pub gender: Option<Gender>,
}

/// Lower-cased name without the resource suffix (`.#dr`, `.#ft`, `.#dd`).
pub fn stem(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.rfind(".#") {
        Some(at) if lower.len() - at <= 4 => lower[..at].to_string(),
        _ => lower,
    }
}

/// The rig token (`fr1`, `mr1`, `fsk1`, `mb1`, `mo1`, `fb1`, `fo1`) as a gender.
fn rig_gender(token: &str) -> Option<Gender> {
    let bytes = token.as_bytes();
    let tail_ok = matches!(
        token.get(1..token.len().saturating_sub(1)),
        Some("r" | "b" | "o" | "sk")
    );
    if bytes.len() < 3 || !bytes[bytes.len() - 1].is_ascii_digit() || !tail_ok {
        return None;
    }
    match bytes[0] {
        b'f' => Some(Gender::Female),
        b'm' => Some(Gender::Male),
        _ => None,
    }
}

pub fn classify(name: &str) -> NameInfo {
    let stem = stem(name);
    let tokens: Vec<&str> = stem.split('_').collect();
    let gender = tokens.iter().find_map(|token| rig_gender(token));
    let has = |prefix: &str| tokens.iter().any(|token| token.starts_with(prefix));
    let class = if has("bodygenitals") {
        NameClass::Blocked(Category::Genitals)
    } else if has("bodynipples") {
        NameClass::Blocked(Category::Nipples)
    } else if has("bodylowergen")
        || has("bodyuppergen") && (gender == Some(Gender::Female) || tokens[0] == "sc")
    {
        NameClass::Blocked(Category::BareBody)
    } else if tokens[0] == "gmt" && has("mannequin") && tokens.contains(&"f") {
        NameClass::Blocked(Category::Mannequin)
    } else if matches!(tokens[0], "hand" | "feet") && tokens.contains(&"nude") {
        NameClass::BaseBody(if tokens[0] == "hand" {
            BodySlot::Upper
        } else {
            BodySlot::Lower
        })
    } else if tokens.iter().any(|token| {
        token.starts_with("undies")
            || token.starts_with("bikini") && !token.starts_with("bikinitop")
    }) {
        NameClass::Cover(BodySlot::Lower)
    } else if tokens.iter().any(|token| {
        *token == "bra" || token.starts_with("brabikini") || token.starts_with("bikinitop")
    }) {
        NameClass::Cover(BodySlot::Upper)
    } else {
        NameClass::Other
    };
    NameInfo { class, gender }
}

/// Name to its hashes: the 32-bit lower-cased joaat and the 64-bit Murmur3 keys of the spellings the game's data
/// might use (the stem, and the stem with each resource suffix).
pub fn name_keys(name: &str) -> (u32, Vec<u64>) {
    let stem = stem(name);
    let mut keys = vec![murmur64(stem.as_bytes())];
    for suffix in [".#dr", ".#ft"] {
        keys.push(murmur64(format!("{stem}{suffix}").as_bytes()));
    }
    (joaat_lower(&stem), keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> NameClass {
        classify(name).class
    }

    #[test]
    fn modular_nude_patches_are_blocked() {
        assert_eq!(
            class("bodygenitalsgen_fr1_008_regular_ws00_full.#dr"),
            NameClass::Blocked(Category::Genitals)
        );
        assert_eq!(
            class("bodygenitalsgen_mr1_000_regular_ws00_full_alt_01"),
            NameClass::Blocked(Category::Genitals)
        );
        assert_eq!(
            class("bodynipplesgen_fr1_000_regular_ws00_full.#dr"),
            NameClass::Blocked(Category::Nipples)
        );
    }

    #[test]
    fn base_bodies_are_kept_and_clothes_are_not_blocked() {
        assert_eq!(
            class("hand_fr1_000_nude_cull.#dr"),
            NameClass::BaseBody(BodySlot::Upper)
        );
        assert_eq!(
            class("hand_mb1_000_nude_cull_fp.#dr"),
            NameClass::BaseBody(BodySlot::Upper)
        );
        assert_eq!(
            class("feet_mr1_000_nude_cull.#dr"),
            NameClass::BaseBody(BodySlot::Lower)
        );
        assert_eq!(class("feet_fr1_000_full.#dr"), NameClass::Other);
        assert_eq!(
            class("player_story000_00_pantsjeans_fr1_002_regular_hw_mid_full.#dr"),
            NameClass::Other
        );
        assert_eq!(
            class("player_story000_00_head_fr1_lat_000_m.#dr"),
            NameClass::Other
        );
    }

    #[test]
    fn underwear_and_bikini_are_cover() {
        assert_eq!(
            class("undiesgstring_fr1_002_slim_hw_full.#dr"),
            NameClass::Cover(BodySlot::Lower)
        );
        assert_eq!(
            class("brabikini_fr1_012_regular_ws00_full.#dr"),
            NameClass::Cover(BodySlot::Upper)
        );
    }

    #[test]
    fn bare_scene_bodies_and_mannequins() {
        assert_eq!(
            class("sc_fast_demon_bodyuppergen_fr1_000_m_full.#dr"),
            NameClass::Blocked(Category::BareBody)
        );
        assert_eq!(
            class("sc_fast_demon_bodylowergen_fr1_000_m_full.#dr"),
            NameClass::Blocked(Category::BareBody)
        );
        assert_eq!(
            class("cs_farmm_bodyuppergen_mr1_000_m_full.#dr"),
            NameClass::Other
        );
        assert_eq!(
            class("gmt_mannequin_proxy_f_01a.#dr"),
            NameClass::Blocked(Category::Mannequin)
        );
        assert_eq!(class("gmt_mannequin_m_01a.#dr"), NameClass::Other);
    }

    #[test]
    fn rig_tokens_give_gender() {
        assert_eq!(
            classify("undiesbriefs_mr1_000_full").gender,
            Some(Gender::Male)
        );
        assert_eq!(
            classify("hand_fsk1_000_nude_cull").gender,
            Some(Gender::Female)
        );
        assert_eq!(classify("hand_xx_000").gender, None);
    }

    #[test]
    fn stem_drops_suffix_and_case() {
        assert_eq!(stem("Hand_FR1_000_Nude_Cull.#dr"), "hand_fr1_000_nude_cull");
        assert_eq!(stem("plain"), "plain");
    }
}
