//! Presentation: how the title's back buffers reach a window, and the frame events it waits on.
//!
//! The console title never makes a swap chain. It creates back buffers as ordinary committed resources, asks the device
//! for a frame interval and an origin event, waits on that event for a token each frame, renders, and hands the buffer
//! to the queue's `PresentX` with that token. `Presentation` is the object the device and queue slots forward to
//! (`SetFrameIntervalX`, `ScheduleFrameEventX`, `WaitFrameEventX`, `GetFrameStatisticsX`, `PresentX`, `SuspendX`,
//! `ResumeX`). The window is the layer's own: the game's Win32 calls go to the host's user32, and `PresentX` carries no
//! window handle, so frames are shown in a `winit` window (or any `HWND` passed in) through a Vulkan swap chain.

use std::any::Any;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, RwLock};

pub mod convert;
pub mod frames;
pub mod generate;
pub mod render;
pub mod vulkan;
pub mod window;

#[cfg(test)]
mod generate_tests;
#[cfg(test)]
mod gpu_tests;

pub use convert::{ScaleMode, SdrEotf};
pub use frames::{
    ConsolePolicy, DXGI_ERROR_UNSUPPORTED, E_FAIL, E_INVALID_STATE, E_INVALIDARG,
    FRAME_EVENT_ORIGIN, FRAME_INTERVAL_30_HZ, FRAME_INTERVAL_60_HZ, FRAME_INTERVAL_120_HZ,
    FlipKind, FramePolicy, Pacing, S_FALSE, S_OK, StatEntry, StatKind, SyncMode, WaitResult,
};
pub use vulkan::{SourceImage, VulkanContext};
pub use window::{WindowHost, WindowMode, WindowOptions, WindowState};

use convert::{Params, Rect, Space};
use frames::{Clock, FrameEvents, SystemClock};
use render::{Draw, Path, Renderer};
use vulkan::{OffscreenTarget, SwapchainTarget, Target, TargetWant};

#[derive(Debug, thiserror::Error)]
pub enum PresentError {
    #[error("Vulkan: {0}")]
    Vulkan(String),
    #[error("not presentable: {0}")]
    Unsupported(String),
    #[error("compose shader: {0}")]
    Shader(String),
    #[error("window: {0}")]
    Window(String),
}

impl From<ash::vk::Result> for PresentError {
    fn from(r: ash::vk::Result) -> Self {
        PresentError::Vulkan(r.to_string())
    }
}

/// A window that already exists, by its Win32 handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindow {
    pub hwnd: isize,
    pub hinstance: Option<isize>,
}

/// `D3D12XBOX_PRESENT_PLANE_PARAMETERS`, x64 layout. Only the fields up to `ColorSpace` and `ScaleFilter` are read.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PlaneParameters {
    pub token: u64,
    pub resource_count: u32,
    pub resources: *const *mut c_void,
    pub src_view_rects: *const ViewRect,
    pub dest_placement_base: *const ViewRect,
    pub color_space: i32,
    pub scale_filter: i32,
    pub extended_desc_count: u32,
    pub extended_descs: *const c_void,
    pub flags: u32,
}

/// `D3D12XBOX_PRESENT_PARAMETERS`, x64 layout.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PresentParameters {
    pub immediate_threshold_percent: f32,
    pub view_count: u32,
    pub extended_desc_count: u32,
    pub extended_descs: *const c_void,
    pub flags: u32,
}

/// Four floats: left, top, right, bottom.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

const _: () = assert!(std::mem::size_of::<PlaneParameters>() == 0x48);
const _: () = assert!(std::mem::size_of::<PresentParameters>() == 0x20);
const _: () = assert!(std::mem::offset_of!(PlaneParameters, color_space) == 0x28);
const _: () = assert!(std::mem::offset_of!(PlaneParameters, flags) == 0x40);
const _: () = assert!(std::mem::offset_of!(PresentParameters, flags) == 0x18);

/// More planes than this are refused (the console's own limit).
pub const MAX_PLANES: usize = 2;

/// Turns the title's `ID3D12Resource*` into the Vulkan image behind it. The resource module implements it.
pub trait ResourceResolver {
    fn source_image(&self, resource: *mut c_void) -> Option<SourceImage>;
}

