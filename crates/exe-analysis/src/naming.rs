//! Names that come from evidence in the executable. A name is only set when something in the
//! file says so, and the evidence is kept next to it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::model::{ClassInfo, FunctionInfo, GlobalInfo, NameEvidence, SourceEvidence, VtableInfo};

pub(crate) struct Evidence<'a> {
    pub texts: &'a HashMap<u32, String>,
    /// `(function RVA, name, ordinal)` of named exports.
    pub exports: Vec<(u32, String, u32)>,
    /// `(function RVA, import name)` of functions that only jump through the import table.
    pub thunks: Vec<(u32, String)>,
    pub vtables: &'a [VtableInfo],
    pub classes: &'a [ClassInfo],
}

fn index_of(functions: &[FunctionInfo], rva: u32) -> Option<usize> {
    functions.binary_search_by_key(&rva, |f| f.rva).ok()
}

fn set_name(function: &mut FunctionInfo, name: String, evidence: NameEvidence) {
    if function.name.is_some() {
        return;
    }
    if function.class.is_none() {
        function.class = name.rsplit_once("::").map(|(class, _)| class.to_string());
    }
    function.name = Some(name);
    function.name_evidence = Some(evidence);
}

/// Functions need to be sorted by RVA.
pub(crate) fn name_functions(functions: &mut [FunctionInfo], evidence: &Evidence<'_>) {
    for (rva, name, ordinal) in &evidence.exports {
        if let Some(index) = index_of(functions, *rva) {
            set_name(
                &mut functions[index],
                name.clone(),
                NameEvidence::Export { ordinal: *ordinal },
            );
        }
    }
    for (rva, import) in &evidence.thunks {
        if let Some(index) = index_of(functions, *rva) {
            set_name(
                &mut functions[index],
                import.clone(),
                NameEvidence::ImportThunk {
                    import: import.clone(),
                },
            );
        }
    }
    name_from_strings(functions, evidence.texts);
    source_files(functions, evidence.texts);
    name_virtual_methods(functions, evidence);
}

fn references(functions: &[FunctionInfo]) -> BTreeMap<u32, Vec<usize>> {
    let mut references: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, function) in functions.iter().enumerate() {
        for string in &function.strings {
            references.entry(*string).or_default().push(index);
        }
    }
    references
}

fn name_from_strings(functions: &mut [FunctionInfo], texts: &HashMap<u32, String>) {
    let mut candidates: BTreeMap<usize, Vec<(String, u32)>> = BTreeMap::new();
    for (string, users) in references(functions) {
        let [only] = users[..] else { continue };
        if let Some(name) = texts.get(&string).and_then(|t| function_name_in(t)) {
            candidates.entry(only).or_default().push((name, string));
        }
    }
    for (index, found) in candidates {
        let names: BTreeSet<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        if names.len() != 1 {
            continue;
        }
        let (name, string) = found[0].clone();
        set_name(&mut functions[index], name, NameEvidence::String { string });
    }
}

fn source_files(functions: &mut [FunctionInfo], texts: &HashMap<u32, String>) {
    let references = references(functions);
    let mut found: BTreeMap<usize, BTreeMap<&str, u32>> = BTreeMap::new();
    for (string, users) in &references {
        let Some(path) = texts.get(string).filter(|t| is_source_path(t)) else {
            continue;
        };
        for user in users {
            found
                .entry(*user)
                .or_default()
                .entry(path)
                .or_insert(*string);
        }
    }
    for (index, paths) in found {
        let mut distinct = paths.into_iter();
        let (Some((path, string)), None) = (distinct.next(), distinct.next()) else {
            continue;
        };
        functions[index].source_file = Some(path.to_string());
        functions[index].source_evidence = Some(SourceEvidence {
            string,
            sole_reference: references[&string].len() == 1,
        });
    }
}

fn name_virtual_methods(functions: &mut [FunctionInfo], evidence: &Evidence<'_>) {
    let ancestors: HashMap<&str, (usize, &[String])> = evidence
        .classes
        .iter()
        .map(|c| (c.name.as_str(), (c.ancestors.len(), c.ancestors.as_slice())))
        .collect();
    let mut holders: BTreeMap<usize, Vec<(&VtableInfo, u32)>> = BTreeMap::new();
    for vtable in evidence.vtables.iter().filter(|v| v.offset == 0) {
        for (slot, target) in vtable.slots.iter().enumerate() {
            if let Some(index) = index_of(functions, *target) {
                holders
                    .entry(index)
                    .or_default()
                    .push((vtable, slot as u32));
            }
        }
    }
    for (index, mut holding) in holders {
        holding.sort_by_key(|(v, slot)| {
            let depth = ancestors.get(v.class.as_str()).map_or(0, |a| a.0);
            (depth, v.class.as_str(), *slot, v.rva)
        });
        let (owner, slot) = holding[0];
        let descends = holding.iter().all(|(v, _)| {
            v.class == owner.class
                || ancestors
                    .get(v.class.as_str())
                    .is_some_and(|a| a.1.contains(&owner.class))
        });
        let same_slot = holding.iter().all(|(_, s)| *s == slot);
        if !descends || !same_slot {
            continue;
        }
        let function = &mut functions[index];
        function.class = Some(owner.class.clone());
        let name = format!("{}::vf{slot}", owner.class);
        set_name(
            function,
            name,
            NameEvidence::VirtualSlot {
                vtable: owner.rva,
                slot,
            },
        );
    }
}

