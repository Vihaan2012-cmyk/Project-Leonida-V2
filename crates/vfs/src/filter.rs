//! The content filter: rules that hide or replace what the tree would otherwise serve.
//!
//! It sits above every other layer, so no package can bring back a hidden file. The rules come from outside
//! (the nudity-filter work supplies them); this module is only the mechanism.

use serde::Deserialize;

use crate::path::{VPath, fold, glob_match, joaat};

/// What a file must look like for a rule to apply. Every field that is set must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Match {
    /// A pattern over the whole virtual path: `*`, `?`, and `**` for any number of folders.
    pub path: Option<String>,
    /// The file's name, with or without its extension.
    pub name: Option<String>,
    /// The `joaat` hash of the lower-cased name, with or without its extension.
    pub joaat: Option<u32>,
    pub sha256: Option<[u8; 32]>,
    /// Narrows a hash rule so that only files of this size are read to be hashed.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The path does not exist, and listings leave it out.
    Hide,
    /// The path serves the content of another virtual path, as that path is without any filtering.
    Replace(VPath),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub when: Match,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    #[error("rules file is not valid: {0}")]
    Parse(String),
    #[error("rule `{id}`: {why}")]
    Rule { id: String, why: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict<'a> {
    Keep,
    Hide(&'a Rule),
    Replace(&'a Rule, &'a VPath),
}

#[derive(Debug, Clone, Default)]
pub struct ContentFilter {
    rules: Vec<Rule>,
    folded: Vec<Compiled>,
}

#[derive(Debug, Clone, Default)]
struct Compiled {
    path: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    #[serde(default)]
    rule: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    action: String,
    path: Option<String>,
    name: Option<String>,
    joaat: Option<String>,
    sha256: Option<String>,
    size: Option<u64>,
    with: Option<String>,
}

impl ContentFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rules file:
    ///
    /// ```toml
    /// [[rule]]
    /// id = "swap-texture"
    /// path = "Characters/**/skin_a.dds"   # and/or name, joaat ("0x1234abcd"), sha256 (hex), size
    /// action = "replace"                   # or "hide"
    /// with = "Filters/skin_a_clothed.dds"  # virtual path, replace only
    /// ```
    pub fn from_toml(text: &str) -> Result<ContentFilter, FilterError> {
        let file: RulesFile =
            toml::from_str(text).map_err(|e| FilterError::Parse(e.message().to_string()))?;
        let mut filter = ContentFilter::new();
        for raw in file.rule {
            filter.push(raw.into_rule()?)?;
        }
        Ok(filter)
    }

    pub fn push(&mut self, rule: Rule) -> Result<(), FilterError> {
        let when = &rule.when;
        if when == &Match::default() {
            return Err(FilterError::Rule {
                id: rule.id,
                why: "matches nothing: give a path, name, joaat or sha256".into(),
            });
        }
        let path = when
            .path
            .as_ref()
            .map(|pattern| fold(pattern.replace('\\', "/").trim_matches('/')));
        self.folded.push(Compiled {
            path,
            name: when.name.as_deref().map(fold),
        });
        self.rules.push(rule);
        Ok(())
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The first rule that applies to the file at `path` of `size` bytes. `hash` is called at most once, and
    /// only when a rule's other conditions already hold and it needs the content.
    pub fn verdict(
        &self,
        path: &VPath,
        size: u64,
        hash: &mut dyn FnMut() -> Option<[u8; 32]>,
    ) -> Verdict<'_> {
        let mut cached: Option<Option<[u8; 32]>> = None;
        let file_name = fold(path.file_name());
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name.as_str(), |(s, _)| s);
        for (rule, folded) in self.rules.iter().zip(&self.folded) {
            let when = &rule.when;
            if folded
                .path
                .as_deref()
                .is_some_and(|p| !glob_match(p, path.key()))
            {
                continue;
            }
            if folded
                .name
                .as_deref()
                .is_some_and(|n| n != file_name && n != stem)
            {
                continue;
            }
            if when.size.is_some_and(|s| s != size) {
                continue;
            }
            if when
                .joaat
                .is_some_and(|h| h != joaat(file_name.as_bytes()) && h != joaat(stem.as_bytes()))
            {
                continue;
            }
            if let Some(want) = when.sha256 {
                let got = *cached.get_or_insert_with(&mut *hash);
                if got != Some(want) {
                    continue;
                }
            }
            return match &rule.action {
                Action::Hide => Verdict::Hide(rule),
                Action::Replace(with) => Verdict::Replace(rule, with),
            };
        }
        Verdict::Keep
    }

    /// True when some rule needs file content, so a caller can skip hashing otherwise.
    pub fn needs_content(&self) -> bool {
        self.rules.iter().any(|r| r.when.sha256.is_some())
    }
}

