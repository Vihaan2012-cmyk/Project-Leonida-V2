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
use upscale::dlss::{DlssConfig, DlssStatus, DlssUpscaler, probe};
use upscale::fsr1::Fsr1Upscaler;
use upscale::temporal::{TemporalConfig, TemporalUpscaler};
use upscale::{ColorSpace, Image, ImageRef, SharedCompute, UpscaleError, Upscaler};

use super::plan::RuntimePlan;

/// The upscaler that runs. A DLSS choice runs DLSS when NVIDIA's files and an NVIDIA GPU are there, and otherwise the
/// temporal upscaler (see [`choose`] and [`build`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fsr1,
    Temporal,
    Dlss,
}

enum Held {
    Fsr1(Fsr1Upscaler),
    Temporal(Box<TemporalUpscaler>),
    Dlss(Box<DlssUpscaler>),
}

impl Held {
    fn upscale(&mut self, inputs: &upscale::UpscaleInputs<'_>) -> Result<Image, UpscaleError> {
        match self {
            Held::Fsr1(upscaler) => upscaler.upscale(inputs),
            Held::Temporal(upscaler) => upscaler.upscale(inputs),
            Held::Dlss(upscaler) => upscaler.upscale(inputs),
        }
    }

    fn reset(&mut self) {
        match self {
            Held::Fsr1(upscaler) => upscaler.reset(),
            Held::Temporal(upscaler) => upscaler.reset(),
            Held::Dlss(upscaler) => upscaler.reset(),
        }
    }
}