/// Names globals after the one named function that writes them.
pub(crate) fn name_globals(globals: &mut [GlobalInfo], functions: &[FunctionInfo]) {
    let mut by_writer: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, global) in globals.iter().enumerate() {
        let [writer] = global.writers[..] else {
            continue;
        };
        let Some(function) = index_of(functions, writer).map(|i| &functions[i]) else {
            continue;
        };
        let named = matches!(
            function.name_evidence,
            Some(NameEvidence::Export { .. } | NameEvidence::String { .. })
        );
        let others_read = global.readers.iter().any(|r| *r != writer);
        let others_take = global.address_taken.iter().any(|r| *r != writer);
        if named && others_read && !others_take {
            by_writer.entry(writer).or_default().push(index);
        }
    }
    for (writer, owned) in by_writer {
        let function = &functions[index_of(functions, writer).unwrap_or_default()];
        let mut base = String::new();
        for c in function.name.as_deref().unwrap_or_default().chars() {
            if c.is_ascii_alphanumeric() {
                base.push(c);
            } else if !base.ends_with('_') {
                base.push('_');
            }
        }
        let many = owned.len() > 1;
        for index in owned {
            let global = &mut globals[index];
            global.name = Some(if many {
                format!("g_{base}_{:x}", global.rva)
            } else {
                format!("g_{base}")
            });
            global.name_evidence = Some(NameEvidence::GlobalWriter { function: writer });
        }
    }
}

/// The qualified function name such a string spells out, such as `CFoo::Bar` in
/// `"CFoo::Bar: out of memory"`. The name has to open the text or follow "in" or "function".
pub(crate) fn function_name_in(text: &str) -> Option<String> {
    let opening = text.trim_start_matches(|c: char| c.is_whitespace() || "[(\"'".contains(c));
    let lower = text.to_ascii_lowercase();
    let mut places = vec![opening];
    for marker in ["in function ", "in ", "function ", "func: ", "function: "] {
        if let Some(at) = lower.find(marker) {
            places.push(&text[at + marker.len()..]);
        }
    }
    places.into_iter().find_map(leading_name)
}

fn leading_name(rest: &str) -> Option<String> {
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '~')))
        .unwrap_or(rest.len());
    if let Some(next) = rest[end..].chars().next()
        && !" ()[]|,;!-'\"".contains(next)
    {
        return None;
    }
    let token = rest[..end].trim_end_matches(':');
    let segments: Vec<&str> = token.split("::").collect();
    let valid = segments.len() >= 2
        && token.len() <= 256
        && segments.iter().all(|s| {
            s.trim_start_matches('~')
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        });
    valid.then(|| token.to_string())
}

pub(crate) fn is_source_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() > 260
        || bytes.len() < 6
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
        || text.chars().any(char::is_control)
    {
        return false;
    }
    let Some((_, extension)) = text.rsplit_once('.') else {
        return false;
    };
    ["cpp", "cc", "cxx", "c", "h", "hpp", "hxx", "inl", "ipp"]
        .iter()
        .any(|e| extension.eq_ignore_ascii_case(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_function_names_in_log_text() {
        let name = |t| function_name_in(t);
        assert_eq!(name("CFoo::Bar"), Some("CFoo::Bar".into()));
        assert_eq!(name("CFoo::Bar: out of memory"), Some("CFoo::Bar".into()));
        assert_eq!(name("[CFoo::~CFoo] released"), Some("CFoo::~CFoo".into()));
        assert_eq!(
            name("Assert failed in Net::Peer::send (x)"),
            Some("Net::Peer::send".into())
        );
        assert_eq!(
            name("error in function Zoo::Dog::bark!"),
            Some("Zoo::Dog::bark".into())
        );
    }

    #[test]
    fn rejects_text_that_only_mentions_scopes() {
        let name = |t| function_name_in(t);
        assert_eq!(name("Bar"), None);
        assert_eq!(name("C:\\dir\\file.cpp"), None);
        assert_eq!(name("loading std::vector<int> failed"), None);
        assert_eq!(name("see CFoo::Bar for details"), None);
        assert_eq!(name("Foo::123"), None);
        assert_eq!(name("http://example.com"), None);
    }

    #[test]
    fn recognises_source_paths() {
        assert!(is_source_path("x:\\game\\src\\foo.cpp"));
        assert!(is_source_path("D:/work/Bar.H"));
        assert!(!is_source_path("foo.cpp"));
        assert!(!is_source_path("x:\\game\\src\\foo.txt"));
        assert!(!is_source_path("x:\\game\\foo.cpp is missing"));
    }
}
