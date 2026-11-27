//! `upscaling.toml`: the files an upscaler needs that Leonida cannot ship.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Problems, Section};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Upscaling {
    /// Your own copy of NVIDIA's `nvngx_dlss.dll`, used when `upscaler = "dlss"` in graphics.toml.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dlss_path: Option<PathBuf>,
}

impl Section for Upscaling {
    const FILE: &'static str = "upscaling.toml";
    fn default_text() -> String {
        "\
# Leonida upscaler files. Delete this file (or run `leonida config reset`) to get it back with the defaults.

# DLSS needs NVIDIA's own nvngx_dlss.dll, which Leonida does not include: point this at your copy, and set
# upscaler = \"dlss\" in graphics.toml. Without it, a DLSS choice falls back to the temporal upscaler.
# dlss_path = \"C:/Games/DLSS/nvngx_dlss.dll\"
"
        .to_string()
    }

    fn check(&self, problems: &mut Problems) {
        let Some(path) = &self.dlss_path else {
            return;
        };
        if path.as_os_str().is_empty() {
            problems.add(
                "dlss_path",
                "must not be empty; remove the line to leave DLSS unconfigured",
            );
        } else if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
        {
            problems.add("dlss_path", "must be the nvngx_dlss.dll file, not a folder");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_dlss_path_by_default_and_the_path_is_read() {
        assert_eq!(Upscaling::default().dlss_path, None);
        assert_eq!(
            Upscaling::parse(&Upscaling::default_text()).unwrap(),
            Upscaling::default()
        );
        let chosen = Upscaling::parse("dlss_path = \"C:/x/nvngx_dlss.dll\"\n").unwrap();
        assert_eq!(chosen.dlss_path, Some(PathBuf::from("C:/x/nvngx_dlss.dll")));
        assert!(chosen.to_text().contains("nvngx_dlss.dll"));
    }

    #[test]
    fn the_path_must_name_a_dll() {
        let text = Upscaling::parse("dlss_path = \"C:/x\"\n")
            .unwrap_err()
            .to_string();
        assert!(
            text.contains("dlss_path: must be the nvngx_dlss.dll file"),
            "{text}"
        );
        assert!(Upscaling::parse("dlss_path = \"\"\n").is_err());
    }
}
