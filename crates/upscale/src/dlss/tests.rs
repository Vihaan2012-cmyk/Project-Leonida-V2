use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;

use super::ffi::{self, names, slots};
use super::ngx::ParamMap;
use super::*;
use crate::fsr1::Fsr1Upscaler;
use crate::{Image, ImageRef, PixelFormat, UpscaleError, UpscaleInputs, Upscaler, UpscalerKind};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("leonida-dlss-tests").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn parameter_names_are_the_ngx_strings() {
    assert_eq!(names::WIDTH, "Width");
    assert_eq!(names::OUT_HEIGHT, "OutHeight");
    assert_eq!(names::PERF_QUALITY_VALUE, "PerfQualityValue");
    assert_eq!(
        names::DLSS_FEATURE_CREATE_FLAGS,
        "DLSS.Feature.Create.Flags"
    );
    assert_eq!(
        names::DLSS_ENABLE_OUTPUT_SUBRECTS,
        "DLSS.Enable.Output.Subrects"
    );
    assert_eq!(names::JITTER_OFFSET_X, "Jitter.Offset.X");
    assert_eq!(names::MV_SCALE_Y, "MV.Scale.Y");
    assert_eq!(names::PRE_EXPOSURE, "DLSS.Pre.Exposure");
    assert_eq!(names::EXPOSURE_SCALE, "DLSS.Exposure.Scale");
    assert_eq!(
        names::RENDER_SUBRECT_WIDTH,
        "DLSS.Render.Subrect.Dimensions.Width"
    );
    assert_eq!(names::SUPER_SAMPLING_AVAILABLE, "SuperSampling.Available");
    assert_eq!(
        names::OPTIMAL_SETTINGS_CALLBACK,
        "DLSSOptimalSettingsCallback"
    );
    assert_eq!(
        names::DYNAMIC_MAX_RENDER_WIDTH,
        "DLSS.Get.Dynamic.Max.Render.Width"
    );
}

#[derive(Debug, PartialEq)]
enum Call {
    SetF32(String, f32),
    SetU32(String, u32),
    SetI32(String, i32),
    SetU64(String, u64),
    SetPtr(String, usize),
    GetU32(String),
    GetI32(String),
    GetF32(String),
    GetPtr(String),
}

thread_local! {
    static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
}

