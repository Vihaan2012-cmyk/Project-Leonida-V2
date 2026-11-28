use std::collections::HashMap;
use std::ffi::c_void;
use std::time::Duration;

use ash::vk;

use super::names::{self, resource_flags};
use super::pacing::{self, Shown};
use super::params::{self, CreateParams, EvalOptions, IDENTITY, Mat4, Resources, Sink};
use super::support;
use crate::dlss::ffi::{self, ResourceVk};

#[derive(Debug, Clone, PartialEq)]
enum Value {
    U32(u32),
    F32(f32),
    Ptr(usize),
}

#[derive(Default)]
struct Recording {
    values: HashMap<String, Value>,
    order: Vec<String>,
}

impl Recording {
    fn u32(&self, name: &str) -> u32 {
        match self.values.get(name) {
            Some(Value::U32(v)) => *v,
            other => panic!("{name} is {other:?}"),
        }
    }

    fn f32(&self, name: &str) -> f32 {
        match self.values.get(name) {
            Some(Value::F32(v)) => *v,
            other => panic!("{name} is {other:?}"),
        }
    }

    fn ptr(&self, name: &str) -> usize {
        match self.values.get(name) {
            Some(Value::Ptr(v)) => *v,
            other => panic!("{name} is {other:?}"),
        }
    }
}

impl Sink for Recording {
    fn set_u32(&mut self, name: &str, value: u32) {
        self.order.push(name.to_string());
        self.values.insert(name.to_string(), Value::U32(value));
    }

    fn set_f32(&mut self, name: &str, value: f32) {
        self.order.push(name.to_string());
        self.values.insert(name.to_string(), Value::F32(value));
    }

    unsafe fn set_ptr(&mut self, name: &str, value: *mut c_void) {
        self.order.push(name.to_string());
        self.values
            .insert(name.to_string(), Value::Ptr(value as usize));
    }
}

fn resource() -> ResourceVk {
    ResourceVk::image_view(
        vk::ImageView::null(),
        vk::Image::null(),
        vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        },
        vk::Format::R16G16B16A16_SFLOAT,
        64,
        64,
        false,
    )
}

fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            out[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

#[test]
fn creation_writes_sizes_format_and_resource_hints() {
    let create = CreateParams::new([3840, 2160], vk::Format::R16G16B16A16_SFLOAT, [2560, 1440]);
    create.check().unwrap();
    let mut sink = Recording::default();
    params::apply_create(&mut sink, &create);
    assert_eq!(sink.u32(ffi::names::WIDTH), 3840);
    assert_eq!(sink.u32(ffi::names::HEIGHT), 2160);
    assert_eq!(sink.u32(names::WIDTH), 3840);
    assert_eq!(sink.u32(names::HEIGHT), 2160);
    assert_eq!(sink.u32(names::INTERNAL_WIDTH), 2560);
    assert_eq!(sink.u32(names::INTERNAL_HEIGHT), 1440);
    assert_eq!(
        sink.u32(names::BACKBUFFER_FORMAT),
        vk::Format::R16G16B16A16_SFLOAT.as_raw() as u32
    );
    assert_eq!(sink.u32(ffi::names::CREATION_NODE_MASK), 1);
    assert_eq!(sink.u32(ffi::names::VISIBILITY_NODE_MASK), 1);
    assert_eq!(sink.u32(names::DYNAMIC_RESOLUTION), 0);
    let never = sink.u32(names::NEVER_PROVIDED_FLAGS);
    for flag in [
        resource_flags::HUDLESS,
        resource_flags::UI,
        resource_flags::UI_ALPHA,
    ] {
        assert_ne!(never & flag, 0);
    }
    assert_eq!(
        never & (resource_flags::BACKBUFFER | resource_flags::DEPTH | resource_flags::MVECS),
        0
    );
}

#[test]
fn creation_sizes_outside_the_documented_range_are_refused() {
    let format = vk::Format::R8G8B8A8_UNORM;
    assert!(
        CreateParams::new([3, 100], format, [3, 100])
            .check()
            .is_err()
    );
    assert!(
        CreateParams::new([7681, 100], format, [100, 100])
            .check()
            .is_err()
    );
    assert!(
        CreateParams::new([1920, 1080], vk::Format::UNDEFINED, [1920, 1080])
            .check()
            .is_err()
    );
    assert!(
        CreateParams::new([4, 4], format, [7680, 7680])
            .check()
            .is_ok()
    );
}

#[test]
fn every_resource_is_written_and_an_absent_one_is_cleared_with_null() {
    let (back, depth, motion, out) = (resource(), resource(), resource(), resource());
    let resources = Resources {
        backbuffer: Some(&back),
        depth: Some(&depth),
        motion_vectors: Some(&motion),
        output_interpolated: Some(&out),
        ..Resources::default()
    };
    assert_eq!(resources.missing_required(), None);
    let mut sink = Recording::default();
    // SAFETY: the recording keeps the addresses as numbers and never reads them.
    unsafe { params::apply_resources(&mut sink, &resources) };
    assert_eq!(sink.ptr(names::BACKBUFFER), &back as *const _ as usize);
    assert_eq!(sink.ptr(names::DEPTH), &depth as *const _ as usize);
    assert_eq!(sink.ptr(names::MVECS), &motion as *const _ as usize);
    assert_eq!(
        sink.ptr(names::OUTPUT_INTERPOLATED),
        &out as *const _ as usize
    );
    for absent in [
        names::HUDLESS,
        names::UI,
        names::UI_ALPHA,
        names::BIDIRECTIONAL_DISTORTION_FIELD,
        names::OUTPUT_REAL,
        names::OUTPUT_DISABLE_INTERPOLATION,
    ] {
        assert_eq!(sink.ptr(absent), 0, "{absent}");
    }
}

#[test]
fn a_missing_required_resource_is_named() {
    let back = resource();
    let resources = Resources {
        backbuffer: Some(&back),
        ..Resources::default()
    };
    assert_eq!(resources.missing_required(), Some("depth"));
    assert_eq!(Resources::default().missing_required(), Some("backbuffer"));
}

#[test]
fn engine_data_is_written_with_flags_as_integers_and_matrices_as_pointers() {
    let mut options = EvalOptions {
        reset: true,
        depth_inverted: true,
        hdr: false,
        jitter_offset: [0.25, -0.5],
        mvec_scale: [1920.0, -1080.0],
        camera_near: 0.1,
        camera_far: 800.0,
        camera_fov: 1.0,
        camera_aspect_ratio: 1.5,
        ..EvalOptions::default()
    };
    options.clip_to_prev_clip[3][0] = 0.5;
    let mut sink = Recording::default();
    // SAFETY: the recording keeps the addresses as numbers and never reads them.
    unsafe { params::apply_options(&mut sink, &options) };
    assert_eq!(sink.u32(names::RESET), 1);
    assert_eq!(sink.u32(names::DEPTH_INVERTED), 1);
    assert_eq!(sink.u32(names::COLOR_BUFFERS_HDR), 0);
    assert_eq!(sink.u32(names::CAMERA_MOTION_INCLUDED), 1);
    assert_eq!(sink.u32(names::MULTI_FRAME_COUNT), 1);
    assert_eq!(sink.u32(names::MULTI_FRAME_INDEX), 1);
    assert_eq!(sink.f32(names::JITTER_OFFSET_X), 0.25);
    assert_eq!(sink.f32(names::JITTER_OFFSET_Y), -0.5);
    assert_eq!(sink.f32(names::MVEC_SCALE_X), 1920.0);
    assert_eq!(sink.f32(names::MVEC_SCALE_Y), -1080.0);
    assert_eq!(sink.f32(names::CAMERA_NEAR), 0.1);
    assert_eq!(sink.f32(names::CAMERA_FAR), 800.0);
    assert_eq!(sink.f32(names::CAMERA_FOV), 1.0);
    assert_eq!(sink.f32(names::CAMERA_ASPECT_RATIO), 1.5);
    assert_eq!(sink.f32(names::CAMERA_FWD[2]), 1.0);
    assert_eq!(sink.f32(names::CAMERA_UP[1]), 1.0);
    assert_eq!(sink.f32(names::CAMERA_RIGHT[0]), 1.0);
    assert_eq!(
        sink.ptr(names::CLIP_TO_PREV_CLIP),
        std::ptr::addr_of!(options.clip_to_prev_clip) as usize
    );
    assert_eq!(
        sink.ptr(names::PREV_CLIP_TO_CLIP),
        std::ptr::addr_of!(options.prev_clip_to_clip) as usize
    );
    assert_eq!(
        sink.ptr(names::CAMERA_VIEW_TO_CLIP),
        std::ptr::addr_of!(options.camera_view_to_clip) as usize
    );
    assert_ne!(sink.ptr(names::CLIP_TO_LENS_CLIP), 0);
}

#[test]
fn engine_data_writes_every_subrect_with_the_header_names() {
    let options = EvalOptions {
        backbuffer_subrect: params::Subrect {
            x: 8,
            y: 16,
            width: 1904,
            height: 1048,
        },
        ..EvalOptions::default()
    };
    let mut sink = Recording::default();
    // SAFETY: as above.
    unsafe { params::apply_options(&mut sink, &options) };
    assert_eq!(sink.u32("DLSSG.InputBackbufferSubrectBaseX"), 8);
    assert_eq!(sink.u32("DLSSG.InputBackbufferSubrectBaseY"), 16);
    assert_eq!(sink.u32("DLSSG.InputBackbufferSubrectWidth"), 1904);
    assert_eq!(sink.u32("DLSSG.InputBackbufferSubrectHeight"), 1048);
    for name in [
        "DLSSG.MVecsSubrectWidth",
        "DLSSG.DepthSubrectHeight",
        "DLSSG.HUDLessSubrectBaseX",
        "DLSSG.UISubrectBaseY",
        "DLSSG.UIAlphaSubrectWidth",
        "DLSSG.OutputInterpolatedSubrectHeight",
        "DLSSG.OutputRealSubrectBaseX",
    ] {
        assert_eq!(sink.u32(name), 0, "{name}");
    }
}

#[test]
fn the_multi_frame_index_must_lie_in_the_count() {
    let mut options = EvalOptions::default();
    assert!(options.check().is_ok());
    options.multi_frame_count = 3;
    options.multi_frame_index = 3;
    assert!(options.check().is_ok());
    options.multi_frame_index = 4;
    assert!(options.check().is_err());
    options.multi_frame_index = 0;
    assert!(options.check().is_err());
    options = EvalOptions {
        mvec_scale: [0.0, 1.0],
        ..EvalOptions::default()
    };
    assert!(options.check().is_err());
    options = EvalOptions {
        mvec_scale: [f32::NAN, 1.0],
        ..EvalOptions::default()
    };
    assert!(options.check().is_err());
}

#[test]
fn the_projection_maps_near_and_far_to_the_clip_range() {
    let clip_depth = |m: &Mat4, z: f32| {
        let v = [0.0f32, 0.0, z, 1.0];
        let mut out = [0.0f32; 4];
        for (j, o) in out.iter_mut().enumerate() {
            *o = (0..4).map(|i| v[i] * m[i][j]).sum();
        }
        out[2] / out[3]
    };
    let normal = params::perspective(1.0, 16.0 / 9.0, 0.1, 100.0, false);
    assert!(clip_depth(&normal, 0.1).abs() < 1e-5);
    assert!((clip_depth(&normal, 100.0) - 1.0).abs() < 1e-5);
    let reversed = params::perspective(1.0, 16.0 / 9.0, 0.1, 100.0, true);
    assert!((clip_depth(&reversed, 0.1) - 1.0).abs() < 1e-5);
    assert!(clip_depth(&reversed, 100.0).abs() < 1e-5);
    let infinite = params::perspective(1.0, 1.0, 0.1, f32::INFINITY, true);
    assert!((clip_depth(&infinite, 0.1) - 1.0).abs() < 1e-5);
    assert!(clip_depth(&infinite, 1.0e6) < 1e-6);
}

#[test]
fn the_inverse_projection_undoes_the_projection() {
    let projection = params::perspective(0.9, 1.7, 0.2, 500.0, true);
    let inverse = params::invert(&projection).unwrap();
    let product = multiply(&projection, &inverse);
    for i in 0..4 {
        for j in 0..4 {
            let want = IDENTITY[i][j];
            assert!((product[i][j] - want).abs() < 1e-4, "{product:?}");
        }
    }
    assert!(params::invert(&[[0.0; 4]; 4]).is_none());
}

#[test]
fn engine_data_from_a_field_of_view_carries_the_projection_pair() {
    let options = params::options_from_projection([1920, 1080], 0.8, 0.1, 1000.0, true);
    assert!((options.camera_aspect_ratio - 16.0 / 9.0).abs() < 1e-6);
    assert!(options.depth_inverted && options.camera_motion_included);
    let product = multiply(&options.camera_view_to_clip, &options.clip_to_camera_view);
    for (i, row) in product.iter().enumerate() {
        assert!((row[i] - 1.0).abs() < 1e-4);
    }
    assert_eq!(options.clip_to_prev_clip, IDENTITY);
    let infinite = params::options_from_projection([1280, 720], 0.8, 0.1, f32::INFINITY, false);
    assert!(infinite.camera_far.is_finite());
}

#[test]
fn feature_ids_and_flag_bits_match_the_headers() {
    assert_eq!(names::FEATURE_FRAME_GENERATION, 11);
    assert_eq!(resource_flags::BACKBUFFER, 1);
    assert_eq!(resource_flags::MVECS, 2);
    assert_eq!(resource_flags::DEPTH, 4);
    assert_eq!(resource_flags::HUDLESS, 8);
    assert_eq!(resource_flags::UI, 16);
    assert_eq!(resource_flags::UI_ALPHA, 128);
    assert_eq!(resource_flags::OUTPUT_INTERPOLATED, 1 << 24);
    assert_eq!(resource_flags::OUTPUT_DISABLE_INTERPOLATION, 1 << 26);
}

#[test]
fn two_times_shows_the_generated_frame_then_the_real_one_half_an_interval_apart() {
    let plan = pacing::schedule(Duration::from_millis(32), 1, Duration::from_millis(2));
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].shown, Shown::Generated { index: 1 });
    assert_eq!(plan[1].shown, Shown::Real);
    assert_eq!(plan[0].delay, Duration::from_millis(2));
    assert_eq!(plan[1].delay, Duration::from_millis(18));
    assert_eq!(pacing::frames_per_real(1), 2);
}