/// What the user chose about how frames look in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentConfig {
    pub scale: ScaleMode,
    /// Nearest-neighbour instead of bilinear when the frame is scaled.
    pub point_filter: bool,
    pub sdr_eotf: SdrEotf,
    /// Nits that SDR white maps to in an HDR10 output; 0 takes the default.
    pub sdr_white_nits: f32,
    pub hdr_peak_nits: f32,
    /// Use an HDR10 swap chain when the display offers one and the title presents HDR10.
    pub prefer_hdr: bool,
}

impl Default for PresentConfig {
    fn default() -> Self {
        PresentConfig {
            scale: ScaleMode::Fit,
            point_filter: false,
            sdr_eotf: SdrEotf::Srgb,
            sdr_white_nits: 0.0,
            hdr_peak_nits: convert::DEFAULT_HDR_PEAK_NITS,
            prefer_hdr: true,
        }
    }
}

impl PresentConfig {
    fn params(&self) -> Params {
        Params {
            sdr_eotf: self.sdr_eotf,
            sdr_white_nits: if self.sdr_white_nits > 0.0 {
                self.sdr_white_nits
            } else {
                convert::DEFAULT_SDR_WHITE_NITS
            },
            hdr_peak_nits: self.hdr_peak_nits,
        }
    }
}

/// One frame for a backend: what to draw and how the display should take it.
pub struct Frame {
    pub draw: Option<Draw>,
    pub sync: SyncMode,
    pub hdr10: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    /// False when nothing was drawn (a minimised window).
    pub shown: bool,
    pub path: Path,
}

/// Where frames go. The Vulkan backend is the real one; tests use a recording one.
pub trait PresentBackend: Send {
    fn supports_hdr10(&self) -> bool;
    fn present(&mut self, frame: Frame) -> Result<Shown, PresentError>;
    fn as_any(&mut self) -> &mut dyn Any;
}

/// Frames drawn by the renderer into a target (a swap chain, or an offscreen image).
pub struct VulkanBackend<T: Target + 'static> {
    pub renderer: Renderer,
    pub target: T,
    window: Option<Arc<WindowHost>>,
}

impl VulkanBackend<SwapchainTarget> {
    pub fn for_window(ctx: &VulkanContext, host: Arc<WindowHost>) -> Result<Self, PresentError> {
        let target = SwapchainTarget::new(ctx, host.native())?;
        Ok(VulkanBackend {
            renderer: Renderer::new(ctx)?,
            target,
            window: Some(host),
        })
    }

    /// For a window the caller owns, given its handles.
    pub fn for_native(ctx: &VulkanContext, window: NativeWindow) -> Result<Self, PresentError> {
        Ok(VulkanBackend {
            renderer: Renderer::new(ctx)?,
            target: SwapchainTarget::new(ctx, window)?,
            window: None,
        })
    }
}

impl VulkanBackend<OffscreenTarget> {
    pub fn offscreen(ctx: &VulkanContext, target: OffscreenTarget) -> Result<Self, PresentError> {
        Ok(VulkanBackend {
            renderer: Renderer::new(ctx)?,
            target,
            window: None,
        })
    }
}

impl<T: Target + 'static> PresentBackend for VulkanBackend<T> {
    fn supports_hdr10(&self) -> bool {
        self.target.supports_hdr10()
    }

    fn present(&mut self, frame: Frame) -> Result<Shown, PresentError> {
        let size = match &self.window {
            Some(w) => {
                let state = w.state();
                if state.minimized {
                    return Ok(Shown {
                        shown: false,
                        path: Path::Blank,
                    });
                }
                state.size
            }
            None => self.target.extent(),
        };
        let srgb = frame
            .draw
            .as_ref()
            .is_some_and(|d| vulkan::is_srgb(d.source.format));
        let want = TargetWant {
            hdr10: frame.hdr10,
            srgb,
            sync: frame.sync,
            size,
        };
        let out = self
            .renderer
            .render(&mut self.target, frame.draw.as_ref(), &want)?;
        Ok(Shown {
            shown: out.shown,
            path: out.path,
        })
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// What the title's `IDXGIAdapter::GetDesc` shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterDesc {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_video_memory: u64,
}

/// What `IDXGIOutput` shows: the display the console is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDesc {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_millihz: u32,
    pub hdr: bool,
}

/// The adapter and outputs behind `IDXGIDevice1` -> `GetAdapter` -> `EnumOutputs`. The console has one output; the
/// handle of output `i` is what the title passes back to `SetFrameIntervalX`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayTopology {
    pub adapter: AdapterDesc,
    pub outputs: Vec<OutputDesc>,
}

const OUTPUT_HANDLE_BASE: u64 = 0x1000;

