use std::collections::HashMap;

use crate::scan::Entry;
use crate::symbols::Symbols;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Source,
    Class,
    Range,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Source => "source file",
            Kind::Class => "class",
            Kind::Range => "address range",
        }
    }
}

#[derive(Debug)]
pub struct Group {
    /// Relative to the export folder, with forward slashes.
    pub path: String,
    pub kind: Kind,
    /// Indices into the entries, ascending.
    pub members: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub groups: Vec<Group>,
    /// The group each entry went to.
    pub group_of: Vec<u32>,
    pub names: Vec<String>,
    pub classes: Vec<Option<String>>,
}

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

const MAX_COMPONENT: usize = 60;

pub(crate) fn component(text: &str) -> Option<String> {
    let mut out: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(MAX_COMPONENT);
    let trimmed = out.trim_matches(|c| c == '.' || c == ' ');
    if trimmed.is_empty() {
        return None;
    }
    let stem = trimmed
        .split('.')
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return Some(format!("_{trimmed}"));
    }
    Some(trimmed.to_string())
}

/// `x:\a\b\file.cpp` becomes `sources/a/b/file.c`.
pub fn source_path(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    let without_drive = match normalized.split_once(':') {
        Some((drive, rest)) if drive.len() == 1 => rest,
        _ => &normalized,
    };
    let mut parts: Vec<String> = without_drive
        .split('/')
        .filter(|p| !matches!(*p, "" | "." | ".."))
        .filter_map(component)
        .collect();
    let file = parts.pop()?;
    let stem = ["cpp", "cxx", "cc", "c"]
        .iter()
        .find_map(|ext| file.strip_suffix(&format!(".{ext}")))
        .unwrap_or(&file);
    let keep = parts.len().saturating_sub(5);
    parts.drain(..keep);
    parts.insert(0, "sources".to_string());
    parts.push(stem.to_string());
    Some(parts.join("/"))
}

/// `rage::fwEntity<int>` becomes `classes/rage/fwEntity_int_` (the file gets `.c` or `.h`).
pub fn class_path(name: &str) -> Option<String> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' => depth = depth.saturating_sub(1),
            b':' if depth == 0 && bytes.get(i + 1) == Some(&b':') => {
                parts.push(&name[start..i]);
                start = i + 2;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&name[start..]);
    let mut parts: Vec<String> = parts.into_iter().filter_map(component).collect();
    if parts.is_empty() {
        return None;
    }
    let keep = parts.len().saturating_sub(4);
    parts.drain(..keep);
    parts.insert(0, "classes".to_string());
    Some(parts.join("/"))
}

pub fn range_path(first: u32) -> String {
    format!("unsorted/0x{first:08x}")
}

pub fn build(entries: &[Entry], symbols: &dyn Symbols, per_file: usize) -> Plan {
    let per_file = per_file.max(1);
    let mut plan = Plan {
        group_of: vec![u32::MAX; entries.len()],
        names: entries
            .iter()
            .map(|e| symbols.function_name(e.rva))
            .collect(),
        classes: entries.iter().map(|e| symbols.class_of(e.rva)).collect(),
        groups: Vec::new(),
    };
    let mut by_path: HashMap<String, u32> = HashMap::new();
    let mut unsorted = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let target = symbols
            .source_file(entry.rva)
            .and_then(|s| source_path(&s))
            .map(|p| (p, Kind::Source))
            .or_else(|| {
                plan.classes[index]
                    .as_deref()
                    .and_then(class_path)
                    .map(|p| (p, Kind::Class))
            });
        let Some((path, kind)) = target else {
            unsorted.push(index as u32);
            continue;
        };
        let group = *by_path.entry(path.to_ascii_lowercase()).or_insert_with(|| {
            plan.groups.push(Group {
                path: path.clone(),
                kind,
                members: Vec::new(),
            });
            (plan.groups.len() - 1) as u32
        });
        plan.groups[group as usize].members.push(index as u32);
        plan.group_of[index] = group;
    }
    for chunk in unsorted.chunks(per_file) {
        let group = plan.groups.len() as u32;
        for &index in chunk {
            plan.group_of[index as usize] = group;
        }
        plan.groups.push(Group {
            path: range_path(entries[chunk[0] as usize].rva),
            kind: Kind::Range,
            members: chunk.to_vec(),
        });
    }
    plan.groups.sort_by(|a, b| a.path.cmp(&b.path));
    for (index, group) in plan.groups.iter().enumerate() {
        for &member in &group.members {
            plan.group_of[member as usize] = index as u32;
        }
    }
    plan
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Rva(u32),
    Text(String),
}

impl Selector {
    pub fn parse(text: &str) -> Selector {
        let hex = text
            .strip_prefix("0x")
            .or_else(|| text.strip_prefix("sub_"))
            .and_then(|digits| u32::from_str_radix(digits, 16).ok());
        match hex {
            Some(rva) => Selector::Rva(rva),
            None => Selector::Text(text.to_string()),
        }
    }
}

/// The groups `--only` picked: a function picked takes its whole file with it.
#[derive(Debug, Default)]
pub struct Selection {
    pub picked: Vec<bool>,
}

impl Selection {
    pub fn everything(plan: &Plan) -> Selection {
        Selection {
            picked: vec![true; plan.groups.len()],
        }
    }
}

