//! The game's FidelityFX upscale dispatches, served by the `upscale` crate.
//!
//! The game hands `ffxDispatch` console texture objects. This module reads the colour, depth and motion vectors
//! through the `TextureAccess` the D3D12 layer registers, runs the upscaler the settings chose and writes the result
//! into the output texture. Until the D3D12 layer registers its access a dispatch fails with a trace note and the
//! game keeps its own picture. The compute device comes from that access too; without one the first dispatch opens
//! its own Vulkan device.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Mutex;

use anyhow::Result;
use config::Upscaler as Choice;
use shims::fidelityfx::integration::{
    TextureAccess, UpscaleFailure, UpscaleInputs, UpscaleResource, set_upscaler, texture_access,
};
use upscale::dlss::{DlssConfig, DlssStatus, probe};
use upscale::fsr1::Fsr1Upscaler;
use upscale::temporal::{TemporalConfig, TemporalUpscaler};
use upscale::{ColorSpace, Image, ImageRef, SharedCompute, UpscaleError, Upscaler};

use super::plan::RuntimePlan;

/// The upscaler that runs. DLSS is not among them: it cannot be created from the public NGX documentation, so a
/// DLSS choice runs the temporal upscaler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fsr1,
    Temporal,
}

enum Held {
    Fsr1(Fsr1Upscaler),
    Temporal(Box<TemporalUpscaler>),
}

impl Held {
    fn upscale(&mut self, inputs: &upscale::UpscaleInputs<'_>) -> Result<Image, UpscaleError> {
        match self {
            Held::Fsr1(upscaler) => upscaler.upscale(inputs),
            Held::Temporal(upscaler) => upscaler.upscale(inputs),
        }
    }

    fn reset(&mut self) {
        match self {
            Held::Fsr1(upscaler) => upscaler.reset(),
            Held::Temporal(upscaler) => upscaler.reset(),
        }
    }
}

struct State {
    kind: Kind,
    compute: Option<SharedCompute>,
    contexts: HashMap<usize, Held>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// What the settings ask for, and a note when the choice could not be kept as it is.
pub fn choose(choice: Choice) -> (Option<Kind>, Option<String>) {
    match choice {
        Choice::Off => (None, None),
        Choice::Fsr1 => (Some(Kind::Fsr1), None),
        Choice::Temporal => (Some(Kind::Temporal), None),
        Choice::Dlss => {
            let note = match probe(&DlssConfig::default()) {
                DlssStatus::Present { .. } => "DLSS cannot be driven yet".to_string(),
                DlssStatus::Unavailable(reason) => format!("DLSS is not available: {reason}"),
            };
            (
                Some(Kind::Temporal),
                Some(format!("{note}; using the temporal upscaler instead")),
            )
        }
    }
}

/// Registers the upscaler for the game's dispatches when the settings turn one on. Returns a note to print.
pub fn configure(plan: &RuntimePlan) -> Result<Option<String>> {
    let (kind, note) = choose(plan.upscaler);
    let Some(kind) = kind else {
        set_upscaler(None);
        return Ok(None);
    };
    if let Ok(mut state) = STATE.lock() {
        *state = Some(State {
            kind,
            compute: None,
            contexts: HashMap::new(),
        });
    }
    set_upscaler(Some(dispatch));
    Ok(note)
}

fn failure(error: impl std::fmt::Display) -> UpscaleFailure {
    UpscaleFailure(error.to_string())
}

fn read(
    access: &dyn TextureAccess,
    resource: &UpscaleResource,
) -> Result<Option<Image>, UpscaleFailure> {
    if resource.resource == 0 {
        return Ok(None);
    }
    access.read(resource).map(Some)
}

/// The game's dispatch described the way the `upscale` crate wants it.
pub fn convert<'a>(
    inputs: &UpscaleInputs,
    color: &'a Image,
    depth: Option<&'a Image>,
    motion: Option<&'a Image>,
    reactive: Option<&'a Image>,
) -> upscale::UpscaleInputs<'a> {
    let mut converted = upscale::UpscaleInputs::new(
        ImageRef::new(color.width, color.height, color.format, &color.data),
        inputs.render_size,
        inputs.upscale_size,
    );
    let as_ref =
        |image: &'a Image| ImageRef::new(image.width, image.height, image.format, &image.data);
    converted.depth = depth.map(as_ref);
    converted.motion = motion.map(as_ref);
    converted.reactive = reactive.map(as_ref);
    converted.color_space = ColorSpace::Linear;
    converted.jitter = inputs.jitter;
    converted.motion_vector_scale = inputs.motion_vector_scale;
    converted.depth_reversed = inputs.depth_inverted;
    converted.camera_near = inputs.camera_near;
    if inputs.camera_far.is_finite() && inputs.camera_far > inputs.camera_near {
        converted.camera_far = inputs.camera_far;
    }
    converted.camera_fov_y = inputs.camera_fov_vertical;
    converted.exposure = if inputs.pre_exposure > 0.0 {
        inputs.pre_exposure
    } else {
        1.0
    };
    converted.sharpness = inputs.sharpness.unwrap_or(0.0);
    converted.reset = inputs.reset;
    converted.frame_time_ms = inputs.frame_time_ms;
    converted
}

