//! The FidelityFX loader's entry points, called with the public structures.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::sync::Mutex;

use shims::fidelityfx::api::*;
use shims::fidelityfx::integration::{UpscaleFailure, UpscaleInputs, set_upscaler};
use shims::fidelityfx::{jitter_offset, jitter_phase_count, upscale_ratio};

type Create = unsafe extern "win64" fn(*mut usize, *const Header, *const u8) -> u32;
type Destroy = unsafe extern "win64" fn(*mut usize, *const u8) -> u32;
type Query = unsafe extern "win64" fn(*mut usize, *mut Header) -> u32;
type Dispatch = unsafe extern "win64" fn(*mut usize, *const Header) -> u32;

fn shim<T: Copy>(name: &str) -> T {
    let address = shims::builtin()
        .into_iter()
        .find(|s| s.name == name && s.name.starts_with("ffx"))
        .unwrap_or_else(|| panic!("{name} registered"))
        .address;
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `T` is the function pointer type of the shim's shape.
    unsafe { std::mem::transmute_copy(&address) }
}

fn header(kind: u64, next: *const Header) -> Header {
    Header { kind, next }
}

/// A create chain: the upscale description, then the version.
struct Chain {
    upscale: Box<CreateContextDescUpscale>,
    _version: Box<CreateContextDescUpscaleVersion>,
}

fn chain(version: u32) -> Chain {
    let version = Box::new(CreateContextDescUpscaleVersion {
        header: header(CREATE_UPSCALE_VERSION, std::ptr::null()),
        version,
    });
    let upscale = Box::new(CreateContextDescUpscale {
        header: header(CREATE_UPSCALE, &version.header),
        flags: FLAG_HIGH_DYNAMIC_RANGE | FLAG_DEPTH_INVERTED | FLAG_AUTO_EXPOSURE,
        max_render_size: Dimensions2D {
            width: 1920,
            height: 1080,
        },
        max_upscale_size: Dimensions2D {
            width: 3840,
            height: 2160,
        },
        message: 0,
    });
    Chain {
        upscale,
        _version: version,
    }
}

fn create(chain: &Chain) -> (u32, usize) {
    let create: Create = shim("ffxCreateContext");
    let mut context = 0usize;
    // SAFETY: a valid chain and out pointer.
    let code = unsafe { create(&mut context, &chain.upscale.header, std::ptr::null()) };
    (code, context)
}

fn resource(id: usize, width: u32, height: u32) -> Resource {
    Resource {
        resource: id,
        description: ResourceDescription {
            kind: 2,
            format: 4,
            width,
            height,
            depth: 1,
            mip_count: 1,
            flags: 0,
            usage: 0,
        },
        state: 4,
    }
}

fn dispatch_desc(color: usize, output: usize) -> DispatchDescUpscale {
    DispatchDescUpscale {
        header: header(DISPATCH_UPSCALE, std::ptr::null()),
        command_list: 0x1000,
        color: resource(color, 1280, 720),
        depth: resource(0xD, 1280, 720),
        motion_vectors: resource(0xE, 1280, 720),
        exposure: Resource::default(),
        reactive: Resource::default(),
        transparency_and_composition: Resource::default(),
        output: resource(output, 1920, 1080),
        jitter_offset: FloatCoords2D { x: 0.25, y: -0.125 },
        motion_vector_scale: FloatCoords2D {
            x: 1280.0,
            y: 720.0,
        },
        render_size: Dimensions2D {
            width: 1280,
            height: 720,
        },
        upscale_size: Dimensions2D {
            width: 1920,
            height: 1080,
        },
        enable_sharpening: 1,
        sharpness: 0.5,
        frame_time_delta: 16.6,
        pre_exposure: 1.0,
        reset: 1,
        camera_near: 0.1,
        camera_far: 10000.0,
        camera_fov_angle_vertical: 1.0,
        view_space_to_meters_factor: 1.0,
        flags: 0,
    }
}

#[test]
fn contexts_are_created_and_destroyed() {
    let destroy: Destroy = shim("ffxDestroyContext");
    let c = chain(pack_version(4, 1, 1));
    let (code, mut context) = create(&c);
    assert_eq!(code, OK);
    assert_ne!(context, 0);
    // SAFETY: the handle just made.
    unsafe {
        assert_eq!(destroy(&mut context, std::ptr::null()), OK);
        assert_eq!(context, 0);
        assert_eq!(destroy(&mut context, std::ptr::null()), ERROR_PARAMETER);
    }
}