fn text(name: *const c_char) -> String {
    // SAFETY: the tests pass NUL-terminated names.
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

fn log(call: Call) {
    CALLS.with(|c| c.borrow_mut().push(call));
}

extern "C" fn nop(_: *mut ffi::Parameter, _: *const c_char) {}
extern "C" fn set_u64(_: *mut ffi::Parameter, n: *const c_char, v: u64) {
    log(Call::SetU64(text(n), v));
}
extern "C" fn set_f32(_: *mut ffi::Parameter, n: *const c_char, v: f32) {
    log(Call::SetF32(text(n), v));
}
extern "C" fn set_f64(_: *mut ffi::Parameter, _: *const c_char, _: f64) {
    panic!("the double overload must not be used");
}
extern "C" fn set_u32(_: *mut ffi::Parameter, n: *const c_char, v: u32) {
    log(Call::SetU32(text(n), v));
}
extern "C" fn set_i32(_: *mut ffi::Parameter, n: *const c_char, v: i32) {
    log(Call::SetI32(text(n), v));
}
extern "C" fn set_ptr(_: *mut ffi::Parameter, n: *const c_char, v: *mut c_void) {
    log(Call::SetPtr(text(n), v as usize));
}
extern "C" fn get_u32(_: *mut ffi::Parameter, n: *const c_char, out: *mut u32) -> u32 {
    log(Call::GetU32(text(n)));
    // SAFETY: the caller passes a valid out-pointer.
    unsafe { *out = 77 };
    ffi::RESULT_SUCCESS
}
extern "C" fn get_i32(_: *mut ffi::Parameter, n: *const c_char, out: *mut i32) -> u32 {
    log(Call::GetI32(text(n)));
    if text(n) == "Missing" {
        return ffi::RESULT_FAIL_INVALID_PARAMETER;
    }
    // SAFETY: the caller passes a valid out-pointer.
    unsafe { *out = -5 };
    ffi::RESULT_SUCCESS
}
extern "C" fn get_f32(_: *mut ffi::Parameter, n: *const c_char, out: *mut f32) -> u32 {
    log(Call::GetF32(text(n)));
    // SAFETY: the caller passes a valid out-pointer.
    unsafe { *out = 0.25 };
    ffi::RESULT_SUCCESS
}
extern "C" fn get_ptr(_: *mut ffi::Parameter, n: *const c_char, out: *mut *mut c_void) -> u32 {
    log(Call::GetPtr(text(n)));
    // SAFETY: the caller passes a valid out-pointer.
    unsafe { *out = 0x1000 as *mut c_void };
    ffi::RESULT_SUCCESS
}
extern "C" fn get_unused(_: *mut ffi::Parameter, _: *const c_char, _: *mut c_void) -> u32 {
    panic!("an overload the wrapper must not use");
}

/// A fake parameter object laid out like the C++ one: a pointer to a table of 17 slots.
struct FakeParameter {
    table: [usize; 17],
    vptr: *const usize,
}

impl FakeParameter {
    fn new() -> Box<FakeParameter> {
        let mut table = [0usize; 17];
        table[slots::SET_U64] = set_u64 as *const () as usize;
        table[slots::SET_F32] = set_f32 as *const () as usize;
        table[2] = set_f64 as *const () as usize;
        table[slots::SET_U32] = set_u32 as *const () as usize;
        table[slots::SET_I32] = set_i32 as *const () as usize;
        table[5] = nop as *const () as usize;
        table[6] = nop as *const () as usize;
        table[slots::SET_PTR] = set_ptr as *const () as usize;
        table[slots::GET_U64] = get_unused as *const () as usize;
        table[slots::GET_F32] = get_f32 as *const () as usize;
        table[10] = get_unused as *const () as usize;
        table[slots::GET_U32] = get_u32 as *const () as usize;
        table[slots::GET_I32] = get_i32 as *const () as usize;
        table[13] = get_unused as *const () as usize;
        table[14] = get_unused as *const () as usize;
        table[slots::GET_PTR] = get_ptr as *const () as usize;
        table[slots::RESET] = nop as *const () as usize;
        let mut boxed = Box::new(FakeParameter {
            table,
            vptr: std::ptr::null(),
        });
        boxed.vptr = boxed.table.as_ptr();
        boxed
    }

    fn map(&mut self) -> ParamMap {
        // SAFETY: `vptr` is the first field and points at the table, which lives as long as `self`.
        unsafe { ParamMap::from_raw(&mut self.vptr as *mut *const usize as *mut ffi::Parameter) }
    }
}

#[test]
fn parameters_go_through_the_matching_virtual_table_slots() {
    CALLS.with(|c| c.borrow_mut().clear());
    let mut fake = FakeParameter::new();
    let mut map = fake.map();
    map.set_u32(names::WIDTH, 1280);
    map.set_i32(names::PERF_QUALITY_VALUE, 2);
    map.set_f32(names::JITTER_OFFSET_X, -0.25);
    map.set_u64("Big", 1 << 40);
    // SAFETY: the fake records the pointer and never dereferences it.
    unsafe { map.set_ptr(names::COLOR, 0x40 as *mut c_void) };
    assert_eq!(map.get_u32(names::OUT_WIDTH), Some(77));
    assert_eq!(map.get_i32(names::SUPER_SAMPLING_AVAILABLE), Some(-5));
    assert_eq!(map.get_i32("Missing"), None);
    assert_eq!(map.get_f32(names::SHARPNESS), Some(0.25));
    assert_eq!(
        map.get_ptr(names::OPTIMAL_SETTINGS_CALLBACK),
        Some(0x1000 as *mut c_void)
    );
    let calls = CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()));
    assert_eq!(
        calls,
        vec![
            Call::SetU32("Width".into(), 1280),
            Call::SetI32("PerfQualityValue".into(), 2),
            Call::SetF32("Jitter.Offset.X".into(), -0.25),
            Call::SetU64("Big".into(), 1 << 40),
            Call::SetPtr("Color".into(), 0x40),
            Call::GetU32("OutWidth".into()),
            Call::GetI32("SuperSampling.Available".into()),
            Call::GetI32("Missing".into()),
            Call::GetF32("Sharpness".into()),
            Call::GetPtr("DLSSOptimalSettingsCallback".into()),
        ]
    );
}

#[test]
fn availability_explains_why_dlss_is_not_available() {
    let base = Availability {
        available: false,
        needs_updated_driver: false,
        min_driver: None,
        feature_init_result: None,
    };
    assert_eq!(
        Availability {
            available: true,
            ..base.clone()
        }
        .reason(),
        None
    );
    let old = Availability {
        needs_updated_driver: true,
        min_driver: Some((470, 57)),
        ..base.clone()
    };
    assert!(old.reason().unwrap().contains("470.57"));
    let failed = Availability {
        feature_init_result: Some(ffi::RESULT_FAIL_FEATURE_NOT_SUPPORTED),
        ..base.clone()
    };
    assert!(failed.reason().unwrap().contains("FeatureNotSupported"));
    assert!(base.reason().unwrap().contains("unavailable"));
}