impl DisplayTopology {
    /// The adapter of the Vulkan device, and a console-like output at `width` x `height`.
    pub fn from_vulkan(ctx: &VulkanContext, width: u32, height: u32) -> DisplayTopology {
        // SAFETY: plain property queries on the physical device of this context.
        let (props, mem) = unsafe {
            (
                ctx.instance.get_physical_device_properties(ctx.physical),
                ctx.instance
                    .get_physical_device_memory_properties(ctx.physical),
            )
        };
        let local: u64 = (0..mem.memory_heap_count as usize)
            .map(|i| mem.memory_heaps[i])
            .filter(|h| h.flags.contains(ash::vk::MemoryHeapFlags::DEVICE_LOCAL))
            .map(|h| h.size)
            .sum();
        DisplayTopology {
            adapter: AdapterDesc {
                name: props
                    .device_name_as_c_str()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                vendor_id: props.vendor_id,
                device_id: props.device_id,
                dedicated_video_memory: local,
            },
            outputs: vec![OutputDesc {
                name: "Leonida display".into(),
                width,
                height,
                refresh_millihz: 60_000,
                hdr: false,
            }],
        }
    }

    pub fn enum_outputs(&self, index: usize) -> Option<(u64, &OutputDesc)> {
        self.outputs
            .get(index)
            .map(|o| (OUTPUT_HANDLE_BASE + index as u64, o))
    }

    pub fn output(&self, handle: u64) -> Option<&OutputDesc> {
        handle
            .checked_sub(OUTPUT_HANDLE_BASE)
            .and_then(|i| self.outputs.get(i as usize))
    }
}

/// One plane of a present, after the pointers are resolved.
#[derive(Clone)]
pub struct Plane {
    pub token: u64,
    pub resource_count: u32,
    pub source: Option<SourceImage>,
    pub src_rect: Option<Rect>,
    pub placement: Option<[f32; 4]>,
    pub color_space: i32,
    pub scale_filter: i32,
}

pub struct Presentation {
    events: FrameEvents,
    backend: Mutex<Box<dyn PresentBackend>>,
    policy: Arc<dyn FramePolicy>,
    config: RwLock<PresentConfig>,
    sync: Mutex<SyncMode>,
    topology: Mutex<Option<DisplayTopology>>,
    window: Option<Arc<WindowHost>>,
    notes: Box<dyn Fn(&str) + Send + Sync>,
    noted: Mutex<HashSet<&'static str>>,
}

impl Presentation {
    pub fn new(
        backend: Box<dyn PresentBackend>,
        policy: Arc<dyn FramePolicy>,
        config: PresentConfig,
    ) -> Presentation {
        Presentation::with_clock(backend, policy, config, Box::new(SystemClock::default()))
    }

    pub fn with_clock(
        backend: Box<dyn PresentBackend>,
        policy: Arc<dyn FramePolicy>,
        config: PresentConfig,
        clock: Box<dyn Clock>,
    ) -> Presentation {
        Presentation {
            events: FrameEvents::new(clock),
            backend: Mutex::new(backend),
            policy,
            config: RwLock::new(config),
            sync: Mutex::new(SyncMode::Vsync),
            topology: Mutex::new(None),
            window: None,
            notes: Box::new(|_| {}),
            noted: Mutex::new(HashSet::new()),
        }
    }

    /// A window opened here, a swap chain on it and the console policy.
    pub fn open_window(
        ctx: &VulkanContext,
        options: WindowOptions,
        policy: Arc<dyn FramePolicy>,
        config: PresentConfig,
    ) -> Result<Presentation, PresentError> {
        let host = Arc::new(WindowHost::open(options)?);
        let backend = VulkanBackend::for_window(ctx, host.clone())?;
        let mut p = Presentation::new(Box::new(backend), policy, config);
        p.window = Some(host);
        Ok(p)
    }

    /// Where unmeasured behaviour is reported, once per kind.
    pub fn set_notes(&mut self, notes: Box<dyn Fn(&str) + Send + Sync>) {
        self.notes = notes;
    }

    pub fn set_topology(&self, topology: DisplayTopology) {
        *self.topology.lock().unwrap_or_else(|e| e.into_inner()) = Some(topology);
    }