#[test]
fn the_version_description_is_checked() {
    assert_eq!(create(&chain(pack_version(4, 0, 0))).0, OK);
    assert_eq!(create(&chain(pack_version(4, 1, 2))).0, ERROR_PARAMETER);
    let create_fn: Create = shim("ffxCreateContext");
    let none = header(0x99, std::ptr::null());
    let mut context = 0usize;
    // SAFETY: valid header and out pointer.
    unsafe {
        assert_eq!(create_fn(&mut context, &none, std::ptr::null()), ERROR);
        assert_eq!(
            create_fn(std::ptr::null_mut(), &none, std::ptr::null()),
            ERROR_PARAMETER
        );
    }
}

#[test]
fn queries_follow_the_sdk() {
    let query: Query = shim("ffxQuery");
    let mut ratio = 0f32;
    let mut q = QueryDescUpscaleRatio {
        header: header(QUERY_UPSCALE_RATIO_FROM_QUALITY, std::ptr::null()),
        quality_mode: QUALITY_PERFORMANCE,
        out_ratio: &mut ratio,
    };
    // SAFETY: valid query structures throughout.
    unsafe {
        assert_eq!(query(std::ptr::null_mut(), &mut q.header), OK);
        assert_eq!(ratio, 2.0);
        q.quality_mode = 77;
        assert_eq!(query(std::ptr::null_mut(), &mut q.header), ERROR_PARAMETER);

        let (mut w, mut h) = (0u32, 0u32);
        let mut r = QueryDescRenderResolution {
            header: header(QUERY_RENDER_RESOLUTION_FROM_QUALITY, std::ptr::null()),
            display_width: 3840,
            display_height: 2160,
            quality_mode: QUALITY_QUALITY,
            out_render_width: &mut w,
            out_render_height: &mut h,
        };
        assert_eq!(query(std::ptr::null_mut(), &mut r.header), OK);
        assert_eq!((w, h), (2560, 1440));

        let mut count = 0i32;
        let mut p = QueryDescJitterPhaseCount {
            header: header(QUERY_JITTER_PHASE_COUNT, std::ptr::null()),
            render_width: 1280,
            display_width: 1920,
            out_phase_count: &mut count,
        };
        assert_eq!(query(std::ptr::null_mut(), &mut p.header), OK);
        assert_eq!(count, 18);
        assert_eq!(count, jitter_phase_count(1280, 1920));

        let (mut x, mut y) = (0f32, 0f32);
        let mut j = QueryDescJitterOffset {
            header: header(QUERY_JITTER_OFFSET, std::ptr::null()),
            index: 0,
            phase_count: 8,
            out_x: &mut x,
            out_y: &mut y,
        };
        assert_eq!(query(std::ptr::null_mut(), &mut j.header), OK);
        assert_eq!((x, y), (0.0, 1.0 / 3.0 - 0.5));
        assert_eq!(jitter_offset(0, 8), Some((x, y)));
        j.phase_count = 0;
        assert_eq!(query(std::ptr::null_mut(), &mut j.header), ERROR_PARAMETER);

        let mut unknown = header(0xBEEF, std::ptr::null());
        assert_eq!(
            query(std::ptr::null_mut(), &mut unknown),
            ERROR_UNKNOWN_DESCTYPE
        );
    }
    assert_eq!(upscale_ratio(QUALITY_NATIVE_AA), Some(1.0));
    assert_eq!(upscale_ratio(QUALITY_ULTRA_PERFORMANCE), Some(3.0));
}

