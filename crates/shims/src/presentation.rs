//! What the player chose about how the game looks and paces, for the D3D12 layer to read when it makes the
//! window and the presenter. `leonida run` sets it before the game starts; the defaults are the console's.

use std::sync::RwLock;

use config::Graphics;
use xbox_d3d12::pacing::FrameLimit;

#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    pub frame_limit: FrameLimit,
    pub graphics: Graphics,
}

impl Default for Presentation {
    fn default() -> Self {
        Presentation {
            frame_limit: FrameLimit::Console,
            graphics: Graphics::default(),
        }
    }
}

static CURRENT: RwLock<Option<Presentation>> = RwLock::new(None);

/// Replaces the choices.
pub fn set(presentation: Presentation) {
    if let Ok(mut slot) = CURRENT.write() {
        *slot = Some(presentation);
    }
}

/// The choices made for this run, or the defaults when none were set.
pub fn get() -> Presentation {
    CURRENT
        .read()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_choices_are_the_consoles_and_set_ones_are_returned() {
        assert_eq!(get().frame_limit, FrameLimit::Console);
        let chosen = Presentation {
            frame_limit: FrameLimit::Cap(90.0),
            graphics: Graphics {
                hdr: true,
                ..Graphics::default()
            },
        };
        set(chosen.clone());
        assert_eq!(get(), chosen);
    }
}