#[test]
fn without_a_configured_library_dlss_is_unavailable() {
    let status = probe(&DlssConfig::default());
    assert!(matches!(status, DlssStatus::Unavailable(r) if r.contains("no DLSS library")));
    assert!(matches!(
        DlssUpscaler::new(&DlssConfig::default()),
        Err(UpscaleError::Unavailable { name: "DLSS", .. })
    ));
}

#[test]
fn the_configured_file_must_be_a_library_named_for_ngx() {
    let dir = scratch("naming");
    let misnamed = dir.join("dlss_3.dll");
    std::fs::write(&misnamed, b"MZ").unwrap();
    let config = DlssConfig {
        dlss_dll: Some(misnamed),
        ngx_runtime: Some(dir.join("absent_nvngx.dll")),
    };
    assert!(matches!(
        probe(&config),
        DlssStatus::Unavailable(r) if r.contains("must be named nvngx_dlss.dll")
    ));
}

fn cpu_fsr(_: UpscalerKind) -> Box<dyn Upscaler> {
    Box::new(Fsr1Upscaler::cpu())
}

fn failing(reason: &str) -> impl FnOnce() -> Result<Box<dyn Upscaler>, UpscaleError> {
    let reason = reason.to_string();
    move || {
        Err(UpscaleError::Unavailable {
            name: "DLSS",
            reason,
        })
    }
}

#[test]
fn other_upscalers_are_built_without_a_notice() {
    let fsr = select_upscaler(
        UpscalerKind::Fsr1,
        UpscalerKind::Temporal,
        failing("unused"),
        cpu_fsr,
    );
    assert_eq!(fsr.kind, UpscalerKind::Fsr1);
    assert!(fsr.notice.is_none());
}

#[test]
fn a_dlss_failure_falls_back_with_the_reason() {
    for reason in [
        "no DLSS library is configured",
        "NVIDIA GeForce RTX 4060 is not an NVIDIA GPU",
        "NVSDK_NGX_VULKAN_CreateFeature failed: OutOfDate (0xbad0000c)",
    ] {
        let chosen = select_upscaler(
            UpscalerKind::Dlss,
            UpscalerKind::Fsr1,
            failing(reason),
            cpu_fsr,
        );
        assert_eq!(chosen.kind, UpscalerKind::Fsr1);
        let notice = chosen.notice.expect("a notice names the fallback");
        assert!(notice.contains(reason), "{notice}");
        assert!(notice.contains("AMD FSR 1"), "{notice}");
        assert!(notice.starts_with("DLSS is not available"), "{notice}");
    }
}

#[test]
fn dlss_as_its_own_fallback_becomes_the_temporal_upscaler() {
    let kinds = RefCell::new(Vec::new());
    let chosen = select_upscaler(
        UpscalerKind::Dlss,
        UpscalerKind::Dlss,
        failing("not NVIDIA"),
        |kind| {
            kinds.borrow_mut().push(kind);
            cpu_fsr(kind)
        },
    );
    assert_eq!(chosen.kind, UpscalerKind::Temporal);
    assert_eq!(*kinds.borrow(), [UpscalerKind::Temporal]);
}

#[test]
fn a_working_dlss_is_used_without_a_notice() {
    let chosen = select_upscaler(
        UpscalerKind::Dlss,
        UpscalerKind::Fsr1,
        || Ok(Box::new(Fsr1Upscaler::cpu()) as Box<dyn Upscaler>),
        |_| panic!("no fallback is built"),
    );
    assert_eq!(chosen.kind, UpscalerKind::Dlss);
    assert!(chosen.notice.is_none());
}

#[test]
fn the_real_factory_falls_back_when_no_library_is_configured() {
    struct NoCompute;
    impl crate::Compute for NoCompute {
        fn dispatch(
            &self,
            _: &[u32],
            _: &mut [&mut [u32]],
            _: [u32; 3],
        ) -> Result<(), UpscaleError> {
            Err(UpscaleError::BadInput {
                what: "test",
                reason: "unused".into(),
            })
        }
    }
    let chosen = create_with_fallback(
        UpscalerKind::Dlss,
        UpscalerKind::Fsr1,
        std::sync::Arc::new(NoCompute),
        &DlssConfig::default(),
        &DlssOptions::default(),
    );
    assert_eq!(chosen.kind, UpscalerKind::Fsr1);
    assert!(
        chosen
            .notice
            .unwrap()
            .contains("no DLSS library is configured")
    );
}