#[test]
fn the_one_provider_is_listed() {
    let query: Query = shim("ffxQuery");
    let mut count = 0u64;
    let mut q = QueryDescGetVersions {
        header: header(QUERY_GET_VERSIONS, std::ptr::null()),
        create_desc_type: CREATE_UPSCALE,
        device: 0,
        output_count: &mut count,
        version_ids: std::ptr::null_mut(),
        version_names: std::ptr::null_mut(),
    };
    // SAFETY: valid query structures.
    unsafe {
        assert_eq!(query(std::ptr::null_mut(), &mut q.header), OK);
        assert_eq!(count, 1);
        let (mut id, mut name) = (0u64, std::ptr::null::<u8>());
        q.version_ids = &mut id;
        q.version_names = &mut name;
        assert_eq!(query(std::ptr::null_mut(), &mut q.header), OK);
        assert_eq!(id, PROVIDER_ID);
        assert_eq!(std::slice::from_raw_parts(name, 6), b"4.1.1\0");
    }
}

static SEEN: Mutex<Vec<UpscaleInputs>> = Mutex::new(Vec::new());

fn record(inputs: &UpscaleInputs) -> Result<(), UpscaleFailure> {
    SEEN.lock().unwrap().push(*inputs);
    if inputs.reset {
        Ok(())
    } else {
        Err(UpscaleFailure("refused".into()))
    }
}

// One test owns the registered upscaler: two would clobber each other.
#[test]
fn a_dispatch_reaches_the_upscaler_with_the_games_inputs() {
    let dispatch: Dispatch = shim("ffxDispatch");
    let c = chain(pack_version(4, 1, 1));
    let (_, mut context) = create(&c);
    let mut d = dispatch_desc(0xC0, 0x0F);

    // SAFETY: valid descriptions and a live context.
    unsafe {
        assert_eq!(
            dispatch(&mut context, &d.header),
            OK,
            "accepted with no upscaler registered"
        );

        set_upscaler(Some(record));
        assert_eq!(dispatch(&mut context, &d.header), OK);
        {
            let seen = SEEN.lock().unwrap();
            let got = seen.last().unwrap();
            assert_eq!(got.color.resource, 0xC0);
            assert_eq!(got.depth.resource, 0xD);
            assert_eq!(got.motion_vectors.resource, 0xE);
            assert_eq!(got.exposure.resource, 0);
            assert_eq!(got.output.resource, 0x0F);
            assert_eq!((got.color.width, got.color.height), (1280, 720));
            assert_eq!(got.jitter, [0.25, -0.125]);
            assert_eq!(got.motion_vector_scale, [1280.0, 720.0]);
            assert_eq!(got.render_size, [1280, 720]);
            assert_eq!(got.upscale_size, [1920, 1080]);
            assert_eq!(got.sharpness, Some(0.5));
            assert_eq!(got.frame_time_ms, 16.6);
            assert!(got.reset && got.hdr && got.depth_inverted && got.auto_exposure);
            assert_eq!(got.camera_far, 10000.0);
            assert_eq!(got.command_list, 0x1000);
            assert_eq!(got.context, context);
        }

        d.reset = 0;
        d.enable_sharpening = 0;
        assert_eq!(
            dispatch(&mut context, &d.header),
            ERROR_RUNTIME_ERROR,
            "the upscaler's failure"
        );
        assert_eq!(SEEN.lock().unwrap().last().unwrap().sharpness, None);

        d.output.resource = 0;
        assert_eq!(
            dispatch(&mut context, &d.header),
            ERROR_PARAMETER,
            "no output to write"
        );
        d.output.resource = 0x0F;
        d.render_size = Dimensions2D {
            width: 2560,
            height: 1440,
        };
        assert_eq!(
            dispatch(&mut context, &d.header),
            ERROR_PARAMETER,
            "past the context's maximum"
        );

        let other = header(0x1_0002, std::ptr::null());
        assert_eq!(dispatch(&mut context, &other), ERROR_UNKNOWN_DESCTYPE);
        let mut stale = 0usize;
        assert_eq!(
            dispatch(&mut stale, &d.header),
            ERROR_PARAMETER,
            "a context that does not exist"
        );
    }
    set_upscaler(None);
    let destroy: Destroy = shim("ffxDestroyContext");
    // SAFETY: the live context.
    assert_eq!(unsafe { destroy(&mut context, std::ptr::null()) }, OK);
}

#[test]
fn all_four_entry_points_are_provided() {
    let names: Vec<_> = shims::builtin()
        .into_iter()
        .filter(|s| s.name.starts_with("ffx"))
        .map(|s| s.name)
        .collect();
    assert_eq!(names.len(), 4);
}