    pub fn topology(&self) -> Option<DisplayTopology> {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_config(&self, config: PresentConfig) {
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = config;
    }

    pub fn config(&self) -> PresentConfig {
        *self.config.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn window(&self) -> Option<&Arc<WindowHost>> {
        self.window.as_ref()
    }

    pub fn events(&self) -> &FrameEvents {
        &self.events
    }

    /// Runs `f` on the backend if it is a `B`.
    pub fn with_backend<B: 'static, R>(&self, f: impl FnOnce(&mut B) -> R) -> Option<R> {
        let mut backend = self.backend.lock().unwrap_or_else(|e| e.into_inner());
        backend.as_any().downcast_mut::<B>().map(f)
    }

    fn note_once(&self, key: &'static str, text: &str) {
        if self
            .noted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key)
        {
            (self.notes)(text);
        }
    }

    /// Device slot 78. `output` is a handle from `DisplayTopology::enum_outputs`.
    pub fn set_frame_interval_x(
        &self,
        output: u64,
        interval_us: u32,
        latency: u32,
        _flags: u32,
    ) -> i32 {
        let known = self.topology().is_none_or(|t| t.output(output).is_some());
        if !known {
            self.note_once(
                "unknown output",
                "SetFrameIntervalX: an output handle that EnumOutputs never gave",
            );
        }
        let pacing = self.policy.pacing(interval_us);
        *self.sync.lock().unwrap_or_else(|e| e.into_inner()) = pacing.sync;
        self.events
            .with(|s, now| s.set_interval(now, pacing, latency));
        S_OK
    }

    /// Device slot 79. Only the origin event is served; the later ones the console has are unmeasured.
    pub fn schedule_frame_event_x(&self, event: u32, offset_us: u32, _flags: u32) -> i32 {
        if event != FRAME_EVENT_ORIGIN {
            self.note_once(
                "later frame event",
                "ScheduleFrameEventX: an event after the origin, served as the origin",
            );
        }
        let ok = self
            .events
            .with(|s, _| s.schedule_origin(u64::from(offset_us)));
        if ok { S_OK } else { E_INVALID_STATE }
    }

    /// Device slot 80. Blocks until the next origin event; `timeout_ms` of `u32::MAX` is `INFINITE`.
    pub fn wait_frame_event_x(&self, event: u32, timeout_ms: u32, flags: u32) -> WaitResult {
        if event != FRAME_EVENT_ORIGIN {
            self.note_once(
                "later frame event",
                "WaitFrameEventX: an event after the origin, served as the origin",
            );
        }
        self.events.wait(timeout_ms, flags)
    }

    /// Device slot 81. `S_FALSE` and no entries mean "not available yet".
    pub fn get_frame_statistics_x(
        &self,
        token: u64,
        mask: u32,
        max: usize,
    ) -> (i32, Vec<StatEntry>) {
        let entries = self
            .events
            .with(|s, now| s.statistics(now, token, mask, max));
        (if entries.is_empty() { S_FALSE } else { S_OK }, entries)
    }

    /// Queue `SuspendX`: drops the interval and the frames in flight, wakes waits with an error, refuses presents.
    pub fn suspend_x(&self, _flags: u32) -> i32 {
        self.events.with(|s, _| s.suspend());
        S_OK
    }

    /// Queue `ResumeX`: presents work again; the title registers its interval and event again.
    pub fn resume_x(&self) -> i32 {
        self.events.with(|s, _| s.resume());
        S_OK
    }

    /// Queue `PresentX` on raw arguments.
    ///
    /// # Safety
    /// `planes` points to `plane_count` valid `PlaneParameters` whose resource arrays and rectangles are valid for
    /// `resource_count` entries (or null), and `params` is null or valid.
    pub unsafe fn present_x(
        &self,
        plane_count: u32,
        planes: *const PlaneParameters,
        params: *const PresentParameters,
        resolver: &dyn ResourceResolver,
    ) -> i32 {
        if plane_count as usize > MAX_PLANES {
            return E_INVALIDARG;
        }
        if plane_count > 0 && planes.is_null() {
            return E_INVALIDARG;
        }
        let mut list = Vec::with_capacity(plane_count as usize);
        for i in 0..plane_count as usize {
            // SAFETY: the caller promises `plane_count` valid entries.
            let p = unsafe { *planes.add(i) };
            let source = if p.resource_count >= 1 && !p.resources.is_null() {
                // SAFETY: the resource array holds `resource_count` entries and the first exists.
                let resource = unsafe { *p.resources };
                resolver.source_image(resource)
            } else {
                None
            };
            let rect = |ptr: *const ViewRect| {
                // SAFETY: a non-null rectangle pointer names at least one rectangle.
                (!ptr.is_null()).then(|| unsafe { *ptr })
            };
            let src_rect = rect(p.src_view_rects).map(|r| Rect {
                left: r.left.round() as i32,
                top: r.top.round() as i32,
                right: r.right.round() as i32,
                bottom: r.bottom.round() as i32,
            });
            let placement = rect(p.dest_placement_base).map(|r| [r.left, r.top, r.right, r.bottom]);
            list.push(Plane {
                token: p.token,
                resource_count: p.resource_count,
                source,
                src_rect,
                placement,
                color_space: p.color_space,
                scale_filter: p.scale_filter,
            });
        }
        let threshold = if params.is_null() {
            0.0
        } else {
            // SAFETY: a non-null parameter block is valid.
            unsafe { (*params).immediate_threshold_percent }
        };
        self.present_planes(&list, threshold)
    }