#[test]
fn the_two_times_schedule_is_the_one_the_presenters_pacer_makes() {
    let mut pacer = crate::framegen::Pacer::new(0.5).with_guard(Duration::from_millis(2));
    assert!(pacer.on_real_frame(Duration::from_millis(0)).is_none());
    let want = pacer.on_real_frame(Duration::from_millis(32)).unwrap();
    let interval = pacer.interval().unwrap();
    let plan = pacing::schedule(interval, 1, Duration::from_millis(2));
    let close = |a: Duration, b: Duration| a.abs_diff(b) < Duration::from_micros(2);
    assert!(close(plan[0].delay, want.generated_delay));
    assert!(close(plan[1].delay, want.real_delay));
}

#[test]
fn multi_frame_generation_spaces_all_frames_equally_and_shows_the_real_one_last() {
    let interval = Duration::from_millis(40);
    let plan = pacing::schedule(interval, 3, Duration::ZERO);
    assert_eq!(plan.len(), 4);
    for (i, present) in plan.iter().enumerate() {
        let shown = if i < 3 {
            Shown::Generated {
                index: i as u32 + 1,
            }
        } else {
            Shown::Real
        };
        assert_eq!(present.shown, shown);
        assert_eq!(present.delay, Duration::from_millis(10 * i as u64));
    }
    assert_eq!(pacing::frames_per_real(3), 4);
    assert!((pacing::phase(1, 3) - 0.25).abs() < 1e-6);
    assert!((pacing::phase(3, 3) - 0.75).abs() < 1e-6);
    assert!((pacing::phase(1, 1) - 0.5).abs() < 1e-6);
}

