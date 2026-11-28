//! `Assembly`: everything one console device is made of, built over one Vulkan device.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::Instant;

use ash::vk;
use shader_translate::FloatMode;

use super::arena::{CodeArena, SpillRing, UploadArena};
use super::bindless::Bindless;
use super::executor::AssemblyExecutor;
use super::frames::{AssemblyFrames, PacerPolicy};
use super::gpu::{Gpu, GpuConfig, GpuError};
use super::hooks::{self, Translator};
use super::pipelines::{self, PipelinesSeen, Service, SharedBuilder};
use super::timeline::VkTimelines;
use super::views::ViewCache;
use crate::com;
use crate::compile_queue::DrawPolicy;
use crate::descriptor::Descriptors;
use crate::device::{DeviceCore, Integration};
use crate::heap::console::{ConsolePlacement, HeapAllocator, NoAllocator};
use crate::heap::{HeapPlacement, Placed, ResourcesLow};
use crate::memory::{self, AddressSpace, Config, MemoryError, SyncPolicy};
use crate::pacing::{FrameLimit, Pacer};
use crate::pipeline::{DirectTranslation, PipelineCache, PipelineEnv, PipelineError};
use crate::present::vulkan::OffscreenTarget;
use crate::present::{
    PresentConfig, PresentError, Presentation, VulkanBackend, WindowMode, WindowOptions,
};
use crate::resource::{ResourceId, Resources};