impl RawRule {
    fn into_rule(self) -> Result<Rule, FilterError> {
        let bad = |why: String| FilterError::Rule {
            id: self.id.clone(),
            why,
        };
        let joaat = match &self.joaat {
            None => None,
            Some(text) => {
                let digits = text
                    .trim()
                    .trim_start_matches("0x")
                    .trim_start_matches("0X");
                Some(
                    u32::from_str_radix(digits, 16)
                        .map_err(|_| bad(format!("joaat `{text}` is not hex")))?,
                )
            }
        };
        let sha256 = match &self.sha256 {
            None => None,
            Some(text) => {
                Some(parse_sha256(text).ok_or_else(|| bad("sha256 must be 64 hex digits".into()))?)
            }
        };
        let action = match (self.action.as_str(), &self.with) {
            ("hide", None) => Action::Hide,
            ("hide", Some(_)) => {
                return Err(bad("`with` only goes with action = \"replace\"".into()));
            }
            ("replace", Some(with)) => {
                Action::Replace(VPath::parse(with).map_err(|e| bad(format!("`with`: {e}")))?)
            }
            ("replace", None) => return Err(bad("action = \"replace\" needs `with`".into())),
            (other, _) => return Err(bad(format!("action `{other}` is not `hide` or `replace`"))),
        };
        Ok(Rule {
            id: self.id.clone(),
            when: Match {
                path: self.path.clone(),
                name: self.name.clone(),
                joaat,
                sha256,
                size: self.size,
            },
            action,
        })
    }
}

fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> Option<[u8; 32]> {
        panic!("hash not needed")
    }

    fn verdict<'a>(filter: &'a ContentFilter, path: &str, size: u64) -> Verdict<'a> {
        filter.verdict(&VPath::parse(path).unwrap(), size, &mut never)
    }

    #[test]
    fn reads_rules_and_applies_the_first_match() {
        let filter = ContentFilter::from_toml(
            r#"
            [[rule]]
            id = "hide-name"
            name = "Secret"
            action = "hide"

            [[rule]]
            id = "swap-path"
            path = "Characters/**/skin_a.dds"
            action = "replace"
            with = "Filters\\clothed.dds"

            [[rule]]
            id = "by-joaat"
            joaat = "0xCA2E9442"
            action = "hide"
            "#,
        )
        .unwrap();
        assert!(
            matches!(verdict(&filter, "x/SECRET.dds", 1), Verdict::Hide(r) if r.id == "hide-name")
        );
        assert!(matches!(verdict(&filter, "x/secret", 1), Verdict::Hide(_)));
        match verdict(&filter, "characters/Lucia/tex/Skin_A.DDS", 1) {
            Verdict::Replace(rule, with) => {
                assert_eq!(rule.id, "swap-path");
                assert_eq!(with.as_str(), "Filters/clothed.dds");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(verdict(&filter, "a/a.bin", 1), Verdict::Hide(r) if r.id == "by-joaat"));
        assert_eq!(verdict(&filter, "a/b.bin", 1), Verdict::Keep);
    }

    #[test]
    fn hash_rules_read_content_only_when_the_rest_matches() {
        use sha2::{Digest, Sha256};
        let digest: [u8; 32] = Sha256::digest(b"payload").into();
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let filter = ContentFilter::from_toml(&format!(
            "[[rule]]\nid = \"h\"\nsha256 = \"{hex}\"\nsize = 7\nsection = 1\naction = \"hide\"\n"
        ));
        assert!(filter.is_err(), "unknown keys are refused");
        let filter = ContentFilter::from_toml(&format!(
            "[[rule]]\nid = \"h\"\nsha256 = \"{hex}\"\nsize = 7\naction = \"hide\"\n"
        ))
        .unwrap();
        assert!(filter.needs_content());
        assert_eq!(verdict(&filter, "a.bin", 8), Verdict::Keep);
        let mut calls = 0;
        let mut hash = || {
            calls += 1;
            Some(digest)
        };
        assert!(matches!(
            filter.verdict(&VPath::parse("a.bin").unwrap(), 7, &mut hash),
            Verdict::Hide(_)
        ));
        assert_eq!(calls, 1);
        let mut other = || Some([0u8; 32]);
        assert_eq!(
            filter.verdict(&VPath::parse("a.bin").unwrap(), 7, &mut other),
            Verdict::Keep
        );
    }

    #[test]
    fn rejects_unusable_rules() {
        for text in [
            "[[rule]]\nid=\"a\"\naction=\"hide\"\n",
            "[[rule]]\nid=\"a\"\nname=\"x\"\naction=\"replace\"\n",
            "[[rule]]\nid=\"a\"\nname=\"x\"\naction=\"hide\"\nwith=\"y\"\n",
            "[[rule]]\nid=\"a\"\nname=\"x\"\naction=\"burn\"\n",
            "[[rule]]\nid=\"a\"\nsha256=\"abc\"\naction=\"hide\"\n",
            "[[rule]]\nid=\"a\"\nname=\"x\"\naction=\"replace\"\nwith=\"../up\"\n",
        ] {
            assert!(ContentFilter::from_toml(text).is_err(), "{text}");
        }
    }
}
