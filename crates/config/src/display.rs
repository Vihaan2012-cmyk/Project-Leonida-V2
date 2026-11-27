//! `display.toml`: which screen the game opens on and how bright its HDR output is.

use serde::{Deserialize, Serialize};

use crate::{Problems, Section};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// Which screen the window opens on, counting from 1 in the order Windows lists them. 0 is the primary screen.
    pub monitor: u32,
    /// The brightest the screen can show, in nits. Used only with `hdr = true` in graphics.toml.
    pub hdr_peak_nits: u32,
    /// How bright an ordinary white (menus, text) is in HDR, in nits.
    pub hdr_sdr_white_nits: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            monitor: 0,
            hdr_peak_nits: 1000,
            hdr_sdr_white_nits: 203,
        }
    }
}

impl Section for DisplayConfig {
    const FILE: &'static str = "display.toml";
    fn default_text() -> String {
        "\
# Leonida display settings. Delete this file (or run `leonida config reset`) to get it back with the defaults.

# Which screen the window opens on, counting from 1 in the order Windows lists them. 0 is the primary screen.
monitor = 0

# Used only when hdr is on in graphics.toml.
# The brightest your screen can show, in nits. 80 to 10000. Many HDR screens are 600, 1000 or 1400.
hdr_peak_nits = 1000
# How bright an ordinary white (menus, text) is, in nits. 80 to 1000. Windows uses 203 by default.
hdr_sdr_white_nits = 203
"
        .to_string()
    }

    fn check(&self, problems: &mut Problems) {
        problems.range_u32("monitor", self.monitor, 0, 16);
        problems.range_u32("hdr_peak_nits", self.hdr_peak_nits, 80, 10_000);
        problems.range_u32("hdr_sdr_white_nits", self.hdr_sdr_white_nits, 80, 1000);
        if self.hdr_sdr_white_nits > self.hdr_peak_nits {
            problems.add(
                "hdr_sdr_white_nits",
                format!(
                    "must not be above hdr_peak_nits ({}), got {}",
                    self.hdr_peak_nits, self.hdr_sdr_white_nits
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_primary_screen_and_common_hdr_levels() {
        let display = DisplayConfig::default();
        assert_eq!(display.monitor, 0);
        assert_eq!(
            (display.hdr_peak_nits, display.hdr_sdr_white_nits),
            (1000, 203)
        );
        assert_eq!(
            DisplayConfig::parse(&DisplayConfig::default_text()).unwrap(),
            display
        );
    }

    #[test]
    fn levels_are_range_checked_and_white_stays_below_peak() {
        let error = DisplayConfig::parse("hdr_peak_nits = 20\nmonitor = 99\n").unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("hdr_peak_nits: must be between 80 and 10000, got 20"),
            "{text}"
        );
        assert!(
            text.contains("monitor: must be between 0 and 16, got 99"),
            "{text}"
        );
        let error =
            DisplayConfig::parse("hdr_peak_nits = 400\nhdr_sdr_white_nits = 500\n").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("hdr_sdr_white_nits: must not be above hdr_peak_nits (400)")
        );
    }
}
