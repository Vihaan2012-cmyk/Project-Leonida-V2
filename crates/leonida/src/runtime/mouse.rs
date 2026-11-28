//! The direct mouse camera patch as one more part of the run's patch set.

use config::Input;
use enhancements::camera_mouse::{self, Layout, MouseLookSettings};
use enhancements::patch::{Finder, Patch};
use pe_image::Image;

/// `mouse_sensitivity` times each axis' own sensitivity, and the inversion flags.
pub fn settings(input: &Input) -> MouseLookSettings {
    MouseLookSettings {
        sensitivity: [
            input.mouse_sensitivity * input.mouse_sensitivity_x,
            input.mouse_sensitivity * input.mouse_sensitivity_y,
        ],
        invert: [input.mouse_invert_x, input.mouse_invert_y],
    }
}

/// The first pair of patches whose bytes overlap, by name.
pub fn overlap<'a>(a: &'a [Patch], b: &'a [Patch]) -> Option<(&'a str, &'a str)> {
    let end = |p: &Patch| p.rva + p.replacement.len() as u32;
    a.iter()
        .flat_map(|x| b.iter().map(move |y| (x, y)))
        .find(|(x, y)| x.rva < end(y) && y.rva < end(x))
        .map(|(x, y)| (x.name.as_str(), y.name.as_str()))
}

/// What the mouse patch adds to a patch set.
#[derive(Debug, PartialEq)]
pub struct Built {
    pub patches: Vec<Patch>,
    /// RVA of the cell the input shim fills.
    pub cell: u32,
}

/// The patches for the executable `bytes`, or why they cannot be built or would collide with `others`.
pub fn build(
    settings: &MouseLookSettings,
    bytes: &[u8],
    layout: &Layout,
    others: &[Patch],
) -> Result<Built, String> {
    let image =
        Image::parse(bytes).map_err(|e| format!("reading the executable's sections: {e}"))?;
    let finder = Finder::new(&image);
    let built = camera_mouse::patches(&finder, layout, settings).map_err(|e| e.to_string())?;
    if let Some((mine, theirs)) = overlap(&built.patches, others) {
        return Err(format!("{mine} overlaps {theirs}"));
    }
    Ok(Built {
        patches: built.patches,
        cell: built.cell,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::synthetic;
    use config::Section;

    fn patch(name: &str, rva: u32, len: usize) -> Patch {
        Patch {
            name: name.into(),
            rva,
            original: vec![0; len],
            replacement: vec![1; len],
        }
    }

    #[test]
    fn sensitivities_multiply_and_inversion_carries_over() {
        let input = Input::parse(
            "mouse_sensitivity = 2.0\nmouse_sensitivity_x = 1.5\nmouse_sensitivity_y = 0.5\nmouse_invert_x = true\n",
        )
        .unwrap();
        let tuned = settings(&input);
        assert_eq!(tuned.sensitivity, [3.0, 1.0]);
        assert_eq!(tuned.invert, [true, false]);
        assert_eq!(settings(&Input::default()), MouseLookSettings::default());
    }

    #[test]
    fn overlap_needs_shared_bytes() {
        let a = [patch("a", 0x10, 4)];
        assert_eq!(overlap(&a, &[patch("b", 0x14, 4)]), None);
        assert_eq!(overlap(&a, &[patch("b", 0x0c, 4)]), None);
        assert_eq!(overlap(&a, &[patch("b", 0x13, 4)]), Some(("a", "b")));
        assert_eq!(overlap(&a, &[patch("b", 0x0d, 4)]), Some(("a", "b")));
    }

    #[test]
    fn the_patches_build_for_an_image_with_the_site() {
        let (bytes, layout) = synthetic::image();
        let built = build(&MouseLookSettings::default(), &bytes, &layout, &[]).unwrap();
        assert_eq!(built.patches.len(), 3);
        assert!(built.cell >= synthetic::DATA && built.cell < synthetic::PDATA);
        assert_eq!(built.patches[2].rva, synthetic::HOOK);
    }

    #[test]
    fn a_changed_site_or_a_collision_is_a_reason_not_a_panic() {
        let (bytes, layout) = synthetic::image();
        let wrong = Layout {
            hash: layout.hash ^ 1,
            ..layout
        };
        let error = build(&MouseLookSettings::default(), &bytes, &wrong, &[]).unwrap_err();
        assert!(!error.is_empty());
        let other = [patch("frame timer", synthetic::HOOK + 4, 8)];
        let error = build(&MouseLookSettings::default(), &bytes, &layout, &other).unwrap_err();
        assert!(
            error.contains("mouse look hook overlaps frame timer"),
            "{error}"
        );
        let error = build(&MouseLookSettings::default(), &[1, 2, 3], &layout, &[]).unwrap_err();
        assert!(error.contains("sections"), "{error}");
    }
}