struct State {
    kind: Kind,
    dlss: DlssConfig,
    compute: Option<SharedCompute>,
    contexts: HashMap<usize, Held>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// What the settings ask for, and a note when the choice could not be kept as it is. DLSS is kept when NVIDIA's
/// library and NGX are found; whether the GPU takes it is only known when the first dispatch builds it, and a failure
/// there falls back to the temporal upscaler with the reason traced.
pub fn choose(choice: Choice, dlss: &DlssConfig) -> (Option<Kind>, Option<String>) {
    match choice {
        Choice::Off => (None, None),
        Choice::Fsr1 => (Some(Kind::Fsr1), None),
        Choice::Temporal => (Some(Kind::Temporal), None),
        Choice::Dlss => match probe(dlss) {
            DlssStatus::Present { dlss, source, .. } => (
                Some(Kind::Dlss),
                Some(format!(
                    "DLSS from {} ({}); the temporal upscaler takes over if the GPU cannot run it",
                    dlss.display(),
                    source.describe()
                )),
            ),
            DlssStatus::Unavailable(reason) => (
                Some(Kind::Temporal),
                Some(format!(
                    "DLSS is not available: {reason}; using the temporal upscaler instead"
                )),
            ),
        },
    }
}

/// Registers the upscaler for the game's dispatches when the settings turn one on. Returns a note to print.
pub fn configure(plan: &RuntimePlan) -> Result<Option<String>> {
    let dlss = DlssConfig {
        dlss_dll: plan.dlss_path.clone(),
        ..DlssConfig::default()
    };
    let (kind, note) = choose(plan.upscaler, &dlss);
    let Some(kind) = kind else {
        set_upscaler(None);
        return Ok(None);
    };
    if let Ok(mut state) = STATE.lock() {
        *state = Some(State {
            kind,
            dlss,
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

fn build(
    kind: Kind,
    compute: Option<&SharedCompute>,
    dlss: &DlssConfig,
) -> Result<Held, UpscaleFailure> {
    match (kind, compute) {
        (Kind::Fsr1, Some(compute)) => Ok(Held::Fsr1(Fsr1Upscaler::new(compute.clone()))),
        (Kind::Fsr1, None) => Ok(Held::Fsr1(Fsr1Upscaler::cpu())),
        (Kind::Temporal, Some(compute)) => Ok(Held::Temporal(Box::new(TemporalUpscaler::new(
            compute.clone(),
            TemporalConfig::default(),
        )))),
        (Kind::Temporal, None) => Err(failure("the temporal upscaler needs a GPU compute device")),
        (Kind::Dlss, compute) => match DlssUpscaler::new(dlss) {
            Ok(upscaler) => Ok(Held::Dlss(Box::new(upscaler))),
            Err(error) => {
                shims::trace::note(format!(
                    "upscaler: DLSS could not start ({error}); using the temporal upscaler"
                ));
                build(Kind::Temporal, compute, dlss)
            }
        },
    }
}

/// One dispatch: read the game's textures, upscale, write the output.
fn run(
    contexts: &mut HashMap<usize, Held>,
    kind: Kind,
    dlss: &DlssConfig,
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
        Entry::Vacant(slot) => slot.insert(build(kind, compute, dlss)?),
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
        &state.dlss,
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
        depth_and_motion: Option<(Image, Image)>,
        written: StdMutex<Vec<(usize, Image)>>,
    }

    impl TextureAccess for Fake {
        fn read(&self, resource: &UpscaleResource) -> Result<Image, UpscaleFailure> {
            match resource.resource {
                1 => Ok(self.color.clone()),
                2 | 3 => match &self.depth_and_motion {
                    Some((depth, motion)) => Ok(if resource.resource == 2 {
                        depth.clone()
                    } else {
                        motion.clone()
                    }),
                    None => Err(UpscaleFailure("no depth".into())),
                },
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
            depth_and_motion: None,
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

    fn no_dlss_anywhere() -> DlssConfig {
        let empty = std::env::temp_dir().join("leonida-runtime-no-dlss");
        std::fs::create_dir_all(&empty).unwrap();
        DlssConfig {
            search_folders: Some(vec![empty]),
            ..DlssConfig::default()
        }
    }

    #[test]
    fn the_settings_name_the_upscaler_and_dlss_falls_back_with_a_reason() {
        let none = no_dlss_anywhere();
        assert_eq!(choose(Choice::Off, &none), (None, None));
        assert_eq!(choose(Choice::Fsr1, &none), (Some(Kind::Fsr1), None));
        assert_eq!(
            choose(Choice::Temporal, &none),
            (Some(Kind::Temporal), None)
        );
        let (kind, note) = choose(Choice::Dlss, &none);
        assert_eq!(kind, Some(Kind::Temporal));
        let note = note.expect("a reason");
        assert!(note.contains("nvngx_dlss.dll was not found"), "{note}");
        assert!(note.contains("temporal upscaler instead"), "{note}");
    }

    #[test]
    fn a_configured_dlss_library_keeps_the_choice_when_ngx_loads() {
        let dir = std::env::temp_dir().join("leonida-runtime-dlss-file");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("nvngx_dlss.dll");
        std::fs::write(&file, b"MZ").unwrap();
        let config = DlssConfig {
            dlss_dll: Some(file),
            ..DlssConfig::default()
        };
        let (kind, note) = choose(Choice::Dlss, &config);
        let note = note.expect("a note either way");
        match upscale::dlss::probe(&config) {
            DlssStatus::Present { .. } => {
                assert_eq!(kind, Some(Kind::Dlss));
                assert!(note.contains("temporal upscaler takes over"), "{note}");
            }
            DlssStatus::Unavailable(_) => {
                assert_eq!(kind, Some(Kind::Temporal));
                assert!(note.contains("not available"), "{note}");
            }
        }
    }

    #[test]
    fn a_dlss_dispatch_that_cannot_start_dlss_takes_the_temporal_path_and_traces_why() {
        let access = fake();
        let mut contexts = HashMap::new();
        let error = run(
            &mut contexts,
            Kind::Dlss,
            &no_dlss_anywhere(),
            None,
            &access,
            &dispatch_inputs(),
        )
        .unwrap_err();
        assert!(error.0.contains("compute device"), "{error:?}");
        let notes = shims::trace::notes();
        assert!(
            notes
                .iter()
                .any(|n| n.contains("DLSS could not start") && n.contains("was not found")),
            "{notes:?}"
        );
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
        run(
            &mut contexts,
            Kind::Fsr1,
            &DlssConfig::default(),
            None,
            &access,
            &dispatch_inputs(),
        )
        .unwrap();
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
        let error = run(
            &mut contexts,
            Kind::Fsr1,
            &DlssConfig::default(),
            None,
            &access,
            &none,
        )
        .unwrap_err();
        assert!(error.0.contains("no colour"), "{error:?}");
        let mut missing = dispatch_inputs();
        missing.depth.resource = 5;
        let error = run(
            &mut contexts,
            Kind::Fsr1,
            &DlssConfig::default(),
            None,
            &access,
            &missing,
        )
        .unwrap_err();
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
            &DlssConfig::default(),
            None,
            &access,
            &dispatch_inputs(),
        )
        .unwrap_err();
        assert!(error.0.contains("compute device"), "{error:?}");
    }

    #[test]
    #[ignore = "needs an NVIDIA GPU and nvngx_dlss.dll (set LEONIDA_DLSS_DLL)"]
    fn a_dlss_dispatch_runs_dlss_and_writes_the_output() {
        let (render, output) = ([640u32, 360], [1280u32, 720]);
        let texels = (render[0] * render[1]) as usize;
        let access = Fake {
            color: Image::from_rgba_f32(
                render[0],
                render[1],
                PixelFormat::Rgba16Float,
                &[0.3, 0.5, 0.7, 1.0].repeat(texels),
            )
            .unwrap(),
            depth_and_motion: Some((
                Image {
                    width: render[0],
                    height: render[1],
                    format: PixelFormat::R32Float,
                    data: std::iter::repeat_n(0.5f32.to_le_bytes(), texels)
                        .flatten()
                        .collect(),
                },
                Image {
                    width: render[0],
                    height: render[1],
                    format: PixelFormat::Rg32Float,
                    data: vec![0; texels * 8],
                },
            )),
            written: StdMutex::new(Vec::new()),
        };
        let mut inputs = dispatch_inputs();
        inputs.depth.resource = 2;
        inputs.motion_vectors.resource = 3;
        inputs.render_size = render;
        inputs.upscale_size = output;
        inputs.sharpness = None;
        let mut contexts = HashMap::new();
        let config = DlssConfig::default();
        let (kind, note) = choose(Choice::Dlss, &config);
        assert_eq!(kind, Some(Kind::Dlss), "{note:?}");
        run(&mut contexts, Kind::Dlss, &config, None, &access, &inputs).unwrap();
        assert!(matches!(contexts.get(&7), Some(Held::Dlss(_))));
        let written = access.written.lock().unwrap();
        let (target, image) = &written[0];
        assert_eq!(
            (*target, image.width, image.height),
            (9, output[0], output[1])
        );
        let rgba = image
            .as_ref()
            .rgba_f32(output[0], output[1], "out")
            .unwrap();
        assert!(rgba.iter().all(|v| v.is_finite()));
        let mean = rgba.chunks_exact(4).map(|p| p[1]).sum::<f32>() / (output[0] * output[1]) as f32;
        assert!((mean - 0.5).abs() < 0.1, "mean {mean}");
    }
}
