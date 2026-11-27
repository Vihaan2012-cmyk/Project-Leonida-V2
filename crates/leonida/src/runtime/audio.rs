//! Sound: the output mode from the settings. XMA streams decode through `audio-codec` inside the audio shims.

use shims::audio::set_output_mode;

use super::plan::RuntimePlan;

pub fn configure(plan: &RuntimePlan) {
    set_output_mode(plan.audio);
}

#[cfg(test)]
mod tests {
    use shims::audio::{OutputMode, with_engine};

    use super::*;

    #[test]
    fn audio_off_never_opens_a_device() {
        set_output_mode(OutputMode::Off);
        let opened = with_engine(|engine| {
            engine.activate();
            engine.has_device()
        });
        assert!(!opened);
    }
}