    /// Queue `PresentX` on resolved planes. No planes presents a black frame.
    pub fn present_planes(&self, planes: &[Plane], immediate_threshold: f32) -> i32 {
        if self.events.with(|s, _| s.is_suspended()) {
            return E_INVALID_STATE;
        }
        if planes.len() > MAX_PLANES {
            return E_INVALIDARG;
        }
        let config = self.config();
        let sync = *self.sync.lock().unwrap_or_else(|e| e.into_inner());
        if planes.is_empty() {
            let frame = Frame {
                draw: None,
                sync,
                hdr10: false,
            };
            return self.hand_over(frame);
        }
        for p in planes {
            if p.resource_count == 0 {
                return E_INVALIDARG;
            }
            if p.resource_count > 1 {
                self.note_once(
                    "resource count",
                    "PresentX: a plane with several resources, only the first is read",
                );
                return DXGI_ERROR_UNSUPPORTED;
            }
            if p.source.is_none() {
                return E_INVALIDARG;
            }
            if !convert::Space::from_dxgi(p.color_space).1 {
                self.note_once(
                    "colour space",
                    "PresentX: a colour space other than G22/P709 and G2084/P2020, shown as SDR",
                );
            }
            if p.scale_filter != 0 {
                self.note_once(
                    "scale filter",
                    "PresentX: a ScaleFilter value, treated as the default (values are unmeasured)",
                );
            }
        }
        let token = planes[0].token;
        if planes.iter().any(|p| p.token != token) {
            return E_INVALIDARG;
        }
        if !self.events.with(|s, _| s.token_valid(token)) {
            return E_INVALIDARG;
        }
        if planes.len() == 2 && planes[0].placement != planes[1].placement {
            self.note_once(
                "plane placement",
                "PresentX: two planes with different placement, one plane is shown",
            );
        }
        let hdr_capable = config.prefer_hdr
            && self
                .backend
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .supports_hdr10();
        let is_hdr = |p: &Plane| Space::from_dxgi(p.color_space).0 == Space::Hdr10;
        let hdr_out = hdr_capable && planes.iter().any(is_hdr);
        let chosen = if hdr_out {
            planes.iter().find(|p| is_hdr(p))
        } else {
            planes.iter().find(|p| !is_hdr(p))
        }
        .unwrap_or(&planes[0]);
        let Some(source) = chosen.source.clone() else {
            return E_INVALIDARG;
        };
        self.events
            .with(|s, now| s.present(now, token, immediate_threshold));
        let draw = Draw {
            source,
            src_rect: chosen.src_rect,
            placement: chosen.placement,
            space: Space::from_dxgi(chosen.color_space).0,
            point_filter: config.point_filter,
            scale: config.scale,
            params: config.params(),
        };
        self.hand_over(Frame {
            draw: Some(draw),
            sync,
            hdr10: hdr_out,
        })
    }

