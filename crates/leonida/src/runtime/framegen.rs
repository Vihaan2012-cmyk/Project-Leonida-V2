//! Frame generation: which maker of the extra frames runs, decided before the game starts.
//!
//! `frame_generation = true` turns the presenter's generation on; `frame_generation_method` says who makes the frames.
//! A DLSS choice is kept only when NVIDIA's frame generation can run on this PC (checked here, on a device of its own);
//! otherwise Leonida's own interpolation runs and the note says why. The presenter falls back the same way, with its
//! own note, if NGX fails once the game is running.

use config::FrameGenerationMethod;
use xbox_d3d12::present::{
    FrameGenMethod, FrameGeneration, set_dlss_library_dir, set_frame_generation,
};

use super::plan::RuntimePlan;

/// What the settings ask for, and what runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub method: Option<FrameGenMethod>,
    pub note: Option<String>,
}

/// The method that runs given the settings and, for a DLSS choice, whether NVIDIA's frame generation can run.
pub fn choose(
    enabled: bool,
    method: FrameGenerationMethod,
    dlss: impl FnOnce() -> Result<(), String>,
) -> Choice {
    if !enabled {
        return Choice {
            method: None,
            note: None,
        };
    }
    match method {
        FrameGenerationMethod::Leonida => Choice {
            method: Some(FrameGenMethod::Own),
            note: Some("Leonida's own frame generation".into()),
        },
        FrameGenerationMethod::Dlss => match dlss() {
            Ok(()) => Choice {
                method: Some(FrameGenMethod::Dlss),
                note: Some(
                    "NVIDIA DLSS frame generation; Leonida's own takes over if it stops working"
                        .into(),
                ),
            },
            Err(reason) => Choice {
                method: Some(FrameGenMethod::Own),
                note: Some(format!(
                    "DLSS frame generation is not available: {reason}; using Leonida's own frame generation instead"
                )),
            },
        },
    }
}

/// Switches the presenter's frame generation as the settings say. Returns a note to print.
pub fn configure(plan: &RuntimePlan) -> Option<String> {
    let mut library_dir = None;
    let choice = choose(plan.frame_generation, plan.frame_generation_method, || {
        let report = upscale::dlss::dlssg::report(plan.dlss_path.as_deref());
        library_dir = report.library_dir.clone();
        if report.available {
            Ok(())
        } else {
            Err(report.summary)
        }
    });
    let method = choice.method?;
    set_dlss_library_dir(library_dir);
    set_frame_generation(FrameGeneration {
        method,
        ..FrameGeneration::on()
    });
    choice.note
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dlss_unavailable() -> Result<(), String> {
        Err("this PC has no GeForce RTX 40 series GPU".into())
    }

    #[test]
    fn nothing_changes_when_frame_generation_is_off() {
        let choice = choose(false, FrameGenerationMethod::Dlss, || {
            panic!("DLSS is not even asked")
        });
        assert_eq!(choice.method, None);
        assert_eq!(choice.note, None);
    }

    #[test]
    fn the_default_is_leonida_s_own_and_never_asks_nvidia() {
        let choice = choose(true, FrameGenerationMethod::default(), || {
            panic!("DLSS is not asked")
        });
        assert_eq!(choice.method, Some(FrameGenMethod::Own));
    }

    #[test]
    fn dlss_is_used_when_it_can_run() {
        let choice = choose(true, FrameGenerationMethod::Dlss, || Ok(()));
        assert_eq!(choice.method, Some(FrameGenMethod::Dlss));
        assert!(choice.note.unwrap().contains("DLSS"));
    }

    #[test]
    fn a_dlss_choice_falls_back_to_the_own_frames_and_says_why() {
        let choice = choose(true, FrameGenerationMethod::Dlss, dlss_unavailable);
        assert_eq!(choice.method, Some(FrameGenMethod::Own));
        let note = choice.note.unwrap();
        assert!(note.contains("RTX 40 series"), "{note}");
        assert!(note.contains("using Leonida's own"), "{note}");
    }
}
