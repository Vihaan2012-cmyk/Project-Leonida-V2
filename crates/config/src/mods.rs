//! `mods.toml`: which mods load, and in what order.

use serde::{Deserialize, Serialize};

use crate::{Problems, Section};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Mods {
    /// Names of mod folders under Mods, first to last. A later mod overrides an earlier one.
    pub load_order: Vec<String>,
}

impl Section for Mods {
    const FILE: &'static str = "mods.toml";
    fn default_text() -> String {
        "\
# The mods Leonida loads. Only mods named here are used.

# Names of mod folders under Mods, first to last. A later mod overrides an earlier one where they change the
# same file. Example: load_order = [\"better-roads\", \"new-radio\"]
load_order = []
"
        .to_string()
    }

    fn check(&self, problems: &mut Problems) {
        for (index, name) in self.load_order.iter().enumerate() {
            let field = format!("load_order[{index}]");
            if name.trim().is_empty() {
                problems.add(&field, "must not be empty");
            } else if name.contains(['/', '\\'])
                || name == ".."
                || name == "."
                || name.contains(':')
            {
                problems.add(
                    &field,
                    format!("\"{name}\" must be a folder name under Mods, not a path"),
                );
            } else if self.load_order[..index]
                .iter()
                .any(|earlier| earlier.eq_ignore_ascii_case(name))
            {
                problems.add(&field, format!("\"{name}\" is listed twice"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_order_keeps_its_order() {
        let mods = Mods::parse("load_order = [\"b\", \"a\", \"c\"]\n").unwrap();
        assert_eq!(mods.load_order, ["b", "a", "c"]);
    }

    #[test]
    fn bad_names_are_reported() {
        let error =
            Mods::parse("load_order = [\"a\", \"A\", \"..\", \"x/y\", \"\"]\n").unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("load_order[1]: \"A\" is listed twice"),
            "{text}"
        );
        assert!(text.contains("load_order[2]"), "{text}");
        assert!(text.contains("load_order[3]"), "{text}");
        assert!(text.contains("load_order[4]: must not be empty"), "{text}");
    }
}
