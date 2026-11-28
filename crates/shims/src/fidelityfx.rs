//! amd_fidelityfx_loader_xbox.dll: the upscaler entry points, routed to the PC upscalers.
//!
//! The game imports `ffxCreateContext`, `ffxDestroyContext`, `ffxQuery` and `ffxDispatch` from the loader.
//! They follow AMD's public FidelityFX API (the MIT headers the console upscaler, version 4.1.1, is built
//! from), so every structure the game passes is the public one: a chain of `{type, pNext}` headers.
//!
//! - `ffxCreateContext` needs the upscale description in the chain (`0x10000`). The console DLL also wants a
//!   backend description with the device; the PC upscalers need no device from here, so none is required.
//!   An upscaler version description (`0x1000b`) above 4.1.1 is refused, as the console DLL does.
//! - `ffxQuery` answers the CPU-only upscale queries as the SDK does (ratios, render resolution, jitter) and
//!   `GET_VERSIONS` with the one provider the console DLL lists, `0xf5a5ca1e01001001` "4.1.1".
//! - `ffxDispatch` of the upscale description (`0x10001`) hands the colour, depth and motion vector
//!   resources, the jitter and the camera values to [`integration::upscale_dispatch`].
//!
//! Not measured: dispatch descriptions other than the upscale one (they fail with the SDK's unknown
//! description code and a trace note) and the console backend descriptions in a create chain (ignored).

pub mod api {
    pub type ReturnCode = u32;

    pub const OK: ReturnCode = 0;
    pub const ERROR: ReturnCode = 1;
    pub const ERROR_UNKNOWN_DESCTYPE: ReturnCode = 2;
    pub const ERROR_RUNTIME_ERROR: ReturnCode = 3;
    pub const ERROR_PARAMETER: ReturnCode = 6;

    pub const QUERY_GET_VERSIONS: u64 = 4;
    pub const CREATE_UPSCALE: u64 = 0x1_0000;
    pub const DISPATCH_UPSCALE: u64 = 0x1_0001;
    pub const QUERY_UPSCALE_RATIO_FROM_QUALITY: u64 = 0x1_0002;
    pub const QUERY_RENDER_RESOLUTION_FROM_QUALITY: u64 = 0x1_0003;
    pub const QUERY_JITTER_PHASE_COUNT: u64 = 0x1_0004;
    pub const QUERY_JITTER_OFFSET: u64 = 0x1_0005;
    pub const QUERY_GPU_MEMORY_USAGE: u64 = 0x1_0006;
    pub const CREATE_UPSCALE_VERSION: u64 = 0x1_000b;

    pub const FLAG_HIGH_DYNAMIC_RANGE: u32 = 1;
    pub const FLAG_DEPTH_INVERTED: u32 = 1 << 3;
    pub const FLAG_AUTO_EXPOSURE: u32 = 1 << 5;

    pub const QUALITY_NATIVE_AA: u32 = 0;
    pub const QUALITY_QUALITY: u32 = 1;
    pub const QUALITY_BALANCED: u32 = 2;
    pub const QUALITY_PERFORMANCE: u32 = 3;
    pub const QUALITY_ULTRA_PERFORMANCE: u32 = 4;