#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    #[error("{0}")]
    Gpu(#[from] GpuError),
    #[error("address space: {0}")]
    Memory(#[from] MemoryError),
    #[error("Vulkan: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("pipelines: {0}")]
    Pipeline(#[from] PipelineError),
    #[error("presentation: {0}")]
    Present(#[from] PresentError),
}

/// Where frames go.
#[derive(Debug, Clone)]
pub enum Output {
    /// A window of the layer's own, with a swap chain.
    Window(WindowOptionsConfig),
    /// An offscreen image of this size, which tests read back.
    Offscreen(u32, u32),
}

/// `WindowOptions` without the title string's lifetime worries: what the runtime chooses about the window.
#[derive(Debug, Clone)]
pub struct WindowOptionsConfig {
    pub title: String,
    pub size: (u32, u32),
    pub borderless: bool,
    pub hotkeys: bool,
}

impl Default for WindowOptionsConfig {
    fn default() -> Self {
        let d = WindowOptions::default();
        WindowOptionsConfig {
            title: d.title,
            size: d.size,
            borderless: false,
            hotkeys: d.hotkeys,
        }
    }
}

impl From<&WindowOptionsConfig> for WindowOptions {
    fn from(c: &WindowOptionsConfig) -> WindowOptions {
        WindowOptions {
            title: c.title.clone(),
            size: c.size,
            mode: if c.borderless {
                WindowMode::Borderless
            } else {
                WindowMode::Windowed
            },
            hotkeys: c.hotkeys,
            ..WindowOptions::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct AssemblyConfig {
    pub output: Output,
    pub present: PresentConfig,
    pub frame_limit: FrameLimit,
    pub pipeline_cache: Option<PathBuf>,
    /// The log of pipelines the title created, replayed at start so the next session finds them built. `None`
    /// records and replays nothing.
    pub pipelines_seen: Option<PipelinesSeen>,
    /// What a draw does when its pipeline is still being built.
    pub draw_policy: DrawPolicy,
    /// Threads that build pipelines; `None` leaves two processors to the game.
    pub pipeline_workers: Option<usize>,
    pub gpu: GpuConfig,
    /// Make the `XMem*` shims report the game's memory to this device's address space.
    pub memory_hooks: bool,
}

impl AssemblyConfig {
    /// A window, the frame limit set with `set_frame_limit` and `LEONIDA_GPU` and `LEONIDA_VALIDATION`.
    pub fn windowed() -> AssemblyConfig {
        AssemblyConfig {
            output: Output::Window(WindowOptionsConfig::default()),
            present: PresentConfig::default(),
            frame_limit: super::frame_limit(),
            pipeline_cache: std::env::var_os("LEONIDA_PIPELINE_CACHE").map(PathBuf::from),
            pipelines_seen: std::env::var_os("LEONIDA_PIPELINES_SEEN").map(PipelinesSeen::beside),
            draw_policy: DrawPolicy::Wait,
            pipeline_workers: None,
            gpu: GpuConfig::from_env(true),
            memory_hooks: true,
        }
    }

    /// An offscreen target, for tests and tools.
    pub fn offscreen(width: u32, height: u32) -> AssemblyConfig {
        AssemblyConfig {
            output: Output::Offscreen(width, height),
            gpu: GpuConfig::from_env(false),
            memory_hooks: false,
            ..AssemblyConfig::windowed()
        }
    }
}

/// What one submission works with at a time.
pub struct ExecState {
    pub pool: vk::CommandPool,
    pub fence: vk::Fence,
    pub spill: SpillRing,
}

pub struct Assembly {
    pub gpu: Arc<Gpu>,
    pub resources: Arc<Mutex<Resources>>,
    pub descriptors: Mutex<Descriptors>,
    pub views: ViewCache,
    pub bindless: Bindless,
    pub pipelines: Arc<PipelineEnv>,
    /// Builds the pipelines the title creates on worker threads.
    pub pipeline_service: Service,
    pub code: Arc<CodeArena>,
    pub upload: Arc<UploadArena>,
    pub exec: Mutex<ExecState>,
    pub presentation: Arc<Presentation>,
    pub pacer: Arc<Pacer>,
    pub core: OnceLock<Arc<DeviceCore>>,
    pub(super) scans: Mutex<HashMap<u64, Instant>>,
    pub(super) neutral: Mutex<Option<ResourceId>>,
    pub(super) uploaded: Mutex<std::collections::HashSet<ResourceId>>,
    pub(super) latest_token: std::sync::atomic::AtomicU64,
}

/// Places each descriptor heap as `inner` says and keeps the resource step's own addresses out of it.
struct NotingPlacement {
    inner: Arc<dyn HeapPlacement>,
    resources: Arc<Mutex<Resources>>,
}

impl HeapPlacement for NotingPlacement {
    fn place(&self, bytes: usize, shader_visible: bool) -> Option<Placed> {
        let placed = self.inner.place(bytes, shader_visible)?;
        self.resources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .note_game_range(placed.cpu, bytes as u64);
        Some(placed)
    }
}

/// The address space at the largest size the GPU takes: the default, and when the driver cannot back a window that
/// large (an RTX 4060 refuses 2^40 bytes with out of device memory) the next powers of two below.
fn open_address_space(gpu: &Gpu) -> Result<AddressSpace, MemoryError> {
    let mut last = None;
    for size in [
        None,
        Some(1u64 << 39),
        Some(1 << 38),
        Some(1 << 37),
        Some(1 << 36),
    ] {
        match AddressSpace::new(
            gpu.handles.clone(),
            Config {
                size,
                ..Config::default()
            },
        ) {
            Ok(space) => return Ok(space),
            Err(e @ MemoryError::Vulkan(_)) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("the loop ran"))
}

impl Assembly {
    pub fn build(config: &AssemblyConfig) -> Result<Arc<Assembly>, AssemblyError> {
        Self::build_with(config, |env| env)
    }

    /// As `build`, with the pipeline builder the service uses made from the device's `PipelineEnv` by `wrap`.
    pub fn build_with(
        config: &AssemblyConfig,
        wrap: impl FnOnce(Arc<PipelineEnv>) -> SharedBuilder,
    ) -> Result<Arc<Assembly>, AssemblyError> {
        let gpu = Arc::new(Gpu::create(&config.gpu)?);
        let space = Arc::new(Mutex::new(open_address_space(&gpu)?));
        if config.memory_hooks {
            memory::hooks::install_address_space(space.clone());
        }
        let resources = Arc::new(Mutex::new(Resources::shared(space)));
        let device = gpu.ctx.device.clone();
        let bindless = Bindless::new(&gpu)?;
        let code = Arc::new(CodeArena::new(resources.clone()));
        let cache = PipelineCache::open(&gpu.ctx, config.pipeline_cache.as_deref())?;
        let mut pipelines = PipelineEnv::new(
            gpu.ctx.clone(),
            bindless.layout,
            cache,
            Arc::new(DirectTranslation::new(FloatMode::default())),
            code.clone(),
        )?;
        pipelines.note = Arc::new(|text| com::note_once(text.to_string()));
        let pipelines = Arc::new(pipelines);
        let pipeline_service = pipelines::start(
            config.pipelines_seen.as_ref(),
            config.pipeline_workers,
            config.draw_policy,
            wrap(pipelines.clone()),
        );
        let pacer = Arc::new(Pacer::new(config.frame_limit));
        let policy = Arc::new(PacerPolicy::new(pacer.clone()));
        let mut presentation = match &config.output {
            Output::Window(options) => {
                Presentation::open_window(&gpu.vulkan, options.into(), policy, config.present)?
            }
            Output::Offscreen(width, height) => {
                let target = OffscreenTarget::new(
                    &gpu.vulkan,
                    (*width, *height),
                    vk::Format::R8G8B8A8_UNORM,
                    false,
                )?;
                let backend = VulkanBackend::offscreen(&gpu.vulkan, target)?;
                Presentation::new(Box::new(backend), policy, config.present)
            }
        };
        presentation.set_notes(Box::new(|text| com::note_once(text.to_string())));
        if let Some(window) = presentation.window() {
            let translator = Mutex::new(Translator);
            window.set_motion_sink(Box::new(hooks::forward_motion));
            window.set_event_sink(Box::new(move |event| {
                let translated = translator
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .translate(event);
                if let Some(event) = translated {
                    hooks::forward(event);
                }
            }));
        }
        let (width, height) = match &config.output {
            Output::Window(o) => o.size,
            Output::Offscreen(w, h) => (*w, *h),
        };
        presentation.set_topology(crate::present::DisplayTopology::from_vulkan(
            &gpu.vulkan,
            width,
            height,
        ));
        // SAFETY: plain create calls on the live device; they live as long as the assembly, which is the process.
        let exec = unsafe {
            ExecState {
                pool: device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(gpu.ctx.family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )?,
                fence: device.create_fence(&vk::FenceCreateInfo::default(), None)?,
                spill: SpillRing::new(&resources),
            }
        };
        Ok(Arc::new(Assembly {
            views: ViewCache::new(device),
            gpu,
            upload: Arc::new(UploadArena::new(resources.clone())),
            resources,
            descriptors: Mutex::new(Descriptors::new()),
            bindless,
            pipelines,
            pipeline_service,
            code,
            exec: Mutex::new(exec),
            presentation: Arc::new(presentation),
            pacer,
            core: OnceLock::new(),
            scans: Mutex::new(HashMap::new()),
            neutral: Mutex::new(None),
            uploaded: Mutex::new(std::collections::HashSet::new()),
            latest_token: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    /// What `create_device` takes: every trait of the device implemented by the real parts. `allocator` is the
    /// title's allocation hook (the XMem layer), or `NoAllocator` while it is not wired.
    pub fn integration_with(self: &Arc<Self>, allocator: Arc<dyn HeapAllocator>) -> Integration {
        let low = Arc::new(ResourcesLow::new(
            self.resources.clone(),
            SyncPolicy::WriteWatch,
        ));
        let placement = Arc::new(NotingPlacement {
            inner: Arc::new(ConsolePlacement::new(allocator, low)),
            resources: self.resources.clone(),
        });
        Integration {
            executor: Arc::new(AssemblyExecutor::new(self.clone())),
            timelines: Arc::new(VkTimelines::new(self.gpu.ctx.device.clone())),
            placement,
            frame_events: Arc::new(AssemblyFrames::new(self.clone())),
            upload: self.upload.clone(),
            device_methods: super::devmethods::table(),
            queue_methods: super::frames::queue_table(),
            list_methods: Vec::new(),
        }
    }

    pub fn integration(self: &Arc<Self>) -> Integration {
        self.integration_with(Arc::new(NoAllocator))
    }

    pub fn resources(&self) -> std::sync::MutexGuard<'_, Resources> {
        self.resources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn descriptors(&self) -> std::sync::MutexGuard<'_, Descriptors> {
        self.descriptors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The offscreen target's pixels, when frames go to one.
    pub fn offscreen_pixels(&self) -> Option<Vec<u8>> {
        self.presentation
            .with_backend::<VulkanBackend<OffscreenTarget>, _>(|b| b.target.read_pixels().ok())
            .flatten()
    }
}

static REGISTRY: RwLock<Vec<(usize, Arc<Assembly>)>> = RwLock::new(Vec::new());
static CURRENT: RwLock<Option<Arc<Assembly>>> = RwLock::new(None);

/// Remembers which assembly a device made by `create_device` belongs to.
pub fn register(core: &Arc<DeviceCore>, assembly: &Arc<Assembly>) {
    let _ = assembly.core.set(core.clone());
    REGISTRY
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .push((Arc::as_ptr(core) as usize, assembly.clone()));
}

/// A device object was destroyed, so every object it made is gone: when it was the assembly's last device, the
/// pipeline service stops recording and writes the pipeline cache.
pub(crate) fn device_released(core: &Arc<DeviceCore>) {
    let key = Arc::as_ptr(core) as usize;
    let last = {
        let mut registry = REGISTRY.write().unwrap_or_else(PoisonError::into_inner);
        let Some(at) = registry.iter().position(|(k, _)| *k == key) else {
            return;
        };
        let (_, assembly) = registry.remove(at);
        let remaining = registry.iter().any(|(_, a)| Arc::ptr_eq(a, &assembly));
        (!remaining).then_some(assembly)
    };
    if let Some(assembly) = last {
        assembly.pipeline_service.shutdown();
    }
}

/// The assembly behind a device object.
///
/// # Safety
/// `device` is a device object made by `create_device`.
pub unsafe fn assembly_of(device: *mut std::ffi::c_void) -> Option<Arc<Assembly>> {
    // SAFETY: as the caller promises.
    let key = Arc::as_ptr(unsafe { crate::device::device_core(device) }) as usize;
    REGISTRY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, a)| a.clone())
}

pub fn set_current(assembly: Option<Arc<Assembly>>) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = assembly;
}

pub fn current() -> Option<Arc<Assembly>> {
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}
