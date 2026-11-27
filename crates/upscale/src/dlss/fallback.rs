//! Choosing an upscaler when DLSS was asked for: DLSS when it can run, otherwise FSR 1 or the temporal upscaler with a
//! message that says why.

use super::{DlssConfig, DlssOptions, DlssUpscaler};
use crate::{SharedCompute, UpscaleError, Upscaler, UpscalerKind, fsr1, temporal};

/// The upscaler to use and, when it is not the one asked for, the message to show the player.
pub struct Selection {
    pub upscaler: Box<dyn Upscaler>,
    pub kind: UpscalerKind,
    pub notice: Option<String>,
}

/// The reason text of an error that made DLSS unavailable.
fn reason_of(error: &UpscaleError) -> String {
    match error {
        UpscaleError::Unavailable { reason, .. } | UpscaleError::Unimplemented { reason, .. } => {
            reason.clone()
        }
        other => other.to_string(),
    }
}

/// The message shown when DLSS was asked for but `fallback` is used instead.
pub fn fallback_notice(reason: &str, fallback_name: &str) -> String {
    format!("DLSS is not available: {reason}. Using {fallback_name} instead.")
}

/// Pure selection logic. `try_dlss` builds DLSS; `make` builds the other kinds. When `requested` is
/// [`UpscalerKind::Dlss`] and `try_dlss` fails, `fallback` (FSR 1 or temporal; DLSS itself counts as temporal) is built
/// and the notice names the reason.
pub fn select_upscaler(
    requested: UpscalerKind,
    fallback: UpscalerKind,
    try_dlss: impl FnOnce() -> Result<Box<dyn Upscaler>, UpscaleError>,
    make: impl Fn(UpscalerKind) -> Box<dyn Upscaler>,
) -> Selection {
    if requested != UpscalerKind::Dlss {
        return Selection {
            upscaler: make(requested),
            kind: requested,
            notice: None,
        };
    }
    match try_dlss() {
        Ok(upscaler) => Selection {
            upscaler,
            kind: UpscalerKind::Dlss,
            notice: None,
        },
        Err(error) => {
            let kind = match fallback {
                UpscalerKind::Dlss => UpscalerKind::Temporal,
                other => other,
            };
            let upscaler = make(kind);
            let notice = fallback_notice(&reason_of(&error), upscaler.name());
            Selection {
                upscaler,
                kind,
                notice: Some(notice),
            }
        }
    }
}

/// Builds the upscaler of `requested` kind. DLSS is created on its own NVIDIA device (see
/// [`DlssUpscaler::with_device`] for the renderer's); when it cannot be, the result is `fallback` with a notice.
pub fn create_with_fallback(
    requested: UpscalerKind,
    fallback: UpscalerKind,
    compute: SharedCompute,
    config: &DlssConfig,
    options: &DlssOptions,
) -> Selection {
    select_upscaler(
        requested,
        fallback,
        || {
            DlssUpscaler::with_options(config, options.clone())
                .map(|u| Box::new(u) as Box<dyn Upscaler>)
        },
        |kind| match kind {
            UpscalerKind::Fsr1 => Box::new(fsr1::Fsr1Upscaler::new(compute.clone())),
            _ => Box::new(temporal::TemporalUpscaler::new(
                compute.clone(),
                temporal::TemporalConfig::default(),
            )),
        },
    )
}