fn build(kind: Kind, compute: Option<&SharedCompute>) -> Result<Held, UpscaleFailure> {
    match (kind, compute) {
        (Kind::Fsr1, Some(compute)) => Ok(Held::Fsr1(Fsr1Upscaler::new(compute.clone()))),
        (Kind::Fsr1, None) => Ok(Held::Fsr1(Fsr1Upscaler::cpu())),
        (Kind::Temporal, Some(compute)) => Ok(Held::Temporal(Box::new(TemporalUpscaler::new(
            compute.clone(),
            TemporalConfig::default(),
        )))),
        (Kind::Temporal, None) => Err(failure("the temporal upscaler needs a GPU compute device")),
    }
}

/// One dispatch: read the game's textures, upscale, write the output.
fn run(
    contexts: &mut HashMap<usize, Held>,
    kind: Kind,
    compute: Option<&SharedCompute>,
    access: &dyn TextureAccess,
    inputs: &UpscaleInputs,
) -> Result<(), UpscaleFailure> {
    let color =
        read(access, &inputs.color)?.ok_or_else(|| failure("the dispatch has no colour"))?;
    let depth = read(access, &inputs.depth)?;
    let motion = read(access, &inputs.motion_vectors)?;
    let reactive = read(access, &inputs.reactive)?;
    let held = match contexts.entry(inputs.context) {
        Entry::Occupied(held) => held.into_mut(),
        Entry::Vacant(slot) => slot.insert(build(kind, compute)?),
    };
    if inputs.reset {
        held.reset();
    }
    let converted = convert(
        inputs,
        &color,
        depth.as_ref(),
        motion.as_ref(),
        reactive.as_ref(),
    );
    let result = held.upscale(&converted).map_err(failure)?;
    access.write(&inputs.output, &result)
}