/// Whether this machine has NGX at all; the GPU tests skip when it does not.
fn ngx_present() -> bool {
    match ngx::Api::open(None) {
        Ok(_) => true,
        Err(reason) => {
            eprintln!("skipping: {reason}");
            false
        }
    }
}

#[test]
fn ngx_names_the_vulkan_extensions_it_needs() {
    if !ngx_present() {
        return;
    }
    let extensions = required_extensions_with(&DlssConfig::default()).expect("NGX answers");
    eprintln!("NGX instance extensions: {:?}", extensions.instance);
    eprintln!("NGX device extensions: {:?}", extensions.device);
    assert!(
        extensions
            .instance
            .iter()
            .chain(&extensions.device)
            .all(|name| name.starts_with("VK_")),
        "{extensions:?}"
    );
    assert_eq!(required_extensions(), extensions);
}

#[test]
fn ngx_initialises_on_the_nvidia_gpu_and_reports_availability() {
    if !ngx_present() {
        return;
    }
    let capabilities = match query_capabilities(&DlssConfig::default(), [1920, 1080]) {
        Ok(capabilities) => capabilities,
        Err(reason) => {
            eprintln!("skipping: {reason}");
            return;
        }
    };
    eprintln!("{capabilities:#?}");
    assert!(!capabilities.device_name.is_empty());
    assert_eq!(capabilities.modes.len(), DlssMode::ALL.len());
    if capabilities.availability.available {
        for support in &capabilities.modes {
            if support.offered() {
                let size = support.settings.unwrap().render_size;
                assert!(size[0] <= 1920 && size[1] <= 1080, "{support:?}");
            }
        }
        let dlaa = capabilities
            .modes
            .iter()
            .find(|m| m.mode == DlssMode::Dlaa)
            .unwrap();
        if dlaa.offered() {
            assert_eq!(dlaa.settings.unwrap().render_size, [1920, 1080]);
        }
    } else {
        assert!(capabilities.availability.reason().is_some());
    }
}

fn synthetic_frame(size: [u32; 2], value: f32) -> (Image, Vec<u8>, Vec<u8>) {
    let texels = (size[0] * size[1]) as usize;
    let color = Image::from_rgba_f32(
        size[0],
        size[1],
        PixelFormat::Rgba16Float,
        &[value, value, value, 1.0].repeat(texels),
    )
    .unwrap();
    let depth: Vec<u8> = std::iter::repeat_n(0.5f32.to_le_bytes(), texels)
        .flatten()
        .collect();
    let motion = vec![0u8; texels * 8];
    (color, depth, motion)
}

#[test]
fn a_real_dlss_library_upscales_a_flat_image() {
    let Some(dll) = std::env::var_os("LEONIDA_DLSS_DLL").map(PathBuf::from) else {
        eprintln!("skipping: LEONIDA_DLSS_DLL names no nvngx_dlss.dll");
        return;
    };
    let config = DlssConfig {
        dlss_dll: Some(dll),
        ngx_runtime: None,
    };
    let mut dlss = match DlssUpscaler::new(&config) {
        Ok(dlss) => dlss,
        Err(error) => panic!("DLSS could not start with the configured library: {error}"),
    };
    let output = [1280, 720];
    let settings = dlss
        .optimal_settings(output, DlssMode::Quality)
        .expect("optimal settings");
    let render = settings.render_size;
    assert!(render[0] > 0 && render[0] < output[0], "{settings:?}");
    let (color, depth, motion) = synthetic_frame(render, 0.5);
    let mut last = None;
    for frame in 0..4 {
        let jitter = crate::jitter_offset(frame, 8);
        let mut inputs = UpscaleInputs::new(color.as_ref(), render, output);
        inputs.depth = Some(ImageRef::new(
            render[0],
            render[1],
            PixelFormat::R32Float,
            &depth,
        ));
        inputs.motion = Some(ImageRef::new(
            render[0],
            render[1],
            PixelFormat::Rg32Float,
            &motion,
        ));
        inputs.jitter = jitter;
        inputs.reset = frame == 0;
        last = Some(dlss.upscale(&inputs).expect("DLSS evaluates"));
    }
    let result = last.unwrap();
    assert_eq!([result.width, result.height], output);
    let rgba = result
        .as_ref()
        .rgba_f32(output[0], output[1], "result")
        .unwrap();
    assert!(rgba.iter().all(|v| v.is_finite()));
    let mean = rgba.chunks_exact(4).map(|p| p[0]).sum::<f32>() / (output[0] * output[1]) as f32;
    assert!(
        (mean - 0.5).abs() < 0.1,
        "a flat 0.5 image came back as {mean}"
    );
}