    /// The version the console upscaler reports, and the newest one a create chain may name.
    pub const PROVIDER_ID: u64 = 0xf5a5_ca1e_0100_1001;
    pub const PROVIDER_NAME: &[u8] = b"4.1.1\0";
    pub const fn pack_version(major: u32, minor: u32, patch: u32) -> u32 {
        (major << 22) | (minor << 12) | patch
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Header {
        pub kind: u64,
        pub next: *const Header,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
    pub struct Dimensions2D {
        pub width: u32,
        pub height: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CreateContextDescUpscale {
        pub header: Header,
        pub flags: u32,
        pub max_render_size: Dimensions2D,
        pub max_upscale_size: Dimensions2D,
        pub message: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CreateContextDescUpscaleVersion {
        pub header: Header,
        pub version: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct QueryDescGetVersions {
        pub header: Header,
        pub create_desc_type: u64,
        pub device: usize,
        pub output_count: *mut u64,
        pub version_ids: *mut u64,
        pub version_names: *mut *const u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct QueryDescUpscaleRatio {
        pub header: Header,
        pub quality_mode: u32,
        pub out_ratio: *mut f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct QueryDescRenderResolution {
        pub header: Header,
        pub display_width: u32,
        pub display_height: u32,
        pub quality_mode: u32,
        pub out_render_width: *mut u32,
        pub out_render_height: *mut u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct QueryDescJitterPhaseCount {
        pub header: Header,
        pub render_width: i32,
        pub display_width: i32,
        pub out_phase_count: *mut i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct QueryDescJitterOffset {
        pub header: Header,
        pub index: i32,
        pub phase_count: i32,
        pub out_x: *mut f32,
        pub out_y: *mut f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct QueryDescGpuMemoryUsage {
        pub header: Header,
        pub usage: *mut MemoryUsage,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct MemoryUsage {
        pub total_bytes: u64,
        pub aliasable_bytes: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ResourceDescription {
        pub kind: u32,
        pub format: u32,
        pub width: u32,
        pub height: u32,
        pub depth: u32,
        pub mip_count: u32,
        pub flags: u32,
        pub usage: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Resource {
        pub resource: usize,
        pub description: ResourceDescription,
        pub state: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct FloatCoords2D {
        pub x: f32,
        pub y: f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct DispatchDescUpscale {
        pub header: Header,
        pub command_list: usize,
        pub color: Resource,
        pub depth: Resource,
        pub motion_vectors: Resource,
        pub exposure: Resource,
        pub reactive: Resource,
        pub transparency_and_composition: Resource,
        pub output: Resource,
        pub jitter_offset: FloatCoords2D,
        pub motion_vector_scale: FloatCoords2D,
        pub render_size: Dimensions2D,
        pub upscale_size: Dimensions2D,
        pub enable_sharpening: u8,
        pub sharpness: f32,
        pub frame_time_delta: f32,
        pub pre_exposure: f32,
        pub reset: u8,
        pub camera_near: f32,
        pub camera_far: f32,
        pub camera_fov_angle_vertical: f32,
        pub view_space_to_meters_factor: f32,
        pub flags: u32,
    }
}

/// The upscale ratio of a quality mode, as the SDK defines them.
pub fn upscale_ratio(quality_mode: u32) -> Option<f32> {
    match quality_mode {
        api::QUALITY_NATIVE_AA => Some(1.0),
        api::QUALITY_QUALITY => Some(1.5),
        api::QUALITY_BALANCED => Some(1.7),
        api::QUALITY_PERFORMANCE => Some(2.0),
        api::QUALITY_ULTRA_PERFORMANCE => Some(3.0),
        _ => None,
    }
}

fn halton(mut index: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

/// The sub-pixel offset of a frame in the SDK's Halton(2,3) jitter sequence.
pub fn jitter_offset(index: i32, phase_count: i32) -> Option<(f32, f32)> {
    if phase_count <= 0 {
        return None;
    }
    let at = (index + 1).rem_euclid(phase_count) as u32;
    Some((halton(at, 2) - 0.5, halton(at, 3) - 0.5))
}

/// The number of jitter phases the SDK uses for a render and display width.
pub fn jitter_phase_count(render_width: i32, display_width: i32) -> i32 {
    if render_width <= 0 {
        return 0;
    }
    (8.0 * (display_width as f32 / render_width as f32).powi(2)) as i32
}

// integration: what the game hands the upscaler, and where it goes. `leonida run` registers an upscaler built on the `upscale` crate
// with `set_upscaler`; until one is registered a dispatch is accepted and draws nothing.
pub mod integration {
    use std::sync::Mutex;

    /// A GPU resource of the game as the dispatch described it: the console texture object and its description.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct UpscaleResource {
        /// The game's texture object (null when the dispatch left the input out).
        pub resource: usize,
        pub format: u32,
        pub width: u32,
        pub height: u32,
        pub mips: u32,
        pub usage: u32,
        pub state: u32,
    }

    /// Everything one upscale dispatch carries.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct UpscaleInputs {
        /// Names the context this dispatch belongs to; history belongs to it.
        pub context: usize,
        pub command_list: usize,
        pub color: UpscaleResource,
        pub depth: UpscaleResource,
        pub motion_vectors: UpscaleResource,
        pub exposure: UpscaleResource,
        pub reactive: UpscaleResource,
        pub transparency_and_composition: UpscaleResource,
        pub output: UpscaleResource,
        pub jitter: [f32; 2],
        pub motion_vector_scale: [f32; 2],
        pub render_size: [u32; 2],
        pub upscale_size: [u32; 2],
        /// `None` when sharpening is off.
        pub sharpness: Option<f32>,
        pub frame_time_ms: f32,
        pub pre_exposure: f32,
        /// The history is to be discarded (a camera cut).
        pub reset: bool,
        pub camera_near: f32,
        pub camera_far: f32,
        pub camera_fov_vertical: f32,
        pub view_space_to_meters: f32,
        pub flags: u32,
        /// From the context's creation.
        pub hdr: bool,
        pub depth_inverted: bool,
        pub auto_exposure: bool,
        pub max_render_size: [u32; 2],
        pub max_upscale_size: [u32; 2],
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct UpscaleFailure(pub String);

    pub type Upscaler = fn(&UpscaleInputs) -> Result<(), UpscaleFailure>;

    /// How the upscaler reaches the game's textures and the GPU. The D3D12 layer owns both, so it implements this
    /// and registers it with [`set_texture_access`] once it has a device; until then upscaler dispatches fail with a
    /// trace note and the game keeps its own picture.
    pub trait TextureAccess: Send + Sync {
        /// The texels of a game texture as a host image (the top-left `render_size` at least).
        fn read(&self, resource: &UpscaleResource) -> Result<upscale::Image, UpscaleFailure>;
        /// Writes an image into the top-left of a game texture.
        fn write(
            &self,
            resource: &UpscaleResource,
            image: &upscale::Image,
        ) -> Result<(), UpscaleFailure>;
        /// The Vulkan compute device the D3D12 layer renders with, so the upscaler shares it instead of opening a
        /// second one. `None` lets the upscaler open its own.
        fn compute(&self) -> Option<upscale::SharedCompute> {
            None
        }
    }

    static TEXTURES: Mutex<Option<std::sync::Arc<dyn TextureAccess>>> = Mutex::new(None);

    /// Registers the texture access of the D3D12 layer, or removes it.
    pub fn set_texture_access(access: Option<std::sync::Arc<dyn TextureAccess>>) {
        if let Ok(mut slot) = TEXTURES.lock() {
            *slot = access;
        }
    }

    /// The registered texture access, if the D3D12 layer has supplied one.
    pub fn texture_access() -> Option<std::sync::Arc<dyn TextureAccess>> {
        TEXTURES.lock().ok().and_then(|slot| slot.clone())
    }

    static UPSCALER: Mutex<Option<Upscaler>> = Mutex::new(None);

    /// Registers the upscaler that dispatches go to, or removes it.
    pub fn set_upscaler(upscaler: Option<Upscaler>) {
        if let Ok(mut slot) = UPSCALER.lock() {
            *slot = upscaler;
        }
    }

    /// One upscale dispatch of the game: colour, depth, motion vectors and jitter in, the output resource written.
    pub fn upscale_dispatch(inputs: &UpscaleInputs) -> Result<(), UpscaleFailure> {
        let upscaler = UPSCALER.lock().ok().and_then(|slot| *slot);
        match upscaler {
            Some(upscaler) => upscaler(inputs),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::integration::*;
    use std::sync::Arc;

    struct Empty;

    impl TextureAccess for Empty {
        fn read(&self, _: &UpscaleResource) -> Result<upscale::Image, UpscaleFailure> {
            Err(UpscaleFailure("no device".into()))
        }
        fn write(&self, _: &UpscaleResource, _: &upscale::Image) -> Result<(), UpscaleFailure> {
            Ok(())
        }
    }

    #[test]
    fn the_d3d12_layer_registers_and_removes_its_texture_access() {
        set_texture_access(Some(Arc::new(Empty)));
        let access = texture_access().expect("registered");
        assert!(access.read(&UpscaleResource::default()).is_err());
        assert!(access.compute().is_none());
        set_texture_access(None);
        assert!(texture_access().is_none());
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::api::*;
    use super::integration::{UpscaleInputs, UpscaleResource, upscale_dispatch};
    use super::{jitter_offset, jitter_phase_count, upscale_ratio};
    use crate::trace::note;

    struct Context {
        flags: u32,
        max_render: Dimensions2D,
        max_upscale: Dimensions2D,
        dispatches: u64,
    }

    static CONTEXTS: Mutex<BTreeMap<usize, Context>> = Mutex::new(BTreeMap::new());
    static NO_UPSCALER: AtomicBool = AtomicBool::new(false);
    static UNKNOWN_DISPATCH: AtomicBool = AtomicBool::new(false);

    const CHAIN_LIMIT: usize = 64;

    /// Finds the node of a type in a chain.
    ///
    /// # Safety
    /// `head` is null or the start of a chain of valid headers.
    unsafe fn find(head: *const Header, kind: u64) -> *const Header {
        let mut at = head;
        for _ in 0..CHAIN_LIMIT {
            if at.is_null() {
                break;
            }
            // SAFETY: as the caller promises.
            let header = unsafe { at.read_unaligned() };
            if header.kind == kind {
                return at;
            }
            at = header.next;
        }
        std::ptr::null()
    }

    /// # Safety
    /// `context` is writable; `desc` heads a chain of valid headers.
    pub unsafe extern "win64" fn create_context(
        context: *mut usize,
        desc: *const Header,
        _allocation_callbacks: *const c_void,
    ) -> ReturnCode {
        if context.is_null() || desc.is_null() {
            return ERROR_PARAMETER;
        }
        // SAFETY: the chain is the caller's.
        let upscale = unsafe { find(desc, CREATE_UPSCALE) }.cast::<CreateContextDescUpscale>();
        if upscale.is_null() {
            return ERROR;
        }
        // SAFETY: a node of the upscale type is that description.
        let upscale = unsafe { upscale.read_unaligned() };
        // SAFETY: as above.
        let version =
            unsafe { find(desc, CREATE_UPSCALE_VERSION) }.cast::<CreateContextDescUpscaleVersion>();
        if !version.is_null() {
            // SAFETY: a node of the version type is that description.
            let version = unsafe { version.read_unaligned() }.version;
            if !(pack_version(4, 0, 0)..=pack_version(4, 1, 1)).contains(&version) {
                return ERROR_PARAMETER;
            }
        }
        let handle = Box::into_raw(Box::new(0u64)) as usize;
        if let Ok(mut contexts) = CONTEXTS.lock() {
            contexts.insert(
                handle,
                Context {
                    flags: upscale.flags,
                    max_render: upscale.max_render_size,
                    max_upscale: upscale.max_upscale_size,
                    dispatches: 0,
                },
            );
        }
        // SAFETY: `context` is writable.
        unsafe { context.write_unaligned(handle) };
        OK
    }

    /// # Safety
    /// `context` is null or points at a handle `create_context` made.
    pub unsafe extern "win64" fn destroy_context(
        context: *mut usize,
        _allocation_callbacks: *const c_void,
    ) -> ReturnCode {
        if context.is_null() {
            return ERROR_PARAMETER;
        }
        // SAFETY: as the caller promises.
        let handle = unsafe { context.read_unaligned() };
        let removed = CONTEXTS.lock().ok().and_then(|mut c| c.remove(&handle));
        if removed.is_none() {
            return ERROR_PARAMETER;
        }
        // SAFETY: the box `create_context` made, removed from the table so it is freed once.
        drop(unsafe { Box::from_raw(handle as *mut u64) });
        // SAFETY: `context` is writable.
        unsafe { context.write_unaligned(0) };
        OK
    }

    /// # Safety
    /// `desc` heads a query description whose pointers are valid.
    pub unsafe extern "win64" fn query(_context: *mut usize, desc: *mut Header) -> ReturnCode {
        if desc.is_null() {
            return ERROR_PARAMETER;
        }
        // SAFETY: as the caller promises.
        let kind = unsafe { desc.read_unaligned() }.kind;
        // SAFETY (all arms): a description of a type is that type's structure.
        unsafe {
            match kind {
                QUERY_GET_VERSIONS => {
                    let d = desc.cast::<QueryDescGetVersions>().read_unaligned();
                    if d.output_count.is_null() {
                        return ERROR_PARAMETER;
                    }
                    let providers = u64::from(d.create_desc_type == CREATE_UPSCALE);
                    if d.version_ids.is_null() && d.version_names.is_null() {
                        d.output_count.write_unaligned(providers);
                    } else {
                        let room = d.output_count.read_unaligned();
                        if providers > 0 && room > 0 {
                            if !d.version_ids.is_null() {
                                d.version_ids.write_unaligned(PROVIDER_ID);
                            }
                            if !d.version_names.is_null() {
                                d.version_names.write_unaligned(PROVIDER_NAME.as_ptr());
                            }
                        }
                        d.output_count.write_unaligned(providers);
                    }
                    OK
                }
                QUERY_UPSCALE_RATIO_FROM_QUALITY => {
                    let d = desc.cast::<QueryDescUpscaleRatio>().read_unaligned();
                    let (Some(ratio), false) =
                        (upscale_ratio(d.quality_mode), d.out_ratio.is_null())
                    else {
                        return ERROR_PARAMETER;
                    };
                    d.out_ratio.write_unaligned(ratio);
                    OK
                }
                QUERY_RENDER_RESOLUTION_FROM_QUALITY => {
                    let d = desc.cast::<QueryDescRenderResolution>().read_unaligned();
                    let Some(ratio) = upscale_ratio(d.quality_mode) else {
                        return ERROR_PARAMETER;
                    };
                    if d.out_render_width.is_null() || d.out_render_height.is_null() {
                        return ERROR_PARAMETER;
                    }
                    d.out_render_width
                        .write_unaligned((d.display_width as f32 / ratio) as u32);
                    d.out_render_height
                        .write_unaligned((d.display_height as f32 / ratio) as u32);
                    OK
                }
                QUERY_JITTER_PHASE_COUNT => {
                    let d = desc.cast::<QueryDescJitterPhaseCount>().read_unaligned();
                    if d.out_phase_count.is_null() {
                        return ERROR_PARAMETER;
                    }
                    d.out_phase_count
                        .write_unaligned(jitter_phase_count(d.render_width, d.display_width));
                    OK
                }
                QUERY_JITTER_OFFSET => {
                    let d = desc.cast::<QueryDescJitterOffset>().read_unaligned();
                    let (Some((x, y)), false) = (
                        jitter_offset(d.index, d.phase_count),
                        d.out_x.is_null() || d.out_y.is_null(),
                    ) else {
                        return ERROR_PARAMETER;
                    };
                    d.out_x.write_unaligned(x);
                    d.out_y.write_unaligned(y);
                    OK
                }
                QUERY_GPU_MEMORY_USAGE => {
                    let d = desc.cast::<QueryDescGpuMemoryUsage>().read_unaligned();
                    if d.usage.is_null() {
                        return ERROR_PARAMETER;
                    }
                    d.usage.write_unaligned(MemoryUsage::default());
                    OK
                }
                _ => ERROR_UNKNOWN_DESCTYPE,
            }
        }
    }

    fn resource(r: &Resource) -> UpscaleResource {
        UpscaleResource {
            resource: r.resource,
            format: r.description.format,
            width: r.description.width,
            height: r.description.height,
            mips: r.description.mip_count,
            usage: r.description.usage,
            state: r.state,
        }
    }

    /// Hands the frame's depth and motion vectors to frame generation, which the presenter reads at `PresentX`.
    fn submit_frame_generation_inputs(inputs: &UpscaleInputs) {
        let res = |r: &UpscaleResource| upscale::framegen::InputResource {
            handle: r.resource,
            width: r.width,
            height: r.height,
            format: r.format,
        };
        upscale::framegen::submit_inputs(&upscale::framegen::FrameInputs {
            depth: res(&inputs.depth),
            motion: res(&inputs.motion_vectors),
            render_size: inputs.render_size,
            output_size: inputs.upscale_size,
            motion_vector_scale: inputs.motion_vector_scale,
            jitter: inputs.jitter,
            depth_reversed: inputs.depth_inverted,
            camera_near: inputs.camera_near,
            camera_far: inputs.camera_far,
            camera_fov_y: inputs.camera_fov_vertical,
            reset: inputs.reset,
        });
    }

    /// # Safety
    /// `desc` heads a dispatch description.
    pub unsafe extern "win64" fn dispatch(context: *mut usize, desc: *const Header) -> ReturnCode {
        if context.is_null() || desc.is_null() {
            return ERROR_PARAMETER;
        }
        // SAFETY: as the caller promises.
        let (handle, kind) = unsafe { (context.read_unaligned(), desc.read_unaligned().kind) };
        if kind != DISPATCH_UPSCALE {
            if !UNKNOWN_DISPATCH.swap(true, Ordering::Relaxed) {
                note(format!(
                    "ffxDispatch of description type {kind:#x} is not an upscale dispatch; refused"
                ));
            }
            return ERROR_UNKNOWN_DESCTYPE;
        }
        // SAFETY: a description of the upscale type is that structure.
        let d = unsafe { desc.cast::<DispatchDescUpscale>().read_unaligned() };
        let (flags, max_render, max_upscale) = {
            let Ok(mut contexts) = CONTEXTS.lock() else {
                return ERROR;
            };
            let Some(context) = contexts.get_mut(&handle) else {
                return ERROR_PARAMETER;
            };
            context.dispatches += 1;
            (context.flags, context.max_render, context.max_upscale)
        };
        let render = d.render_size;
        let upscale = d.upscale_size;
        let oversize = |size: Dimensions2D, max: Dimensions2D| {
            (max.width != 0 && size.width > max.width)
                || (max.height != 0 && size.height > max.height)
        };
        if d.color.resource == 0
            || d.output.resource == 0
            || render.width == 0
            || render.height == 0
            || oversize(render, max_render)
            || oversize(upscale, max_upscale)
        {
            return ERROR_PARAMETER;
        }
        let inputs = UpscaleInputs {
            context: handle,
            command_list: d.command_list,
            color: resource(&d.color),
            depth: resource(&d.depth),
            motion_vectors: resource(&d.motion_vectors),
            exposure: resource(&d.exposure),
            reactive: resource(&d.reactive),
            transparency_and_composition: resource(&d.transparency_and_composition),
            output: resource(&d.output),
            jitter: [d.jitter_offset.x, d.jitter_offset.y],
            motion_vector_scale: [d.motion_vector_scale.x, d.motion_vector_scale.y],
            render_size: [render.width, render.height],
            upscale_size: [upscale.width, upscale.height],
            sharpness: (d.enable_sharpening != 0).then_some(d.sharpness),
            frame_time_ms: d.frame_time_delta,
            pre_exposure: d.pre_exposure,
            reset: d.reset != 0,
            camera_near: d.camera_near,
            camera_far: d.camera_far,
            camera_fov_vertical: d.camera_fov_angle_vertical,
            view_space_to_meters: d.view_space_to_meters_factor,
            flags: d.flags,
            hdr: flags & FLAG_HIGH_DYNAMIC_RANGE != 0,
            depth_inverted: flags & FLAG_DEPTH_INVERTED != 0,
            auto_exposure: flags & FLAG_AUTO_EXPOSURE != 0,
            max_render_size: [max_render.width, max_render.height],
            max_upscale_size: [max_upscale.width, max_upscale.height],
        };
        submit_frame_generation_inputs(&inputs);
        match upscale_dispatch(&inputs) {
            Ok(()) => OK,
            Err(failure) => {
                if !NO_UPSCALER.swap(true, Ordering::Relaxed) {
                    note(format!("ffxDispatch: the upscaler failed: {}", failure.0));
                }
                ERROR_RUNTIME_ERROR
            }
        }
    }
}

use crate::Shim;

pub fn shims() -> Vec<Shim> {
    #[cfg(windows)]
    {
        let list: [(&str, usize); 4] = [
            (
                "ffxCreateContext",
                imp::create_context as *const () as usize,
            ),
            (
                "ffxDestroyContext",
                imp::destroy_context as *const () as usize,
            ),
            ("ffxDispatch", imp::dispatch as *const () as usize),
            ("ffxQuery", imp::query as *const () as usize),
        ];
        list.into_iter()
            .map(|(name, address)| Shim {
                dll: "amd_fidelityfx_loader_xbox.dll",
                name,
                address,
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