fn dispatch(inputs: &UpscaleInputs) -> Result<(), UpscaleFailure> {
    let access = texture_access().ok_or_else(|| {
        failure("the D3D12 layer has not registered its texture access, so the upscaler has no pictures to read")
    })?;
    let mut guard = STATE
        .lock()
        .map_err(|_| failure("the upscaler state is locked"))?;
    let state = guard
        .as_mut()
        .ok_or_else(|| failure("no upscaler is configured"))?;
    if state.compute.is_none() {
        state.compute = match access.compute() {
            Some(compute) => Some(compute),
            None => Some(std::sync::Arc::new(gpu::Gpu::new().map_err(failure)?) as SharedCompute),
        };
    }
    let compute = state.compute.clone();
    run(
        &mut state.contexts,
        state.kind,
        compute.as_ref(),
        access.as_ref(),
        inputs,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use upscale::PixelFormat;

    use super::*;

    struct Fake {
        color: Image,
        written: StdMutex<Vec<(usize, Image)>>,
    }

    impl TextureAccess for Fake {
        fn read(&self, resource: &UpscaleResource) -> Result<Image, UpscaleFailure> {
            match resource.resource {
                1 => Ok(self.color.clone()),
                other => Err(UpscaleFailure(format!("no texture {other}"))),
            }
        }

        fn write(&self, resource: &UpscaleResource, image: &Image) -> Result<(), UpscaleFailure> {
            self.written
                .lock()
                .unwrap()
                .push((resource.resource, image.clone()));
            Ok(())
        }
    }

    fn fake() -> Fake {
        let pixels: Vec<f32> = (0..16)
            .flat_map(|i| [i as f32 / 16.0, 0.5, 0.25, 1.0])
            .collect();
        Fake {
            color: Image::from_rgba_f32(4, 4, PixelFormat::Rgba16Float, &pixels).unwrap(),
            written: StdMutex::new(Vec::new()),
        }
    }

    fn dispatch_inputs() -> UpscaleInputs {
        let color = UpscaleResource {
            resource: 1,
            ..UpscaleResource::default()
        };
        let output = UpscaleResource {
            resource: 9,
            ..UpscaleResource::default()
        };
        UpscaleInputs {
            context: 7,
            color,
            output,
            render_size: [4, 4],
            upscale_size: [8, 8],
            sharpness: Some(0.5),
            pre_exposure: 0.0,
            camera_far: f32::INFINITY,
            depth_inverted: true,
            jitter: [0.25, -0.25],
            motion_vector_scale: [2.0, 3.0],
            ..UpscaleInputs::default()
        }
    }

    #[test]
    fn the_settings_name_the_upscaler_and_dlss_falls_back_with_a_reason() {
        assert_eq!(choose(Choice::Off), (None, None));
        assert_eq!(choose(Choice::Fsr1), (Some(Kind::Fsr1), None));
        assert_eq!(choose(Choice::Temporal), (Some(Kind::Temporal), None));
        let (kind, note) = choose(Choice::Dlss);
        assert_eq!(kind, Some(Kind::Temporal));
        let note = note.expect("a reason");
        assert!(note.contains("DLSS"), "{note}");
        assert!(note.contains("temporal upscaler instead"), "{note}");
    }

    #[test]
    fn the_games_values_become_the_upscalers_inputs() {
        let color = fake().color;
        let inputs = dispatch_inputs();
        let converted = convert(&inputs, &color, None, None, None);
        assert_eq!(converted.render_size, [4, 4]);
        assert_eq!(converted.output_size, [8, 8]);
        assert_eq!(converted.jitter, [0.25, -0.25]);
        assert_eq!(converted.motion_vector_scale, [2.0, 3.0]);
        assert!(converted.depth_reversed);
        assert_eq!(converted.sharpness, 0.5);
        assert_eq!(converted.exposure, 1.0);
        assert!(converted.camera_far.is_infinite());
        assert!(converted.depth.is_none() && converted.motion.is_none());
    }

    #[test]
    fn a_dispatch_reads_the_colour_and_writes_the_upscaled_output() {
        let access = fake();
        let mut contexts = HashMap::new();
        run(&mut contexts, Kind::Fsr1, None, &access, &dispatch_inputs()).unwrap();
        let written = access.written.lock().unwrap();
        assert_eq!(written.len(), 1);
        let (target, image) = &written[0];
        assert_eq!(*target, 9);
        assert_eq!((image.width, image.height), (8, 8));
        assert!(contexts.contains_key(&7));
    }

    #[test]
    fn a_dispatch_without_colour_or_with_an_unreadable_texture_fails_cleanly() {
        let access = fake();
        let mut contexts = HashMap::new();
        let mut none = dispatch_inputs();
        none.color.resource = 0;
        let error = run(&mut contexts, Kind::Fsr1, None, &access, &none).unwrap_err();
        assert!(error.0.contains("no colour"), "{error:?}");
        let mut missing = dispatch_inputs();
        missing.depth.resource = 5;
        let error = run(&mut contexts, Kind::Fsr1, None, &access, &missing).unwrap_err();
        assert!(error.0.contains("no texture 5"), "{error:?}");
        assert!(access.written.lock().unwrap().is_empty());
    }

    #[test]
    fn the_temporal_upscaler_needs_a_compute_device() {
        let access = fake();
        let mut contexts = HashMap::new();
        let error = run(
            &mut contexts,
            Kind::Temporal,
            None,
            &access,
            &dispatch_inputs(),
        )
        .unwrap_err();
        assert!(error.0.contains("compute device"), "{error:?}");
    }
}