pub fn select(
    plan: &Plan,
    entries: &[Entry],
    symbols: &dyn Symbols,
    selectors: &[Selector],
) -> Selection {
    let mut selection = Selection {
        picked: vec![false; plan.groups.len()],
    };
    for selector in selectors {
        match selector {
            Selector::Rva(rva) => {
                let at = entries.partition_point(|e| e.rva <= *rva);
                let Some(index) = at.checked_sub(1) else {
                    continue;
                };
                let entry = entries[index];
                if entry.rva == *rva || *rva - entry.rva < entry.size {
                    selection.picked[plan.group_of[index] as usize] = true;
                }
            }
            Selector::Text(text) => {
                let lower = text.to_ascii_lowercase();
                for (g, group) in plan.groups.iter().enumerate() {
                    if group.path.to_ascii_lowercase().contains(&lower) {
                        selection.picked[g] = true;
                    }
                }
                for (index, entry) in entries.iter().enumerate() {
                    let group = plan.group_of[index] as usize;
                    if selection.picked[group] {
                        continue;
                    }
                    let hit = plan.names[index].eq_ignore_ascii_case(text)
                        || plan.classes[index]
                            .as_deref()
                            .is_some_and(|c| c.eq_ignore_ascii_case(text))
                        || symbols
                            .source_file(entry.rva)
                            .is_some_and(|s| s.to_ascii_lowercase().contains(&lower));
                    selection.picked[group] |= hit;
                }
            }
        }
    }
    selection
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::default_name;

    #[derive(Default)]
    struct Fake {
        source: HashMap<u32, &'static str>,
        class: HashMap<u32, &'static str>,
        names: HashMap<u32, &'static str>,
    }

    impl Symbols for Fake {
        fn function_name(&self, rva: u32) -> String {
            self.names
                .get(&rva)
                .map_or_else(|| default_name(rva), |n| n.to_string())
        }
        fn class_of(&self, rva: u32) -> Option<String> {
            self.class.get(&rva).map(|c| c.to_string())
        }
        fn source_file(&self, rva: u32) -> Option<String> {
            self.source.get(&rva).map(|c| c.to_string())
        }
        fn notes(&self, _: u32) -> Vec<String> {
            Vec::new()
        }
        fn global_name(&self, _: u32) -> Option<String> {
            None
        }
    }

    fn entries(count: u32) -> Vec<Entry> {
        (0..count)
            .map(|i| Entry {
                rva: 0x1000 + i * 0x40,
                size: 0x30,
            })
            .collect()
    }

    #[test]
    fn source_paths_lose_drive_and_dots() {
        assert_eq!(
            source_path(r"X:\game\..\src\Ped.cpp").as_deref(),
            Some("sources/game/src/Ped")
        );
        assert_eq!(
            source_path("a/b/util.h").as_deref(),
            Some("sources/a/b/util.h")
        );
        assert_eq!(source_path("con.c").as_deref(), Some("sources/_con"));
        assert_eq!(source_path(r"\\").as_deref(), None);
    }

    #[test]
    fn class_paths_split_on_top_level_scope_only() {
        assert_eq!(
            class_path("rage::fwEntity").as_deref(),
            Some("classes/rage/fwEntity")
        );
        assert_eq!(
            class_path("std::vector<a::b>").as_deref(),
            Some("classes/std/vector_a__b_")
        );
        assert_eq!(class_path("::").as_deref(), None);
    }

    #[test]
    fn source_beats_class_and_the_rest_is_ranged() {
        let mut symbols = Fake::default();
        symbols.source.insert(0x1000, "src/a.cpp");
        symbols.class.insert(0x1000, "Foo");
        symbols.class.insert(0x1040, "Foo");
        symbols.source.insert(0x1080, "SRC/A.cpp");
        let entries = entries(6);
        let plan = build(&entries, &symbols, 2);
        let paths: Vec<&str> = plan.groups.iter().map(|g| g.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "classes/Foo",
                "sources/src/a",
                "unsorted/0x000010c0",
                "unsorted/0x00001140"
            ]
        );
        assert_eq!(plan.groups[0].members, vec![1]);
        assert_eq!(plan.groups[1].members, vec![0, 2]);
        assert_eq!(plan.groups[2].members, vec![3, 4]);
        assert_eq!(plan.groups[3].members, vec![5]);
        assert_eq!(plan.group_of, vec![1, 0, 1, 2, 2, 3]);
    }

    #[test]
    fn selectors_pick_functions_and_files() {
        let mut symbols = Fake::default();
        symbols.source.insert(0x1000, "src/a.cpp");
        symbols.source.insert(0x1040, "src/a.cpp");
        symbols.names.insert(0x1080, "Update");
        let entries = entries(4);
        let plan = build(&entries, &symbols, 10);
        let by_rva = select(&plan, &entries, &symbols, &[Selector::parse("0x1090")]);
        assert_eq!(by_rva.picked, vec![false, true]);
        let by_name = select(&plan, &entries, &symbols, &[Selector::parse("update")]);
        assert_eq!(by_name.picked, vec![false, true]);
        let by_file = select(&plan, &entries, &symbols, &[Selector::parse("a.c")]);
        assert_eq!(by_file.picked, vec![true, false]);
        assert_eq!(Selector::parse("sub_00001000"), Selector::Rva(0x1000));
        assert_eq!(Selector::parse("Update"), Selector::Text("Update".into()));
    }
}
