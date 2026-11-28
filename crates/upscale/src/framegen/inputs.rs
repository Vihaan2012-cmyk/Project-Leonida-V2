//! What the game's upscaler dispatch tells frame generation about the frame it renders, and the mailbox that carries
//! it from the dispatch to the presenter.
//!
//! The dispatch runs on the game's render thread and only names its resources; the presenter resolves the names to
//! images when the frame is presented. One dispatch's inputs belong to the next frame presented: a frame whose
//! dispatch never happened (a menu, a loading screen, a video) finds the mailbox empty.

use std::sync::Mutex;

/// A game resource as the dispatch described it: the handle the game passed and the description it came with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputResource {
    /// The game's texture object; 0 when the dispatch left the input out.
    pub handle: usize,
    pub width: u32,
    pub height: u32,
    /// The console's format code, kept for diagnostics: the presenter reads the real format from the image.
    pub format: u32,
}

impl InputResource {
    pub fn is_present(&self) -> bool {
        self.handle != 0 && self.width != 0 && self.height != 0
    }
}

/// What one upscaler dispatch hands over besides the colour: depth and motion vectors at render or output resolution,
/// the jitter, the camera, and whether the history is to be dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameInputs {
    pub depth: InputResource,
    pub motion: InputResource,
    pub render_size: [u32; 2],
    pub output_size: [u32; 2],
    pub motion_vector_scale: [f32; 2],
    /// Sub-pixel camera offset of this frame in render pixels; the upscaled frame no longer carries it.
    pub jitter: [f32; 2],
    pub depth_reversed: bool,
    pub camera_near: f32,
    pub camera_far: f32,
    /// Vertical field of view in radians; 0 when the dispatch did not give one.
    pub camera_fov_y: f32,
    /// A camera cut or a history reset: nothing may be generated between the previous frame and this one.
    pub reset: bool,
}

impl FrameInputs {
    /// Depth and motion vectors both present, with sizes that make sense together.
    pub fn is_usable(&self) -> bool {
        self.depth.is_present()
            && self.motion.is_present()
            && self.render_size[0] != 0
            && self.render_size[1] != 0
            && self.motion_vector_scale.iter().all(|s| s.is_finite())
    }
}

static MAILBOX: Mutex<Option<FrameInputs>> = Mutex::new(None);

/// Called by the upscaler dispatch for every frame it handles. A second call before the frame is presented replaces
/// the first.
pub fn submit_inputs(inputs: &FrameInputs) {
    *MAILBOX.lock().unwrap_or_else(|e| e.into_inner()) = Some(*inputs);
}

/// Called by the presenter for every real frame: the inputs submitted since the last call, if any.
pub fn take_inputs() -> Option<FrameInputs> {
    MAILBOX.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Forgets a submission nobody presented (the presenter was switched off, a test starts).
pub fn clear_inputs() {
    MAILBOX.lock().unwrap_or_else(|e| e.into_inner()).take();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(handle: usize) -> InputResource {
        InputResource {
            handle,
            width: 1920,
            height: 1080,
            format: 0,
        }
    }

    #[test]
    fn a_submission_is_taken_once_and_the_newest_wins() {
        clear_inputs();
        assert!(take_inputs().is_none());
        let mut inputs = FrameInputs {
            depth: resource(1),
            motion: resource(2),
            render_size: [1920, 1080],
            output_size: [3840, 2160],
            motion_vector_scale: [1.0, 1.0],
            ..FrameInputs::default()
        };
        submit_inputs(&inputs);
        inputs.reset = true;
        submit_inputs(&inputs);
        let taken = take_inputs().unwrap();
        assert!(taken.reset && taken.depth.handle == 1);
        assert!(take_inputs().is_none());
    }

    #[test]
    fn missing_depth_or_motion_is_not_usable() {
        let mut inputs = FrameInputs {
            depth: resource(1),
            motion: resource(2),
            render_size: [1920, 1080],
            motion_vector_scale: [1.0, 1.0],
            ..FrameInputs::default()
        };
        assert!(inputs.is_usable());
        inputs.motion = InputResource::default();
        assert!(!inputs.is_usable());
        inputs.motion = resource(2);
        inputs.depth.handle = 0;
        assert!(!inputs.is_usable());
    }
}