#[test]
fn the_guard_never_exceeds_a_quarter_of_the_spacing() {
    let plan = pacing::schedule(Duration::from_millis(16), 1, Duration::from_millis(50));
    assert_eq!(plan[0].delay, Duration::from_millis(2));
}

#[test]
fn the_library_is_found_in_the_named_places_only() {
    let dir = std::env::temp_dir().join("leonida-dlssg-locate");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(support::LIBRARY_NAME);
    std::fs::write(&file, b"MZ fake").unwrap();
    let found = support::locate_with(Some(file.clone()), None).unwrap();
    assert_eq!(found.path, file);
    let wrong_name = dir.join("other.dll");
    std::fs::write(&wrong_name, b"MZ fake").unwrap();
    assert!(support::locate_with(Some(wrong_name), None).is_none());
    let beside = support::locate_with(None, Some(&dir.join("nvngx_dlss.dll"))).unwrap();
    assert_eq!(beside.path, file);
    assert_eq!(support::find_in(&dir, 0), Some(file));
    let not_a_dll = dir.join("sub");
    std::fs::create_dir_all(&not_a_dll).unwrap();
    std::fs::write(not_a_dll.join(support::LIBRARY_NAME), b"text").unwrap();
    assert_eq!(support::find_in(&not_a_dll, 0), None);
}

#[test]
fn capabilities_explain_why_frame_generation_is_unavailable() {
    let available = support::Capabilities {
        available: true,
        needs_updated_driver: false,
        min_driver: None,
        feature_init_result: None,
        multi_frame_count_max: 1,
    };
    assert_eq!(available.reason(), None);
    assert_eq!(available.max_multiplier(), 2);
    let old_driver = support::Capabilities {
        available: false,
        needs_updated_driver: true,
        min_driver: Some((520, 0)),
        ..available.clone()
    };
    assert!(old_driver.reason().unwrap().contains("520.0"));
    let old_gpu = support::Capabilities {
        available: false,
        feature_init_result: Some(ffi::RESULT_FAIL_FEATURE_NOT_SUPPORTED),
        ..available.clone()
    };
    let reason = old_gpu.reason().unwrap();
    assert!(reason.contains("RTX 40"), "{reason}");
    assert!(reason.contains("FeatureNotSupported"), "{reason}");
    let four_times = support::Capabilities {
        multi_frame_count_max: 3,
        ..available
    };
    assert_eq!(four_times.max_multiplier(), 4);
}

#[test]
fn extension_lists_are_merged_without_repeats() {
    let a = vec!["VK_A".to_string(), "VK_B".to_string()];
    let b = vec!["VK_B".to_string(), "VK_C".to_string()];
    assert_eq!(support::union(&a, &b), ["VK_A", "VK_B", "VK_C"]);
}