    fn hand_over(&self, frame: Frame) -> i32 {
        let result = self
            .backend
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .present(frame);
        match result {
            Ok(_) => S_OK,
            Err(PresentError::Unsupported(why)) => {
                self.note_once("unsupported frame", &format!("PresentX: {why}"));
                DXGI_ERROR_UNSUPPORTED
            }
            Err(e) => {
                self.note_once("present failed", &format!("PresentX failed: {e}"));
                E_FAIL
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ash::vk;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Log = Arc<Mutex<Vec<(bool, bool, SyncMode, i32)>>>;

    struct Recorder {
        frames: Log,
        hdr: bool,
        fail: Option<fn() -> PresentError>,
    }

    impl PresentBackend for Recorder {
        fn supports_hdr10(&self) -> bool {
            self.hdr
        }

        fn present(&mut self, frame: Frame) -> Result<Shown, PresentError> {
            if let Some(f) = self.fail {
                return Err(f());
            }
            self.frames.lock().unwrap().push((
                frame.draw.is_some(),
                frame.hdr10,
                frame.sync,
                frame
                    .draw
                    .as_ref()
                    .map_or(-1, |d| i32::from(d.space == Space::Hdr10)),
            ));
            Ok(Shown {
                shown: true,
                path: Path::Blit,
            })
        }

        fn as_any(&mut self) -> &mut dyn Any {
            self
        }
    }

    fn make(hdr: bool) -> (Presentation, Log) {
        let log: Log = Arc::default();
        let p = Presentation::new(
            Box::new(Recorder {
                frames: log.clone(),
                hdr,
                fail: None,
            }),
            Arc::new(ConsolePolicy),
            PresentConfig::default(),
        );
        (p, log)
    }

    fn source() -> SourceImage {
        SourceImage {
            image: vk::Image::null(),
            format: vk::Format::B8G8R8A8_UNORM,
            extent: (1920, 1080),
            layout: vk::ImageLayout::PRESENT_SRC_KHR,
            usage: vk::ImageUsageFlags::TRANSFER_SRC,
            samples: vk::SampleCountFlags::TYPE_1,
            waits: Vec::new(),
        }
    }

    fn plane(token: u64, space: i32) -> Plane {
        Plane {
            token,
            resource_count: 1,
            source: Some(source()),
            src_rect: None,
            placement: None,
            color_space: space,
            scale_filter: 0,
        }
    }

    fn start(p: &Presentation) -> u64 {
        assert_eq!(p.set_frame_interval_x(0x1000, 1000, 1, 0), S_OK);
        assert_eq!(p.schedule_frame_event_x(FRAME_EVENT_ORIGIN, 0, 0), S_OK);
        let w = p.wait_frame_event_x(FRAME_EVENT_ORIGIN, 1000, 0);
        assert_eq!(w.hresult, S_OK);
        w.token
    }

    #[test]
    fn abi_structs_have_the_console_sizes() {
        assert_eq!(std::mem::size_of::<PlaneParameters>(), 0x48);
        assert_eq!(std::mem::size_of::<PresentParameters>(), 0x20);
        assert_eq!(std::mem::offset_of!(PlaneParameters, resources), 0x10);
        assert_eq!(std::mem::offset_of!(PlaneParameters, src_view_rects), 0x18);
        assert_eq!(
            std::mem::offset_of!(PlaneParameters, dest_placement_base),
            0x20
        );
        assert_eq!(std::mem::offset_of!(PlaneParameters, scale_filter), 0x2C);
    }

    #[test]
    fn no_planes_presents_a_blank_frame_without_a_token() {
        let (p, log) = make(false);
        assert_eq!(p.present_planes(&[], 0.0), S_OK);
        assert!(!log.lock().unwrap()[0].0);
    }

    #[test]
    fn a_token_that_was_never_handed_out_is_refused_and_null_is_allowed() {
        let (p, log) = make(false);
        assert_eq!(p.present_planes(&[plane(5, 0)], 0.0), E_INVALIDARG);
        assert_eq!(p.present_planes(&[plane(0, 0)], 0.0), S_OK);
        let token = start(&p);
        assert_eq!(p.present_planes(&[plane(token, 0)], 0.0), S_OK);
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[test]
    fn plane_arguments_are_checked() {
        let (p, log) = make(false);
        let three = [plane(0, 0), plane(0, 0), plane(0, 0)];
        assert_eq!(p.present_planes(&three, 0.0), E_INVALIDARG);
        let mut none = plane(0, 0);
        none.source = None;
        assert_eq!(p.present_planes(&[none], 0.0), E_INVALIDARG);
        let mut zero = plane(0, 0);
        zero.resource_count = 0;
        assert_eq!(p.present_planes(&[zero], 0.0), E_INVALIDARG);
        let mut many = plane(0, 0);
        many.resource_count = 2;
        assert_eq!(p.present_planes(&[many], 0.0), DXGI_ERROR_UNSUPPORTED);
        assert_eq!(
            p.present_planes(&[plane(0, 0), plane(7, 0)], 0.0),
            E_INVALIDARG
        );
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn the_plane_matching_the_output_is_shown() {
        let (p, log) = make(true);
        let token = start(&p);
        assert_eq!(
            p.present_planes(
                &[plane(token, convert::COLOR_SPACE_HDR10), plane(token, 0)],
                0.0
            ),
            S_OK
        );
        let (sdr, log2) = make(false);
        let token = start(&sdr);
        assert_eq!(
            sdr.present_planes(
                &[plane(token, convert::COLOR_SPACE_HDR10), plane(token, 0)],
                0.0
            ),
            S_OK
        );
        let hdr_frame = log.lock().unwrap()[0];
        assert_eq!((hdr_frame.1, hdr_frame.3), (true, 1));
        let sdr_frame = log2.lock().unwrap()[0];
        assert_eq!((sdr_frame.1, sdr_frame.3), (false, 0));
    }

    #[test]
    fn hdr_on_an_sdr_display_is_still_shown_converted() {
        let (p, log) = make(false);
        let token = start(&p);
        assert_eq!(
            p.present_planes(&[plane(token, convert::COLOR_SPACE_HDR10)], 0.0),
            S_OK
        );
        assert_eq!(log.lock().unwrap()[0].3, 1);
        assert!(!log.lock().unwrap()[0].1);
    }

    #[test]
    fn the_policy_sets_the_grid_and_the_sync_mode() {
        struct Uncapped;
        impl FramePolicy for Uncapped {
            fn pacing(&self, _: u32) -> Pacing {
                Pacing {
                    interval_us: 0,
                    sync: SyncMode::Immediate,
                }
            }
        }
        let log: Log = Arc::default();
        let p = Presentation::new(
            Box::new(Recorder {
                frames: log.clone(),
                hdr: false,
                fail: None,
            }),
            Arc::new(Uncapped),
            PresentConfig::default(),
        );
        p.set_frame_interval_x(0, FRAME_INTERVAL_60_HZ, 1, 0);
        p.schedule_frame_event_x(FRAME_EVENT_ORIGIN, 0, 0);
        let first = p.wait_frame_event_x(FRAME_EVENT_ORIGIN, 0, 0);
        let second = p.wait_frame_event_x(FRAME_EVENT_ORIGIN, 0, 0);
        assert_eq!((first.hresult, first.token), (S_OK, 1));
        assert_eq!((second.hresult, second.token), (S_OK, 2));
        p.present_planes(&[plane(2, 0)], 0.0);
        assert_eq!(log.lock().unwrap()[0].2, SyncMode::Immediate);
    }

    #[test]
    fn suspend_refuses_presents_and_waits_until_the_title_registers_again() {
        let (p, _) = make(false);
        let token = start(&p);
        assert_eq!(p.suspend_x(0), S_OK);
        assert_eq!(p.present_planes(&[plane(token, 0)], 0.0), E_INVALID_STATE);
        assert_eq!(p.wait_frame_event_x(0, 0, 0).hresult, E_INVALID_STATE);
        assert_eq!(p.resume_x(), S_OK);
        assert_eq!(p.wait_frame_event_x(0, 0, 0).hresult, E_INVALIDARG);
        let token = start(&p);
        assert!(token > 1);
        assert_eq!(p.present_planes(&[plane(token, 0)], 0.0), S_OK);
    }

    #[test]
    fn statistics_follow_presented_frames() {
        let (p, _) = make(false);
        let (hr, none) = p.get_frame_statistics_x(0, u32::MAX, 8);
        assert_eq!((hr, none.len()), (S_FALSE, 0));
        let token = start(&p);
        p.present_planes(&[plane(token, 0)], 0.0);
        let mask = StatKind::Event.bit() | StatKind::Present.bit();
        let (hr, entries) = p.get_frame_statistics_x(token, mask, 8);
        assert_eq!(hr, S_OK);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn backend_errors_become_hresults_and_notes() {
        let log: Log = Arc::default();
        let notes = Arc::new(AtomicUsize::new(0));
        let mut p = Presentation::new(
            Box::new(Recorder {
                frames: log,
                hdr: false,
                fail: Some(|| PresentError::Unsupported("multisampled".into())),
            }),
            Arc::new(ConsolePolicy),
            PresentConfig::default(),
        );
        let n = notes.clone();
        p.set_notes(Box::new(move |_| {
            n.fetch_add(1, Ordering::SeqCst);
        }));
        for _ in 0..3 {
            assert_eq!(
                p.present_planes(&[plane(0, 0)], 0.0),
                DXGI_ERROR_UNSUPPORTED
            );
        }
        assert_eq!(notes.load(Ordering::SeqCst), 1);
        p.with_backend::<Recorder, _>(|r| r.fail = Some(|| PresentError::Vulkan("lost".into())));
        assert_eq!(p.present_planes(&[plane(0, 0)], 0.0), E_FAIL);
    }

    struct Resolver;

    impl ResourceResolver for Resolver {
        fn source_image(&self, resource: *mut c_void) -> Option<SourceImage> {
            (!resource.is_null()).then(source)
        }
    }

    #[test]
    fn raw_planes_are_read_with_their_rectangles_and_placement() {
        let (p, log) = make(false);
        let token = start(&p);
        let mut resource = 0x10usize;
        let resources = [std::ptr::from_mut(&mut resource).cast::<c_void>()];
        let rect = ViewRect {
            left: 0.0,
            top: 0.0,
            right: 1280.0,
            bottom: 720.0,
        };
        let place = ViewRect {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 1.0,
        };
        let plane = PlaneParameters {
            token,
            resource_count: 1,
            resources: resources.as_ptr(),
            src_view_rects: &rect,
            dest_placement_base: &place,
            color_space: 0,
            scale_filter: 0,
            extended_desc_count: 0,
            extended_descs: std::ptr::null(),
            flags: 0,
        };
        let params = PresentParameters {
            immediate_threshold_percent: 10.0,
            view_count: 0,
            extended_desc_count: 0,
            extended_descs: std::ptr::null(),
            flags: 0,
        };
        // SAFETY: one plane whose pointers all name live locals.
        let hr = unsafe { p.present_x(1, &plane, &params, &Resolver) };
        assert_eq!(hr, S_OK);
        assert_eq!(log.lock().unwrap().len(), 1);
        // SAFETY: zero planes need no pointers.
        assert_eq!(
            unsafe { p.present_x(0, std::ptr::null(), std::ptr::null(), &Resolver) },
            S_OK
        );
        // SAFETY: a count above the limit is refused before any pointer is read.
        assert_eq!(
            unsafe { p.present_x(3, std::ptr::null(), std::ptr::null(), &Resolver) },
            E_INVALIDARG
        );
        // SAFETY: a null plane array with a non-zero count is refused before it is read.
        assert_eq!(
            unsafe { p.present_x(1, std::ptr::null(), std::ptr::null(), &Resolver) },
            E_INVALIDARG
        );
    }

    #[test]
    fn an_unresolvable_resource_is_an_invalid_argument() {
        let (p, _) = make(false);
        let resources = [std::ptr::null_mut::<c_void>()];
        let plane = PlaneParameters {
            token: 0,
            resource_count: 1,
            resources: resources.as_ptr(),
            src_view_rects: std::ptr::null(),
            dest_placement_base: std::ptr::null(),
            color_space: 0,
            scale_filter: 0,
            extended_desc_count: 0,
            extended_descs: std::ptr::null(),
            flags: 0,
        };
        // SAFETY: one plane whose pointers are live or null.
        assert_eq!(
            unsafe { p.present_x(1, &plane, std::ptr::null(), &Resolver) },
            E_INVALIDARG
        );
    }

    #[test]
    fn output_handles_round_trip_and_unknown_ones_are_noted() {
        let topology = DisplayTopology {
            adapter: AdapterDesc {
                name: "gpu".into(),
                vendor_id: 1,
                device_id: 2,
                dedicated_video_memory: 16 << 30,
            },
            outputs: vec![OutputDesc {
                name: "tv".into(),
                width: 3840,
                height: 2160,
                refresh_millihz: 60_000,
                hdr: true,
            }],
        };
        let (handle, desc) = topology.enum_outputs(0).unwrap();
        assert_eq!(desc.width, 3840);
        assert_eq!(topology.output(handle).unwrap().name, "tv");
        assert!(topology.output(0).is_none());
        assert!(topology.enum_outputs(1).is_none());
        let (mut p, _) = make(false);
        let notes = Arc::new(AtomicUsize::new(0));
        let n = notes.clone();
        p.set_notes(Box::new(move |_| {
            n.fetch_add(1, Ordering::SeqCst);
        }));
        p.set_topology(topology);
        p.set_frame_interval_x(handle, 1000, 1, 0);
        assert_eq!(notes.load(Ordering::SeqCst), 0);
        p.set_frame_interval_x(7, 1000, 1, 0);
        assert_eq!(notes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn config_params_use_the_default_white_when_none_is_set() {
        let mut c = PresentConfig::default();
        assert_eq!(c.params().sdr_white_nits, 203.0);
        c.sdr_white_nits = 120.0;
        assert_eq!(c.params().sdr_white_nits, 120.0);
    }
}
