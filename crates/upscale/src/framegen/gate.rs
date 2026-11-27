//! Decides, for each real frame, whether a frame may be generated between it and the one before.

use super::FrameInputs;

/// Why a real frame is shown alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// No previous frame to interpolate from.
    FirstFrame,
    /// The dispatch never ran for this frame, or left out depth or motion vectors: a menu, a loading screen, a video.
    NoInputs,
    /// A camera cut or history reset.
    Reset,
    /// The display or render size changed.
    SizeChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Generate,
    Hold(Hold),
}

impl Verdict {
    /// Whether the frame can serve as the previous frame of the next one.
    pub fn keeps_frame(self) -> bool {
        !matches!(self, Verdict::Hold(Hold::NoInputs))
    }
}

#[derive(Default)]
pub struct Gate {
    last: Option<([u32; 2], [u32; 2])>,
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }

    /// `color_size` is the real frame's size. A frame without usable inputs forgets the previous one, so the first
    /// frame after a menu is never interpolated from the menu.
    pub fn on_real_frame(&mut self, color_size: [u32; 2], inputs: Option<&FrameInputs>) -> Verdict {
        let Some(inputs) = inputs.filter(|i| i.is_usable()) else {
            self.last = None;
            return Verdict::Hold(Hold::NoInputs);
        };
        let shape = (color_size, inputs.render_size);
        let previous = self.last.replace(shape);
        if inputs.reset {
            return Verdict::Hold(Hold::Reset);
        }
        match previous {
            None => Verdict::Hold(Hold::FirstFrame),
            Some(p) if p != shape => Verdict::Hold(Hold::SizeChanged),
            Some(_) => Verdict::Generate,
        }
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::super::InputResource;
    use super::*;

    fn inputs() -> FrameInputs {
        let r = InputResource {
            handle: 1,
            width: 960,
            height: 540,
            format: 0,
        };
        FrameInputs {
            depth: r,
            motion: r,
            render_size: [960, 540],
            output_size: [1920, 1080],
            motion_vector_scale: [1.0, 1.0],
            ..FrameInputs::default()
        }
    }

    const SIZE: [u32; 2] = [1920, 1080];

    #[test]
    fn frames_are_generated_after_the_first() {
        let mut gate = Gate::new();
        assert_eq!(
            gate.on_real_frame(SIZE, Some(&inputs())),
            Verdict::Hold(Hold::FirstFrame)
        );
        assert_eq!(gate.on_real_frame(SIZE, Some(&inputs())), Verdict::Generate);
        assert_eq!(gate.on_real_frame(SIZE, Some(&inputs())), Verdict::Generate);
    }

    #[test]
    fn nothing_is_generated_across_a_cut() {
        let mut gate = Gate::new();
        gate.on_real_frame(SIZE, Some(&inputs()));
        gate.on_real_frame(SIZE, Some(&inputs()));
        let cut = FrameInputs {
            reset: true,
            ..inputs()
        };
        let verdict = gate.on_real_frame(SIZE, Some(&cut));
        assert_eq!(verdict, Verdict::Hold(Hold::Reset));
        assert!(verdict.keeps_frame());
        assert_eq!(gate.on_real_frame(SIZE, Some(&inputs())), Verdict::Generate);
    }

    #[test]
    fn a_frame_without_inputs_breaks_the_chain_on_both_sides() {
        let mut gate = Gate::new();
        gate.on_real_frame(SIZE, Some(&inputs()));
        let menu = gate.on_real_frame(SIZE, None);
        assert_eq!(menu, Verdict::Hold(Hold::NoInputs));
        assert!(!menu.keeps_frame());
        assert_eq!(
            gate.on_real_frame(SIZE, Some(&inputs())),
            Verdict::Hold(Hold::FirstFrame)
        );
        let missing_motion = FrameInputs {
            motion: InputResource::default(),
            ..inputs()
        };
        assert_eq!(
            gate.on_real_frame(SIZE, Some(&missing_motion)),
            Verdict::Hold(Hold::NoInputs)
        );
    }

    #[test]
    fn a_size_change_restarts_interpolation() {
        let mut gate = Gate::new();
        gate.on_real_frame(SIZE, Some(&inputs()));
        assert_eq!(
            gate.on_real_frame([2560, 1440], Some(&inputs())),
            Verdict::Hold(Hold::SizeChanged)
        );
        assert_eq!(
            gate.on_real_frame([2560, 1440], Some(&inputs())),
            Verdict::Generate
        );
    }
}
