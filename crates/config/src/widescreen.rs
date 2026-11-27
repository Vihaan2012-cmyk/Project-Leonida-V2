//! `widescreen.toml`: the widescreen code patches, each off by default.

use serde::{Deserialize, Serialize};

use crate::{Problems, Section};

/// One flag per patch group of `enhancements::widescreen`. The field of view factor is `fov_scale` in graphics.toml.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Widescreen {
    /// `live_aspect`: the camera, HUD and effects use the window's real width over height instead of 16:9 or 4:3.
    pub live_aspect: bool,
    /// `no_pillarbox`: the scene viewport takes the whole window for shapes other than 16:9.
    pub no_pillarbox: bool,
}

impl Section for Widescreen {
    const FILE: &'static str = "widescreen.toml";
    fn default_text() -> String {
        "\
# Leonida widescreen patches. Delete this file (or run `leonida config reset`) to get it back with the defaults.
# They change the game's own code, so they are off until you turn them on. The field of view factor is
# fov_scale in graphics.toml.

# The camera, HUD and effects use the window's real width over height instead of 16:9 or 4:3.
live_aspect = false

# The scene takes the whole window for shapes other than 16:9, with no black bars.
no_pillarbox = false
"
        .to_string()
    }

    fn check(&self, _problems: &mut Problems) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_patch_group_is_off_by_default() {
        assert_eq!(
            Widescreen::default(),
            Widescreen {
                live_aspect: false,
                no_pillarbox: false
            }
        );
        assert_eq!(
            Widescreen::parse(&Widescreen::default_text()).unwrap(),
            Widescreen::default()
        );
    }

    #[test]
    fn groups_are_turned_on_one_at_a_time() {
        let chosen = Widescreen::parse("no_pillarbox = true\n").unwrap();
        assert!(chosen.no_pillarbox && !chosen.live_aspect);
        assert!(Widescreen::parse("live_aspect = 1\n").is_err());
    }
}
