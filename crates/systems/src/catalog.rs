//! Where each kind of game behaviour data goes under `Systems/`, and what is not decoded yet.
//!
//! Archive tables of contents are encrypted, so a file is placed by what it contains: the root element of an XML or
//! RBF0 document, or the shape of a string table. The archive reader may pass a name when it has one.

/// Root element names with a folder of their own: `(root, folder under Systems)`.
pub const EXACT_ROOTS: &[(&str, &str)] = &[
    ("CContentsOfWeatherXmlFile", "World/Weather"),
    ("ShoreWavePlacementData", "World/Water"),
    ("OceanSimPresets", "World/Water"),
    ("GameShape__Data__LocationNameLoopkup", "World"),
    ("PathingStyleData", "World"),
    ("CTrainConfigs", "World"),
    ("CHazardNodes", "World"),
    ("CPairedPedDataBlock", "Peds"),
    ("CEnergyConfigInfos", "Peds"),
    ("CDoorInteractionDataList", "Peds/AI"),
    ("CCombatInfoMgr", "Peds/AI"),
    ("CFiringPatternInfoManager", "Peds/AI"),
    ("CFleeDecisionMakerData", "Peds/AI"),
    ("CAimUpperBodyDataBlock", "Weapons/Animation"),
    ("CDamageDataManager", "Weapons/Damage"),
    ("CCombatMeleeGroup__Tunables__Tuning", "Weapons/Tuning"),
    ("CTuningFile", "Tuning"),
    ("CCoverProcessorTuning", "Tuning"),
    ("GridAtlasInfo", "Tuning"),
    ("PatchConstants", "Tuning"),
    ("LandingPageVideos", "Audio"),
    ("CTVPlaylistContainer", "Audio"),
    ("naSpeechEventTypeBatchRequestParams", "Audio"),
    ("CDynamicScenarioManager", "Missions"),
    ("CMissions", "Missions"),
    ("Respawn__MetaData", "Missions"),
    ("CConversationList", "Missions/Conversations"),
    ("CConversationDictionary", "Missions/Conversations"),
    ("CMovieSubtitleContainer", "UI/Subtitles"),
    ("UIScreen", "UI"),
    ("UIMovieView", "UI"),
    ("uiFeedbackContext", "UI"),
    ("uiProgressDataset", "UI"),
    ("uiSafezone", "UI"),
    ("uiSettingsGamepadControlsData", "UI"),
    ("InputContextStyleStore", "UI"),
    ("rage__SafeframeConfig", "UI"),
    ("rage__crRigSetManager", "Animation"),
];
/// Root element name prefixes: `(prefix, folder under Systems)`. Anything else goes to `Misc`.
pub const PREFIX_ROOTS: &[(&str, &str)] = &[
    ("timecycle_", "World/Timecycle"),
    ("CVehicle", "Vehicles"),
    ("CTyre", "Vehicles"),
    ("CPed", "Peds"),
    ("Combat__", "Peds/AI"),
    ("WeaponAnimations__", "Weapons/Animation"),
    ("CDataFileMgr__", "Content"),
    ("rage__fwui", "UI"),
];

/// The `Systems/` folder for a document with this root element.
pub fn directory_for_root(root: &str) -> &'static str {
    EXACT_ROOTS
        .iter()
        .find(|(name, _)| *name == root)
        .or_else(|| {
            PREFIX_ROOTS
                .iter()
                .find(|(prefix, _)| root.starts_with(prefix))
        })
        .map_or("Misc", |&(_, dir)| dir)
}

fn file_stem(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `Systems/<dir>/<name>.<extension>`: the file's own name when the archive reader knows it, otherwise the root
/// element's name.
pub fn systems_path(root: &str, name_hint: Option<&str>, extension: &str) -> String {
    let stem = name_hint.map_or_else(|| file_stem(root), file_stem);
    format!("Systems/{}/{stem}.{extension}", directory_for_root(root))
}

/// `Systems/Text/<language>.json`.
pub fn text_path(language: &str) -> String {
    format!("Systems/Text/{}.json", file_stem(language))
}

/// A system whose format is not decoded yet.
#[derive(Debug, Clone, Copy)]
pub struct Undecoded {
    pub dir: &'static str,
    pub what: &'static str,
    pub missing: &'static str,
}

pub const UNDECODED: &[Undecoded] = &[
    Undecoded {
        dir: "Peds/Catalogues",
        what: "Ped and wardrobe appearance catalogues (struct 0x8dcb4cf5) in packs.rpf as 0DRP v122 resources and as one PSIN file.",
        missing: "A reader for the 0DRP v122 block directory and the PSIN schema sections is not written. The record layouts are \
                  measured (presets 88 bytes, drawable state 184 bytes, parent chain depth 3) but drawable and texture hashes \
                  are 64-bit murmur values with no name dictionary, so a text form would be numbers only.",
    },
    Undecoded {
        dir: "Audio/Radio",
        what: "Radio station descriptors and content-id groups in the audio config at the start of audio.rpf.",
        missing: "Station hashes are joaat of radio_<slug> and need a name dictionary. The span and group graph is measured \
                  but not a playlist: link hashes are unresolved, so track order and station ownership are not decoded.",
    },
    Undecoded {
        dir: "Missions",
        what: "Compiled mission scripts and the PSO conversation streams.",
        missing: "Scripts are listed, not decompiled (a disassembler is a separate piece). Conversation keys and labels are \
                  measured, but their pairing with text needs the PSO reader and the string tables of every language.",
    },
    Undecoded {
        dir: "World/Placements",
        what: "Map placement resources in levels.rpf: 23,535 maps and 1.67 million entities in 0DRP v122 resources.",
        missing: "Only a census exists: counts per map, not an entity reader. The table of contents is encrypted, so maps have \
                  no names, only 32-bit hashes.",
    },
    Undecoded {
        dir: "World/Heightmaps",
        what: "The MHIU 2048x2048 and PAMH max/min height maps, the SPS1 shader preset table and the binary tables in common.rpf.",
        missing: "Layouts are measured for MHIU, PAMH and SPS1, but they are binary grids, not XML. The 1,296-byte records \
                  of the 801-entry table and the 3-region spatial grid are only partly decoded.",
    },
    Undecoded {
        dir: "Navigation",
        what: "Navigation meshes in navmeshes.rpf.",
        missing: "Not measured by the documents this crate follows; no reader exists. A glTF preview belongs to the assets crate.",
    },
    Undecoded {
        dir: "Weapons",
        what: "Weapon statistics and handling tables.",
        missing: "common.rpf has no weapon-stat or handling root element; those tables are not XML and their archive is not \
                  identified. Weapon loadouts, damage and animation XML are exported under Weapons/ and Peds/AI.",
    },
    Undecoded {
        dir: "Camera",
        what: "Camera settings.",
        missing: "No camera data has been located in any measured archive.",
    },
    Undecoded {
        dir: "Text/Dialogue",
        what: "Dialogue lines joined to their subtitles and speakers.",
        missing: "String tables are exported by label hash. Joining them to dialogue keys needs the PSO conversation streams, \
                  and label names need a hash dictionary built from outside the game files.",
    },
];

/// The text of `Systems/<dir>/NOT_DECODED.md`.
pub fn render_note(u: &Undecoded) -> String {
    format!(
        "# Not decoded yet\n\nWhat this is: {}\n\nWhat is missing: {}\n",
        u.what, u.missing
    )
}

/// Every note file as `(path under the output folder, contents)`.
pub fn note_files() -> Vec<(String, String)> {
    UNDECODED
        .iter()
        .map(|u| (format!("Systems/{}/NOT_DECODED.md", u.dir), render_note(u)))
        .collect()
}

/// How a reference string in a content-set manifest names its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference<'a> {
    Common(&'a str),
    Target(&'a str),
    Root(&'a str),
    Levels(&'a str),
    Platform(&'a str),
    /// `common:/`, which is not one of the four documented prefix rules and is left unresolved.
    CommonScheme(&'a str),
    Relative(&'a str),
}

pub fn classify_reference(text: &str) -> Reference<'_> {
    if let Some(r) = text.strip_prefix("$(common)/") {
        Reference::Common(r)
    } else if let Some(r) = text.strip_prefix("$(target)/") {
        Reference::Target(r)
    } else if let Some(r) = text.strip_prefix("$(root)/") {
        Reference::Root(r)
    } else if let Some(r) = text.strip_prefix("platform:/levels/") {
        Reference::Levels(r)
    } else if let Some(r) = text.strip_prefix("platform:/") {
        Reference::Platform(r)
    } else if let Some(r) = text.strip_prefix("common:/") {
        Reference::CommonScheme(r)
    } else {
        Reference::Relative(text)
    }
}

/// The key a `parentPath` locator of the form `../Item[@key='K']` selects among its siblings.
pub fn parent_path_key(locator: &str) -> Option<&str> {
    let rest = locator.strip_prefix("../")?;
    let (tag, rest) = rest.split_once("[@")?;
    let (attr, rest) = rest.split_once("='")?;
    let key = rest.strip_suffix("']")?;
    (!tag.is_empty() && !attr.is_empty() && !key.contains('\'')).then_some(key)
}
